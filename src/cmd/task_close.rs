//! `flow task close`: the supervisor retires a task's worker pane by closing its herdr tab, only
//! while herdr lists it under a label naming the task and its pane holds the task's worker or
//! nothing; under another label herdr reused the id. An adopted task's tab is left open.

use std::sync::Arc;

use jiff::Timestamp;
use serde_json::{Value, json};

use super::Refused;
use crate::herdr::{self, Herdr};
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

/// Closes the herdr tab of task `id`, appends a `close` event and clears the task's herdr
/// target, then prints `closed <id> (tab <tab> on <host>)`, or `closed <id> (tab already gone)`
/// when herdr no longer knows the tab. herdr closes the `flow` workspace with its last tab. An
/// adopted task's tab is left open, its event marked `workspace_left`, and it prints `closed <id>
/// (pane left to its owner)`. A tab herdr lists under another label is left open and the target
/// retired. Refuses a task still open (anything but `done` or `orphaned`) unless `force`, one
/// with no herdr target, and one whose pane holds another agent.
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
        return Err(Refused(format!("task {id} has no herdr tab to close")).into());
    };
    let mut data = json!({
        "host": target.host,
        "workspace_id": target.workspace_id,
        "tab_id": target.tab_id,
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
        let lock = supervisor::load(state)?;
        let tasks = store.list_tasks()?;
        match close_task_tab(&herdr, host, &target, id, lock.as_ref(), &tasks)? {
            Closing::Closed => {
                data["already_gone"] = Value::Bool(false);
                format!("closed {id} (tab {} on {})\n", target.tab_id, target.host)
            }
            Closing::Gone => {
                data["already_gone"] = Value::Bool(true);
                format!("closed {id} (tab already gone)\n")
            }
            Closing::Reused(why) => {
                owner::retire_target(&store, id, &target, &why)?;
                data["reused"] = Value::String(why.clone());
                format!("closed {id} ({why}; closed nothing)\n")
            }
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

/// How closing a task's tab ended.
#[derive(Debug, PartialEq, Eq)]
enum Closing {
    Closed,
    /// herdr no longer lists the tab.
    Gone,
    /// herdr lists the tab under another label, why: it reused the id.
    Reused(String),
}

/// Closes `target`'s tab once herdr lists it under a label naming task `id`, with the task's
/// worker (see `owner::owner`, given `lock` and `tasks`) or no agent in the task's pane. Refuses,
/// closing nothing, when another agent is there.
fn close_task_tab(
    herdr: &Herdr,
    host: &Host,
    target: &HerdrTarget,
    id: &str,
    lock: Option<&Lock>,
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<Closing> {
    let Some(tabs) = herdr.tabs_if_running(host)? else {
        return Ok(Closing::Gone);
    };
    let Some(tab) = tabs.iter().find(|tab| tab.tab_id == target.tab_id) else {
        return Ok(Closing::Gone);
    };
    if !herdr::label_names_task(&tab.label, id) {
        return Ok(Closing::Reused(format!(
            "herdr reused tab {} on {}, now labelled \"{}\"",
            target.tab_id, host.name, tab.label
        )));
    }
    let task = Owner::Task(id.to_owned());
    let mut agents = herdr.agents(host)?;
    herdr::label_agents(&tabs, &mut agents);
    let other = agents.iter().any(|agent| {
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
    Ok(if herdr.close_tab(host, &target.tab_id)? {
        Closing::Closed
    } else {
        Closing::Gone
    })
}
