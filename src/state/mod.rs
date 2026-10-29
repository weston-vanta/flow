pub mod config;
pub mod events;
pub mod fs;
pub mod ids;
pub mod requests;
pub mod task;

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::paths::StateDir;
use events::Event;
use task::{Decision, Status, TaskRecord};

const TASK_FILE: &str = "task.json";
const STATUS_FILE: &str = "status.json";
const BRIEF_FILE: &str = "brief.md";
const REPORT_FILE: &str = "report.md";
const DECISIONS_FILE: &str = "decisions.jsonl";
const EVENT_SOURCE: &str = "flow";

/// Task records under `state/tasks/<task-id>/`.
#[derive(Debug, Clone)]
pub struct Store {
    tasks: PathBuf,
}

impl Store {
    pub fn open(state: &StateDir) -> Store {
        Store {
            tasks: state.root().join("tasks"),
        }
    }

    /// The directory holding task `id`'s files.
    pub fn task_dir(&self, id: &str) -> anyhow::Result<PathBuf> {
        if !ids::is_task_id(id) {
            bail!("invalid task id: {id:?}");
        }
        Ok(self.tasks.join(id))
    }

    /// `task_dir`, failing when the task does not exist.
    fn existing_task_dir(&self, id: &str) -> anyhow::Result<PathBuf> {
        let dir = self.task_dir(id)?;
        if !dir.is_dir() {
            bail!("no such task: {id}");
        }
        Ok(dir)
    }

    /// Writes the task's files into a hidden staging directory and renames it into place, so
    /// readers never see a half-created task.
    pub fn create_task(&self, rec: TaskRecord, brief: &str) -> anyhow::Result<()> {
        let dir = self.task_dir(&rec.id)?;
        if dir.exists() {
            bail!("task {} already exists", rec.id);
        }
        std::fs::create_dir_all(&self.tasks)
            .with_context(|| format!("creating {}", self.tasks.display()))?;
        let staging = self.tasks.join(format!(".{}.creating", rec.id));
        std::fs::create_dir(&staging).with_context(|| format!("creating {}", staging.display()))?;
        let result = write_new_task(&staging, &rec, brief).and_then(|()| {
            std::fs::rename(&staging, &dir)
                .with_context(|| format!("moving {} into place", staging.display()))
        });
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }

    pub fn task(&self, id: &str) -> anyhow::Result<(TaskRecord, Status)> {
        let dir = self.existing_task_dir(id)?;
        Ok((
            read_json(&dir.join(TASK_FILE))?,
            read_json(&dir.join(STATUS_FILE))?,
        ))
    }

    /// The task's `brief.md`, verbatim.
    pub fn brief(&self, id: &str) -> anyhow::Result<String> {
        let path = self.brief_path(id)?;
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    /// The path of the task's `brief.md`, failing when the task does not exist.
    pub fn brief_path(&self, id: &str) -> anyhow::Result<PathBuf> {
        Ok(self.existing_task_dir(id)?.join(BRIEF_FILE))
    }

    /// Every task, oldest first (ids sort as strings by creation time).
    pub fn list_tasks(&self) -> anyhow::Result<Vec<(TaskRecord, Status)>> {
        self.task_ids()?.iter().map(|id| self.task(id)).collect()
    }

    /// Ids of every task directory, sorted.
    fn task_ids(&self) -> anyhow::Result<Vec<String>> {
        let entries = match std::fs::read_dir(&self.tasks) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(err).with_context(|| format!("listing {}", self.tasks.display()));
            }
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", self.tasks.display()))?;
            if let Some(name) = entry.file_name().to_str()
                && ids::is_task_id(name)
                && entry.file_type()?.is_dir()
            {
                ids.push(name.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Replaces the task's `report.md` with `report` and returns its path.
    pub fn write_report(&self, id: &str, report: &[u8]) -> anyhow::Result<PathBuf> {
        let dir = self.existing_task_dir(id)?;
        let path = dir.join(REPORT_FILE);
        fs::write_atomic(&path, report).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Appends `decision` as one line of the task's `decisions.jsonl`.
    pub fn append_decision(&self, id: &str, decision: &Decision) -> anyhow::Result<()> {
        let dir = self.existing_task_dir(id)?;
        let path = dir.join(DECISIONS_FILE);
        let mut line = serde_json::to_vec(decision)?;
        line.push(b'\n');
        fs::append_line(&path, &line).with_context(|| format!("appending to {}", path.display()))
    }

    /// Updates `status.json` under its lock and sets `last_event` to now before calling `f`.
    /// When `f` changes `state`, also appends a `status` event with `{from, to}`.
    pub fn set_status(&self, id: &str, f: impl FnOnce(&mut Status)) -> anyhow::Result<()> {
        self.update_status(id, |dir, status| {
            let now = Timestamp::now();
            let from = status.state;
            status.last_event = now;
            f(status);
            if status.state != from {
                let ev = Event {
                    ts: now,
                    source: EVENT_SOURCE.to_owned(),
                    kind: "status".to_owned(),
                    data: json!({"from": from, "to": status.state}),
                };
                events::append(dir, &ev).map_err(io::Error::other)?;
            }
            Ok(())
        })
    }

    /// Clears the task's herdr target and appends `ev`, under the `status.json` lock, only if
    /// the target is still `target`; `last_event` stays as it was. `false` when it was not.
    pub fn clear_target(
        &self,
        id: &str,
        target: &task::HerdrTarget,
        ev: &Event,
    ) -> anyhow::Result<bool> {
        let mut cleared = false;
        self.update_status(id, |dir, status| {
            if status.herdr.as_ref() == Some(target) {
                status.herdr = None;
                events::append(dir, ev).map_err(io::Error::other)?;
                cleared = true;
            }
            Ok(())
        })?;
        Ok(cleared)
    }

    /// Appends `ev` to the task's `events.jsonl` and advances `last_event` to `ev.ts`, both under
    /// the `status.json` lock.
    pub fn append_event(&self, id: &str, ev: &Event) -> anyhow::Result<()> {
        self.update_status(id, |dir, status| {
            events::append(dir, ev).map_err(io::Error::other)?;
            status.last_event = status.last_event.max(ev.ts);
            Ok(())
        })
    }

    /// Locked read-modify-write of `status.json`; `f` gets the task directory and the status.
    fn update_status(
        &self,
        id: &str,
        f: impl FnOnce(&Path, &mut Status) -> io::Result<()>,
    ) -> anyhow::Result<()> {
        let dir = self.task_dir(id)?;
        let path = dir.join(STATUS_FILE);
        if !path.exists() {
            bail!("no such task: {id}");
        }
        fs::with_lock(&path, || {
            let mut status: Status = serde_json::from_slice(&std::fs::read(&path)?)?;
            f(&dir, &mut status)?;
            write_json(&path, &status)
        })
        .with_context(|| format!("updating {}", path.display()))
    }
}

fn write_new_task(dir: &Path, rec: &TaskRecord, brief: &str) -> anyhow::Result<()> {
    let now = Timestamp::now();
    write_json(&dir.join(TASK_FILE), rec)?;
    write_json(&dir.join(STATUS_FILE), &Status::pending(now))?;
    fs::write_atomic(&dir.join(BRIEF_FILE), brief.as_bytes())?;
    events::append(
        dir,
        &Event {
            ts: now,
            source: EVENT_SOURCE.to_owned(),
            kind: "task.created".to_owned(),
            data: json!({}),
        },
    )
}

fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write_atomic(path, &bytes)
}
