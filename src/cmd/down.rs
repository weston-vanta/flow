//! `flow down`: stops the supervisor, leaving every worker and task running. It closes the pane
//! herdr shows the supervisor's Claude Code session in, marks the lock stopped keeping that
//! session to resume, and removes the watch lock, which ends the watcher at its next cycle.

use std::sync::Arc;

use crate::herdr::Herdr;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::task::{Status, TaskRecord};
use crate::supervisor::{self, Lock};
use crate::watch::lock as watch_lock;

/// Stops the supervisor `state/supervisor/lock.json` records. Fails, changing nothing, when the
/// supervisor's host cannot be reached.
pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let Some(lock) = supervisor::load(state)? else {
        return super::print("no supervisor recorded; nothing to stop\n");
    };
    if lock.stopped.is_some() {
        return super::print("the supervisor is already stopped; nothing to stop\n");
    }
    let tasks = Store::open(state).list_tasks()?;
    if let Some(host) = reachable_host(state, &lock)? {
        let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
        let sessions = Config::load(state)?.listed_sessions(&lock.herdr.session);
        close_supervisor(&runner, &host, &lock, &sessions, &tasks)?;
    }
    supervisor::mark_stopped(state)?;
    let kept = lock
        .claude_session
        .as_ref()
        .map(|session| format!(", keeping session {session} to resume"))
        .unwrap_or_default();
    super::print(&format!(
        "marked the supervisor stopped in state/supervisor/lock.json{kept}\n"
    ))?;
    if watch_lock::clear(state)? {
        super::print("removed state/supervisor/watch.lock; the watcher ends at its next cycle\n")?;
    }
    let open = tasks
        .iter()
        .filter(|(_, status)| status.state.is_open())
        .count();
    super::print(&format!(
        "{open} open tasks keep running; flow up resumes the supervisor\n"
    ))
}

/// The host the supervisor runs on, or `None`, saying so, when it is unknown, gone, or not
/// running: the supervisor went with it.
fn reachable_host(state: &StateDir, lock: &Lock) -> anyhow::Result<Option<Host>> {
    let gone = |why: String| {
        super::print(&format!(
            "the supervisor's pane {} on {} is already gone: {why}\n",
            lock.herdr.pane_id, lock.herdr.host
        ))
    };
    let Some(record) = inventory::load(state, &lock.host_env_id)? else {
        gone(format!("{} is not a known host", lock.host_env_id))?;
        return Ok(None);
    };
    if record.gone {
        gone(format!("{} is gone", record.name))?;
        return Ok(None);
    }
    if !record.should_probe() {
        gone(format!("{} is not running", record.name))?;
        return Ok(None);
    }
    Ok(Some(record.host()))
}

/// Closes the supervisor's pane on `host`, wherever herdr shows it in `sessions`. When the
/// supervisor runs nowhere, closes nothing, since a pane id alone does not say whose pane it is.
fn close_supervisor(
    runner: &Arc<dyn Runner>,
    host: &Host,
    lock: &Lock,
    sessions: &[String],
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<()> {
    let unreachable = |err: anyhow::Error| {
        err.context(format!(
            "closing the supervisor's pane on {}; nothing stopped, and the lock is kept",
            host.name
        ))
    };
    let nowhere = || {
        super::print(&format!(
            "the supervisor runs in no pane on {}; closed nothing, so any pane it left stays \
             open\n",
            host.name
        ))
    };
    let found = supervisor::find(runner, host, lock, sessions, tasks).map_err(unreachable)?;
    let Some(agent) = found else {
        return nowhere();
    };
    let herdr = Herdr::with_session(runner.clone(), &agent.session);
    if !herdr
        .close_pane(host, &agent.pane_id)
        .map_err(unreachable)?
    {
        return nowhere();
    }
    super::print(&format!(
        "closed the supervisor's pane {} on {} (herdr session {})\n",
        agent.pane_id, host.name, agent.session
    ))
}
