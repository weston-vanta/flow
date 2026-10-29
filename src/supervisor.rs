//! The supervisor session's lock, `state/supervisor/lock.json`: which host and herdr pane run
//! the supervisor, and the Claude Code session to resume when it has to be started again. `flow
//! down` marks it stopped rather than removing it, so the next `flow up` resumes that session;
//! a stopped lock names no running supervisor pane.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::{self, Agent, Herdr};
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::runner::Runner;
use crate::state::config::Host;
use crate::state::fs::{with_lock, write_atomic};
use crate::state::task::{HerdrTarget, Status, TaskRecord};

pub const SUPERVISOR_ENV: &str = crate::vars::SUPERVISOR;
/// Whether this process runs under the supervisor session `flow up` started, as opposed to
/// another Claude Code session in the same repo, whose hooks must leave supervision alone.
pub fn is_supervisor_session() -> bool {
    std::env::var_os(SUPERVISOR_ENV).is_some()
}

/// The supervisor always runs from the clone's fixed path, because the harness keys
/// transcripts by cwd and `--resume` must find the same conversation on every host.
pub const SUPERVISOR_CWD: &str = "/workspaces/flow";
/// The supervisor's herdr workspace label and Claude Code session name. An agent herdr reports no
/// session for is the supervisor only in the lock's pane under this label (see `owner::owner`).
pub const SUPERVISOR_NAME: &str = "flow-supervisor";
const LOCK_FILE: &str = "supervisor/lock.json";

/// Where the supervisor runs, and the Claude Code session to resume when it has to start again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub host_env_id: String,
    pub herdr: HerdrTarget,
    pub claude_session: Option<String>,
    pub since: Timestamp,
    /// When `flow down` stopped the supervisor; `None` while it runs, or ran until it died.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Timestamp>,
}

