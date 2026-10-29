//! The supervisor session's lock, `state/supervisor/lock.json`: which host and herdr pane run
//! the supervisor, and the Claude Code session to resume when it has to be started again.

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

/// Set in the supervisor's environment, so its hooks can tell it from other sessions started in
/// the same repo.
pub const SUPERVISOR_ENV: &str = "FLOW_SUPERVISOR";
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
const LOCK_FILE: &str = "supervisor/lock.json";

/// Where the supervisor runs, and the Claude Code session to resume when it has to start again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub host_env_id: String,
    pub herdr: HerdrTarget,
    pub claude_session: Option<String>,
    pub since: Timestamp,
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
        };
        write(&path, &lock)?;
        Ok(lock)
    })
    .with_context(|| format!("writing {}", path.display()))
}

/// Sets the supervisor's Claude Code session to resume, or clears it with `None`. Returns
/// `false`, changing nothing, when there is no lock.
pub fn set_session(state: &StateDir, session_id: Option<&str>) -> anyhow::Result<bool> {
    let path = lock_path(state);
    if !path.exists() {
        return Ok(false);
    }
    with_lock(&path, || {
        let Some(mut lock) = read(&path).map_err(io::Error::other)? else {
            return Ok(false);
        };
        lock.claude_session = session_id.map(str::to_owned);
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
