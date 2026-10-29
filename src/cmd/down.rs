//! `flow down`: stops the supervisor, leaving every worker and task running. It closes the
//! supervisor's pane, marks the lock stopped keeping the Claude Code session to resume, and
//! removes the watch lock, which ends the watcher at its next cycle.

use std::process::ExitCode;
use std::sync::Arc;

use super::Refused;
use crate::herdr::Herdr;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::Host;
use crate::state::task::{Status, TaskRecord};
use crate::supervisor::{self, Identity, Lock};
use crate::watch::lock as watch_lock;

/// Stops the supervisor `state/supervisor/lock.json` records. With `forget`, closes nothing.
/// Fails, changing nothing, when the supervisor's host cannot be reached, and refuses beside an
/// agent in its pane flow cannot tell from the supervisor.
pub fn run(state: &StateDir, forget: bool) -> anyhow::Result<ExitCode> {
    let Some(lock) = supervisor::load(state)? else {
        super::print("no supervisor recorded; nothing to stop\n")?;
        return Ok(ExitCode::SUCCESS);
    };
    if lock.stopped.is_some() {
        super::print("the supervisor is already stopped; nothing to stop\n")?;
        return Ok(ExitCode::SUCCESS);
    }
    let tasks = Store::open(state).list_tasks()?;
    if forget {
        super::print(&format!(
            "left pane {} on {} as it is (--forget)\n",
            lock.herdr.pane_id, lock.herdr.host
        ))?;
    } else if let Some(host) = reachable_host(state, &lock)? {
        let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
        let herdr = Herdr::with_session(runner, &lock.herdr.session);
        close_supervisor(&herdr, &host, &lock, &tasks)?;
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
    ))?;
    Ok(ExitCode::SUCCESS)
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

/// Closes the supervisor's pane on `host`. An agent there that is clearly another's is left to
/// its owner; one flow cannot tell from the supervisor is refused.
fn close_supervisor(
    herdr: &Herdr,
    host: &Host,
    lock: &Lock,
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<()> {
    let pane = &lock.herdr.pane_id;
    let gone = || {
        super::print(&format!(
            "the supervisor's pane {pane} on {} is already gone\n",
            host.name
        ))
    };
    let unreachable = |err: anyhow::Error| {
        err.context(format!(
            "closing the supervisor's pane on {}; nothing stopped, and the lock is kept",
            host.name
        ))
    };
    let agents = herdr
        .agents_if_running(host)
        .map_err(unreachable)?
        .unwrap_or_default();
    let Some(agent) = agents.iter().find(|agent| agent.pane_id == *pane) else {
        return gone();
    };
    match lock.identify(&host.name, agent, tasks) {
        Identity::Supervisor => {}
        Identity::Unconfirmed => {
            return Err(Refused(format!(
                "{}. flow down stopped nothing and kept the lock. {}",
                supervisor::unconfirmed(lock, &host.name, agent),
                supervisor::UNCONFIRMED_WAY_OUT
            ))
            .into());
        }
        Identity::Other => {
            return super::print(&format!(
                "the supervisor is gone from pane {pane} on {}, which now runs {}; flow down \
                 leaves that pane to its owner\n",
                host.name,
                supervisor::not_the_supervisor(lock, &host.name, agent, tasks)
            ));
        }
    }
    if !herdr.close_pane(host, pane).map_err(unreachable)? {
        return gone();
    }
    super::print(&format!(
        "closed the supervisor's pane {pane} on {} (herdr session {})\n",
        host.name,
        herdr.session()
    ))
}
