//! The herdr `agent wait` `flow watch` keeps running per watched pane: a task's worker or a hand
//! session, an agent in neither a task's nor the supervisor's pane. A wait ends on the statuses
//! worth waking for, which for a hand session the user does not follow is `blocked` alone; once one
//! of those was read, the next wait ends on any other status instead, so a status already observed
//! does not answer again at once. After `working` the next wait also ends on any other status, so a
//! worker known to be working is known to stop, idle included. An adopted task's wait also ends on
//! `working` when it has no change to wait for, so the `idle` that ends each of its turns is seen,
//! standing in for the turn-end hook such a session lacks. A wait started on a pane whose status an
//! earlier wake reported starts as if it had just read that status: it ends on any other one first,
//! so a change made while no wait ran still reaches the watcher. herdr may not yet see the agent
//! in a dispatched task's pane right after its launch; until `LAUNCH_GRACE` has passed since the
//! task was created, a wait answered with no agent asks again instead of reporting it gone.

use std::io::Read;
use std::process::Child;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};

use super::classify::Observation;
use crate::herdr::{self, AgentStatus, Herdr, WaitEnd};
use crate::state::config::Host;

/// How long a herdr wait that could not run waits before it is started again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// How long after a dispatched task was created herdr may see no agent in its pane without the
/// agent being gone. `flow dispatch` records the pane as the task's before it launches Claude
/// Code there, and herdr answers `agent_not_found` until it detects the agent, so a watcher
/// already running can ask too early. The same 60 s `flow dispatch` gives Claude Code to start.
const LAUNCH_GRACE: SignedDuration = SignedDuration::from_secs(60);
/// How long a wait answered with no agent during `LAUNCH_GRACE` waits before it asks again.
const NOT_YET_RETRY_AFTER: Duration = Duration::from_secs(5);
/// The worker statuses worth waking for.
const TASK_WAKE_STATUSES: &[AgentStatus] = &[AgentStatus::Blocked, AgentStatus::Done];
/// What an adopted task's wait ends on when it has no change to wait for: the wake statuses,
/// and `working`, so the `idle` that ends a turn is seen after it.
const ADOPTED_TASK_STATUSES: &[AgentStatus] = &[
    AgentStatus::Working,
    AgentStatus::Blocked,
    AgentStatus::Done,
];
/// The followed hand-session statuses worth waking for.
const FOLLOWED_WAKE_STATUSES: &[AgentStatus] =
    &[AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done];
/// The statuses worth waking for of a hand session the user does not follow.
const HAND_WAKE_STATUSES: &[AgentStatus] = &[AgentStatus::Blocked];
/// The statuses a wait for a change from one of them ends on.
const WAITABLE_STATUSES: &[AgentStatus] = &[
    AgentStatus::Idle,
    AgentStatus::Working,
    AgentStatus::Blocked,
    AgentStatus::Done,
];

