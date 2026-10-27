//! `flow task close`: the supervisor retires a task's worker pane by closing its herdr
//! workspace. The task record, its working directory and any branch stay as they are.

use std::sync::Arc;

use jiff::Timestamp;
use serde_json::json;

use super::Refused;
use crate::herdr::Herdr;
use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::State;

const EVENT_SOURCE: &str = "supervisor";

/// Closes the herdr workspace of task `id`, appends a `close` event and clears the task's
/// herdr target, then prints `closed <id> (workspace <ws> on <host>)`, or `closed <id>
/// (workspace already gone)` when herdr no longer knows the workspace. Refuses a task still
/// `working` or `pending` unless `force`, and one with no herdr target.
pub fn run(state: &StateDir, id: &str, force: bool) -> anyhow::Result<()> {
    let store = Store::open(state);
    let (_, status) = store.task(id)?;
    if matches!(status.state, State::Working | State::Pending) && !force {
        return Err(Refused(format!(
            "task {id} is {}; closing its pane ends the worker's session (pass --force to close \
             it anyway)",
            status.state
        ))
        .into());
    }
    let Some(target) = status.herdr else {
        return Err(Refused(format!("task {id} has no herdr workspace to close")).into());
    };
    let hosts = inventory::hosts(state)?;
    let host = host::find(&hosts, &target.host)?;
    let herdr = Herdr::new(Arc::from(runner::from_env()?));
    let closed = herdr.close_workspace(host, &target.workspace_id)?;
    store.set_status(id, |status| status.herdr = None)?;
    store.append_event(
        id,
        &Event {
            ts: Timestamp::now(),
            source: EVENT_SOURCE.to_owned(),
            kind: "close".to_owned(),
            data: json!({
                "host": target.host,
                "workspace_id": target.workspace_id,
                "pane_id": target.pane_id,
                "forced": force,
                "already_gone": !closed,
            }),
        },
    )?;
    if !closed {
        return super::print(&format!("closed {id} (workspace already gone)\n"));
    }
    super::print(&format!(
        "closed {id} (workspace {} on {})\n",
        target.workspace_id, target.host
    ))
}
