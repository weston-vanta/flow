//! `flow sessions`, `flow follow`, `flow unfollow`: herdr agents across hosts, matched to tasks or
//! the supervisor or shown as observed, and the hand-started sessions the supervisor follows. Any
//! observed session wakes the supervisor when it turns blocked; a followed one also when it goes
//! idle or done. `follow` and `unfollow` name an observed session by its title as `flow sessions`
//! shows it, or by its pane id, in any herdr session flow lists.

use std::path::Path;
use std::sync::Arc;

use jiff::Timestamp;

use super::Refused;
use crate::herdr::{Agent, AgentStatus};
use crate::host;
use crate::inventory::{self, FlowServer, HostInventory};
use crate::observe::{self, FollowList, Followed, MatchedAgent};
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::task::{Status, TaskRecord};
use crate::supervisor;

const OBSERVED: &str = "observed";
const FOLLOWED: &str = "followed";
const SUPERVISOR: &str = "supervisor";
const UNCONFIRMED: &str = "unconfirmed";
const HEADER: [&str; 4] = ["HOST", "STATUS", "TASK", "TITLE"];
const SUMMARY_WIDTH: usize = 100;

/// Lists every herdr agent on every live host, asking herdr now. A host that answers gets
/// its cached agents and herdr error replaced; one that does not shows its cached agents.
/// Session summaries are read for `json` and `summaries`.
pub fn run(state: &StateDir, json: bool, summaries: bool) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let hosts = inventory::live_hosts(state)?;
    let inventories = live_inventories(state, &runner, &config, &hosts, Timestamp::now())?;
    let tasks = Store::open(state).list_tasks()?;
    let supervisor = supervisor::running(state)?;
    let claude_home = observe::claude_home()?;
    let registry = observe::registry_sessions(&claude_home)?;
    let mut matched = observe::match_agents(
        &agents_by_host(&inventories),
        &tasks,
        supervisor.as_ref(),
        &registry,
    );
    mark_followed(&mut matched, &FollowList::load(state)?.follow);
    if json || summaries {
        add_summaries(&mut matched, &claude_home);
    }
    if json {
        super::print_json(&matched)
    } else if matched.is_empty() {
        super::print("no agents\n")
    } else {
        let local = host::local_env_id();
        let local_host = inventories
            .iter()
            .find(|inv| Some(inv.env_id.as_str()) == local.as_deref())
            .map(|inv| inv.name.as_str());
        super::print(&render_text(&matched, &tasks, local_host, summaries))
    }
}

/// How many cached agents run neither a task nor the supervisor: the observed sessions as of
/// each host's last inventory.
pub fn count_observed(state: &StateDir, tasks: &[(TaskRecord, Status)]) -> anyhow::Result<usize> {
    let inventories: Vec<HostInventory> = inventory::load_all(state)?
        .into_iter()
        .filter(|inv| !inv.gone)
        .collect();
    let supervisor = supervisor::running(state)?;
    let matched = observe::match_agents(
        &agents_by_host(&inventories),
        tasks,
        supervisor.as_ref(),
        &[],
    );
    Ok(matched
        .iter()
        .filter(|entry| entry.owner == Owner::Observed)
        .count())
}

fn agents_by_host(inventories: &[HostInventory]) -> Vec<(&str, &[Agent])> {
    inventories
        .iter()
        .map(|inv| (inv.name.as_str(), inv.agents.as_slice()))
        .collect()
}

/// Adds the hand session `target` names on `host` to the follow list, recording its title.
pub fn run_follow(state: &StateDir, host: &str, target: &str) -> anyhow::Result<()> {
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let listed = host_agents(state, &runner, host)?;
    let agent = resolve(&listed, host, target)?;
    let followed = Followed {
        host: host.to_owned(),
        pane_id: agent.pane_id.clone(),
        session: agent.session.clone(),
        title: (!agent.title.is_empty()).then(|| agent.title.clone()),
    };
    FollowList::update(state, |list| {
        let existing = list
            .follow
            .iter_mut()
            .find(|entry| entry.is(host, &followed.session, &followed.pane_id));
        match existing {
            Some(entry) => entry.title = followed.title,
            None => list.follow.push(followed),
        }
    })
}

