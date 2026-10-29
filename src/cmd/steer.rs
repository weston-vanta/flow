//! `flow steer`: sends the supervisor's message to a task's worker and logs it. Steering a task
//! that reported `needs-decision` or `blocked` hands it back to the worker: it reads `working`
//! again, and a hold on the supervisor is lifted, as when the worker reports `working` itself.
//! herdr reuses pane ids, so it prompts only an agent `owner::owner` gives the task.

use std::sync::Arc;

use anyhow::anyhow;
use jiff::Timestamp;
use serde_json::json;

use super::Refused;
use crate::herdr::{AgentStatus, Herdr};
use crate::host;
use crate::inventory;
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::runner;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::{HoldOn, State};
use crate::supervisor;

pub(super) const EVENT_SOURCE: &str = "supervisor";
/// The prompt counts as delivered once the worker starts on it, or blocks on it; waiting for
/// the worker's turn to end could hold the supervisor for hours.
pub(super) const DELIVERED: [AgentStatus; 2] = [AgentStatus::Working, AgentStatus::Blocked];

/// Prompts the worker in the task's herdr pane, in the herdr session it lives in, with `text`,
/// appends a `steer` event, moves a `needs-decision` or `blocked` task back to `working`, and
/// prints `steered <id> (<pane>): <status>`. Refuses, sending nothing, when the pane holds no
/// agent of the task's.
pub fn run(state: &StateDir, id: &str, text: &str) -> anyhow::Result<()> {
    let store = Store::open(state);
    let (_, status) = store.task(id)?;
    let target = status
        .herdr
        .ok_or_else(|| anyhow!("task {id} has no herdr target"))?;
    let hosts = inventory::hosts(state)?;
    let host = host::find(&hosts, &target.host)?;
    let herdr = Herdr::with_session(Arc::from(runner::from_env()?), &target.session);
    let tasks = store.list_tasks()?;
    let lock = supervisor::running(state)?;
    let owner = herdr
        .agents(host)?
        .iter()
        .find(|agent| agent.pane_id == target.pane_id)
        .map(|agent| owner::owner(&target.host, agent, lock.as_ref(), &tasks));
    if owner != Some(Owner::Task(id.to_owned())) {
        let holds = match owner {
            None => "no agent".to_owned(),
            Some(Owner::Supervisor) => "the supervisor".to_owned(),
            Some(Owner::Task(other)) => format!("task {other}'s worker"),
            Some(Owner::Unconfirmed) => "an agent flow cannot tell from the supervisor".to_owned(),
            Some(Owner::Observed) => {
                "another agent than the task's worker, since herdr reuses pane ids".to_owned()
            }
        };
        return Err(Refused(format!(
            "task {id}'s pane {} on {} holds {holds}; flow sent nothing",
            target.pane_id, target.host
        ))
        .into());
    }
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
