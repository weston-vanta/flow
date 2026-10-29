//! The supervisor session's lock, `state/supervisor/lock.json`: which host and herdr pane run
//! the supervisor, and the Claude Code session to resume when it has to be started again. `flow
//! down` marks it stopped rather than removing it, so the next `flow up` resumes that session;
//! a stopped lock names no running supervisor pane.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::{self, Agent, Workspace};
use crate::observe;
use crate::paths::StateDir;
use crate::runner;
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
/// The supervisor's herdr workspace label and Claude Code session name.
pub const SUPERVISOR_NAME: &str = "flow-supervisor";
/// How the refusals beside an agent `Lock::identify` leaves unconfirmed tell the user out.
pub const UNCONFIRMED_WAY_OUT: &str = "If it is your own session, `flow down --forget` marks \
     the supervisor stopped and leaves the pane alone; if it is the supervisor, exit it and run \
     `flow down`; then `flow up`.";
const LOCK_FILE: &str = "supervisor/lock.json";

/// What an agent in the lock's pane is to the supervisor, as `Lock::identify` tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    Supervisor,
    /// Not the supervisor: it sits outside the lock's workspace, and herdr does not report the
    /// supervisor's session for it.
    Other,
    /// Possibly the supervisor: it sits in the lock's workspace, but herdr's view does not
    /// confirm it.
    Unconfirmed,
}

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

impl Lock {
    /// Whether `workspace` is the supervisor's: the lock's workspace under the supervisor's
    /// label, since herdr reuses workspace ids and an id alone could name a worker's.
    pub fn names_workspace(&self, workspace: &Workspace) -> bool {
        workspace.workspace_id == self.herdr.workspace_id && workspace.label == SUPERVISOR_NAME
    }

    /// What `agent`, which herdr lists on `host`, is to the supervisor when it sits in the
    /// lock's pane, `tasks` naming the workers it may be instead; `None` when it sits elsewhere.
    pub fn in_pane(
        &self,
        host: &str,
        agent: &Agent,
        tasks: &[(TaskRecord, Status)],
    ) -> Option<Identity> {
        self.herdr
            .is(host, &agent.session, &agent.pane_id)
            .then(|| self.identify(host, agent, tasks))
    }

    /// What `agent`, found in the lock's pane on `host`, is to the supervisor. herdr reuses pane
    /// ids after a server restart, so the pane alone could hold anyone's agent.
    /// - When the lock has the supervisor's Claude Code session and herdr reports the agent's,
    ///   equal ids confirm it.
    /// - Otherwise an agent outside the lock's workspace is another, and so is one whose session
    ///   an open task of `tasks` recorded.
    /// - One inside it is confirmed when herdr reports no session, or flow has not recorded the
    ///   supervisor's, and it runs in the supervisor's cwd, in a workspace still labelled as the
    ///   supervisor's. Different ids, as after a missed SessionStart hook, another label, or
    ///   another cwd leave it unconfirmed, unless an open task's herdr target is its pane and
    ///   its session does not conflict, which makes it that task's worker: its cwd cannot rule
    ///   it out, since herdr may report the checkout's path through a symlink, and a second
    ///   supervisor beside the live one would resume the same session. The cwd and label
    ///   outweigh such a target, which a task dispatched before a herdr restart can leave on
    ///   the pane the supervisor holds now.
    pub fn identify(&self, host: &str, agent: &Agent, tasks: &[(TaskRecord, Status)]) -> Identity {
        let ids = (
            self.claude_session.as_deref(),
            agent.agent_session_id.as_deref(),
        );
        if let (Some(recorded), Some(reported)) = ids
            && recorded == reported
        {
            return Identity::Supervisor;
        }
        if agent.workspace_id != self.herdr.workspace_id
            || worker_by_session(agent, tasks).is_some()
        {
            return Identity::Other;
        }
        let at_home = agent.cwd == Path::new(SUPERVISOR_CWD)
            && agent.workspace_label.as_deref() == Some(SUPERVISOR_NAME);
        match ids {
            (Some(_), Some(_)) => {}
            _ if at_home => return Identity::Supervisor,
            _ => {}
        }
        match worker_by_target(host, agent, tasks) {
            Some(_) => Identity::Other,
            None => Identity::Unconfirmed,
        }
    }
}

/// The open task of `tasks` whose worker `agent`, which herdr lists on `host`, is, by session
/// (see `worker_by_session`) or else by herdr target (see `worker_by_target`).
pub fn worker_of<'a>(
    host: &str,
    agent: &Agent,
    tasks: &'a [(TaskRecord, Status)],
) -> Option<&'a TaskRecord> {
    worker_by_session(agent, tasks).or_else(|| worker_by_target(host, agent, tasks))
}

/// The open task of `tasks` that recorded the Claude Code session herdr reports for `agent`.
fn worker_by_session<'a>(
    agent: &Agent,
    tasks: &'a [(TaskRecord, Status)],
) -> Option<&'a TaskRecord> {
    let reported = agent.agent_session_id.as_deref()?;
    tasks
        .iter()
        .find(|(_, status)| {
            status.state.is_open()
                && status
                    .claude_session
                    .as_ref()
                    .is_some_and(|claude| claude.session_id == reported)
        })
        .map(|(record, _)| record)
}