/// Removes the hand session `target` names on `host` from the follow list. A pane the list
/// holds, by its pane id or `<session>/<pane id>`, is removed even when no session runs there any
/// more; a pane id the list holds in several herdr sessions is refused.
pub fn run_unfollow(state: &StateDir, host: &str, target: &str) -> anyhow::Result<()> {
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let listed = host_agents(state, &runner, host)?;
    let (session, pane_id) = match resolve(&listed, host, target) {
        Ok(agent) => (agent.session.clone(), agent.pane_id.clone()),
        Err(refused) => {
            let follow = FollowList::load(state)?.follow;
            match stale_entries(&follow, host, target).as_slice() {
                [] => return Err(refused.into()),
                [entry] => (entry.session.clone(), entry.pane_id.clone()),
                several => {
                    return Err(several_in_pane(
                        host,
                        &several[0].pane_id,
                        several.iter().map(|entry| {
                            let title = entry.title.as_deref().unwrap_or_default();
                            (entry.session.as_str(), title)
                        }),
                    )
                    .into());
                }
            }
        }
    };
    let removed = FollowList::update(state, |list| {
        let before = list.follow.len();
        list.follow
            .retain(|entry| !entry.is(host, &session, &pane_id));
        list.follow.len() < before
    })?;
    if !removed {
        return Err(Refused(format!("not following {target} on {host}")).into());
    }
    Ok(())
}

/// A host's herdr agents in every listed herdr session, split into the observed sessions and
/// the rest.
#[derive(Debug, Default)]
pub(super) struct HostAgents {
    pub observed: Vec<Agent>,
    /// Each other agent, with what holds its pane, as `is <...>` reads.
    pub held: Vec<(Agent, String)>,
}

/// The agents on `host`, after one listing of the host refreshed its cached inventory. When the
/// listing fails, the cached agents, saying so on stderr; none when `host` is not a live host.
pub(super) fn host_agents(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    host: &str,
) -> anyhow::Result<HostAgents> {
    let Some(host) = inventory::live_hosts(state)?
        .into_iter()
        .find(|known| known.name == host)
    else {
        return Ok(HostAgents::default());
    };
    let config = Config::load(state)?;
    let listed = inventory::list_agents(
        state,
        runner,
        &config.herdr_sessions,
        FlowServer::Start(&config.herdr_session),
        &host,
        Timestamp::now(),
    );
    let cached = |problem: &str| {
        eprintln!(
            "could not list agents on {}, using the cached ones: {problem}",
            host.name
        );
    };
    let agents = match listed {
        Ok(listed) => {
            match &listed.error {
                Some(error) if listed.unreachable => cached(error),
                Some(error) => eprintln!(
                    "could not list every herdr session on {}: {error}",
                    host.name
                ),
                None => {}
            }
            listed.inventory.agents
        }
        Err(err) => {
            cached(&format!("{err:#}"));
            inventory::load(state, &host.env_id)?
                .map(|cached| cached.agents)
                .unwrap_or_default()
        }
    };
    let tasks = Store::open(state).list_tasks()?;
    let supervisor = supervisor::running(state)?;
    let mut split = HostAgents::default();
    for agent in agents {
        let holder = match owner::owner(&host.name, &agent, supervisor.as_ref(), &tasks) {
            Owner::Observed => {
                split.observed.push(agent);
                continue;
            }
            Owner::Task(id) => format!("already task {id}"),
            Owner::Supervisor => "the supervisor's".to_owned(),
            Owner::Unconfirmed => "the supervisor's, and herdr reports no Claude Code session for \
                                   the agent there, so flow cannot tell it from the supervisor"
                .to_owned(),
        };
        split.held.push((agent, holder));
    }
    Ok(split)
}