/// Whose agent a wait watches: a task's worker, with whether it was adopted and when the task
/// was created, or a hand session, with whether the user follows it and its title as of the
/// cached inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WaitTarget {
    Task {
        id: String,
        adopted: bool,
        created: Timestamp,
    },
    Observed {
        followed: bool,
        title: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WaitSpec {
    pub(super) target: WaitTarget,
    pub(super) host: String,
    pub(super) session: String,
    pub(super) pane_id: String,
}

impl WaitSpec {
    /// The key `classify::pane_key` gives this pane's observations.
    pub(super) fn key(&self) -> String {
        match &self.target {
            WaitTarget::Task { id, .. } => id.clone(),
            WaitTarget::Observed { .. } => {
                format!("{}/{}/{}", self.host, self.session, self.pane_id)
            }
        }
    }

    pub(super) fn wake_statuses(&self) -> &'static [AgentStatus] {
        match self.target {
            WaitTarget::Task { .. } => TASK_WAKE_STATUSES,
            WaitTarget::Observed { followed: true, .. } => FOLLOWED_WAKE_STATUSES,
            WaitTarget::Observed {
                followed: false, ..
            } => HAND_WAKE_STATUSES,
        }
    }

    /// What the wait ends on when it has no change to wait for.
    fn base_statuses(&self) -> &'static [AgentStatus] {
        match self.target {
            WaitTarget::Task { adopted: true, .. } => ADOPTED_TASK_STATUSES,
            _ => self.wake_statuses(),
        }
    }

    /// Whether, at `now`, herdr may not see this wait's agent yet: it is a dispatched task's
    /// worker and `LAUNCH_GRACE` has not passed since the task was created.
    fn launching(&self, now: Timestamp) -> bool {
        match self.target {
            WaitTarget::Task {
                adopted: false,
                created,
                ..
            } => now.duration_since(created) < LAUNCH_GRACE,
            _ => false,
        }
    }

    /// Whether this waits on a hand session the user does not follow.
    pub(super) fn unfollowed_hand(&self) -> bool {
        matches!(
            self.target,
            WaitTarget::Observed {
                followed: false,
                ..
            }
        )
    }

    pub(super) fn observe(&self, status: AgentStatus) -> Observation {
        match &self.target {
            WaitTarget::Task { id, .. } => Observation::AgentState {
                task: id.clone(),
                status,
            },
            WaitTarget::Observed { title, .. } => self.observed(title, status),
        }
    }

    /// herdr sees no agent in the pane, or no server for it. A hand session's agent then reads
    /// as `unknown`.
    pub(super) fn gone(&self) -> Observation {
        match &self.target {
            WaitTarget::Task { id, .. } => Observation::AgentGone { task: id.clone() },
            WaitTarget::Observed { title, .. } => self.observed(title, AgentStatus::Unknown),
        }
    }

    fn observed(&self, title: &str, status: AgentStatus) -> Observation {
        Observation::ObservedChanged {
            host: self.host.clone(),
            session: self.session.clone(),
            pane_id: self.pane_id.clone(),
            title: title.to_owned(),
            status,
        }
    }
}

/// One herdr `agent wait` kept running on a pane.
pub(super) struct PaneWait {
    pub(super) spec: WaitSpec,
    pub(super) host: Host,
    herdr: Herdr,
    pub(super) child: Option<Child>,
    /// The status the last wait ended on; the next one waits for a change from it when it is
    /// one worth waking for, since that was already observed, or `working`.
    settled: Option<AgentStatus>,
    /// `settled` before the last wait ended.
    previous: Option<AgentStatus>,
    /// The agent's status as of the last wait that ended, `None` once a wait could not tell.
    known: Option<AgentStatus>,
    retry_at: Option<Instant>,
    /// Set once herdr sees no agent in the pane, or no server for it, other than during
    /// `LAUNCH_GRACE`; nothing more is asked about it.
    gone: bool,
}

pub(super) enum Polled {
    Running,
    Reached(AgentStatus),
    TimedOut,
    /// herdr sees no agent in a dispatched task's pane yet; the wait asks again shortly.
    NotYet,
    Gone,
    Unreachable,
    Failed(String),
}

impl PaneWait {
    /// A wait on `spec`'s pane, `reported` being the status an earlier wake reported for it.
    pub(super) fn new(
        spec: WaitSpec,
        host: Host,
        herdr: Herdr,
        reported: Option<AgentStatus>,
    ) -> PaneWait {
        PaneWait {
            spec,
            host,
            herdr,
            child: None,
            settled: reported,
            previous: None,
            known: None,
            retry_at: None,
            gone: false,
        }
    }

    /// Whether herdr last reported the agent `working` and no wait since ended otherwise.
    pub(super) fn working(&self) -> bool {
        self.known == Some(AgentStatus::Working)
    }

    /// Whether the wait that just ended saw an adopted task's agent go from `working` to `idle`:
    /// the end of a turn, which a dispatched worker's hooks report themselves.
    pub(super) fn turn_ended(&self) -> bool {
        matches!(self.spec.target, WaitTarget::Task { adopted: true, .. })
            && self.previous == Some(AgentStatus::Working)
            && self.settled == Some(AgentStatus::Idle)
    }