/// Looks for `lock`'s supervisor on `host` in the lock's herdr session, then in the rest of
/// `sessions`.
pub fn find(
    runner: &Arc<dyn Runner>,
    host: &Host,
    lock: &Lock,
    sessions: &[String],
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<Option<Agent>> {
    let others = sessions
        .iter()
        .filter(|session| **session != lock.herdr.session);
    for session in std::iter::once(&lock.herdr.session).chain(others) {
        let herdr = Herdr::with_session(runner.clone(), session);
        let mut agents = herdr.agents_if_running(host)?.unwrap_or_default();
        owner::label_where_needed(&herdr, host, &mut agents, Some(lock), tasks)?;
        let supervisor = agents
            .into_iter()
            .find(|agent| owner::owner(&host.name, agent, Some(lock), tasks) == Owner::Supervisor);
        if supervisor.is_some() {
            return Ok(supervisor);
        }
    }
    Ok(None)
}

/// flow's herdr session, where the supervisor and workers run: the one the lock records, which
/// `flow up` ran in, else herdr's `default`.
pub fn herdr_session(state: &StateDir) -> anyhow::Result<String> {
    Ok(load(state)?.map_or_else(
        || herdr::DEFAULT_SESSION.to_owned(),
        |lock| lock.herdr.session,
    ))
}

pub fn lock_path(state: &StateDir) -> PathBuf {
    state.root().join(LOCK_FILE)
}

/// The lock, or `None` when no supervisor was ever started.
pub fn load(state: &StateDir) -> anyhow::Result<Option<Lock>> {
    let path = lock_path(state);
    read(&path).with_context(|| format!("reading {}", path.display()))
}

/// Records the supervisor as running at `herdr` on the host `host_env_id`, keeping the
/// Claude Code session of the lock it replaces.
pub fn take(state: &StateDir, host_env_id: &str, herdr: HerdrTarget) -> anyhow::Result<Lock> {
    let path = lock_path(state);
    let dir = path.parent().expect("the lock file has a parent directory");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    with_lock(&path, || {
        let claude_session = read(&path)
            .map_err(io::Error::other)?
            .and_then(|lock| lock.claude_session);
        let lock = Lock {
            host_env_id: host_env_id.to_owned(),
            herdr,
            claude_session,
            since: Timestamp::now(),
            stopped: None,
        };
        write(&path, &lock)?;
        Ok(lock)
    })
    .with_context(|| format!("writing {}", path.display()))
}

/// Marks the supervisor stopped now, keeping the Claude Code session to resume.
pub fn mark_stopped(state: &StateDir) -> anyhow::Result<bool> {
    update(state, |lock| lock.stopped = Some(Timestamp::now()))
}

/// The lock of the supervisor, unless it was stopped.
pub fn running(state: &StateDir) -> anyhow::Result<Option<Lock>> {
    Ok(load(state)?.filter(|lock| lock.stopped.is_none()))
}

/// Sets the supervisor's Claude Code session to resume, or clears it with `None`.
pub fn set_session(state: &StateDir, session_id: Option<&str>) -> anyhow::Result<bool> {
    update(state, |lock| {
        lock.claude_session = session_id.map(str::to_owned)
    })
}

/// Records that herdr shows the supervisor in `herdr` now.
pub fn set_target(state: &StateDir, herdr: HerdrTarget) -> anyhow::Result<bool> {
    update(state, |lock| lock.herdr = herdr)
}

/// Applies `change` to the lock under its file lock. Returns `false`, changing nothing, when
/// there is no lock.
fn update(state: &StateDir, change: impl FnOnce(&mut Lock)) -> anyhow::Result<bool> {
    let path = lock_path(state);
    if !path.exists() {
        return Ok(false);
    }
    with_lock(&path, || {
        let Some(mut lock) = read(&path).map_err(io::Error::other)? else {
            return Ok(false);
        };
        change(&mut lock);
        write(&path, &lock)?;
        Ok(true)
    })
    .with_context(|| format!("updating {}", path.display()))
}

fn read(path: &Path) -> anyhow::Result<Option<Lock>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn write(path: &Path, lock: &Lock) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(lock)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> Lock {
        Lock {
            host_env_id: "env-0002".to_owned(),
            herdr: HerdrTarget {
                host: "host-b".to_owned(),
                workspace_id: "w1".to_owned(),
                pane_id: "w1:p1".to_owned(),
                agent: "claude".to_owned(),
                session: "default".to_owned(),
            },
            claude_session: None,
            since: "2026-10-26T12:00:00Z".parse().unwrap(),
            stopped: None,
        }
    }

    fn state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("state")).unwrap();
        let state = StateDir::resolve(dir.path()).unwrap();
        (dir, state)
    }

    #[test]
    fn a_lock_without_a_stop_reads_as_running() {
        let lock: Lock = serde_json::from_str(
            r#"{"host_env_id": "env-0002", "herdr": {"host": "host-b", "workspace_id": "w1",
                "pane_id": "w1:p1", "agent": "claude", "session": "default"},
                "claude_session": "s9", "since": "2026-10-26T12:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(lock.stopped, None);
        let json = serde_json::to_value(&lock).unwrap();
        assert!(json.get("stopped").is_none(), "{json}");
    }

    #[test]
    fn a_stopped_lock_keeps_its_session_and_names_no_running_supervisor() {
        let (_dir, state) = state();
        assert!(!mark_stopped(&state).unwrap());
        assert_eq!(running(&state).unwrap(), None);
        take(&state, "env-0002", lock().herdr).unwrap();
        set_session(&state, Some("s9")).unwrap();
        assert_eq!(
            running(&state).unwrap().map(|lock| lock.herdr),
            Some(lock().herdr)
        );

        assert!(mark_stopped(&state).unwrap());

        let stopped = load(&state).unwrap().unwrap();
        assert!(stopped.stopped.is_some());
        assert_eq!(stopped.claude_session.as_deref(), Some("s9"));
        assert_eq!(running(&state).unwrap(), None);
        let resumed = take(&state, "env-0001", lock().herdr).unwrap();
        assert_eq!(resumed.stopped, None);
        assert_eq!(resumed.claude_session.as_deref(), Some("s9"));
    }

    #[test]
    fn flows_herdr_session_is_the_locks_even_once_stopped() {
        let (_dir, state) = state();
        assert_eq!(herdr_session(&state).unwrap(), "default");
        let mut herdr = lock().herdr;
        herdr.session = "work".to_owned();
        take(&state, "env-0002", herdr).unwrap();
        mark_stopped(&state).unwrap();
        assert_eq!(herdr_session(&state).unwrap(), "work");
    }
}
