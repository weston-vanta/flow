//! `flow sessions`, `flow follow`, `flow unfollow`: herdr agents across hosts, matched to tasks or
//! shown as adopted, and the adopted sessions the supervisor follows.

use std::path::Path;
use std::sync::Arc;
use std::thread;

use jiff::Timestamp;

use super::Refused;
use crate::adopt::{self, Adopted, FollowList, Followed};
use crate::herdr::{self, Agent};
use crate::inventory::{self, HostInventory};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::task::{Status, TaskRecord};

const ADOPTED: &str = "adopted";
const SUMMARY_WIDTH: usize = 80;

/// Lists every herdr agent on every live host, asking herdr now. A host that answers gets
/// its cached agents and herdr error replaced; one that does not shows its cached agents.
pub fn run(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let hosts = inventory::live_hosts(state)?;
    let inventories = live_inventories(state, &runner, &config, &hosts, Timestamp::now())?;
    let tasks = Store::open(state).list_tasks()?;
    let claude_home = adopt::claude_home()?;
    let registry = adopt::registry_sessions(&claude_home)?;
    let mut adopted: Vec<Adopted> = inventories
        .iter()
        .flat_map(|inv| adopt::match_agents(&inv.name, &inv.agents, &tasks, &registry))
        .collect();
    add_summaries(&mut adopted, &claude_home);
    if json {
        super::print_json(&adopted)
    } else if adopted.is_empty() {
        super::print("no agents\n")
    } else {
        super::print(&render_text(&adopted))
    }
}

/// How many cached agents run no task: the adopted sessions as of each host's last inventory.
pub fn count_adopted(state: &StateDir, tasks: &[(TaskRecord, Status)]) -> anyhow::Result<usize> {
    Ok(inventory::load_all(state)?
        .iter()
        .filter(|inv| !inv.gone)
        .flat_map(|inv| adopt::match_agents(&inv.name, &inv.agents, tasks, &[]))
        .filter(|adopted| adopted.task.is_none())
        .count())
}

/// Adds the agent in `pane_id` on `host`, as of the cached inventory, to the follow list.
/// A pane id can repeat across herdr sessions; a session other than flow's, where hand-started
/// sessions run, is preferred.
pub fn run_follow(state: &StateDir, host: &str, pane_id: &str) -> anyhow::Result<()> {
    let agents: Vec<Agent> = inventory::load_all(state)?
        .into_iter()
        .filter(|inv| !inv.gone && inv.name == host)
        .flat_map(|inv| inv.agents)
        .filter(|agent| agent.pane_id == pane_id)
        .collect();
    let agent = agents
        .iter()
        .find(|agent| agent.session != herdr::SESSION)
        .or_else(|| agents.first())
        .ok_or_else(|| {
            Refused(format!(
                "no agent in pane {pane_id} on {host} (run flow sessions)"
            ))
        })?;
    let followed = Followed {
        host: host.to_owned(),
        pane_id: pane_id.to_owned(),
        session: agent.session.clone(),
    };
    FollowList::update(state, |list| {
        if !list.follow.contains(&followed) {
            list.follow.push(followed);
        }
    })
}

/// Removes every follow-list entry for `pane_id` on `host`.
pub fn run_unfollow(state: &StateDir, host: &str, pane_id: &str) -> anyhow::Result<()> {
    let removed = FollowList::update(state, |list| {
        let before = list.follow.len();
        list.follow
            .retain(|followed| !(followed.host == host && followed.pane_id == pane_id));
        list.follow.len() < before
    })?;
    if !removed {
        return Err(Refused(format!("not following {pane_id} on {host}")).into());
    }
    Ok(())
}

/// Each of `hosts`' inventory with live agents, asked in parallel, in the order given.
fn live_inventories(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    hosts: &[Host],
    now: Timestamp,
) -> anyhow::Result<Vec<HostInventory>> {
    thread::scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .map(|host| scope.spawn(move || live_inventory(state, runner, config, host, now)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("agent listing thread panicked"))
            .collect()
    })
}

/// `host`'s cached inventory with the agents and herdr error herdr reports now, saved; or, when
/// the host does not answer, its cached inventory as is. Any herdr error is printed on stderr.
fn live_inventory(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    host: &Host,
    now: Timestamp,
) -> anyhow::Result<HostInventory> {
    let cached = inventory::load(state, &host.env_id)?;
    let found = inventory::herdr_agents(runner, host, &config.herdr_sessions);
    if let Some(error) = &found.error {
        eprintln!("{error}");
    }
    if found.unreachable {
        return Ok(cached.unwrap_or_else(|| inventory::unreachable(host, None, now)));
    }
    let updated = inventory::with_agents(host, cached, found, now);
    inventory::save(state, &updated)?;
    Ok(updated)
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

/// `host  pane  status  task-or-adopted  title  summary`, the summary cut to `SUMMARY_WIDTH`.
fn render_text(adopted: &[Adopted]) -> String {
    let rows: Vec<Vec<String>> = adopted
        .iter()
        .map(|entry| {
            let mut row = vec![
                entry.host.clone(),
                entry.agent.pane_id.clone(),
                entry.agent.status.as_str().to_owned(),
                entry.task.clone().unwrap_or_else(|| ADOPTED.to_owned()),
                entry.agent.title.clone(),
                entry
                    .summary
                    .as_deref()
                    .map_or_else(String::new, |summary| {
                        render::truncate(summary, SUMMARY_WIDTH)
                    }),
            ];
            while row.last().is_some_and(String::is_empty) {
                row.pop();
            }
            row
        })
        .collect();
    render::table(&rows)
}
