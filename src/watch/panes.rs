//! The herdr `agent wait` `flow watch` keeps running per watched pane: a task's worker or a
//! hand session, an agent outside flow's herdr session. A wait ends on the statuses worth waking
//! for, which for a hand session the user does not follow is `blocked` alone; once one of those
//! was read, the next wait ends on any other status instead, so a status already observed does
//! not answer again at once. After `working` the next wait also ends on any other status, so a
//! worker known to be working is known to stop, idle included. A wait started on a pane whose
//! status an earlier wake reported starts as if it had just read that status: it ends on any
//! other one first, so a change made while no wait ran still reaches the watcher.

use std::io::Read;
use std::process::Child;
use std::time::{Duration, Instant};

use super::classify::Observation;
use crate::herdr::{self, AgentStatus, Herdr, WaitEnd};
use crate::state::config::Host;

/// How long a herdr wait that could not run waits before it is started again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The worker statuses worth waking for.
const TASK_WAKE_STATUSES: &[AgentStatus] = &[AgentStatus::Blocked, AgentStatus::Done];
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

/// Whose agent a wait watches: a task's worker or a hand session, with whether the user follows
/// it and its title as of the cached inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WaitTarget {
    Task(String),
    Adopted { followed: bool, title: String },
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
            WaitTarget::Task(task) => task.clone(),
            WaitTarget::Adopted { .. } => {
                format!("{}/{}/{}", self.host, self.session, self.pane_id)
            }
        }
    }

    pub(super) fn wake_statuses(&self) -> &'static [AgentStatus] {
        match self.target {
            WaitTarget::Task(_) => TASK_WAKE_STATUSES,
            WaitTarget::Adopted { followed: true, .. } => FOLLOWED_WAKE_STATUSES,
            WaitTarget::Adopted {
                followed: false, ..
            } => HAND_WAKE_STATUSES,
        }
    }

    /// Whether this waits on a hand session the user does not follow.
    pub(super) fn unfollowed_hand(&self) -> bool {
        matches!(
            self.target,
            WaitTarget::Adopted {
                followed: false,
                ..
            }
        )
    }

    pub(super) fn observe(&self, status: AgentStatus) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentState {
                task: task.clone(),
                status,
            },
            WaitTarget::Adopted { title, .. } => self.adopted(title, status),
        }
    }

    /// herdr no longer knows the pane. A hand session's agent then reads as `unknown`.
    pub(super) fn gone(&self) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentGone { task: task.clone() },
            WaitTarget::Adopted { title, .. } => self.adopted(title, AgentStatus::Unknown),
        }
    }

    fn adopted(&self, title: &str, status: AgentStatus) -> Observation {
        Observation::AdoptedChanged {
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
    /// The agent's status as of the last wait that ended, `None` once a wait could not tell.
    known: Option<AgentStatus>,
    retry_at: Option<Instant>,
    /// Set once herdr no longer knows the pane; nothing more is asked about it.
    gone: bool,
}

pub(super) enum Polled {
    Running,
    Reached(AgentStatus),
    TimedOut,
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
            known: None,
            retry_at: None,
            gone: false,
        }
    }

    /// Whether herdr last reported the agent `working` and no wait since ended otherwise.
    pub(super) fn working(&self) -> bool {
        self.known == Some(AgentStatus::Working)
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
            _ => wake.to_vec(),
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
                self.settled = Some(status);
                self.known = Some(status);
                Polled::Reached(status)
            }
            WaitEnd::TimedOut => Polled::TimedOut,
            WaitEnd::Gone => {
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
        self.child = None;
        self.known = None;
        self.retry_at = Some(Instant::now() + RETRY_AFTER);
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
    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:wait"}"#;

    fn answer(status: i32, stdout: &str) -> Scripted {
        Scripted {
            status,
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    fn task_wait(script: Vec<Scripted>) -> (PaneWait, Arc<FakeRunner>) {
        wait_on(WaitTarget::Task("aaaaaaab".to_owned()), script)
    }

    fn hand_wait(followed: bool) -> PaneWait {
        let target = WaitTarget::Adopted {
            followed,
            title: "Tidy docs".to_owned(),
        };
        wait_on(target, Vec::new()).0
    }

    fn wait_on(target: WaitTarget, script: Vec<Scripted>) -> (PaneWait, Arc<FakeRunner>) {
        let fake = Arc::new(FakeRunner::new(script));
        let session = match target {
            WaitTarget::Task(_) => herdr::SESSION,
            WaitTarget::Adopted { .. } => "default",
        };
        let spec = WaitSpec {
            target,
            host: "host-a".to_owned(),
            session: session.to_owned(),
            pane_id: "w1:p1".to_owned(),
        };
        let host = Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        };
        let wait = PaneWait::new(spec, host, Herdr::new(fake.clone()), None);
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
            WaitTarget::Adopted {
                followed: false,
                title: String::new(),
            },
            Vec::new(),
        );
        let seeded = PaneWait::new(
            wait.spec.clone(),
            wait.host.clone(),
            Herdr::new(Arc::new(FakeRunner::new(Vec::new()))),
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
    fn gone_pane_is_not_asked_again() {
        let (mut wait, fake) = task_wait(vec![answer(1, NOT_FOUND)]);
        assert!(matches!(run_once(&mut wait), Polled::Gone));
        assert!(matches!(wait.poll(1000), Polled::Running));
        assert_eq!(fake.calls().len(), 1);
    }
}
