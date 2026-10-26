use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// `task.json`: what a task is. Written once at creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub title: String,
    pub cde: String,
    pub worktree: PathBuf,
    pub harness: Harness,
    pub created: Timestamp,
    pub owner: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Claude,
}

/// `status.json`: where a task is now. Rewritten under lock by `Store::set_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub last_event: Timestamp,
    #[serde(default)]
    pub hold: Option<Hold>,
    #[serde(default)]
    pub herdr: Option<HerdrTarget>,
}

impl Status {
    pub fn pending(now: Timestamp) -> Status {
        Status {
            state: State::Pending,
            last_event: now,
            hold: None,
            herdr: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Pending,
    Working,
    Blocked,
    NeedsDecision,
    Done,
    Orphaned,
}

/// Work on the task waits until `on` answers `what`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub on: HoldOn,
    pub what: String,
    pub since: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HoldOn {
    User,
    Supervisor,
}

/// The herdr pane running the task's agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrTarget {
    pub cde: String,
    pub workspace_id: String,
    pub pane_id: String,
    pub agent: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_serialize_kebab_case() {
        let states = [
            State::Pending,
            State::Working,
            State::Blocked,
            State::NeedsDecision,
            State::Done,
            State::Orphaned,
        ];
        let json = serde_json::to_string(&states).unwrap();
        assert_eq!(
            json,
            r#"["pending","working","blocked","needs-decision","done","orphaned"]"#
        );
    }
}
