//! `flow down`: stops the supervisor, leaving every worker and task running. It closes the pane
//! herdr shows the supervisor's Claude Code session in, marks the lock stopped keeping that
//! session to resume, and removes the watch lock, which ends the watcher at its next cycle.

use std::sync::Arc;

use super::Refused;
use crate::herdr::Herdr;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::task::{Status, TaskRecord};
use crate::supervisor::{self, Found, Lock};
use crate::watch::lock as watch_lock;

/// Stops the supervisor `state/supervisor/lock.json` records. With `forget`, closes nothing.
/// Fails, changing nothing, when the supervisor's host cannot be reached, and refuses beside an
/// agent in its pane flow cannot tell from the supervisor.
pub fn run(state: &StateDir, forget: bool) -> anyhow::Result<()> {
    let Some(lock) = supervisor::load(state)? else {
        return super::print("no supervisor recorded; nothing to stop\n");
    };
    if lock.stopped.is_some() {
        return super::print("the supervisor is already stopped; nothing to stop\n");
    }
    let tasks = Store::open(state).list_tasks()?;
    if forget {
        super::print(&format!(
            "left pane {} on {} as it is (--forget)\n",
            lock.herdr.pane_id, lock.herdr.host
        ))?;
    } else if let Some(host) = reachable_host(state, &lock)? {
        let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
        let sessions = Config::load(state)?.herdr_sessions;
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

/// Closes the supervisor's pane on `host`, wherever herdr shows it in `sessions`. Refuses
/// beside an unconfirmed agent in the lock's pane. When the supervisor runs nowhere, closes the
/// lock's pane only if herdr still lists it, in the lock's workspace, with no agent in it.
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
    let found = supervisor::find(runner, host, lock, sessions, tasks).map_err(unreachable)?;
    let (session, pane_id, what) = match found {
        Found::Supervisor(agent) => (agent.session, agent.pane_id, "the supervisor's pane"),
        Found::Unconfirmed => {
            return Err(Refused(format!(
                "{}. flow down stopped nothing and kept the lock. {}",
                supervisor::unconfirmed(lock),
                supervisor::UNCONFIRMED_WAY_OUT
            ))
            .into());
        }
        Found::Nowhere => {
            let herdr = Herdr::with_session(runner.clone(), &lock.herdr.session);
            if !left_empty(&herdr, host, lock).map_err(unreachable)? {
                return super::print(&format!(
                    "the supervisor runs in no pane on {}; closed nothing\n",
                    host.name
                ));
            }
            let target = &lock.herdr;
            (
                target.session.clone(),
                target.pane_id.clone(),
                "the supervisor's empty pane",
            )
        }
    };
    let herdr = Herdr::with_session(runner.clone(), &session);
    if herdr.close_pane(host, &pane_id).map_err(unreachable)? {
        super::print(&format!(
            "closed {what} {pane_id} on {} (herdr session {session})\n",
            host.name
        ))
    } else {
        super::print(&format!(
            "the supervisor runs in no pane on {}; closed nothing\n",
            host.name
        ))
    }
}

/// Whether herdr still lists the lock's pane in the lock's workspace with no agent in it.
fn left_empty(herdr: &Herdr, host: &Host, lock: &Lock) -> anyhow::Result<bool> {
    let target = &lock.herdr;
    let listed = herdr.panes_if_running(host)?.unwrap_or_default();
    if !listed
        .iter()
        .any(|pane| pane.pane_id == target.pane_id && pane.workspace_id == target.workspace_id)
    {
        return Ok(false);
    }
    let agents = herdr.agents_if_running(host)?.unwrap_or_default();
    Ok(!agents.iter().any(|agent| agent.pane_id == target.pane_id))
}
