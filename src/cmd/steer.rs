//! `flow steer`: sends the supervisor's message to a task's worker and logs it. Steering a task
//! that reported `needs-decision` or `blocked` hands it back to the worker: it reads `working`
//! again, and a hold on the supervisor is lifted, as when the worker reports `working` itself.

use std::sync::Arc;

use anyhow::anyhow;
use jiff::Timestamp;
use serde_json::json;

use crate::herdr::{AgentStatus, Herdr};
use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::{HoldOn, State};

const EVENT_SOURCE: &str = "supervisor";
/// The prompt counts as delivered once the worker starts on it, or blocks on it; waiting for
/// the worker's turn to end could hold the supervisor for hours.
const DELIVERED: [AgentStatus; 2] = [AgentStatus::Working, AgentStatus::Blocked];

/// Prompts the worker in the task's herdr pane with `text`, appends a `steer` event, moves a
/// `needs-decision` or `blocked` task back to `working`, and prints
/// `steered <id> (<pane>): <status>`.
pub fn run(state: &StateDir, id: &str, text: &str) -> anyhow::Result<()> {
    let store = Store::open(state);
    let (_, status) = store.task(id)?;
    let target = status
        .herdr
        .ok_or_else(|| anyhow!("task {id} has no herdr target"))?;
    let hosts = inventory::hosts(state)?;
    let host = host::find(&hosts, &target.host)?;
    let herdr = Herdr::new(Arc::from(runner::from_env()?));
    let reached = herdr.prompt(host, &target.pane_id, text, &DELIVERED)?;
    store.append_event(
        id,
        &Event {
            ts: Timestamp::now(),
            source: EVENT_SOURCE.to_owned(),
            kind: "steer".to_owned(),
            data: json!({ "text": text }),
        },
    )?;
    store.set_status(id, |status| {
        if matches!(status.state, State::NeedsDecision | State::Blocked) {
            status.state = State::Working;
            if status
                .hold
                .as_ref()
                .is_some_and(|hold| hold.on == HoldOn::Supervisor)
            {
                status.hold = None;
            }
        }
    })?;
    super::print(&format!(
        "steered {id} ({}): {}\n",
        target.pane_id,
        reached.as_str()
    ))
}