/// The observed session titled `target`, else the one in pane `target`, a pane id or
/// `<session>/<pane id>`. Refuses when several share the title or the pane id, listing them, or
/// when none matches, naming what holds the pane when `target` names a task's or the
/// supervisor's agent instead.
pub(super) fn resolve<'a>(
    listed: &'a HostAgents,
    host: &str,
    target: &str,
) -> Result<&'a Agent, Refused> {
    let hands = &listed.observed;
    let titled: Vec<&Agent> = hands.iter().filter(|agent| agent.title == target).collect();
    match titled.as_slice() {
        [hand] => return Ok(hand),
        [] => {}
        several => {
            let candidates: Vec<String> = several
                .iter()
                .map(|agent| format!("{}  {}", agent.pane_id, agent.title))
                .collect();
            return Err(Refused(format!(
                "several hand sessions on {host} are titled \"{target}\"; pass the pane id:\n{}",
                candidates.join("\n")
            )));
        }
    }
    let (session, pane_id) = pane_ref(target);
    let in_pane: Vec<&Agent> = hands
        .iter()
        .filter(|agent| {
            agent.pane_id == pane_id && session.is_none_or(|session| agent.session == session)
        })
        .collect();
    let held = listed.held.iter().find(|(agent, _)| {
        agent.title == target
            || (agent.pane_id == pane_id && session.is_none_or(|session| agent.session == session))
    });
    match (in_pane.as_slice(), held) {
        ([hand], _) => Ok(hand),
        ([], Some((agent, holder))) => Err(Refused(format!(
            "pane {} on {host} is {holder}",
            agent.pane_id
        ))),
        ([], None) => Err(Refused(format!(
            "no hand session titled or in pane {target} on {host}; run flow sessions"
        ))),
        (several, _) => Err(several_in_pane(
            host,
            pane_id,
            several
                .iter()
                .map(|agent| (agent.session.as_str(), agent.title.as_str())),
        )),
    }
}

/// A pane named as `<session>/<pane id>`, or by its pane id alone.
fn pane_ref(target: &str) -> (Option<&str>, &str) {
    match target.split_once('/') {
        Some((session, pane_id)) => (Some(session), pane_id),
        None => (None, target),
    }
}

/// The follow list's entries for `target` on `host`, a pane id or `<session>/<pane id>`.
fn stale_entries<'a>(follow: &'a [Followed], host: &str, target: &str) -> Vec<&'a Followed> {
    let (session, pane_id) = pane_ref(target);
    follow
        .iter()
        .filter(|entry| {
            entry.host == host
                && entry.pane_id == pane_id
                && session.is_none_or(|session| entry.session == session)
        })
        .collect()
}

/// The refusal for a pane id that several herdr sessions on `host` hold, listing each
/// `(session, title)` as `<session>/<pane id>  <title>`.
fn several_in_pane<'a>(
    host: &str,
    pane_id: &str,
    candidates: impl Iterator<Item = (&'a str, &'a str)>,
) -> Refused {
    let lines: Vec<String> = candidates
        .map(|(session, title)| format!("{session}/{pane_id}  {title}"))
        .collect();
    Refused(format!(
        "several hand sessions on {host} are in pane {pane_id}; pass <session>/<pane id>:\n{}",
        lines.join("\n")
    ))
}

/// Marks each observed session `follow` names as followed.
fn mark_followed(matched: &mut [MatchedAgent], follow: &[Followed]) {
    for entry in matched {
        let agent = &entry.agent;
        entry.followed = entry.owner == Owner::Observed
            && follow
                .iter()
                .any(|followed| followed.is(&entry.host, &agent.session, &agent.pane_id));
    }
}

/// Each of `hosts`' inventory with live agents, asked in parallel, in the order given. Any
/// herdr error is printed on stderr.
fn live_inventories(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    hosts: &[Host],
    now: Timestamp,
) -> anyhow::Result<Vec<HostInventory>> {
    let sessions = &config.herdr_sessions;
    let start = FlowServer::Start(&config.herdr_session);
    inventory::list_agents_on(state, runner, sessions, start, hosts, now)
        .into_iter()
        .map(|listed| {
            let listed = listed?;
            if let Some(error) = &listed.error {
                eprintln!("{error}");
            }
            Ok(listed.inventory)
        })
        .collect()
}

/// Reads each matched session's latest summary. A transcript that cannot be read leaves the
/// summary empty and is reported on stderr.
fn add_summaries(matched: &mut [MatchedAgent], claude_home: &Path) {
    for entry in matched {
        let Some(session) = &entry.session else {
            continue;
        };
        match observe::latest_summary(&observe::transcript_path(claude_home, session)) {
            Ok(summary) => entry.summary = summary,
            Err(err) => eprintln!("{err:#}"),
        }
    }
}

