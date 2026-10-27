//! The herdr `agent wait` `flow watch` keeps running per watched pane: a task's worker or a
//! followed adopted session. A wait ends on the statuses worth waking for; once one of those
//! was read, the next wait ends on any other status instead, so a status already observed does
//! not answer again at once.

use std::io::Read;
use std::process::Child;
use std::time::{Duration, Instant};

use super::classify::Observation;
use crate::herdr::{self, AgentStatus, Herdr, WaitEnd};
use crate::state::config::Cde;

/// How long a herdr wait that could not run waits before it is started again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The worker statuses worth waking for.
const TASK_WAKE_STATUSES: &[AgentStatus] = &[AgentStatus::Blocked, AgentStatus::Done];
/// The adopted-session statuses worth waking for.
const ADOPTED_WAKE_STATUSES: &[AgentStatus] =
    &[AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done];
/// The statuses a wait for a change from one of them ends on.
const WAITABLE_STATUSES: &[AgentStatus] = &[
    AgentStatus::Idle,
    AgentStatus::Working,
    AgentStatus::Blocked,
    AgentStatus::Done,
];

/// Whose agent a wait watches: a task's worker or a followed adopted session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WaitTarget {
    Task(String),
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WaitSpec {
    pub(super) target: WaitTarget,
    pub(super) cde: String,
    pub(super) session: String,
    pub(super) pane_id: String,
}

impl WaitSpec {
    /// The key `classify::pane_key` gives this pane's observations.
    pub(super) fn key(&self) -> String {
        match &self.target {
            WaitTarget::Task(task) => task.clone(),
            WaitTarget::Adopted => format!("{}/{}/{}", self.cde, self.session, self.pane_id),
        }
    }

    pub(super) fn wake_statuses(&self) -> &'static [AgentStatus] {
        match self.target {
            WaitTarget::Task(_) => TASK_WAKE_STATUSES,
            WaitTarget::Adopted => ADOPTED_WAKE_STATUSES,
        }
    }

    pub(super) fn observe(&self, status: AgentStatus) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentState {
                task: task.clone(),
                status,
            },
            WaitTarget::Adopted => self.adopted(status),
        }
    }

    /// herdr no longer knows the pane. An adopted session's agent then reads as `unknown`.
    pub(super) fn gone(&self) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentGone { task: task.clone() },
            WaitTarget::Adopted => self.adopted(AgentStatus::Unknown),
        }
    }

    fn adopted(&self, status: AgentStatus) -> Observation {
        Observation::AdoptedChanged {
            cde: self.cde.clone(),
            session: self.session.clone(),
            pane_id: self.pane_id.clone(),
            status,
        }
    }
}

/// One herdr `agent wait` kept running on a pane.
pub(super) struct PaneWait {
    pub(super) spec: WaitSpec,
    pub(super) cde: Cde,
    herdr: Herdr,
    pub(super) child: Option<Child>,
    /// The status the last wait ended on; the next one waits for a change from it when it is
    /// one worth waking for, since that was already observed.
    settled: Option<AgentStatus>,
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
    pub(super) fn new(spec: WaitSpec, cde: Cde, herdr: Herdr) -> PaneWait {
        PaneWait {
            spec,
            cde,
            herdr,
            child: None,
            settled: None,
            retry_at: None,
            gone: false,
        }
    }

    /// The statuses the next wait ends on.
    fn until(&self) -> Vec<AgentStatus> {
        let wake = self.spec.wake_statuses();
        match self.settled {
            Some(settled) if wake.contains(&settled) => WAITABLE_STATUSES
                .iter()
                .copied()
                .filter(|status| *status != settled)
                .collect(),
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
                &self.cde,
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
                Polled::Reached(status)
            }
            WaitEnd::TimedOut => Polled::TimedOut,
            WaitEnd::Gone => {
                self.gone = true;
                Polled::Gone
            }
            WaitEnd::Unreachable => self.back_off(Polled::Unreachable),
            WaitEnd::Failed(problem) => self.back_off(Polled::Failed(format!(
                "{}: herdr wait on {} {problem}",
                self.spec.key(),
                self.cde.name
            ))),
        }
    }

    fn back_off(&mut self, polled: Polled) -> Polled {
        self.child = None;
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
        let fake = Arc::new(FakeRunner::new(script));
        let spec = WaitSpec {
            target: WaitTarget::Task("aaaaaaab".to_owned()),
            cde: "cde-a".to_owned(),
            session: herdr::SESSION.to_owned(),
            pane_id: "w1:p1".to_owned(),
        };
        let cde = Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        };
        let wait = PaneWait::new(spec, cde, Herdr::new(fake.clone()));
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
    fn until_after_a_non_wake_status_waits_for_wake_statuses() {
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
        assert_eq!(wait.until(), [AgentStatus::Blocked, AgentStatus::Done]);
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
