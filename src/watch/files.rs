//! What `flow watch` reads from files: each task's `status.json` state and the lines appended
//! to its `events.jsonl`, and, on the task's CDE, whether its worktree was written since its
//! last event.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::runner::Runner;
use crate::state::config::Cde;
use crate::state::events::Event;
use crate::state::task::State;

/// How long a worktree check may run, on the CDE and here.
const WORKTREE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// The exit code ssh uses for its own failures.
const SSH_FAILURE_STATUS: i32 = 255;

/// How far the watcher has read one task's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Seen {
    pub(super) state: State,
    /// Bytes of `events.jsonl` read.
    pub(super) events: u64,
}

pub(super) enum NewEvents {
    /// Each complete new line's event kind and the byte offset just after it.
    Lines(Vec<(String, u64)>),
    /// The file is shorter than what was read; this is its length.
    Shrunk(u64),
}

/// The complete lines of `events.jsonl` after byte `offset`. A line that does not parse reads
/// as an event of empty kind.
pub(super) fn new_events(path: &Path, offset: u64) -> anyhow::Result<NewEvents> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(NewEvents::Lines(Vec::new()));
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let len = file.metadata()?.len();
    if len < offset {
        return Ok(NewEvents::Shrunk(len));
    }
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_to_end(&mut bytes))
        .with_context(|| format!("reading {}", path.display()))?;
    let mut lines = Vec::new();
    let mut end = offset;
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        end += line.len() as u64;
        if line.trim_ascii().is_empty() {
            continue;
        }
        let kind = serde_json::from_slice::<Event>(line)
            .map(|ev| ev.kind)
            .unwrap_or_default();
        lines.push((kind, end));
    }
    Ok(NewEvents::Lines(lines))
}

pub(super) fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// A `find` on a task's CDE for the first file under its worktree, `.git` aside, modified after
/// a time. It runs in the background so a slow walk or a hung connection cannot stall the poll
/// loop, and is killed after `WORKTREE_CHECK_TIMEOUT`.
pub(super) struct WorktreeCheck {
    child: Child,
    deadline: Instant,
}

pub(super) enum Checked {
    Running,
    /// The CDE answered: whether a file was written.
    Written(bool),
    Unreachable,
    /// The deadline passed and the check was killed.
    TimedOut,
    Failed(String),
}

impl WorktreeCheck {
    pub(super) fn start(
        runner: &dyn Runner,
        cde: &Cde,
        worktree: &Path,
        since: Timestamp,
    ) -> anyhow::Result<WorktreeCheck> {
        let child = runner.spawn(cde, &worktree_write_argv(worktree, since))?;
        Ok(WorktreeCheck {
            child,
            deadline: Instant::now() + WORKTREE_CHECK_TIMEOUT,
        })
    }

    pub(super) fn poll(&mut self) -> Checked {
        let exit = match self.child.try_wait() {
            Ok(Some(exit)) => exit,
            Ok(None) if Instant::now() < self.deadline => return Checked::Running,
            Ok(None) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Checked::TimedOut;
            }
            Err(err) => return Checked::Failed(format!("waiting on find: {err}")),
        };
        let mut stdout = String::new();
        if let Some(mut pipe) = self.child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        if exit.code() == Some(SSH_FAILURE_STATUS) && stdout.is_empty() {
            return Checked::Unreachable;
        }
        Checked::Written(!stdout.trim().is_empty())
    }
}

#[cfg(test)]
impl WorktreeCheck {
    /// Blocks until the check's process exits.
    pub(super) fn finish(&mut self) {
        self.child.wait().unwrap();
    }
}

impl Drop for WorktreeCheck {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// `find` printing the first file under `worktree`, `.git` aside, modified after `since`,
/// under the CDE's `timeout`.
fn worktree_write_argv(worktree: &Path, since: Timestamp) -> Vec<String> {
    let timeout = WORKTREE_CHECK_TIMEOUT.as_secs().to_string();
    let since = since.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
    [
        "timeout",
        &timeout,
        "find",
        &worktree.to_string_lossy(),
        "-path",
        "*/.git",
        "-prune",
        "-o",
        "-type",
        "f",
        "-newermt",
        &since,
        "-print",
        "-quit",
    ]
    .map(str::to_owned)
    .to_vec()
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use super::*;
    use crate::runner::{FakeRunner, Scripted};

    fn cde() -> Cde {
        Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn check(status: i32, stdout: &str) -> (Checked, FakeRunner) {
        let fake = FakeRunner::new(vec![Scripted {
            status,
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }]);
        let since = "2026-10-26T12:00:00.5Z".parse().unwrap();
        let mut check =
            WorktreeCheck::start(&fake, &cde(), Path::new("/work/repo.wt/feature"), since).unwrap();
        check.finish();
        (check.poll(), fake)
    }

    #[test]
    fn worktree_check_runs_find_under_timeout() {
        let (checked, fake) = check(0, "");
        assert!(matches!(checked, Checked::Written(false)));
        assert_eq!(
            fake.calls()[0].argv,
            [
                "timeout",
                "30",
                "find",
                "/work/repo.wt/feature",
                "-path",
                "*/.git",
                "-prune",
                "-o",
                "-type",
                "f",
                "-newermt",
                "2026-10-26T12:00:00Z",
                "-print",
                "-quit",
            ]
        );
    }

    #[test]
    fn worktree_check_reads_a_found_file_or_ssh_failure() {
        let (checked, _) = check(0, "/work/repo.wt/feature/src/lib.rs\n");
        assert!(matches!(checked, Checked::Written(true)));
        let (checked, _) = check(255, "");
        assert!(matches!(checked, Checked::Unreachable));
    }

    #[test]
    fn worktree_check_past_its_deadline_is_killed() {
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut check = WorktreeCheck {
            child,
            deadline: Instant::now(),
        };
        assert!(matches!(check.poll(), Checked::TimedOut));
        assert!(check.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn new_events_reads_complete_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let line = r#"{"ts":"2026-10-26T12:00:00Z","source":"cli","kind":"report","data":{}}"#;
        std::fs::write(&path, format!("{line}\n{{\"ts\":")).unwrap();
        let NewEvents::Lines(lines) = new_events(&path, 0).unwrap() else {
            panic!("expected lines");
        };
        assert_eq!(lines, [("report".to_owned(), line.len() as u64 + 1)]);
        assert!(matches!(
            new_events(&path, 10_000).unwrap(),
            NewEvents::Shrunk(_)
        ));
    }
}
