//! `flow task close`: the supervisor retires a task's worker pane by closing its herdr
//! workspace. The task record, its working directory and any branch stay as they are. An
//! adopted task's workspace belongs to the user who started it by hand, so closing such a task
//! only stops flow treating the pane as the task's: the workspace is left open.

use std::sync::Arc;

use jiff::Timestamp;
use serde_json::{Value, json};

use super::Refused;
use crate::herdr::Herdr;
use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::Origin;

const EVENT_SOURCE: &str = "supervisor";

/// Closes the herdr workspace of task `id`, appends a `close` event and clears the task's
/// herdr target, then prints `closed <id> (workspace <ws> on <host>)`, or `closed <id>
/// (workspace already gone)` when herdr no longer knows the workspace. An adopted task's
/// workspace is left open, its event marked `workspace_left`, and it prints `closed <id> (pane
/// left to its owner)`. Refuses a task still open (anything but `done` or `orphaned`) unless
/// `force`, and one with no herdr target.
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
        data["workspace_left"] = Value::Bool(true);
        format!("closed {id} (pane left to its owner)\n")
    } else {
        let hosts = inventory::hosts(state)?;
        let host = host::find(&hosts, &target.host)?;
        let herdr = Herdr::with_session(Arc::from(runner::from_env()?), &target.session);
        let closed = herdr.close_workspace(host, &target.workspace_id)?;
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
