//! Who a herdr agent belongs to: the supervisor, a task's worker, or nobody flow manages. herdr
//! reuses pane ids after a server restart, so the Claude Code session id herdr reports decides;
//! a pane decides only for an agent herdr reports no session for.

use crate::herdr::Agent;
use crate::state::task::{HerdrTarget, Status, TaskRecord};
use crate::supervisor::Lock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    Supervisor,
    /// The worker of this task.
    Task(String),
    /// No session reported, in the running supervisor's pane: it may be the supervisor.
    Unconfirmed,
    Observed,
}

impl Owner {
    /// Whether the agent counts as an observed session. An unconfirmed agent does, except that
    /// `flow task adopt` refuses it.
    pub fn is_observed(&self) -> bool {
        matches!(self, Owner::Observed | Owner::Unconfirmed)
    }
}

/// Who owns `agent`, listed on `host`, given the running supervisor's `lock` and `tasks`. A
/// session id equal to the lock's or to a task's not yet closed decides. With no id, the agent
/// is an open task's when its pane is the task's target and the task has recorded no session,
/// else unconfirmed in the lock's pane.
pub fn owner(
    host: &str,
    agent: &Agent,
    lock: Option<&Lock>,
    tasks: &[(TaskRecord, Status)],
) -> Owner {
    if let Some(id) = agent.agent_session_id.as_deref() {
        if lock.and_then(|lock| lock.claude_session.as_deref()) == Some(id) {
            return Owner::Supervisor;
        }
        let task = tasks.iter().find(|(_, status)| {
            status.herdr.is_some()
                && status
                    .claude_session
                    .as_ref()
                    .is_some_and(|claude| claude.session_id == id)
        });
        return match task {
            Some((record, _)) => Owner::Task(record.id.clone()),
            None => Owner::Observed,
        };
    }
    let in_pane = |target: &HerdrTarget| target.is(host, &agent.session, &agent.pane_id);
    let task = tasks
        .iter()
        .filter(|(_, status)| {
            status.state.is_open()
                && status.claude_session.is_none()
                && status.herdr.as_ref().is_some_and(in_pane)
        })
        .max_by_key(|(record, _)| record.created);
    if let Some((record, _)) = task {
        return Owner::Task(record.id.clone());
    }
    if lock.is_some_and(|lock| in_pane(&lock.herdr)) {
        return Owner::Unconfirmed;
    }
    Owner::Observed
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::Timestamp;

    use super::*;
    use crate::herdr::AgentStatus;
    use crate::state::task::{ClaudeSession, Harness, Origin, State};

    fn target(pane_id: &str) -> HerdrTarget {
        HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: "w1".to_owned(),
            pane_id: pane_id.to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        }
    }

    fn agent(pane_id: &str, session_id: Option<&str>) -> Agent {
        Agent {
            pane_id: pane_id.to_owned(),
            workspace_id: "w1".to_owned(),
            agent: "claude".to_owned(),
            status: AgentStatus::Idle,
            cwd: PathBuf::from("/work/repo"),
            title: String::new(),
            session: "default".to_owned(),
            agent_session_id: session_id.map(str::to_owned),
        }
    }

    fn lock(claude_session: Option<&str>) -> Lock {
        Lock {
            host_env_id: "env-0001".to_owned(),
            herdr: target("w1:p1"),
            claude_session: claude_session.map(str::to_owned),
            since: Timestamp::UNIX_EPOCH,
            stopped: None,
        }
    }

    fn task(
        id: &str,
        state: State,
        pane_id: Option<&str>,
        session_id: Option<&str>,
    ) -> (TaskRecord, Status) {
        let record = TaskRecord {
            id: id.to_owned(),
            parent: None,
            title: "a task".to_owned(),
            host: "host-a".to_owned(),
            cwd: PathBuf::from("/work/repo"),
            repo: None,
            harness: Harness::Claude,
            created: Timestamp::UNIX_EPOCH,
            owner: "supervisor".to_owned(),
            origin: Origin::Dispatched,
        };
        let mut status = Status::pending(Timestamp::UNIX_EPOCH);
        status.state = state;
        status.herdr = pane_id.map(target);
        status.claude_session = session_id.map(|session_id| ClaudeSession {
            session_id: session_id.to_owned(),
            transcript_path: PathBuf::new(),
        });
        (record, status)
    }

    fn is_task(id: &str) -> Owner {
        Owner::Task(id.to_owned())
    }

    #[test]
    fn session_ids_decide_and_a_pane_decides_only_without_one() {
        let sup = lock(Some("s-sup"));
        let unrecorded = lock(None);
        let working = |pane, session| task("aaaaaaab", State::Working, pane, session);
        let done = |pane, session| task("aaaaaaab", State::Done, pane, session);
        let cases = [
            (
                "lock's id",
                agent("w9:p9", Some("s-sup")),
                Some(&sup),
                vec![],
                Owner::Supervisor,
            ),
            (
                "lock's id beats a task target",
                agent("w1:p2", Some("s-sup")),
                Some(&sup),
                vec![working(Some("w1:p2"), None)],
                Owner::Supervisor,
            ),
            (
                "task's id anywhere",
                agent("w9:p9", Some("s-w")),
                Some(&sup),
                vec![working(Some("w1:p2"), Some("s-w"))],
                is_task("aaaaaaab"),
            ),
            (
                "done but not closed",
                agent("w1:p2", Some("s-w")),
                None,
                vec![done(Some("w1:p2"), Some("s-w"))],
                is_task("aaaaaaab"),
            ),
            (
                "closed",
                agent("w1:p2", Some("s-w")),
                None,
                vec![done(None, Some("s-w"))],
                Owner::Observed,
            ),
            (
                "another id in a task's pane",
                agent("w1:p2", Some("s-hand")),
                None,
                vec![working(Some("w1:p2"), Some("s-w"))],
                Owner::Observed,
            ),
            (
                "an id in a task's pane before its session is recorded",
                agent("w1:p2", Some("s-hand")),
                None,
                vec![working(Some("w1:p2"), None)],
                Owner::Observed,
            ),
            (
                "no id in an open task's pane before its session is recorded",
                agent("w1:p2", None),
                None,
                vec![working(Some("w1:p2"), None)],
                is_task("aaaaaaab"),
            ),
            (
                "no id in a task's pane after its session is recorded",
                agent("w1:p2", None),
                None,
                vec![working(Some("w1:p2"), Some("s-w"))],
                Owner::Observed,
            ),
            (
                "no id in a done task's pane",
                agent("w1:p2", None),
                None,
                vec![done(Some("w1:p2"), None)],
                Owner::Observed,
            ),
            (
                "no id in the lock's pane",
                agent("w1:p1", None),
                Some(&sup),
                vec![],
                Owner::Unconfirmed,
            ),
            (
                "no id in the lock's pane that is an open task's target",
                agent("w1:p1", None),
                Some(&unrecorded),
                vec![working(Some("w1:p1"), None)],
                is_task("aaaaaaab"),
            ),
            (
                "another id in the lock's pane",
                agent("w1:p1", Some("s-hand")),
                Some(&sup),
                vec![],
                Owner::Observed,
            ),
            (
                "no id in the lock's pane without a running lock",
                agent("w1:p1", None),
                None,
                vec![],
                Owner::Observed,
            ),
        ];
        for (case, agent, lock, tasks, expected) in cases {
            assert_eq!(owner("host-a", &agent, lock, &tasks), expected, "{case}");
        }
    }

    #[test]
    fn a_pane_is_named_by_host_and_herdr_session_too() {
        let tasks = [task("aaaaaaab", State::Working, Some("w1:p2"), None)];
        let elsewhere = Agent {
            session: "flow".to_owned(),
            ..agent("w1:p2", None)
        };
        assert_eq!(owner("host-a", &elsewhere, None, &tasks), Owner::Observed);
        assert_eq!(
            owner("host-b", &agent("w1:p2", None), None, &tasks),
            Owner::Observed
        );
        assert_eq!(
            owner("host-b", &agent("w1:p1", None), Some(&lock(None)), &[]),
            Owner::Observed
        );
    }
}
