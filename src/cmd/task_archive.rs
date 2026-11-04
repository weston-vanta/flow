//! `flow task archive`: the supervisor takes finished or orphaned tasks out of flow's view,
//! moving each task's directory to `state/archive/tasks/<id>/`, or deleting it with `--delete`.
//! The task's working directory and branch on its host are never touched.

use jiff::Timestamp;
use serde_json::json;

use super::Refused;
use crate::inventory;
use crate::paths::StateDir;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::{Status, TaskRecord};

const EVENT_SOURCE: &str = "supervisor";

/// Archives, or with `delete` deletes, every task in `ids`, printing one line per task naming
/// the working directory left on its host. Checks every task first and changes nothing when
/// any is refused: one still open (anything but `done` or `orphaned`), one whose herdr target
/// is on a host flow still knows as live (`flow task close` it first), or one with dispatch
/// requests not yet handled.
pub fn run(state: &StateDir, ids: &[String], delete: bool) -> anyhow::Result<()> {
    let store = Store::open(state);
    let live = inventory::live_hosts(state)?;
    let mut tasks = Vec::new();
    let mut refusals = Vec::new();
    for id in ids {
        let (record, status) = store.task(id)?;
        let pending = store.pending_request_ids(id)?;
        let live_target = |host: &str| live.iter().any(|live| live.name == host);
        match refusal(&record, &status, &pending, live_target) {
            Some(why) => refusals.push(why),
            None => tasks.push(record),
        }
    }
    if !refusals.is_empty() {
        let changed = if ids.len() > 1 {
            "; no task was changed"
        } else {
            ""
        };
        return Err(Refused(format!("{}{changed}", refusals.join("\n"))).into());
    }
    let mut out = String::new();
    for record in tasks {
        let id = &record.id;
        let left = format!(
            "its working directory {} on {} is left as it was",
            record.cwd.display(),
            record.host
        );
        if delete {
            store.delete_task(id)?;
            out.push_str(&format!("deleted {id}; {left}\n"));
        } else {
            store.append_event(
                id,
                &Event {
                    ts: Timestamp::now(),
                    source: EVENT_SOURCE.to_owned(),
                    kind: "archive".to_owned(),
                    data: json!({}),
                },
            )?;
            let path = store.archive_task(id)?;
            out.push_str(&format!("archived {id} to {}; {left}\n", path.display()));
        }
    }
    super::print(&out)
}

/// Why the task cannot be archived, or `None` when it can. `live_host` says whether flow knows
/// a host of that name that is not gone.
fn refusal(
    record: &TaskRecord,
    status: &Status,
    pending_requests: &[String],
    live_host: impl Fn(&str) -> bool,
) -> Option<String> {
    let id = &record.id;
    if status.state.is_open() {
        return Some(format!(
            "task {id} is {}, still open; `flow task orphan` it first if its work will not finish",
            status.state
        ));
    }
    if let Some(target) = &status.herdr
        && live_host(&target.host)
    {
        return Some(format!(
            "task {id} still has herdr pane {} on {}; `flow task close` it first",
            target.pane_id, target.host
        ));
    }
    if !pending_requests.is_empty() {
        return Some(format!(
            "task {id} has dispatch requests not yet handled: {}",
            pending_requests.join(", ")
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::task::{Harness, HerdrTarget, Origin, State};

    fn task(state: State, target_host: Option<&str>) -> (TaskRecord, Status) {
        let now = Timestamp::now();
        let record = TaskRecord {
            id: "aaaaaaab".to_owned(),
            parent: None,
            title: "a task".to_owned(),
            host: "host-a".to_owned(),
            cwd: PathBuf::from("/work/repo"),
            repo: None,
            harness: Harness::Claude,
            created: now,
            owner: "supervisor".to_owned(),
            origin: Origin::Dispatched,
        };
        let mut status = Status::pending(now);
        status.state = state;
        status.herdr = target_host.map(|host| HerdrTarget {
            host: host.to_owned(),
            workspace_id: "w1".to_owned(),
            tab_id: "w1:t1".to_owned(),
            pane_id: "w1:p1".to_owned(),
            agent: "claude".to_owned(),
            session: "flow".to_owned(),
        });
        (record, status)
    }

    fn live(host: &str) -> bool {
        host == "host-a"
    }

    #[test]
    fn done_and_orphaned_tasks_without_a_live_pane_can_be_archived() {
        for state in [State::Done, State::Orphaned] {
            let (record, status) = task(state, None);
            assert_eq!(refusal(&record, &status, &[], live), None);
        }
    }

    #[test]
    fn open_tasks_are_refused() {
        for state in [
            State::Pending,
            State::Working,
            State::Blocked,
            State::NeedsDecision,
        ] {
            let (record, status) = task(state, None);
            let why = refusal(&record, &status, &[], live).unwrap();
            assert!(why.contains("still open"), "{why}");
        }
    }

    #[test]
    fn a_pane_on_a_live_host_is_refused_and_one_on_a_gone_host_is_not() {
        let (record, status) = task(State::Done, Some("host-a"));
        let why = refusal(&record, &status, &[], live).unwrap();
        assert!(why.contains("flow task close"), "{why}");

        let (record, status) = task(State::Orphaned, Some("er"));
        assert_eq!(refusal(&record, &status, &[], live), None);
    }

    #[test]
    fn pending_requests_are_refused() {
        let (record, status) = task(State::Done, None);
        let why = refusal(&record, &status, &["aaaaaaaz".to_owned()], live).unwrap();
        assert!(why.contains("aaaaaaaz"), "{why}");
    }
}
