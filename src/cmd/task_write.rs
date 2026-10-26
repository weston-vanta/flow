//! Commands that report or change a task's state. Workers call `status`, `done`, and
//! `request-dispatch`; the supervisor calls `decide`, `hold`, and `orphan`.

use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::paths::StateDir;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::requests::Request;
use crate::state::task::{Decision, Hold, HoldOn, State};

const EVENT_SOURCE: &str = "cli";

/// The states a worker reports with `flow task status`. `done` and `orphaned` have their own
/// commands, and `pending` is only a new task's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedState {
    Working,
    Blocked,
    NeedsDecision,
}

impl ReportedState {
    fn state(self) -> State {
        match self {
            ReportedState::Working => State::Working,
            ReportedState::Blocked => State::Blocked,
            ReportedState::NeedsDecision => State::NeedsDecision,
        }
    }
}

impl FromStr for ReportedState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "working" => Ok(ReportedState::Working),
            "blocked" => Ok(ReportedState::Blocked),
            "needs-decision" => Ok(ReportedState::NeedsDecision),
            _ => Err("expected working, blocked, or needs-decision \
                 (finish with `flow task done`, abandon with `flow task orphan`)"
                .to_owned()),
        }
    }
}

/// Moves the task to `reported`. `blocked` and `needs-decision` put a hold on the supervisor
/// carrying `message`; `working` lifts a hold on the supervisor but keeps one on the user, which
/// only `flow task decide` answers.
pub fn run_status(
    state: &StateDir,
    id: &str,
    reported: ReportedState,
    message: Option<&str>,
) -> anyhow::Result<()> {
    let store = Store::open(state);
    let hold = match reported {
        ReportedState::Working => None,
        ReportedState::Blocked | ReportedState::NeedsDecision => {
            let Some(what) = message else {
                bail!("--message is required when reporting {}", reported.state());
            };
            Some(Hold {
                on: HoldOn::Supervisor,
                what: what.to_owned(),
                since: Timestamp::now(),
            })
        }
    };
    store.set_status(id, |status| {
        status.state = reported.state();
        match hold {
            Some(hold) => status.hold = Some(hold),
            None => {
                if status
                    .hold
                    .as_ref()
                    .is_some_and(|h| h.on == HoldOn::Supervisor)
                {
                    status.hold = None;
                }
            }
        }
    })?;
    append_event(&store, id, "status.report", json!({ "message": message }))
}

/// Copies `report` into the task as `report.md` and marks the task done.
pub fn run_done(state: &StateDir, id: &str, report: &Path) -> anyhow::Result<()> {
    let store = Store::open(state);
    let body = std::fs::read(report).with_context(|| format!("reading {}", report.display()))?;
    let path = store.write_report(id, &body)?;
    store.set_status(id, |status| {
        status.state = State::Done;
        status.hold = None;
    })?;
    append_event(&store, id, "report", json!({ "path": path }))
}

/// Files a request for the supervisor to dispatch sibling work, and prints the request id.
pub fn run_request_dispatch(
    state: &StateDir,
    id: &str,
    title: &str,
    brief: &Path,
    cde: Option<&str>,
) -> anyhow::Result<()> {
    if !brief.is_file() {
        bail!("brief not found: {}", brief.display());
    }
    let brief_path =
        std::path::absolute(brief).with_context(|| format!("resolving {}", brief.display()))?;
    let store = Store::open(state);
    let req = Request {
        title: title.to_owned(),
        brief_path,
        cde: cde.map(str::to_owned),
        requested_by: id.to_owned(),
    };
    let req_id = store.add_request(id, &req)?;
    append_event(&store, id, "request.dispatch", serde_json::to_value(&req)?)?;
    super::print(&format!("{req_id}\n"))
}

/// Records `answer` to the task's hold (if any) in `decisions.jsonl` and lifts the hold.
pub fn run_decide(state: &StateDir, id: &str, by: HoldOn, answer: &str) -> anyhow::Result<()> {
    let store = Store::open(state);
    let mut question = None;
    store.set_status(id, |status| {
        question = status.hold.take().map(|hold| hold.what);
    })?;
    let decision = Decision {
        ts: Timestamp::now(),
        by,
        answer: answer.to_owned(),
        question,
    };
    store.append_decision(id, &decision)?;
    let data = json!({
        "by": decision.by,
        "answer": decision.answer,
        "question": decision.question,
    });
    append_event(&store, id, "decision", data)
}

/// Puts the task on hold until `on` answers `what`, replacing any existing hold.
pub fn run_hold(state: &StateDir, id: &str, on: HoldOn, what: &str) -> anyhow::Result<()> {
    let store = Store::open(state);
    store.set_status(id, |status| {
        status.hold = Some(Hold {
            on,
            what: what.to_owned(),
            since: Timestamp::now(),
        });
    })?;
    append_event(&store, id, "hold.set", json!({ "on": on, "what": what }))
}

/// Marks the task orphaned: its work will not finish, for `reason`.
pub fn run_orphan(state: &StateDir, id: &str, reason: &str) -> anyhow::Result<()> {
    let store = Store::open(state);
    store.set_status(id, |status| {
        status.state = State::Orphaned;
        status.hold = None;
    })?;
    append_event(&store, id, "orphaned", json!({ "reason": reason }))
}

fn append_event(store: &Store, id: &str, kind: &str, data: impl Serialize) -> anyhow::Result<()> {
    let ev = Event {
        ts: Timestamp::now(),
        source: EVENT_SOURCE.to_owned(),
        kind: kind.to_owned(),
        data: serde_json::to_value(data)?,
    };
    store.append_event(id, &ev)
}