/// A table of `HOST  STATUS  TASK  TITLE`, the local host's agents first, then by host, status
/// (`working`, `blocked`, the rest), and title. TASK is the agent's task, `supervisor` for the
/// supervisor, or `followed` or `observed` for a hand-started session the supervisor follows or
/// not; TITLE is
/// the task's title for a worker, else herdr's title, or the pane id when there is none. With
/// `summaries`, a known summary follows its agent on a line of its own, indented to TITLE and
/// cut to `SUMMARY_WIDTH`.
fn render_text(
    matched: &[MatchedAgent],
    tasks: &[(TaskRecord, Status)],
    local_host: Option<&str>,
    summaries: bool,
) -> String {
    let mut sorted: Vec<(&MatchedAgent, &str)> = matched
        .iter()
        .map(|entry| (entry, title(entry, tasks)))
        .collect();
    sorted.sort_by(|a, b| sort_key(a, local_host).cmp(&sort_key(b, local_host)));
    let rows: Vec<Vec<String>> = std::iter::once(HEADER.map(str::to_owned).to_vec())
        .chain(sorted.iter().map(|(entry, title)| {
            vec![
                entry.host.clone(),
                entry.agent.status.as_str().to_owned(),
                match &entry.owner {
                    Owner::Task(id) => id.clone(),
                    Owner::Supervisor => SUPERVISOR.to_owned(),
                    Owner::Unconfirmed => UNCONFIRMED.to_owned(),
                    Owner::Observed if entry.followed => FOLLOWED.to_owned(),
                    Owner::Observed => OBSERVED.to_owned(),
                },
                (*title).to_owned(),
            ]
        }))
        .collect();
    let indent = " ".repeat(
        (0..HEADER.len() - 1)
            .map(|i| {
                rows.iter()
                    .map(|row| row[i].chars().count())
                    .max()
                    .unwrap_or(0)
                    + 2
            })
            .sum(),
    );
    let table = render::table(&rows);
    let mut lines = table.lines();
    let mut out = String::new();
    let mut push = |line: &str| {
        out.push_str(line.trim_end());
        out.push('\n');
    };
    if let Some(header) = lines.next() {
        push(header);
    }
    for (line, (entry, _)) in lines.zip(&sorted) {
        push(line);
        let summary = (entry.summary.as_deref()).filter(|s| summaries && !s.trim().is_empty());
        if let Some(summary) = summary {
            push(&format!(
                "{indent}{}",
                render::truncate(summary, SUMMARY_WIDTH)
            ));
        }
    }
    out
}

fn sort_key<'a>(
    (entry, title): &(&'a MatchedAgent, &'a str),
    local_host: Option<&str>,
) -> (bool, &'a str, u8, &'a str) {
    let status = match entry.agent.status {
        AgentStatus::Working => 0,
        AgentStatus::Blocked => 1,
        _ => 2,
    };
    (
        Some(entry.host.as_str()) != local_host,
        &entry.host,
        status,
        title,
    )
}

