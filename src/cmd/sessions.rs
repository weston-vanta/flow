//! `flow sessions`, `flow follow`, `flow unfollow`: herdr agents across hosts, matched to tasks or
//! shown as adopted, and the hand-started sessions the supervisor follows. Any hand-started
//! session wakes the supervisor when it turns blocked; a followed one also when it goes idle or
//! done. `follow` and `unfollow` name a session by its title as `flow sessions` shows it, or by
//! its pane id.

use std::path::Path;
use std::sync::Arc;

use jiff::Timestamp;

use super::Refused;
use crate::adopt::{self, Adopted, FollowList, Followed};
use crate::herdr::{self, Agent, AgentStatus};
use crate::host;
use crate::inventory::{self, FlowServer, HostInventory};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::task::{Status, TaskRecord};

const ADOPTED: &str = "adopted";
const FOLLOWED: &str = "followed";
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
    let claude_home = adopt::claude_home()?;
    let registry = adopt::registry_sessions(&claude_home)?;
    let mut adopted = adopt::match_agents(&agents_by_host(&inventories), &tasks, &registry);
    mark_followed(&mut adopted, &FollowList::load(state)?.follow);
    if json || summaries {
        add_summaries(&mut adopted, &claude_home);
    }
    if json {
        super::print_json(&adopted)
    } else if adopted.is_empty() {
        super::print("no agents\n")
    } else {
        let local = host::local_env_id();
        let local_host = inventories
            .iter()
            .find(|inv| Some(inv.env_id.as_str()) == local.as_deref())
            .map(|inv| inv.name.as_str());
        super::print(&render_text(&adopted, &tasks, local_host, summaries))
    }
}

/// How many cached agents run no task: the adopted sessions as of each host's last inventory.
pub fn count_adopted(state: &StateDir, tasks: &[(TaskRecord, Status)]) -> anyhow::Result<usize> {
    let inventories: Vec<HostInventory> = inventory::load_all(state)?
        .into_iter()
        .filter(|inv| !inv.gone)
        .collect();
    let adopted = adopt::match_agents(&agents_by_host(&inventories), tasks, &[]);
    Ok(adopted.iter().filter(|entry| entry.task.is_none()).count())
}

fn agents_by_host(inventories: &[HostInventory]) -> Vec<(&str, &[Agent])> {
    inventories
        .iter()
        .map(|inv| (inv.name.as_str(), inv.agents.as_slice()))
        .collect()
}

