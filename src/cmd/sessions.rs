//! `flow sessions`, `flow follow`, `flow unfollow`: herdr agents across CDEs, matched to tasks or
//! shown as adopted, and the adopted sessions the supervisor follows.

use std::path::Path;
use std::sync::Arc;
use std::thread;

use jiff::Timestamp;

use super::Refused;
use crate::adopt::{self, Adopted, FollowList, Followed};
use crate::herdr::{self, Agent};
use crate::inventory::{self, CdeInventory};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Cde, Config};
use crate::state::task::{Status, TaskRecord};

const ADOPTED: &str = "adopted";
const SUMMARY_WIDTH: usize = 80;

/// Lists every herdr agent on every configured CDE, asking herdr now. A CDE that answers gets
/// its cached agents replaced; one that does not shows its cached agents.
pub fn run(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let inventories = live_inventories(state, &runner, &config, Timestamp::now())?;
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

/// How many cached agents run no task: the adopted sessions as of each CDE's last inventory.
pub fn count_adopted(state: &StateDir, tasks: &[(TaskRecord, Status)]) -> anyhow::Result<usize> {
    Ok(inventory::load_all(state)?
        .iter()
        .flat_map(|inv| adopt::match_agents(&inv.name, &inv.agents, tasks, &[]))
        .filter(|adopted| adopted.task.is_none())
        .count())
}

/// Adds the agent in `pane_id` on `cde`, as of the cached inventory, to the follow list.
/// A pane id can repeat across herdr sessions; a session other than flow's, where hand-started
/// sessions run, is preferred.
pub fn run_follow(state: &StateDir, cde: &str, pane_id: &str) -> anyhow::Result<()> {
    let agents: Vec<Agent> = inventory::load_all(state)?
        .into_iter()
        .filter(|inv| inv.name == cde)
        .flat_map(|inv| inv.agents)
        .filter(|agent| agent.pane_id == pane_id)
        .collect();
    let agent = agents
        .iter()
        .find(|agent| agent.session != herdr::SESSION)
        .or_else(|| agents.first())
        .ok_or_else(|| {
            Refused(format!(
                "no agent in pane {pane_id} on {cde} (run flow sessions)"
            ))
        })?;
    let followed = Followed {
        cde: cde.to_owned(),
        pane_id: pane_id.to_owned(),
        session: agent.session.clone(),
    };
    FollowList::update(state, |list| {
        if !list.follow.contains(&followed) {
            list.follow.push(followed);
        }
    })
}

/// Removes every follow-list entry for `pane_id` on `cde`.
pub fn run_unfollow(state: &StateDir, cde: &str, pane_id: &str) -> anyhow::Result<()> {
    let removed = FollowList::update(state, |list| {
        let before = list.follow.len();
        list.follow
            .retain(|followed| !(followed.cde == cde && followed.pane_id == pane_id));
        list.follow.len() < before
    })?;
    if !removed {
        return Err(Refused(format!("not following {pane_id} on {cde}")).into());
    }
    Ok(())
}

/// Every configured CDE's inventory with live agents, asked in parallel, in config order.
fn live_inventories(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    now: Timestamp,
) -> anyhow::Result<Vec<CdeInventory>> {
    thread::scope(|scope| {
        let handles: Vec<_> = config
            .cdes
            .iter()
            .map(|cde| scope.spawn(move || live_inventory(state, runner, config, cde, now)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("agent listing thread panicked"))
            .collect()
    })
}

/// `cde`'s cached inventory with the agents herdr reports now, saved; or, when the CDE does not
/// answer, its cached inventory as is.
fn live_inventory(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    cde: &Cde,
    now: Timestamp,
) -> anyhow::Result<CdeInventory> {
    let cached = inventory::load(state, &cde.env_id)?;
    let found = inventory::herdr_agents(runner, cde, &config.herdr_sessions);
    if found.unreachable {
        eprintln!("{}", found.error.as_deref().unwrap_or_default());
        return Ok(cached.unwrap_or_else(|| inventory::unreachable(cde, None, now)));
    }
    let updated = inventory::with_agents(cde, cached, found, now);
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

/// `cde  pane  status  task-or-adopted  title  summary`, the summary cut to `SUMMARY_WIDTH`.
fn render_text(adopted: &[Adopted]) -> String {
    let rows: Vec<Vec<String>> = adopted
        .iter()
        .map(|entry| {
            let mut row = vec![
                entry.cde.clone(),
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
