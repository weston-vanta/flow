//! Who a herdr agent belongs to: the supervisor, a task's worker, or nobody flow manages. herdr
//! reuses pane and workspace ids after a server restart, so the Claude Code session id herdr
//! reports decides. A pane decides only for an agent herdr reports no id for: a task's pane in a
//! tab labelled with the task's id, or under any label for a task adopted from a session that
//! reported no id; else the lock's pane, which `flow up` ran in, holds the supervisor.

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
    Observed,
}

/// Who owns `agent` on `host`, given the supervisor's `lock` and `tasks`. A reported
/// session id decides; with none, the newest task that `claims` the agent's pane owns it, else
/// the supervisor does in the lock's pane.
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
        return Owner::Supervisor;
    }
    Owner::Observed
}

fn in_pane(host: &str, agent: &Agent, target: &HerdrTarget) -> bool {
    target.is(host, &agent.session, &agent.pane_id)
}

/// Whether an agent with no id in `task`'s pane is the task's only under a tab label naming it:
/// the task was dispatched, or adopted from a session that reported an id.
fn label_decides((record, status): &(TaskRecord, Status)) -> bool {
    record.origin == Origin::Dispatched || status.claude_session.is_some()
}

/// Whether `task` records `agent`'s pane as its target and, where `label_decides`, the agent's
/// tab label names the task.
fn claims(host: &str, agent: &Agent, task: &(TaskRecord, Status)) -> bool {
    let (record, status) = task;
    status
        .herdr
        .as_ref()
        .is_some_and(|target| in_pane(host, agent, target))
        && (!label_decides(task)
            || agent
                .tab_label
                .as_deref()
                .is_some_and(|label| herdr::label_names_task(label, &record.id)))
}

/// Whether `agent`'s tab label can decide its owner: herdr reports no id for it, and a task
/// whose label decides records its pane.
pub fn needs_label(host: &str, agent: &Agent, tasks: &[(TaskRecord, Status)]) -> bool {
    agent.agent_session_id.is_none()
        && tasks.iter().any(|task| {
            label_decides(task)
                && task
                    .1
                    .herdr
                    .as_ref()
                    .is_some_and(|target| in_pane(host, agent, target))
        })
}

/// Labels `agents`, listed in `herdr`'s session on `host`, with one tab listing when any of them
/// `needs_label`.
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
        let tabs = herdr.tabs_if_running(host)?.unwrap_or_default();
        herdr::label_agents(&tabs, agents);
    }
    Ok(())
}

/// The tasks among `tasks` whose recorded pane on `host` holds one of `agents` with no id under
/// a tab label naming another, where the label decides, with the target and why: herdr
/// reused the pane.
pub fn reused_targets(
    host: &str,
    agents: &[Agent],
    tasks: &[(TaskRecord, Status)],
) -> Vec<(String, HerdrTarget, String)> {
    let mut reused = Vec::new();
    for agent in agents
        .iter()
        .filter(|agent| agent.agent_session_id.is_none())
    {
        let Some(label) = agent.tab_label.as_deref() else {
            continue;
        };
        for task @ (record, status) in tasks {
            let Some(target) = status
                .herdr
                .as_ref()
                .filter(|target| in_pane(host, agent, target))
            else {
                continue;
            };
            if label_decides(task) && !herdr::label_names_task(label, &record.id) {
                let why = format!(
                    "herdr reused pane {} on {host}: its tab {} is labelled \"{label}\", not \
                     task {}'s",
                    agent.pane_id, agent.tab_id, record.id
                );
                reused.push((record.id.clone(), target.clone(), why));
            }
        }
    }
    reused
}

/// Clears task `id`'s herdr target if it is still `target`, as for a tab already gone, and
/// logs `why` as a `target.retired` event; `false` when the target had changed.
pub fn retire_target(
    store: &Store,
    id: &str,
    target: &HerdrTarget,
    why: &str,
) -> anyhow::Result<bool> {
    let event = Event {
        ts: Timestamp::now(),
        source: "flow".to_owned(),
        kind: TARGET_RETIRED.to_owned(),
        data: json!({ "reason": why }),
    };
    store.clear_target(id, target, &event)
}