/// Adds the hand session `target` names on `host` to the follow list, recording its title.
pub fn run_follow(state: &StateDir, host: &str, target: &str) -> anyhow::Result<()> {
    let hands = hand_sessions(state, host)?;
    let agent = resolve(&hands, host, target)?;
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

/// Removes the hand session `target` names on `host` from the follow list. A pane id the list
/// holds is removed even when no session runs there any more.
pub fn run_unfollow(state: &StateDir, host: &str, target: &str) -> anyhow::Result<()> {
    let hands = hand_sessions(state, host)?;
    let stale = |entry: &Followed| entry.host == host && entry.pane_id == target;
    let resolved = match resolve(&hands, host, target) {
        Ok(agent) => Some(agent),
        Err(_) if FollowList::load(state)?.follow.iter().any(stale) => None,
        Err(refused) => return Err(refused.into()),
    };
    let removed = FollowList::update(state, |list| {
        let before = list.follow.len();
        list.follow.retain(|entry| match resolved {
            Some(agent) => !entry.is(host, &agent.session, &agent.pane_id),
            None => !stale(entry),
        });
        list.follow.len() < before
    })?;
    if !removed {
        return Err(Refused(format!("not following {target} on {host}")).into());
    }
    Ok(())
}

/// The hand sessions on `host`: its agents outside flow's herdr session, after one listing of
/// the host refreshed its cached inventory. When the listing fails, the cached agents, saying
/// so on stderr; none when `host` is not a live host.
fn hand_sessions(state: &StateDir, host: &str) -> anyhow::Result<Vec<Agent>> {
    let Some(host) = inventory::live_hosts(state)?
        .into_iter()
        .find(|known| known.name == host)
    else {
        return Ok(Vec::new());
    };
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let listed = inventory::list_agents(
        state,
        &runner,
        &config.herdr_sessions,
        FlowServer::Start,
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
    Ok(agents
        .into_iter()
        .filter(|agent| agent.session != herdr::SESSION)
        .collect())
}

/// The hand session titled `target`, else the one in pane `target`. Refuses when several share
/// the title, listing them, or when none matches.
fn resolve<'a>(hands: &'a [Agent], host: &str, target: &str) -> Result<&'a Agent, Refused> {
    let titled: Vec<&Agent> = hands.iter().filter(|agent| agent.title == target).collect();
    match titled.as_slice() {
        [agent] => return Ok(agent),
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
    hands
        .iter()
        .find(|agent| agent.pane_id == target)
        .ok_or_else(|| {
            Refused(format!(
                "no hand session titled or in pane {target} on {host}; run flow sessions"
            ))
        })
}

/// Marks each hand session `follow` names as followed.
fn mark_followed(adopted: &mut [Adopted], follow: &[Followed]) {
    for entry in adopted {
        let agent = &entry.agent;
        entry.followed = entry.task.is_none()
            && agent.session != herdr::SESSION
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
    inventory::list_agents_on(state, runner, sessions, FlowServer::Start, hosts, now)
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
fn add_summaries(adopted: &mut [Adopted], claude_home: &Path) {
    for entry in adopted {
        let Some(session) = &entry.session else {
            continue;
        };
        match adopt::latest_summary(&adopt::transcript_path(claude_home, session)) {
            Ok(summary) => entry.summary = summary,
            Err(err) => eprintln!("{err:#}"),
        }
    }
}

/// A table of `HOST  STATUS  TASK  TITLE`, the local host's agents first, then by host, status
/// (`working`, `blocked`, the rest), and title. TASK is the agent's task, or `followed` or
/// `adopted` for a hand-started session the supervisor follows or not; TITLE is
/// the task's title for a worker, else herdr's title, or the pane id when there is none. With
/// `summaries`, a known summary follows its agent on a line of its own, indented to TITLE and
/// cut to `SUMMARY_WIDTH`.
fn render_text(
    adopted: &[Adopted],
    tasks: &[(TaskRecord, Status)],
    local_host: Option<&str>,
    summaries: bool,
) -> String {
    let mut sorted: Vec<(&Adopted, &str)> = adopted
        .iter()
        .map(|entry| (entry, title(entry, tasks)))
        .collect();
    sorted.sort_by(|a, b| sort_key(a, local_host).cmp(&sort_key(b, local_host)));
    let rows: Vec<Vec<String>> = std::iter::once(HEADER.map(str::to_owned).to_vec())
        .chain(sorted.iter().map(|(entry, title)| {
            vec![
                entry.host.clone(),
                entry.agent.status.as_str().to_owned(),
                entry.task.clone().unwrap_or_else(|| {
                    let mark = if entry.followed { FOLLOWED } else { ADOPTED };
                    mark.to_owned()
                }),
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
    (entry, title): &(&'a Adopted, &'a str),
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
fn title<'a>(entry: &'a Adopted, tasks: &'a [(TaskRecord, Status)]) -> &'a str {
    let task_title = entry.task.as_deref().and_then(|id| {
        tasks
            .iter()
            .find(|(record, _)| record.id == id)
            .map(|(record, _)| record.title.as_str())
    });
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
    use crate::state::task::Harness;

    fn entry(host: &str, status: AgentStatus, title: &str, task: Option<&str>) -> Adopted {
        Adopted {
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
            },
            task: task.map(str::to_owned),
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
        };
        (record, Status::pending(Timestamp::UNIX_EPOCH))
    }

    fn summarized(summary: &str, entry: Adopted) -> Adopted {
        Adopted {
            summary: Some(summary.to_owned()),
            ..entry
        }
    }

    #[test]
    fn render_lays_out_a_header_and_a_summary_line_per_agent() {
        let adopted = [
            entry("host-b", AgentStatus::Idle, "Write notes", Some("2sqz6nfw")),
            summarized(
                "Building feature-x; waiting on review.",
                entry("host-a", AgentStatus::Working, "Build feature-x", None),
            ),
        ];

        assert_eq!(
            render_text(&adopted, &[], Some("host-a"), true),
            "HOST    STATUS   TASK      TITLE\n\
             host-a  working  adopted   Build feature-x\n\
             \x20                          Building feature-x; waiting on review.\n\
             host-b  idle     2sqz6nfw  Write notes\n"
        );
    }

    #[test]
    fn render_puts_the_local_host_first_then_sorts_by_host_status_and_title() {
        let adopted = [
            entry("host-a", AgentStatus::Idle, "s1", None),
            entry("host-c", AgentStatus::Idle, "s2", None),
            entry("host-b", AgentStatus::Done, "s3", None),
            entry("host-b", AgentStatus::Idle, "s4", None),
            entry("host-b", AgentStatus::Blocked, "s5", None),
            entry("host-b", AgentStatus::Unknown, "s0", None),
            entry("host-b", AgentStatus::Working, "s6", None),
        ];

        let titles: Vec<String> = render_text(&adopted, &[], Some("host-c"), true)
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
        let adopted = [summarized(
            &summary,
            entry("host-a", AgentStatus::Idle, "s1", None),
        )];

        let text = render_text(&adopted, &[], None, true);

        let line = text.lines().nth(2).unwrap().trim_start();
        assert_eq!(line, format!("{}é…", "a".repeat(98)));
        assert_eq!(line.chars().count(), 100);
        let short = format!("{}é", "a".repeat(99));
        let adopted = [summarized(
            &short,
            entry("host-a", AgentStatus::Idle, "s1", None),
        )];
        assert_eq!(
            render_text(&adopted, &[], None, true)
                .lines()
                .nth(2)
                .unwrap()
                .trim_start(),
            short
        );
    }

    #[test]
    fn render_names_an_untitled_agent_by_its_pane_and_skips_a_missing_summary() {
        let adopted = [
            entry("host-a", AgentStatus::Idle, "", None),
            summarized("", entry("host-a", AgentStatus::Idle, "s1", None)),
        ];

        assert_eq!(
            render_text(&adopted, &[], None, true),
            "HOST    STATUS  TASK     TITLE\n\
             host-a  idle    adopted  s1\n\
             host-a  idle    adopted  w1:p3\n"
        );
    }

    #[test]
    fn render_has_no_trailing_whitespace() {
        let adopted = [
            summarized(
                "trailing spaces   ",
                entry("host-a", AgentStatus::Working, "padded title  ", None),
            ),
            entry("host-b", AgentStatus::Idle, "s1", Some("aaaaaaab")),
        ];

        let text = render_text(&adopted, &[], Some("host-a"), true);

        assert!(text.ends_with('\n'));
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "{text:?}");
        }
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn render_without_summaries_prints_one_line_per_agent() {
        let adopted = [summarized(
            "Building feature-x; waiting on review.",
            entry("host-a", AgentStatus::Working, "Build feature-x", None),
        )];

        assert_eq!(
            render_text(&adopted, &[], Some("host-a"), false),
            "HOST    STATUS   TASK     TITLE\n\
             host-a  working  adopted  Build feature-x\n"
        );
    }

    #[test]
    fn render_titles_a_worker_by_its_task() {
        let adopted = [
            entry("host-a", AgentStatus::Working, "aaaaaaab", Some("aaaaaaab")),
            entry("host-a", AgentStatus::Idle, "Hand session", None),
        ];
        let tasks = [task("aaaaaaab", "Build feature-x")];

        assert_eq!(
            render_text(&adopted, &tasks, None, false),
            "HOST    STATUS   TASK      TITLE\n\
             host-a  working  aaaaaaab  Build feature-x\n\
             host-a  idle     adopted   Hand session\n"
        );
    }
}
