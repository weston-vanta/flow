//! Who a herdr agent belongs to: the supervisor, a task's worker, or nobody flow manages. herdr
//! reuses pane and workspace ids after a server restart, so the Claude Code session id herdr
//! reports decides; a pane decides only for an agent herdr reports no id for, and for a
//! dispatched task only under a workspace label naming the task.

use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::herdr::{self, Agent, Herdr};
use crate::state::Store;
use crate::state::config::Host;
use crate::state::events::Event;
use crate::state::task::{HerdrTarget, Origin, Status, TaskRecord};
use crate::supervisor::Lock;

/// The event a retired herdr target is logged as.
pub const TARGET_RETIRED: &str = "target.retired";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    Supervisor,
    /// The worker of this task.
    Task(String),
    /// No session reported, in the running supervisor's pane: it may be the supervisor, so flow
    /// neither watches it nor lets it be followed or adopted.
    Unconfirmed,
    Observed,
}

/// Who owns `agent` on `host`, given the running supervisor's `lock` and `tasks`. A reported
/// session id decides; with none, the newest task that `claims` the agent's pane owns it, else it
/// is unconfirmed in the lock's pane.
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
    let task = tasks
        .iter()
        .filter(|task| claims(host, agent, task))
        .max_by_key(|(record, _)| record.created);
    if let Some((record, _)) = task {
        return Owner::Task(record.id.clone());
    }
    if lock.is_some_and(|lock| in_pane(host, agent, &lock.herdr)) {
        return Owner::Unconfirmed;
    }
    Owner::Observed
}

fn in_pane(host: &str, agent: &Agent, target: &HerdrTarget) -> bool {
    target.is(host, &agent.session, &agent.pane_id)
}

/// Whether `task` records `agent`'s pane as its target and, for a dispatched task, the agent's
/// workspace label names the task. An adopted task's workspace keeps the user's label.
fn claims(host: &str, agent: &Agent, (record, status): &(TaskRecord, Status)) -> bool {
    status
        .herdr
        .as_ref()
        .is_some_and(|target| in_pane(host, agent, target))
        && (record.origin == Origin::Adopted
            || agent
                .workspace_label
                .as_deref()
                .is_some_and(|label| herdr::label_names_task(label, &record.id)))
}

/// Whether `agent`'s workspace label can decide its owner: herdr reports no id for it, and a
/// dispatched task records its pane.
pub fn needs_label(host: &str, agent: &Agent, tasks: &[(TaskRecord, Status)]) -> bool {
    agent.agent_session_id.is_none()
        && tasks.iter().any(|(record, status)| {
            record.origin == Origin::Dispatched
                && status
                    .herdr
                    .as_ref()
                    .is_some_and(|target| in_pane(host, agent, target))
        })
}