/// `retire_target` for each of `reused_targets`; returns the retired task ids.
pub fn retire_reused(
    store: &Store,
    host: &str,
    agents: &[Agent],
    tasks: &[(TaskRecord, Status)],
) -> anyhow::Result<Vec<String>> {
    let mut retired = Vec::new();
    for (id, target, why) in reused_targets(host, agents, tasks) {
        if retire_target(store, &id, &target, &why)? {
            retired.push(id);
        }
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
            tab_id: String::new(),
            pane_id: pane_id.to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        }
    }

    fn agent(pane_id: &str, session_id: Option<&str>) -> Agent {
        Agent {
            pane_id: pane_id.to_owned(),
            workspace_id: "w1".to_owned(),
            tab_id: String::new(),
            agent: "claude".to_owned(),
            status: AgentStatus::Idle,
            cwd: PathBuf::from("/work/repo"),
            title: String::new(),
            session: "default".to_owned(),
            agent_session_id: session_id.map(str::to_owned),
            tab_label: Some("a task · aaaaaaab".to_owned()),
        }
    }

    fn labelled(label: Option<&str>, agent: Agent) -> Agent {
        Agent {
            tab_label: label.map(str::to_owned),
            ..agent
        }
    }

    fn lock(claude_session: Option<&str>) -> Lock {
        Lock {
            host_env_id: "env-0001".to_owned(),
            herdr: target("w1:p1"),
            claude_session: claude_session.map(str::to_owned),
            since: Timestamp::UNIX_EPOCH,
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
        let adopted_with = |session| {
            let (record, status) = task("aaaaaaab", State::Working, Some("w1:p2"), session);
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
                "no id under a tab label naming another",
                labelled(Some("Notes"), agent("w1:p2", None)),
                None,
                vec![done(Some("w1:p2"), Some("s-w"))],
                Owner::Observed,
            ),
            (
                "no id under a tab label flow did not look up",
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
                "no id in an adopted task's pane after its session is recorded",
                labelled(Some("Notes"), agent("w1:p2", None)),
                None,
                vec![adopted_with(Some("s-w"))],
                Owner::Observed,
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
                labelled(None, agent("w1:p1", None)),
                Some(&sup),
                vec![],
                Owner::Supervisor,
            ),
            (
                "no id in the lock's pane under any label",
                labelled(Some("Notes"), agent("w1:p1", None)),
                Some(&sup),
                vec![],
                Owner::Supervisor,
            ),
            (
                "no id outside the lock's pane",
                labelled(None, agent("w1:p9", None)),
                Some(&sup),
                vec![],
                Owner::Observed,
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
                "no id in the lock's pane without a lock",
                labelled(None, agent("w1:p1", None)),
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
        assert_eq!(reused[0].1, target("w1:p2"));
        assert!(
            reused[0].2.contains(r#"labelled "Notes""#),
            "{}",
            reused[0].2
        );
        assert!(reused_targets("host-a", &[agent("w1:p2", None)], &tasks).is_empty());
        assert!(reused_targets("host-a", &[unlabelled], &tasks).is_empty());
    }

    #[test]
    fn an_adopted_tasks_pane_is_judged_by_label_once_its_session_is_recorded() {
        let adopted = |session| {
            let (record, status) = task("aaaaaaab", State::Working, Some("w1:p2"), session);
            let record = TaskRecord {
                origin: Origin::Adopted,
                ..record
            };
            [(record, status)]
        };
        let user = labelled(Some("Notes"), agent("w1:p2", None));
        let unlabelled = labelled(None, agent("w1:p2", None));

        assert!(needs_label("host-a", &unlabelled, &adopted(Some("s-w"))));
        assert!(!needs_label("host-a", &unlabelled, &adopted(None)));
        let reused = reused_targets("host-a", std::slice::from_ref(&user), &adopted(Some("s-w")));
        assert_eq!(reused.len(), 1);
        assert!(reused_targets("host-a", &[user], &adopted(None)).is_empty());
    }

    #[test]
    fn retiring_clears_only_the_target_judged_reused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("state")).unwrap();
        let state = crate::paths::StateDir::resolve(dir.path()).unwrap();
        let store = Store::open(&state);
        let (record, _) = task("aaaaaaab", State::Working, None, None);
        store.create_task(record, "brief").unwrap();
        store
            .set_status("aaaaaaab", |status| status.herdr = Some(target("w1:p3")))
            .unwrap();
        let before = store.task("aaaaaaab").unwrap().1.last_event;

        assert!(!retire_target(&store, "aaaaaaab", &target("w1:p2"), "stale").unwrap());
        let status = store.task("aaaaaaab").unwrap().1;
        assert_eq!(status.herdr, Some(target("w1:p3")));

        assert!(retire_target(&store, "aaaaaaab", &target("w1:p3"), "reused").unwrap());
        let status = store.task("aaaaaaab").unwrap().1;
        assert_eq!(status.herdr, None);
        assert_eq!(status.last_event, before);
        let dir = store.task_dir("aaaaaaab").unwrap();
        let kinds: Vec<String> = crate::state::events::read_all(&dir)
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect();
        assert_eq!(
            kinds.iter().filter(|kind| *kind == TARGET_RETIRED).count(),
            1
        );
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