    /// The statuses the next wait ends on.
    fn until(&self) -> Vec<AgentStatus> {
        let wake = self.spec.wake_statuses();
        match self.settled {
            Some(settled) if wake.contains(&settled) || settled == AgentStatus::Working => {
                WAITABLE_STATUSES
                    .iter()
                    .copied()
                    .filter(|status| *status != settled)
                    .collect()
            }
            _ => self.spec.base_statuses().to_vec(),
        }
    }

    /// Starts the wait when none runs and it is not backing off, or reads the running one.
    pub(super) fn poll(&mut self, timeout_ms: u64) -> Polled {
        if self.gone {
            return Polled::Running;
        }
        let Some(child) = &mut self.child else {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return Polled::Running;
            }
            let until = self.until();
            return match self.herdr.spawn_wait(
                &self.host,
                &self.spec.pane_id,
                &until,
                Some(timeout_ms),
            ) {
                Ok(child) => {
                    self.child = Some(child);
                    self.retry_at = None;
                    Polled::Running
                }
                Err(_) => self.back_off(Polled::Unreachable),
            };
        };
        let exit = match child.try_wait() {
            Ok(None) => return Polled::Running,
            Ok(Some(exit)) => exit,
            Err(err) => return self.back_off(Polled::Failed(format!("waiting on herdr: {err}"))),
        };
        let mut stdout = String::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        self.child = None;
        match herdr::wait_end(exit.code(), &stdout) {
            WaitEnd::Reached(status) => {
                self.previous = self.settled;
                self.settled = Some(status);
                self.known = Some(status);
                Polled::Reached(status)
            }
            WaitEnd::TimedOut => Polled::TimedOut,
            WaitEnd::NoAgent if self.spec.launching(Timestamp::now()) => {
                self.back_off_for(NOT_YET_RETRY_AFTER, Polled::NotYet)
            }
            WaitEnd::NoAgent | WaitEnd::NoServer => {
                self.gone = true;
                self.known = None;
                Polled::Gone
            }
            WaitEnd::Unreachable => self.back_off(Polled::Unreachable),
            WaitEnd::Failed(problem) => self.back_off(Polled::Failed(format!(
                "{}: herdr wait on {} {problem}",
                self.spec.key(),
                self.host.name
            ))),
        }
    }

    fn back_off(&mut self, polled: Polled) -> Polled {
        self.back_off_for(RETRY_AFTER, polled)
    }

    fn back_off_for(&mut self, delay: Duration, polled: Polled) -> Polled {
        self.child = None;
        self.known = None;
        self.retry_at = Some(Instant::now() + delay);
        polled
    }
}

impl Drop for PaneWait {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::runner::{FakeRunner, Scripted};

