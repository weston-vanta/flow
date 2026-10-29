//! `flow down`: stops the supervisor and leaves every worker and task as it is. It closes the
//! supervisor's herdr workspace where its lock says it runs, removes the lock, and clears the
//! watch lock, stopping a watcher still running on this machine; a watcher on another host ends
//! on its own at its next cycle once its lock is gone. The next `flow up` starts a new
//! supervisor. When the supervisor ran in a herdr session other than the configured one and
//! that session has no agents left, it offers to stop that session's server; it never stops
//! the configured session's.

use std::process::{Command, ExitCode};
use std::sync::Arc;

use anyhow::Context;

use super::Consent;
use crate::herdr::{self, Herdr};
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::supervisor::{self, Lock};
use crate::watch::lock as watch_lock;

/// Stops the supervisor `state/supervisor/lock.json` records, printing a line per step and how
/// many open tasks keep running. With `yes`, stops an emptied herdr session's server without
/// asking. Fails, changing nothing, when the supervisor's host cannot be reached, since the
/// supervisor may be alive there.
pub fn run(state: &StateDir, yes: bool) -> anyhow::Result<ExitCode> {
    let Some(lock) = supervisor::load(state)? else {
        super::print("no supervisor recorded; nothing to stop\n")?;
        return Ok(ExitCode::SUCCESS);
    };
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let herdr = Herdr::with_session(runner, &lock.herdr.session);
    let host = reachable_host(state, &lock)?;
    if let Some(host) = &host {
        close_workspace(&herdr, host, &lock)?;
    }
    supervisor::remove(state)?;
    super::print("removed state/supervisor/lock.json\n")?;
    clear_watch_lock(state)?;
    if let Some(host) = &host
        && lock.herdr.session != config.herdr_session
    {
        offer_server_stop(&herdr, host, yes)?;
    }
    let open = Store::open(state)
        .list_tasks()?
        .iter()
        .filter(|(_, status)| status.state.is_open())
        .count();
    super::print(&format!(
        "{open} open tasks keep running; flow up starts a new supervisor\n"
    ))?;
    Ok(ExitCode::SUCCESS)
}

/// The host the supervisor runs on, or `None`, saying so, when it is unknown, gone, or not
/// running in Ona: the supervisor went with it.
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

/// Closes the supervisor's workspace on `host` when herdr lists it under the supervisor's
/// label, else says its pane is already gone.
fn close_workspace(herdr: &Herdr, host: &Host, lock: &Lock) -> anyhow::Result<()> {
    let unreachable = |err: anyhow::Error| {
        err.context(format!(
            "closing the supervisor's workspace on {}; nothing stopped, and the lock is kept",
            host.name
        ))
    };
    let workspaces = herdr.workspaces_if_running(host).map_err(unreachable)?;
    let listed = workspaces
        .unwrap_or_default()
        .iter()
        .any(|workspace| lock.names_workspace(workspace));
    let closed = listed
        && herdr
            .close_workspace(host, &lock.herdr.workspace_id)
            .map_err(unreachable)?;
    let line = if closed {
        format!(
            "closed the supervisor's workspace {} on {} (herdr session {})\n",
            lock.herdr.workspace_id,
            host.name,
            herdr.session()
        )
    } else {
        format!(
            "the supervisor's pane {} on {} is already gone\n",
            lock.herdr.pane_id, host.name
        )
    };
    super::print(&line)
}

/// Removes the watch lock, first stopping its holder when it runs on this machine.
fn clear_watch_lock(state: &StateDir) -> anyhow::Result<()> {
    let owner = watch_lock::owner(state)?;
    if let Some(owner) = owner.as_ref().filter(|owner| owner.is_running_here()) {
        let pid = owner.pid.to_string();
        let status = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .context("running kill")?;
        if status.success() {
            super::print(&format!("stopped the watcher (pid {pid})\n"))?;
        } else {
            eprintln!("could not stop the watcher (pid {pid}): kill exited {status}");
        }
    }
    if !watch_lock::clear(state)? {
        return Ok(());
    }
    match owner.filter(|owner| !owner.is_on_this_host()) {
        Some(owner) => super::print(&format!(
            "removed state/supervisor/watch.lock; the watcher on {} ends at its next cycle\n",
            owner.host
        )),
        None => super::print("removed state/supervisor/watch.lock\n"),
    }
}

/// Offers to stop `herdr`'s session's server on `host` when it has no agents left.
fn offer_server_stop(herdr: &Herdr, host: &Host, yes: bool) -> anyhow::Result<()> {
    let session = herdr.session();
    let agents = match herdr.agents_if_running(host) {
        Ok(Some(agents)) => agents,
        Ok(None) => return Ok(()),
        Err(err) => {
            eprintln!(
                "could not list herdr session {session} on {}: {err:#}",
                host.name
            );
            return Ok(());
        }
    };
    if !agents.is_empty() {
        return Ok(());
    }
    super::print(&format!(
        "Herdr:\n - stop the server of herdr session {session} on {}, which has no agents \
         left.\n",
        host.name
    ))?;
    let left = format!(
        "left herdr session {session}'s server running on {}",
        host.name
    );
    match super::confirm(yes)? {
        Consent::Given => {
            let output = herdr.stop_server(host)?;
            super::print(&output.stdout)?;
            eprint!("{}", output.stderr);
            super::print(&format!(
                "stopped herdr session {session}'s server on {}\n",
                host.name
            ))
        }
        Consent::Declined => super::print(&format!("{left}\n")),
        Consent::NoTerminal => super::print(&format!(
            "{left}; to stop it, run there: {}\n",
            herdr::stop_server_argv(session).join(" ")
        )),
    }
}
