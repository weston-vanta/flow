use std::fmt;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr;

/// `task.json`: what a task is. Written once at creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub title: String,
    pub host: String,
    /// The absolute directory on `host` the worker runs in. Records written before tasks had
    /// working directories call it `worktree`.
    #[serde(alias = "worktree")]
    pub cwd: PathBuf,
    /// The configured repo `cwd` is a worktree of, when it was created from `--branch`.
    #[serde(default)]
    pub repo: Option<String>,
    pub harness: Harness,
    pub created: Timestamp,
    pub owner: String,
    /// How the task's session began. Records written before adoption existed were dispatched.
    #[serde(default)]
    pub origin: Origin,
}

/// How a task's session began: started by `flow dispatch`, or started by hand and taken over
/// with `flow task adopt`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    #[default]
    Dispatched,
    Adopted,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Origin::Dispatched => "dispatched",
            Origin::Adopted => "adopted",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Claude,
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Harness::Claude => "claude",
        })
    }
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
    #[serde(default)]
    pub claude_session: Option<ClaudeSession>,
}

impl Status {
    pub fn pending(now: Timestamp) -> Status {
        Status {
            state: State::Pending,
            last_event: now,
            hold: None,
            herdr: None,
            claude_session: None,
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

impl State {
    /// Whether the task still needs attention: not `done` and not `orphaned`.
    pub fn is_open(self) -> bool {
        !matches!(self, State::Done | State::Orphaned)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Pending => "pending",
            State::Working => "working",
            State::Blocked => "blocked",
            State::NeedsDecision => "needs-decision",
            State::Done => "done",
            State::Orphaned => "orphaned",
        })
    }
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

impl fmt::Display for HoldOn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HoldOn::User => "user",
            HoldOn::Supervisor => "supervisor",
        })
    }
}

/// One line of `decisions.jsonl`: an answer to the task's hold, or a standalone ruling when the
/// task had none (`question` is null).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub ts: Timestamp,
    pub by: HoldOn,
    pub answer: String,
    pub question: Option<String>,
}

/// The herdr pane running the task's agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrTarget {
    pub host: String,
    pub workspace_id: String,
    pub pane_id: String,
    pub agent: String,
    /// The herdr session the pane lives in: flow's own, as configured when the worker was
    /// dispatched, or the one an adopted session was started in. Targets written before
    /// sessions were recorded were all in flow's legacy session.
    #[serde(default = "legacy_session")]
    pub session: String,
}

fn legacy_session() -> String {
    herdr::LEGACY_SESSION.to_owned()
}

impl HerdrTarget {
    /// Whether this names the pane `pane_id` in herdr session `session` on `host`.
    pub fn is(&self, host: &str, session: &str, pane_id: &str) -> bool {
        self.host == host && self.session == session && self.pane_id == pane_id
    }
}

/// The Claude Code session a worker runs in, as its `SessionStart` hook reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeSession {
    pub session_id: String,
    pub transcript_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_serialize_and_display_kebab_case() {
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
        let displayed: Vec<_> = states.iter().map(|s| format!("\"{s}\"")).collect();
        assert_eq!(format!("[{}]", displayed.join(",")), json);
    }

    #[test]
    fn records_written_with_a_worktree_still_load() {
        let rec: TaskRecord = serde_json::from_str(
            r#"{"id": "aaaaaaab", "title": "Old task", "host": "host-a",
                "worktree": "/work/repo-worktrees/old", "harness": "claude",
                "created": "2026-10-26T12:00:00Z", "owner": "supervisor"}"#,
        )
        .unwrap();
        assert_eq!(rec.cwd, std::path::Path::new("/work/repo-worktrees/old"));
        assert_eq!(rec.repo, None);
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["cwd"], "/work/repo-worktrees/old");
        assert!(json.get("worktree").is_none(), "{json}");
    }

    #[test]
    fn records_and_targets_written_before_adoption_read_as_dispatched_in_flows_session() {
        let rec: TaskRecord = serde_json::from_str(
            r#"{"id": "aaaaaaab", "title": "Old task", "host": "host-a",
                "cwd": "/work/repo.wt/old", "harness": "claude",
                "created": "2026-10-26T12:00:00Z", "owner": "supervisor"}"#,
        )
        .unwrap();
        assert_eq!(rec.origin, Origin::Dispatched);
        assert_eq!(serde_json::to_value(&rec).unwrap()["origin"], "dispatched");
        let target: HerdrTarget = serde_json::from_str(
            r#"{"host": "host-a", "workspace_id": "w1", "pane_id": "w1:p1", "agent": "claude"}"#,
        )
        .unwrap();
        assert_eq!(target.session, herdr::LEGACY_SESSION);
        let adopted: Origin = serde_json::from_str(r#""adopted""#).unwrap();
        assert_eq!(adopted, Origin::Adopted);
    }

    #[test]
    fn origin_displays_as_serialized() {
        for origin in [Origin::Dispatched, Origin::Adopted] {
            assert_eq!(
                serde_json::to_string(&origin).unwrap(),
                format!("\"{origin}\"")
            );
        }
    }

    #[test]
    fn hold_on_displays_as_serialized() {
        for on in [HoldOn::User, HoldOn::Supervisor] {
            assert_eq!(serde_json::to_string(&on).unwrap(), format!("\"{on}\""));
        }
    }
}