/// The title of `entry`'s task when it has one, else herdr's title, else its pane id.
fn title<'a>(entry: &'a MatchedAgent, tasks: &'a [(TaskRecord, Status)]) -> &'a str {
    let task_title = match &entry.owner {
        Owner::Task(id) => tasks
            .iter()
            .find(|(record, _)| record.id == *id)
            .map(|(record, _)| record.title.as_str()),
        _ => None,
    };
    match task_title {
        Some(title) => title,
        None if entry.agent.title.is_empty() => &entry.agent.pane_id,
        None => &entry.agent.title,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::state::task::{Harness, Origin};

    fn entry(host: &str, status: AgentStatus, title: &str, task: Option<&str>) -> MatchedAgent {
        MatchedAgent {
            host: host.to_owned(),
            agent: Agent {
                pane_id: "w1:p3".to_owned(),
                workspace_id: "w1".to_owned(),
                agent: "claude".to_owned(),
                status,
                cwd: PathBuf::from("/work/app"),
                title: title.to_owned(),
                session: "default".to_owned(),
                agent_session_id: None,
                workspace_label: None,
            },
            owner: task.map_or(Owner::Observed, |id| Owner::Task(id.to_owned())),
            followed: false,
            session: None,
            matched_by: None,
            summary: None,
        }
    }

    fn task(id: &str, title: &str) -> (TaskRecord, Status) {
        let record = TaskRecord {
            id: id.to_owned(),
            parent: None,
            title: title.to_owned(),
            host: "host-a".to_owned(),
            cwd: PathBuf::from("/work/app.wt/feature-x"),
            repo: Some("app".to_owned()),
            harness: Harness::Claude,
            created: Timestamp::UNIX_EPOCH,
            owner: "supervisor".to_owned(),
            origin: Origin::Dispatched,
        };
        (record, Status::pending(Timestamp::UNIX_EPOCH))
    }

    fn summarized(summary: &str, entry: MatchedAgent) -> MatchedAgent {
        MatchedAgent {
            summary: Some(summary.to_owned()),
            ..entry
        }
    }

    fn hand(session: &str, pane_id: &str, title: &str) -> Agent {
        Agent {
            pane_id: pane_id.to_owned(),
            session: session.to_owned(),
            title: title.to_owned(),
            ..entry("host-b", AgentStatus::Idle, title, None).agent
        }
    }

    fn observed(hands: &[Agent]) -> HostAgents {
        HostAgents {
            observed: hands.to_vec(),
            held: Vec::new(),
        }
    }

    #[test]
    fn resolve_refuses_a_pane_id_several_herdr_sessions_hold() {
        let hands = observed(&[
            hand("default", "w1:p1", "Tidy docs"),
            hand("other", "w1:p1", "Fix links"),
            hand("other", "w1:p2", "Upgrade toolchain"),
        ]);

        let refused = resolve(&hands, "host-b", "w1:p1").unwrap_err();

        assert_eq!(
            refused.0,
            "several hand sessions on host-b are in pane w1:p1; pass <session>/<pane id>:\n\
             default/w1:p1  Tidy docs\n\
             other/w1:p1  Fix links"
        );
        assert_eq!(
            resolve(&hands, "host-b", "other/w1:p1").unwrap().title,
            "Fix links"
        );
        assert_eq!(resolve(&hands, "host-b", "w1:p2").unwrap().session, "other");
        assert!(resolve(&hands, "host-b", "default/w1:p2").is_err());
    }

    #[test]
    fn resolve_names_what_holds_a_task_or_supervisor_pane() {
        let listed = HostAgents {
            held: vec![
                (
                    hand("default", "w1:p1", "flow-supervisor"),
                    "the supervisor's".to_owned(),
                ),
                (
                    hand("default", "w2:p1", "Build feature"),
                    "already task aaaaaaab".to_owned(),
                ),
            ],
            ..observed(&[hand("default", "w3:p1", "Tidy docs")])
        };

        assert_eq!(
            resolve(&listed, "host-b", "Build feature").unwrap_err().0,
            "pane w2:p1 on host-b is already task aaaaaaab"
        );
        assert_eq!(
            resolve(&listed, "host-b", "w1:p1").unwrap_err().0,
            "pane w1:p1 on host-b is the supervisor's"
        );
        assert_eq!(
            resolve(&listed, "host-b", "w3:p1").unwrap().title,
            "Tidy docs"
        );
    }

    #[test]
    fn render_marks_the_supervisor() {
        let matched = [MatchedAgent {
            owner: Owner::Supervisor,
            ..entry("host-a", AgentStatus::Idle, "flow-supervisor", None)
        }];

        assert_eq!(
            render_text(&matched, &[], Some("host-a"), false),
            "HOST    STATUS  TASK        TITLE\n\
             host-a  idle    supervisor  flow-supervisor\n"
        );
    }

    #[test]
    fn stale_follow_entries_honour_the_session() {
        let followed = |session: &str| Followed {
            host: "host-b".to_owned(),
            pane_id: "w9:p9".to_owned(),
            session: session.to_owned(),
            title: None,
        };
        let follow = [followed("default"), followed("other")];

        assert_eq!(stale_entries(&follow, "host-b", "w9:p9").len(), 2);
        assert_eq!(
            stale_entries(&follow, "host-b", "other/w9:p9"),
            [&follow[1]]
        );
        assert!(stale_entries(&follow, "host-a", "w9:p9").is_empty());
    }

    #[test]
    fn render_lays_out_a_header_and_a_summary_line_per_agent() {
        let matched = [
            entry("host-b", AgentStatus::Idle, "Write notes", Some("2sqz6nfw")),
            summarized(
                "Building feature-x; waiting on review.",
                entry("host-a", AgentStatus::Working, "Build feature-x", None),
            ),
        ];

        assert_eq!(
            render_text(&matched, &[], Some("host-a"), true),
            "HOST    STATUS   TASK      TITLE\n\
             host-a  working  observed  Build feature-x\n\
             \x20                          Building feature-x; waiting on review.\n\
             host-b  idle     2sqz6nfw  Write notes\n"
        );
    }

    #[test]
    fn render_puts_the_local_host_first_then_sorts_by_host_status_and_title() {
        let matched = [
            entry("host-a", AgentStatus::Idle, "s1", None),
            entry("host-c", AgentStatus::Idle, "s2", None),
            entry("host-b", AgentStatus::Done, "s3", None),
            entry("host-b", AgentStatus::Idle, "s4", None),
            entry("host-b", AgentStatus::Blocked, "s5", None),
            entry("host-b", AgentStatus::Unknown, "s0", None),
            entry("host-b", AgentStatus::Working, "s6", None),
        ];

        let titles: Vec<String> = render_text(&matched, &[], Some("host-c"), true)
            .lines()
            .skip(1)
            .map(|line| line.rsplit(' ').next().unwrap().to_owned())
            .collect();

        assert_eq!(titles, ["s2", "s1", "s6", "s5", "s0", "s3", "s4"]);
    }

    #[test]
    fn render_cuts_the_summary_at_100_characters() {
        // 98 characters, then a two-byte character as the 99th and more text after it.
        let summary = format!("{}éxyz", "a".repeat(98));
        let matched = [summarized(
            &summary,
            entry("host-a", AgentStatus::Idle, "s1", None),
        )];

        let text = render_text(&matched, &[], None, true);

        let line = text.lines().nth(2).unwrap().trim_start();
        assert_eq!(line, format!("{}é…", "a".repeat(98)));
        assert_eq!(line.chars().count(), 100);
        let short = format!("{}é", "a".repeat(99));
        let matched = [summarized(
            &short,
            entry("host-a", AgentStatus::Idle, "s1", None),
        )];
        assert_eq!(
            render_text(&matched, &[], None, true)
                .lines()
                .nth(2)
                .unwrap()
                .trim_start(),
            short
        );
    }

    #[test]
    fn render_names_an_untitled_agent_by_its_pane_and_skips_a_missing_summary() {
        let matched = [
            entry("host-a", AgentStatus::Idle, "", None),
            summarized("", entry("host-a", AgentStatus::Idle, "s1", None)),
        ];

        assert_eq!(
            render_text(&matched, &[], None, true),
            "HOST    STATUS  TASK      TITLE\n\
             host-a  idle    observed  s1\n\
             host-a  idle    observed  w1:p3\n"
        );
    }

    #[test]
    fn render_has_no_trailing_whitespace() {
        let matched = [
            summarized(
                "trailing spaces   ",
                entry("host-a", AgentStatus::Working, "padded title  ", None),
            ),
            entry("host-b", AgentStatus::Idle, "s1", Some("aaaaaaab")),
        ];

        let text = render_text(&matched, &[], Some("host-a"), true);

        assert!(text.ends_with('\n'));
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "{text:?}");
        }
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn render_without_summaries_prints_one_line_per_agent() {
        let matched = [summarized(
            "Building feature-x; waiting on review.",
            entry("host-a", AgentStatus::Working, "Build feature-x", None),
        )];

        assert_eq!(
            render_text(&matched, &[], Some("host-a"), false),
            "HOST    STATUS   TASK      TITLE\n\
             host-a  working  observed  Build feature-x\n"
        );
    }

    #[test]
    fn render_titles_a_worker_by_its_task() {
        let matched = [
            entry("host-a", AgentStatus::Working, "aaaaaaab", Some("aaaaaaab")),
            entry("host-a", AgentStatus::Idle, "Hand session", None),
        ];
        let tasks = [task("aaaaaaab", "Build feature-x")];

        assert_eq!(
            render_text(&matched, &tasks, None, false),
            "HOST    STATUS   TASK      TITLE\n\
             host-a  working  aaaaaaab  Build feature-x\n\
             host-a  idle     observed  Hand session\n"
        );
    }
}
