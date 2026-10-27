//! What `flow watch` makes of what it sees: each observation either wakes the supervisor with a
//! one-line reason or is absorbed. `Reported` remembers what earlier wakes already said, so a
//! condition that persists wakes the supervisor once rather than on every turn end.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::adopt::Followed;
use crate::herdr::AgentStatus;
use crate::state::task::State;

/// The default `Thresholds::quiet_secs`: half an hour without a task event.
pub const DEFAULT_QUIET_SECS: u64 = 1800;
/// The event kind `flow task request-dispatch` appends.
const REQUEST_DISPATCH: &str = "request.dispatch";
/// What `Reported` records for a pane herdr no longer knows.
const GONE: &str = "gone";

/// One thing `flow watch` noticed about the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Observation {
    /// The task's `status.json` moved from one state to another.
    StatusChanged {
        task: String,
        from: State,
        to: State,
    },
    /// A line was appended to the task's `events.jsonl`.
    EventAppended { task: String, kind: String },
    /// herdr reports the task's worker in `status`.
    AgentState { task: String, status: AgentStatus },
    /// herdr no longer knows the task's worker pane.
    AgentGone { task: String },
    /// ssh could not reach the host.
    HostUnreachable { host: String },
    /// The task filed a dispatch request.
    RequestAppeared { task: String, request: String },
    /// The task's last event is `since_secs` old; `worktree_written` says whether files in its
    /// working directory changed after it.
    Quiet {
        task: String,
        since_secs: u64,
        worktree_written: bool,
    },
    /// herdr reports an adopted session's agent in `status`.
    AdoptedChanged {
        host: String,
        session: String,
        pane_id: String,
        status: AgentStatus,
    },
    /// `--heartbeat` seconds passed.
    Heartbeat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Absorb,
    Wake(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thresholds {
    /// How long a task may go without an event, and without writes to its working directory, before it wakes
    /// the supervisor.
    pub quiet_secs: u64,
    /// The adopted sessions whose state changes wake the supervisor, from `follow.json`.
    pub follow: Vec<Followed>,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            quiet_secs: DEFAULT_QUIET_SECS,
            follow: Vec::new(),
        }
    }
}

pub fn classify(obs: &Observation, thresholds: &Thresholds) -> Verdict {
    let reason = match obs {
        Observation::StatusChanged { task, to, .. } => match to {
            State::Done | State::Blocked | State::NeedsDecision => format!("{to}: {task}"),
            State::Pending | State::Working | State::Orphaned => return Verdict::Absorb,
        },
        Observation::EventAppended { task, kind } if kind == REQUEST_DISPATCH => {
            format!("request: {task}")
        }
        Observation::EventAppended { .. } => return Verdict::Absorb,
        Observation::AgentState { task, status } => match status {
            AgentStatus::Blocked | AgentStatus::Done => {
                format!("agent-{}: {task}", status.as_str())
            }
            AgentStatus::Working | AgentStatus::Idle | AgentStatus::Unknown => {
                return Verdict::Absorb;
            }
        },
        Observation::AgentGone { task } => format!("agent-gone: {task}"),
        Observation::HostUnreachable { host } => format!("host-unreachable: {host}"),
        Observation::RequestAppeared { task, .. } => format!("request: {task}"),
        Observation::Quiet {
            task,
            since_secs,
            worktree_written,
        } => {
            if *worktree_written || *since_secs < thresholds.quiet_secs {
                return Verdict::Absorb;
            }
            format!("quiet: {task} {since_secs}s")
        }
        Observation::AdoptedChanged {
            host,
            pane_id,
            status,
            ..
        } => {
            let followed = thresholds
                .follow
                .iter()
                .any(|followed| followed.host == *host && followed.pane_id == *pane_id);
            if !followed {
                return Verdict::Absorb;
            }
            format!("adopted-{}: {host}/{pane_id}", status.as_str())
        }
        Observation::Heartbeat => "heartbeat".to_owned(),
    };
    Verdict::Wake(reason)
}

/// What earlier wakes reported and is still true: hosts reported unreachable and not heard from
/// since, and the agent status last reported per watched pane. Kept across watch runs in
/// `watch-state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    #[serde(default)]
    pub unreachable: BTreeSet<String>,
    /// Keyed by `pane_key`.
    #[serde(default)]
    pub agents_reported: BTreeMap<String, PaneReport>,
}

