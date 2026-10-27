//! `flow inventory`: worktrees, agents, PRs, and load per CDE, from the cache or refreshed.

use std::sync::Arc;
use std::thread;

use anyhow::bail;
use jiff::Timestamp;

use crate::cde;
use crate::herdr::Agent;
use crate::inventory::{self, CdeInventory, Unreachable};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::config::{Cde, Config};

const DETACHED: &str = "(detached)";
const NO_LOAD: &str = "-";
const ERROR_WIDTH: usize = 160;

/// Prints the cached inventory of every CDE, refreshing every configured CDE first when
/// `refresh` is set. A CDE that cannot be refreshed keeps its cache and fails the command once
/// the rest are rendered; an unreachable CDE is recorded as such and does not fail it.
pub fn run(state: &StateDir, refresh: bool, json: bool) -> anyhow::Result<()> {
    let failed = if refresh {
        refresh_all(state)?
    } else {
        Vec::new()
    };
    let inventories = inventory::load_all(state)?;
    if json {
        super::print_json(&inventories)?;
    } else if inventories.is_empty() {
        super::print("no inventory cached; run `flow inventory --refresh`\n")?;
    } else {
        super::print(&render_all(&inventories, Timestamp::now()))?;
    }
    if !failed.is_empty() {
        bail!("could not refresh {}", failed.join(", "));
    }
    Ok(())
}

/// Refreshes every configured CDE in parallel, then joins the user's open PRs, listed once, to
/// each reachable CDE's worktrees, and writes each inventory. Returns the CDEs that failed.
fn refresh_all(state: &StateDir) -> anyhow::Result<Vec<String>> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let now = Timestamp::now();
    let results: Vec<(&Cde, anyhow::Result<CdeInventory>)> = thread::scope(|scope| {
        let handles: Vec<_> = config
            .cdes
            .iter()
            .map(|cde| {
                let runner = &runner;
                let config = &config;
                scope.spawn(move || (cde, refresh_one(state, runner, config, cde, now)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("inventory refresh thread panicked"))
            .collect()
    });

    let mut failed = Vec::new();
    let mut refreshed = Vec::new();
    for (cde, result) in results {
        match result {
            Ok(inventory) => refreshed.push((cde, inventory)),
            Err(err) => {
                eprintln!("{}: {err:#}", cde.name);
                failed.push(cde.name.clone());
            }
        }
    }

    let prs = list_prs(runner.as_ref(), &config, &refreshed);
    for (_, inventory) in refreshed.iter_mut().filter(|(_, inv)| inv.reachable) {
        match &prs {
            Some(prs) => inventory::join_prs(inventory, prs),
            None => {
                let cached = inventory::load(state, &inventory.env_id)?;
                inventory::join_prs(inventory, &cached.map(|c| c.prs).unwrap_or_default());
            }
        }
    }
    for (_, inventory) in &refreshed {
        inventory::save(state, inventory)?;
    }
    Ok(failed)
}

/// `cde`'s fresh inventory, or its cached one marked unreachable when it does not answer.
fn refresh_one(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    config: &Config,
    cde: &Cde,
    now: Timestamp,
) -> anyhow::Result<CdeInventory> {
    match inventory::refresh(
        runner,
        cde,
        &config.monorepo.path,
        &config.worktree_tool,
        now,
    ) {
        Err(err) if err.is::<Unreachable>() => {
            eprintln!("{err:#}");
            Ok(inventory::unreachable(
                cde,
                inventory::load(state, &cde.env_id)?,
                now,
            ))
        }
        result => result,
    }
}

/// The user's open PRs, listed on this machine when it is a reachable configured CDE, else on
/// the first reachable one (gh's login is shared across CDEs). `None`, after a warning, when no
/// CDE is reachable or the listing fails.
fn list_prs(
    runner: &dyn Runner,
    config: &Config,
    refreshed: &[(&Cde, CdeInventory)],
) -> Option<Vec<inventory::Pr>> {
    let local_env_id = cde::local_env_id();
    let mut reachable = refreshed
        .iter()
        .filter(|(_, inv)| inv.reachable)
        .map(|(cde, _)| *cde);
    let host = reachable
        .clone()
        .find(|cde| cde::is_local(cde, local_env_id.as_deref()))
        .or_else(|| reachable.next());
    let Some(host) = host else {
        eprintln!("no CDE reachable to list open PRs; keeping cached PRs");
        return None;
    };
    match inventory::open_prs(runner, host, &config.monorepo.path) {
        Ok(prs) => Some(prs),
        Err(err) => {
            eprintln!("listing open PRs: {err:#}; keeping cached PRs");
            None
        }
    }
}

fn render_all(inventories: &[CdeInventory], now: Timestamp) -> String {
    inventories
        .iter()
        .map(|inv| render_block(inv, now))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The header `name  reachable|unreachable  load <load1>  <N> worktrees  <N> agents  (seen
/// <age>)`, any recorded errors, then `branch  path  [PR #n state]  [agent: <title> <status>]`
/// per worktree.
fn render_block(inv: &CdeInventory, now: Timestamp) -> String {
    let mut out = format!(
        "{}  {}  load {}  {} worktrees  {} agents  (seen {})\n",
        inv.name,
        if inv.reachable {
            "reachable"
        } else {
            "unreachable"
        },
        inv.load1
            .map_or_else(|| NO_LOAD.to_owned(), |load| format!("{load:.2}")),
        inv.worktrees.len(),
        inv.agents.len(),
        render::relative(inv.seen, now),
    );
    for (what, error) in [
        ("worktree tool", &inv.tool_error),
        ("herdr", &inv.herdr_error),
    ] {
        if let Some(error) = error {
            out.push_str(&format!(
                "  {what} failed: {}\n",
                render::truncate(error, ERROR_WIDTH)
            ));
        }
    }
    let rows: Vec<Vec<String>> = inv
        .worktrees
        .iter()
        .map(|wt| {
            let mut row = vec![
                wt.branch.clone().unwrap_or_else(|| DETACHED.to_owned()),
                wt.path.display().to_string(),
                wt.branch
                    .as_deref()
                    .and_then(|branch| inv.prs.iter().find(|pr| pr.branch == branch))
                    .map_or_else(String::new, |pr| format!("PR #{} {}", pr.number, pr.state)),
            ];
            row.extend(
                inv.agents
                    .iter()
                    .filter(|agent| agent.cwd == wt.path)
                    .map(agent_cell),
            );
            while row.last().is_some_and(String::is_empty) {
                row.pop();
            }
            row
        })
        .collect();
    for line in render::table(&rows).lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `agent: <title, or pane id when untitled> <status>`.
fn agent_cell(agent: &Agent) -> String {
    let name = if agent.title.is_empty() {
        &agent.pane_id
    } else {
        &agent.title
    };
    format!("agent: {name} {}", agent.status.as_str())
}