    const WORKING: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"working","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const BLOCKED: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"blocked","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const IDLE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"idle","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:wait"}"#;

    fn answer(status: i32, stdout: &str) -> Scripted {
        Scripted {
            status,
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    /// A wait on a dispatched task created long enough ago that its launch grace has passed.
    fn task_wait(script: Vec<Scripted>) -> (PaneWait, Arc<FakeRunner>) {
        task_wait_created(Timestamp::now() - SignedDuration::from_hours(1), script)
    }

    fn task_wait_created(created: Timestamp, script: Vec<Scripted>) -> (PaneWait, Arc<FakeRunner>) {
        let target = WaitTarget::Task {
            id: "aaaaaaab".to_owned(),
            adopted: false,
            created,
        };
        wait_on(target, script)
    }

    fn hand_wait(followed: bool) -> PaneWait {
        let target = WaitTarget::Observed {
            followed,
            title: "Tidy docs".to_owned(),
        };
        wait_on(target, Vec::new()).0
    }

    fn wait_on(target: WaitTarget, script: Vec<Scripted>) -> (PaneWait, Arc<FakeRunner>) {
        let fake = Arc::new(FakeRunner::new(script));
        let spec = WaitSpec {
            target,
            host: "host-a".to_owned(),
            session: "default".to_owned(),
            pane_id: "w1:p1".to_owned(),
        };
        let host = Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        };
        let herdr = Herdr::with_session(fake.clone(), &spec.session);
        let wait = PaneWait::new(spec, host, herdr, None);
        (wait, fake)
    }

    /// Starts the wait, lets it end, and reads how it ended.
    fn run_once(wait: &mut PaneWait) -> Polled {
        assert!(matches!(wait.poll(1000), Polled::Running));
        wait.child
            .as_mut()
            .expect("the wait started")
            .wait()
            .unwrap();
        wait.poll(1000)
    }

    #[test]
    fn until_waits_for_a_change_after_a_wake_status() {
        let (mut wait, _) = task_wait(Vec::new());
        assert_eq!(wait.until(), [AgentStatus::Blocked, AgentStatus::Done]);
        wait.settled = Some(AgentStatus::Blocked);
        assert_eq!(
            wait.until(),
            [AgentStatus::Idle, AgentStatus::Working, AgentStatus::Done]
        );
    }

    #[test]
    fn until_after_working_waits_for_any_other_status() {
        let (mut wait, _) = task_wait(vec![answer(0, BLOCKED), answer(0, WORKING)]);
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Blocked)
        ));
        assert!(!wait.until().contains(&AgentStatus::Blocked));
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert_eq!(
            wait.until(),
            [AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done]
        );
    }

    #[test]
    fn unfollowed_hand_session_waits_for_blocked_only() {
        let mut wait = hand_wait(false);
        assert_eq!(wait.until(), [AgentStatus::Blocked]);
        wait.settled = Some(AgentStatus::Blocked);
        assert_eq!(
            wait.until(),
            [AgentStatus::Idle, AgentStatus::Working, AgentStatus::Done]
        );
        wait.settled = Some(AgentStatus::Idle);
        assert_eq!(wait.until(), [AgentStatus::Blocked]);
    }

    #[test]
    fn a_wait_seeded_with_a_reported_status_waits_for_a_change_from_it() {
        let (wait, _) = wait_on(
            WaitTarget::Observed {
                followed: false,
                title: String::new(),
            },
            Vec::new(),
        );
        let seeded = PaneWait::new(
            wait.spec.clone(),
            wait.host.clone(),
            Herdr::with_session(Arc::new(FakeRunner::new(Vec::new())), "default"),
            Some(AgentStatus::Blocked),
        );
        assert_eq!(
            seeded.until(),
            [AgentStatus::Idle, AgentStatus::Working, AgentStatus::Done]
        );
        assert!(!seeded.working());
    }

    #[test]
    fn followed_hand_session_waits_for_idle_blocked_and_done() {
        let wait = hand_wait(true);
        assert_eq!(
            wait.until(),
            [AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done]
        );
    }

    #[test]
    fn adopted_task_waits_see_working_and_the_idle_after_it() {
        let target = WaitTarget::Task {
            id: "aaaaaaab".to_owned(),
            adopted: true,
            created: Timestamp::now(),
        };
        let (mut wait, _) = wait_on(target, vec![answer(0, WORKING), answer(0, IDLE)]);
        assert_eq!(
            wait.until(),
            [
                AgentStatus::Working,
                AgentStatus::Blocked,
                AgentStatus::Done
            ]
        );
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert!(!wait.turn_ended());
        assert_eq!(
            wait.until(),
            [AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done]
        );
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Idle)
        ));
        assert!(wait.turn_ended());
        assert_eq!(
            wait.until(),
            [
                AgentStatus::Working,
                AgentStatus::Blocked,
                AgentStatus::Done
            ]
        );
    }

    #[test]
    fn dispatched_task_waits_never_report_a_turn_end() {
        let (mut wait, _) = task_wait(vec![answer(0, WORKING), answer(0, IDLE)]);
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Idle)
        ));
        assert!(!wait.turn_ended());
        assert_eq!(wait.until(), [AgentStatus::Blocked, AgentStatus::Done]);
    }

    #[test]
    fn until_after_idle_waits_for_wake_statuses() {
        let (mut wait, _) = task_wait(Vec::new());
        wait.settled = Some(AgentStatus::Idle);
        assert_eq!(wait.until(), [AgentStatus::Blocked, AgentStatus::Done]);
    }

    #[test]
    fn working_is_known_until_a_wait_ends_otherwise() {
        let (mut wait, _) = task_wait(vec![answer(0, WORKING), answer(255, "")]);
        assert!(!wait.working());
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert!(wait.working());
        assert!(matches!(run_once(&mut wait), Polled::Unreachable));
        assert!(!wait.working());
    }

    #[test]
    fn unreachable_wait_backs_off_then_retries() {
        let (mut wait, fake) = task_wait(vec![answer(255, ""), answer(0, WORKING)]);
        assert!(matches!(run_once(&mut wait), Polled::Unreachable));
        assert!(wait.retry_at.is_some_and(|at| at > Instant::now()));
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1, "no new wait while backing off");

        wait.retry_at = Some(Instant::now() - Duration::from_millis(1));
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert_eq!(fake.calls().len(), 2);
        assert_eq!(wait.retry_at, None);
    }

    #[test]
    fn failed_wait_backs_off() {
        let (mut wait, fake) = task_wait(vec![answer(1, "boom")]);
        let Polled::Failed(problem) = run_once(&mut wait) else {
            panic!("expected a failure");
        };
        assert!(problem.contains("boom"), "{problem}");
        assert!(wait.retry_at.is_some_and(|at| at > Instant::now()));
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn no_agent_right_after_dispatch_retries_shortly_and_is_not_gone() {
        let (mut wait, fake) = task_wait_created(
            Timestamp::now(),
            vec![answer(1, NOT_FOUND), answer(0, WORKING)],
        );
        assert!(matches!(run_once(&mut wait), Polled::NotYet));
        assert!(!wait.gone);
        let retry_at = wait.retry_at.expect("the wait backs off");
        assert!(retry_at <= Instant::now() + NOT_YET_RETRY_AFTER);
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1, "no new wait while backing off");

        wait.retry_at = Some(Instant::now() - Duration::from_millis(1));
        assert!(matches!(
            run_once(&mut wait),
            Polled::Reached(AgentStatus::Working)
        ));
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn no_agent_after_the_launch_grace_is_gone() {
        let created = Timestamp::now() - LAUNCH_GRACE - SignedDuration::from_secs(1);
        let (mut wait, fake) = task_wait_created(created, vec![answer(1, NOT_FOUND)]);
        assert!(matches!(run_once(&mut wait), Polled::Gone));
        assert!(wait.gone);
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn no_server_right_after_dispatch_is_gone() {
        let no_server =
            include_str!("../../tests/fixtures/herdr/agent_list_server_not_running.json");
        let (mut wait, _) = task_wait_created(Timestamp::now(), vec![answer(1, no_server)]);
        assert!(matches!(run_once(&mut wait), Polled::Gone));
    }

    #[test]
    fn no_agent_right_after_adoption_is_gone() {
        let target = WaitTarget::Task {
            id: "aaaaaaab".to_owned(),
            adopted: true,
            created: Timestamp::now(),
        };
        let (mut wait, _) = wait_on(target, vec![answer(1, NOT_FOUND)]);
        assert!(matches!(run_once(&mut wait), Polled::Gone));
    }

    #[test]
    fn gone_pane_is_not_asked_again() {
        let (mut wait, fake) = task_wait(vec![answer(1, NOT_FOUND)]);
        assert!(matches!(run_once(&mut wait), Polled::Gone));
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1);
    }
}
