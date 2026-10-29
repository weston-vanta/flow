//! `flow task close`: the supervisor retires a task's worker pane by closing its herdr workspace,
//! only while herdr lists it under a label naming the task and its pane holds the task's worker
//! or nothing. An adopted task's workspace is the user's, so it is left open.

use std::sync::Arc;

use jiff::Timestamp;
use serde_json::{Value, json};

use super::Refused;
use crate::herdr::Herdr;
use crate::host;
use crate::inventory;
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::runner;
use crate::state::Store;
use crate::state::config::Host;
use crate::state::events::Event;
use crate::state::task::{HerdrTarget, Origin, Status, TaskRecord};
use crate::supervisor::{self, Lock};

const EVENT_SOURCE: &str = "supervisor";

/// Closes the herdr workspace of task `id`, appends a `close` event and clears the task's
/// herdr target, then prints `closed <id> (workspace <ws> on <host>)`, or `closed <id>
/// (workspace already gone)` when herdr no longer knows the workspace. An adopted task's
/// workspace is left open, its event marked `workspace_left`, and it prints `closed <id> (pane
/// left to its owner)`. Refuses a task still open (anything but `done` or `orphaned`) unless
/// `force`, one with no herdr target, and one whose workspace herdr does not show to be the
/// task's.
pub fn run(state: &StateDir, id: &str, force: bool) -> anyhow::Result<()> {
    let store = Store::open(state);
    let (record, status) = store.task(id)?;
    let adopted = record.origin == Origin::Adopted;
    if status.state.is_open() && !force {
        let consequence = if adopted {
            "closing it stops flow watching its pane as the task's"
        } else {
            "closing its pane ends the worker's session"
        };
        return Err(Refused(format!(
            "task {id} is {}, still open; its worker may be running or waiting on its hold, and \
             {consequence} (pass --force to close it anyway)",
            status.state
        ))
        .into());
    }
    let Some(target) = status.herdr else {
        return Err(Refused(format!("task {id} has no herdr workspace to close")).into());
    };
    let mut data = json!({
        "host": target.host,
        "workspace_id": target.workspace_id,
        "pane_id": target.pane_id,
        "forced": force,
    });
    let printed = if adopted {
        data["session"] = Value::String(target.session.clone());
        data["workspace_left"] = Value::Bool(true);
        format!("closed {id} (pane left to its owner)\n")
    } else {
        let hosts = inventory::hosts(state)?;
        let host = host::find(&hosts, &target.host)?;
        let herdr = Herdr::with_session(Arc::from(runner::from_env()?), &target.session);
        let lock = supervisor::running(state)?;
        let tasks = store.list_tasks()?;
        let closed = close_task_workspace(&herdr, host, &target, id, lock.as_ref(), &tasks)?;
        data["already_gone"] = Value::Bool(!closed);
        if closed {
            format!(
                "closed {id} (workspace {} on {})\n",
                target.workspace_id, target.host
            )
        } else {
            format!("closed {id} (workspace already gone)\n")
        }
    };
    store.set_status(id, |status| status.herdr = None)?;
    store.append_event(
        id,
        &Event {
            ts: Timestamp::now(),
            source: EVENT_SOURCE.to_owned(),
            kind: "close".to_owned(),
            data,
        },
    )?;
    super::print(&printed)
}

/// Closes `target`'s workspace once herdr shows it is task `id`'s: listed under a label naming
/// the task, with the task's worker (see `owner::owner`, given the running supervisor's `lock`
/// and `tasks`) or no agent in the task's pane. Returns `false` when herdr no longer lists it.
/// Refuses, closing nothing, otherwise.
fn close_task_workspace(
    herdr: &Herdr,
    host: &Host,
    target: &HerdrTarget,
    id: &str,
    lock: Option<&Lock>,
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<bool> {
    let Some(workspaces) = herdr.workspaces_if_running(host)? else {
        return Ok(false);
    };
    let Some(workspace) = workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == target.workspace_id)
    else {
        return Ok(false);
    };
    if !workspace.names_task(id) {
        return Err(Refused(format!(
            "herdr lists workspace {} on {} as \"{}\", which does not name task {id}; herdr \
             reuses workspace ids, so flow closed nothing",
            target.workspace_id, host.name, workspace.label
        ))
        .into());
    }
    let task = Owner::Task(id.to_owned());
    let other = herdr.agents(host)?.iter().any(|agent| {
        agent.pane_id == target.pane_id && owner::owner(&host.name, agent, lock, tasks) != task
    });
    if other {
        return Err(Refused(format!(
            "task {id}'s pane {} on {} runs another agent than the task's worker; herdr reuses \
             pane ids, so flow closed nothing",
            target.pane_id, host.name
        ))
        .into());
    }
    herdr.close_workspace(host, &target.workspace_id)
}