/// Labels `agents`, listed in `herdr`'s session on `host`, with one workspace listing when any
/// of them `needs_label`.
pub fn label_where_needed(
    herdr: &Herdr,
    host: &Host,
    agents: &mut [Agent],
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<()> {
    if agents
        .iter()
        .any(|agent| needs_label(&host.name, agent, tasks))
    {
        let workspaces = herdr.workspaces_if_running(host)?.unwrap_or_default();
        herdr::label_agents(&workspaces, agents);
    }
    Ok(())
}

/// The dispatched tasks among `tasks` whose recorded pane on `host` holds one of `agents` with
/// no id under a workspace label naming another, with why: herdr reused the pane.
pub fn reused_targets(
    host: &str,
    agents: &[Agent],
    tasks: &[(TaskRecord, Status)],
) -> Vec<(String, String)> {
    let mut reused = Vec::new();
    for agent in agents
        .iter()
        .filter(|agent| agent.agent_session_id.is_none())
    {
        let Some(label) = agent.workspace_label.as_deref() else {
            continue;
        };
        for (record, status) in tasks {
            let recorded = status
                .herdr
                .as_ref()
                .is_some_and(|target| in_pane(host, agent, target));
            if record.origin == Origin::Dispatched
                && recorded
                && !herdr::label_names_task(label, &record.id)
            {
                let why = format!(
                    "herdr reused pane {} on {host}: its workspace {} is labelled \"{label}\", \
                     not task {}'s",
                    agent.pane_id, agent.workspace_id, record.id
                );
                reused.push((record.id.clone(), why));
            }
        }
    }
    reused
}

/// Clears task `id`'s herdr target, as for a workspace already gone, and logs `why` as a
/// `target.retired` event.
pub fn retire_target(store: &Store, id: &str, why: &str) -> anyhow::Result<()> {
    store.set_status(id, |status| status.herdr = None)?;
    store.append_event(
        id,
        &Event {
            ts: Timestamp::now(),
            source: "flow".to_owned(),
            kind: TARGET_RETIRED.to_owned(),
            data: json!({ "reason": why }),
        },
    )
}

/// `retire_target` for each of `reused_targets`; returns the retired task ids.
pub fn retire_reused(
    store: &Store,
    host: &str,
    agents: &[Agent],
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<Vec<String>> {
    let mut retired = Vec::new();
    for (id, why) in reused_targets(host, agents, tasks) {
        retire_target(store, &id, &why)?;
        retired.push(id);
    }
    Ok(retired)
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
            workspace_label: Some("a task · aaaaaaab".to_owned()),
        }
    }

    fn labelled(label: Option<&str>, agent: Agent) -> Agent {
        Agent {
            workspace_label: label.map(str::to_owned),
            ..agent
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
        let adopted = |pane| {
            let (record, status) = task("aaaaaaab", State::Working, pane, None);
            let record = TaskRecord {
                origin: Origin::Adopted,
                ..record
            };
            (record, status)
        };
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
                is_task("aaaaaaab"),
            ),
            (
                "no id in a done task's pane before it is closed",
                agent("w1:p2", None),
                None,
                vec![done(Some("w1:p2"), Some("s-w"))],
                is_task("aaaaaaab"),
            ),
            (
                "no id under a workspace label naming another",
                labelled(Some("Notes"), agent("w1:p2", None)),
                None,
                vec![done(Some("w1:p2"), Some("s-w"))],
                Owner::Observed,
            ),
            (
                "no id under a workspace label flow did not look up",
                labelled(None, agent("w1:p2", None)),
                None,
                vec![working(Some("w1:p2"), None)],
                Owner::Observed,
            ),
            (
                "no id in an adopted task's pane under the user's label",
                labelled(Some("Notes"), agent("w1:p2", None)),
                None,
                vec![adopted(Some("w1:p2"))],
                is_task("aaaaaaab"),
            ),
            (
                "no id in a closed task's pane",
                agent("w1:p2", None),
                None,
                vec![done(None, Some("s-w"))],
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
    fn a_dispatched_tasks_pane_under_another_label_is_reused() {
        let tasks = [task("aaaaaaab", State::Done, Some("w1:p2"), Some("s-w"))];
        let unlabelled = labelled(None, agent("w1:p2", None));
        assert!(needs_label("host-a", &unlabelled, &tasks));
        assert!(!needs_label("host-a", &agent("w1:p2", Some("s-x")), &tasks));
        assert!(!needs_label("host-a", &agent("w1:p9", None), &tasks));

        let user = labelled(Some("Notes"), agent("w1:p2", None));
        let reused = reused_targets("host-a", &[user], &tasks);
        assert_eq!(reused.len(), 1);
        assert_eq!(reused[0].0, "aaaaaaab");
        assert!(
            reused[0].1.contains(r#"labelled "Notes""#),
            "{}",
            reused[0].1
        );
        assert!(reused_targets("host-a", &[agent("w1:p2", None)], &tasks).is_empty());
        assert!(reused_targets("host-a", &[unlabelled], &tasks).is_empty());
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