/// The open task of `tasks` whose herdr target is the pane of `agent`, which herdr lists on
/// `host`, and whose recorded session does not conflict with the one herdr reports, as for a
/// worker started in a reused pane before its SessionStart hook ran.
fn worker_by_target<'a>(
    host: &str,
    agent: &Agent,
    tasks: &'a [(TaskRecord, Status)],
) -> Option<&'a TaskRecord> {
    observe::tasks_in_pane(
        tasks,
        host,
        &agent.session,
        &agent.pane_id,
        agent.agent_session_id.as_deref(),
    )
    .find(|(_, status)| status.state.is_open())
    .map(|(record, _)| record)
}

fn in_supervisor_cwd(agent: &Agent) -> bool {
    agent.cwd == Path::new(SUPERVISOR_CWD)
}

/// Gives each of `agents` in the supervisor's cwd the label `workspaces` lists for its
/// workspace, which `Lock::identify` needs when the Claude Code sessions do not decide.
pub fn label_workspaces(agents: &mut [Agent], workspaces: &[Workspace]) {
    for agent in agents.iter_mut().filter(|agent| in_supervisor_cwd(agent)) {
        agent.workspace_label = workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == agent.workspace_id)
            .map(|workspace| workspace.label.clone());
    }
}

/// `label_workspaces` with the workspaces `herdr` lists on `host`, asked only when one of
/// `agents` runs in the supervisor's cwd.
pub fn look_up_workspace_labels(
    herdr: &herdr::Herdr,
    host: &Host,
    agents: &mut [Agent],
) -> anyhow::Result<()> {
    if agents.iter().any(in_supervisor_cwd) {
        label_workspaces(agents, &herdr.workspaces(host)?);
    }
    Ok(())
}

/// What `agent`, found in `lock`'s pane on `host`, runs instead of the supervisor, for messages:
/// the worker of an open task of `tasks`, its Claude Code session when the lock has one and
/// herdr reports one, else where it runs.
pub fn not_the_supervisor(
    lock: &Lock,
    host: &str,
    agent: &Agent,
    tasks: &[(TaskRecord, Status)],
) -> String {
    if let Some(task) = worker_of(host, agent, tasks) {
        return format!("the worker of task {}", task.id);
    }
    match (
        lock.claude_session.as_deref(),
        agent.agent_session_id.as_deref(),
    ) {
        (Some(recorded), Some(reported)) => {
            format!("Claude Code session {reported}, not the supervisor's {recorded}")
        }
        _ => {
            let kind = if agent.agent.is_empty() {
                "an agent"
            } else {
                agent.agent.as_str()
            };
            let label = agent
                .workspace_label
                .as_deref()
                .map(|label| format!(" labelled \"{label}\""))
                .unwrap_or_default();
            format!(
                "{kind} in {}, workspace {}{label}, not the supervisor in {SUPERVISOR_CWD}, \
                 workspace {} labelled \"{SUPERVISOR_NAME}\"",
                agent.cwd.display(),
                agent.workspace_id,
                lock.herdr.workspace_id
            )
        }
    }
}

/// Why `agent`, an `Identity::Unconfirmed` one in `lock`'s pane on `host`, cannot be told from
/// the supervisor, for messages.
pub fn unconfirmed(lock: &Lock, host: &str, agent: &Agent) -> String {
    let label = match agent.workspace_label.as_deref() {
        _ if !in_supervisor_cwd(agent) => {
            format!("it runs in {}, not {SUPERVISOR_CWD}", agent.cwd.display())
        }
        Some(label) => format!("its workspace is labelled \"{label}\", not \"{SUPERVISOR_NAME}\""),
        None => "its workspace's label could not be read".to_owned(),
    };
    let why = match (
        lock.claude_session.as_deref(),
        agent.agent_session_id.as_deref(),
    ) {
        (Some(recorded), Some(reported)) => {
            format!(
                "herdr reports Claude Code session {reported} for it, not the supervisor's \
                 {recorded}"
            )
        }
        (Some(_), None) => format!("herdr reports no Claude Code session for it, and {label}"),
        (None, _) => {
            format!("flow has not recorded the supervisor's Claude Code session, and {label}")
        }
    };
    format!(
        "pane {} on {host} holds an agent flow cannot tell from the supervisor: it is in the \
         supervisor's workspace {}, but {why}",
        agent.pane_id, agent.workspace_id
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

/// Marks the supervisor stopped now, keeping the Claude Code session to resume. Returns
/// `false`, changing nothing, when there is no lock.
pub fn mark_stopped(state: &StateDir) -> anyhow::Result<bool> {
    let path = lock_path(state);
    if !path.exists() {
        return Ok(false);
    }
    with_lock(&path, || {
        let Some(mut lock) = read(&path).map_err(io::Error::other)? else {
            return Ok(false);
        };
        lock.stopped = Some(Timestamp::now());
        write(&path, &lock)?;
        Ok(true)
    })
    .with_context(|| format!("updating {}", path.display()))
}

/// The lock of the supervisor, unless it was stopped.
pub fn running(state: &StateDir) -> anyhow::Result<Option<Lock>> {
    Ok(load(state)?.filter(|lock| lock.stopped.is_none()))
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
    fn a_lock_written_before_stops_were_recorded_reads_as_running() {
        let lock: Lock = serde_json::from_str(
            r#"{"host_env_id": "env-0002", "herdr": {"host": "host-b", "workspace_id": "w1",
                "pane_id": "w1:p1", "agent": "claude"}, "claude_session": "s9",
                "since": "2026-10-26T12:00:00Z"}"#,
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
