//! The supervisor session's lock, `state/supervisor/lock.json`: which host and herdr pane run
//! the supervisor, and the Claude Code session to resume when it has to be started again. `flow
//! down` marks it stopped rather than removing it, so the next `flow up` resumes that session;
//! a stopped lock names no running supervisor pane.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr;
use crate::paths::StateDir;
use crate::runner;
use crate::state::config::Host;
use crate::state::fs::{with_lock, write_atomic};
use crate::state::task::HerdrTarget;

pub const SUPERVISOR_ENV: &str = crate::vars::SUPERVISOR;
/// Whether this process runs under the supervisor session `flow up` started, as opposed to
/// another Claude Code session in the same repo, whose hooks must leave supervision alone.
pub fn is_supervisor_session() -> bool {
    std::env::var_os(SUPERVISOR_ENV).is_some()
}

/// The supervisor always runs from the clone's fixed path, because the harness keys
/// transcripts by cwd and `--resume` must find the same conversation on every host.
pub const SUPERVISOR_CWD: &str = "/workspaces/flow";
/// The supervisor's herdr workspace label and Claude Code session name.
pub const SUPERVISOR_NAME: &str = "flow-supervisor";
/// How the refusals beside an `Owner::Unconfirmed` agent tell the user out.
pub const UNCONFIRMED_WAY_OUT: &str = "If it is your own session, `flow down --forget` marks \
     the supervisor stopped and leaves the pane alone; if it is the supervisor, exit it and run \
     `flow down`; then `flow up`.";
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

/// Why the agent in `lock`'s pane, an `Owner::Unconfirmed` one, cannot be told from the
/// supervisor, for messages.
pub fn unconfirmed(lock: &Lock) -> String {
    format!(
        "pane {} on {} holds an agent herdr reports no Claude Code session for, so flow cannot \
         tell it from the supervisor",
        lock.herdr.pane_id, lock.herdr.host
    )
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

/// The argv that attaches this terminal to the supervisor's pane: herdr itself when `host`, the
/// lock's host, is this machine, else ssh with a tty, multiplexed through `control_dir`.
pub fn attach_argv(lock: &Lock, host: &Host, local: bool, control_dir: &Path) -> Vec<String> {
    let attach = herdr::attach_argv(&lock.herdr.session, &lock.herdr.pane_id);
    if local {
        return attach;
    }
    let mut argv = vec!["ssh".to_owned(), "-t".to_owned()];
    argv.extend(runner::ssh_connection_options(control_dir));
    argv.push(host.ssh_alias.clone());
    argv.push(runner::remote_command(&attach));
    argv
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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

    fn host() -> Host {
        Host {
            name: "host-b".to_owned(),
            env_id: "env-0002".to_owned(),
            ssh_alias: "host-b.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|word| (*word).to_owned()).collect()
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
    fn attach_argv_runs_herdr_directly_on_the_local_host() {
        assert_eq!(
            attach_argv(&lock(), &host(), true, Path::new("/run/flow")),
            words(&["herdr", "--session", "default", "agent", "attach", "w1:p1"])
        );
    }

    #[test]
    fn attach_argv_goes_through_ssh_with_a_tty_for_a_remote_host() {
        let control_dir = PathBuf::from("/run/flow");
        assert_eq!(
            attach_argv(&lock(), &host(), false, &control_dir),
            words(&[
                "ssh",
                "-t",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/run/flow/ssh-%C",
                "-o",
                "ControlPersist=600",
                "host-b.example",
                "zsh -lc ''\\''herdr'\\'' '\\''--session'\\'' '\\''default'\\'' '\\''agent'\\'' \
                 '\\''attach'\\'' '\\''w1:p1'\\'''",
            ])
        );
    }
}
