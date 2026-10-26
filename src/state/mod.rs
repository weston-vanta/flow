pub mod config;
pub mod events;
pub mod fs;
pub mod ids;
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
use task::{Status, TaskRecord};

const TASK_FILE: &str = "task.json";
const STATUS_FILE: &str = "status.json";
const BRIEF_FILE: &str = "brief.md";
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
        let dir = self.task_dir(id)?;
        if !dir.is_dir() {
            bail!("no such task: {id}");
        }
        Ok((
            read_json(&dir.join(TASK_FILE))?,
            read_json(&dir.join(STATUS_FILE))?,
        ))
    }

    /// Every task, oldest first.
    pub fn list_tasks(&self) -> anyhow::Result<Vec<(TaskRecord, Status)>> {
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
        ids.iter().map(|id| self.task(id)).collect()
    }

    /// Updates `status.json` under its lock. When `f` changes `state`, also appends a `status`
    /// event with `{from, to}`.
    pub fn set_status(&self, id: &str, f: impl FnOnce(&mut Status)) -> anyhow::Result<()> {
        let dir = self.task_dir(id)?;
        let path = dir.join(STATUS_FILE);
        if !path.exists() {
            bail!("no such task: {id}");
        }
        fs::with_lock(&path, || {
            let mut status: Status = serde_json::from_slice(&std::fs::read(&path)?)?;
            let from = status.state;
            f(&mut status);
            write_json(&path, &status)?;
            if status.state != from {
                let ev = Event {
                    ts: Timestamp::now(),
                    source: EVENT_SOURCE.to_owned(),
                    kind: "status".to_owned(),
                    data: json!({"from": from, "to": status.state}),
                };
                events::append(&dir, &ev).map_err(io::Error::other)?;
            }
            Ok(())
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