/// What a wake reported about one pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneReport {
    /// The agent's status, or `gone`.
    pub seen: String,
    /// The length of the task's `events.jsonl` when the report was made; `None` for an adopted
    /// session, which has no event log.
    #[serde(default)]
    pub events: Option<u64>,
}

impl Reported {
    /// `verdict` for `obs`, absorbed when an earlier wake already reported the same thing, and
    /// recorded when it wakes.
    pub fn review(&mut self, obs: &Observation, verdict: Verdict) -> Verdict {
        if verdict == Verdict::Absorb {
            return verdict;
        }
        match obs {
            Observation::HostUnreachable { host } => {
                if !self.unreachable.insert(host.clone()) {
                    return Verdict::Absorb;
                }
            }
            Observation::AgentState { .. }
            | Observation::AgentGone { .. }
            | Observation::AdoptedChanged { .. } => {
                let key = pane_key(obs).expect("agent observations name a pane");
                let seen = pane_report(obs).expect("agent observations report a pane");
                if self
                    .agents_reported
                    .get(&key)
                    .is_some_and(|report| report.seen == seen)
                {
                    return Verdict::Absorb;
                }
                self.agents_reported
                    .insert(key, PaneReport { seen, events: None });
            }
            _ => {}
        }
        verdict
    }

    /// Forgets that `host` was reported unreachable, once it answers again.
    pub fn reachable(&mut self, host: &str) {
        self.unreachable.remove(host);
    }

    /// Records the length of the task's `events.jsonl` when its pane's status was reported.
    pub fn record_events(&mut self, key: &str, events: u64) {
        if let Some(report) = self.agents_reported.get_mut(key) {
            report.events = Some(events);
        }
    }

    /// Forgets the status reported for a task's pane once its `events.jsonl` grew past the
    /// length recorded with the report: the worker did something since, so the same status is
    /// news again.
    pub fn events_grew(&mut self, key: &str, events: u64) {
        if self
            .agents_reported
            .get(key)
            .and_then(|report| report.events)
            .is_some_and(|reported| events > reported)
        {
            self.agents_reported.remove(key);
        }
    }

    /// Forgets the status reported for a pane once herdr reports another one.
    pub fn agent_seen(&mut self, key: &str, status: AgentStatus) {
        if self
            .agents_reported
            .get(key)
            .is_some_and(|report| report.seen != status.as_str())
        {
            self.agents_reported.remove(key);
        }
    }
}

/// What an agent observation says about its pane: the agent's status, or `gone`.
fn pane_report(obs: &Observation) -> Option<String> {
    match obs {
        Observation::AgentState { status, .. } | Observation::AdoptedChanged { status, .. } => {
            Some(status.as_str().to_owned())
        }
        Observation::AgentGone { .. } => Some(GONE.to_owned()),
        _ => None,
    }
}

/// The key `Reported` files an agent observation's pane under: the task id for a worker,
/// `<host>/<session>/<pane>` for an adopted session.
pub fn pane_key(obs: &Observation) -> Option<String> {
    match obs {
        Observation::AgentState { task, .. } | Observation::AgentGone { task } => {
            Some(task.clone())
        }
        Observation::AdoptedChanged {
            host,
            session,
            pane_id,
            ..
        } => Some(format!("{host}/{session}/{pane_id}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK: &str = "aaaaaaab";

    fn verdict(obs: Observation) -> Verdict {
        classify(&obs, &Thresholds::default())
    }

    fn wake(reason: &str) -> Verdict {
        Verdict::Wake(reason.to_owned())
    }

    fn status_changed(to: State) -> Observation {
        Observation::StatusChanged {
            task: TASK.to_owned(),
            from: State::Working,
            to,
        }
    }

    fn agent(status: AgentStatus) -> Observation {
        Observation::AgentState {
            task: TASK.to_owned(),
            status,
        }
    }

    fn quiet(since_secs: u64, worktree_written: bool) -> Observation {
        Observation::Quiet {
            task: TASK.to_owned(),
            since_secs,
            worktree_written,
        }
    }

    fn adopted(pane_id: &str, status: AgentStatus) -> Observation {
        Observation::AdoptedChanged {
            host: "host-a".to_owned(),
            session: "default".to_owned(),
            pane_id: pane_id.to_owned(),
            status,
        }
    }

    fn unreachable(host: &str) -> Observation {
        Observation::HostUnreachable {
            host: host.to_owned(),
        }
    }

    #[test]
    fn classify_status_changed_to_done_wakes() {
        assert_eq!(verdict(status_changed(State::Done)), wake("done: aaaaaaab"));
    }

    #[test]
    fn classify_status_changed_to_blocked_wakes() {
        assert_eq!(
            verdict(status_changed(State::Blocked)),
            wake("blocked: aaaaaaab")
        );
    }

    #[test]
    fn classify_status_changed_to_needs_decision_wakes() {
        assert_eq!(
            verdict(status_changed(State::NeedsDecision)),
            wake("needs-decision: aaaaaaab")
        );
    }

    #[test]
    fn classify_status_changed_to_working_absorbs() {
        let obs = Observation::StatusChanged {
            task: TASK.to_owned(),
            from: State::Blocked,
            to: State::Working,
        };
        assert_eq!(verdict(obs), Verdict::Absorb);
    }

    #[test]
    fn classify_request_dispatch_event_wakes() {
        let obs = Observation::EventAppended {
            task: TASK.to_owned(),
            kind: "request.dispatch".to_owned(),
        };
        assert_eq!(verdict(obs), wake("request: aaaaaaab"));
    }

    #[test]
    fn classify_other_event_absorbs() {
        let obs = Observation::EventAppended {
            task: TASK.to_owned(),
            kind: "status.report".to_owned(),
        };
        assert_eq!(verdict(obs), Verdict::Absorb);
    }

    #[test]
    fn classify_agent_blocked_or_done_wakes() {
        assert_eq!(
            verdict(agent(AgentStatus::Blocked)),
            wake("agent-blocked: aaaaaaab")
        );
        assert_eq!(
            verdict(agent(AgentStatus::Done)),
            wake("agent-done: aaaaaaab")
        );
    }

    #[test]
    fn classify_agent_working_or_idle_absorbs() {
        assert_eq!(verdict(agent(AgentStatus::Working)), Verdict::Absorb);
        assert_eq!(verdict(agent(AgentStatus::Idle)), Verdict::Absorb);
    }

    #[test]
    fn classify_agent_gone_wakes() {
        let obs = Observation::AgentGone {
            task: TASK.to_owned(),
        };
        assert_eq!(verdict(obs), wake("agent-gone: aaaaaaab"));
    }

    #[test]
    fn classify_host_unreachable_wakes() {
        assert_eq!(
            verdict(unreachable("host-b")),
            wake("host-unreachable: host-b")
        );
    }

    #[test]
    fn classify_request_appeared_wakes() {
        let obs = Observation::RequestAppeared {
            task: TASK.to_owned(),
            request: "bbbbbbbc".to_owned(),
        };
        assert_eq!(verdict(obs), wake("request: aaaaaaab"));
    }

    #[test]
    fn classify_quiet_with_worktree_writes_absorbs() {
        assert_eq!(verdict(quiet(7200, true)), Verdict::Absorb);
    }

    #[test]
    fn classify_quiet_past_threshold_wakes() {
        assert_eq!(verdict(quiet(1800, false)), wake("quiet: aaaaaaab 1800s"));
    }

    #[test]
    fn classify_quiet_under_threshold_absorbs() {
        assert_eq!(verdict(quiet(1799, false)), Verdict::Absorb);
    }

    #[test]
    fn classify_adopted_change_of_followed_pane_wakes() {
        let thresholds = Thresholds {
            follow: vec![Followed {
                host: "host-a".to_owned(),
                pane_id: "w1:p1".to_owned(),
                session: "default".to_owned(),
            }],
            ..Thresholds::default()
        };
        assert_eq!(
            classify(&adopted("w1:p1", AgentStatus::Idle), &thresholds),
            wake("adopted-idle: host-a/w1:p1")
        );
    }

    #[test]
    fn classify_adopted_change_of_unfollowed_pane_absorbs() {
        assert_eq!(
            verdict(adopted("w1:p1", AgentStatus::Idle)),
            Verdict::Absorb
        );
    }

    #[test]
    fn classify_heartbeat_wakes() {
        assert_eq!(verdict(Observation::Heartbeat), wake("heartbeat"));
    }

    fn reviewed(reported: &mut Reported, obs: &Observation) -> Verdict {
        let verdict = classify(obs, &Thresholds::default());
        reported.review(obs, verdict)
    }

    #[test]
    fn host_unreachable_wakes_once_per_run() {
        let mut reported = Reported::default();
        let obs = unreachable("host-b");
        assert_eq!(
            reviewed(&mut reported, &obs),
            wake("host-unreachable: host-b")
        );
        assert_eq!(reviewed(&mut reported, &obs), Verdict::Absorb);
        assert_eq!(
            reviewed(&mut reported, &unreachable("host-c")),
            wake("host-unreachable: host-c")
        );
    }

    #[test]
    fn host_unreachable_wakes_again_after_it_answered() {
        let mut reported = Reported::default();
        let obs = unreachable("host-b");
        reviewed(&mut reported, &obs);
        reported.reachable("host-b");
        assert_eq!(
            reviewed(&mut reported, &obs),
            wake("host-unreachable: host-b")
        );
    }

    #[test]
    fn agent_status_wakes_once_until_it_changes() {
        let mut reported = Reported::default();
        let blocked = agent(AgentStatus::Blocked);
        assert_eq!(
            reviewed(&mut reported, &blocked),
            wake("agent-blocked: aaaaaaab")
        );
        assert_eq!(reviewed(&mut reported, &blocked), Verdict::Absorb);
        reported.agent_seen(TASK, AgentStatus::Working);
        assert_eq!(
            reviewed(&mut reported, &blocked),
            wake("agent-blocked: aaaaaaab")
        );
    }

    #[test]
    fn agent_status_wakes_again_after_task_events_grew() {
        let mut reported = Reported::default();
        let blocked = agent(AgentStatus::Blocked);
        reviewed(&mut reported, &blocked);
        reported.record_events(TASK, 120);
        reported.events_grew(TASK, 120);
        assert_eq!(reviewed(&mut reported, &blocked), Verdict::Absorb);
        reported.events_grew(TASK, 180);
        assert_eq!(
            reviewed(&mut reported, &blocked),
            wake("agent-blocked: aaaaaaab")
        );
    }

    #[test]
    fn adopted_report_is_not_cleared_by_events() {
        let mut reported = Reported::default();
        let idle = adopted("w1:p1", AgentStatus::Idle);
        reported.review(&idle, wake("adopted-idle: host-a/w1:p1"));
        let key = pane_key(&idle).unwrap();
        reported.events_grew(&key, 1_000);
        assert_eq!(
            reported.review(&idle, wake("adopted-idle: host-a/w1:p1")),
            Verdict::Absorb
        );
    }

    #[test]
    fn agent_gone_wakes_once() {
        let mut reported = Reported::default();
        let gone = Observation::AgentGone {
            task: TASK.to_owned(),
        };
        assert_eq!(reviewed(&mut reported, &gone), wake("agent-gone: aaaaaaab"));
        assert_eq!(reviewed(&mut reported, &gone), Verdict::Absorb);
    }

    #[test]
    fn absorbed_observations_are_not_recorded() {
        let mut reported = Reported::default();
        reviewed(&mut reported, &agent(AgentStatus::Working));
        assert_eq!(reported, Reported::default());
    }

    #[test]
    fn pane_keys_name_the_task_or_the_adopted_pane() {
        assert_eq!(pane_key(&agent(AgentStatus::Done)).as_deref(), Some(TASK));
        assert_eq!(
            pane_key(&adopted("w1:p1", AgentStatus::Done)).as_deref(),
            Some("host-a/default/w1:p1")
        );
        assert_eq!(pane_key(&Observation::Heartbeat), None);
    }

    #[test]
    fn observations_serialize_tagged_by_kind() {
        let json = serde_json::to_value(status_changed(State::NeedsDecision)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "status-changed",
                "task": TASK,
                "from": "working",
                "to": "needs-decision",
            })
        );
        let back: Observation = serde_json::from_value(json).unwrap();
        assert_eq!(back, status_changed(State::NeedsDecision));
    }
}
