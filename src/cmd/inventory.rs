//! `flow inventory`: worktrees, agents, PRs, and load per host, from the cache or refreshed.

use std::sync::Arc;
use std::thread;

use anyhow::bail;
use jiff::Timestamp;

use crate::herdr::Agent;
use crate::host;
use crate::inventory::{self, HostInventory, Unreachable, Worktree, discovery};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::config::{Config, Host};

const DETACHED: &str = "(detached)";
const NO_LOAD: &str = "-";
const ERROR_WIDTH: usize = 160;
/// What every Ona phase name starts with, as in `ENVIRONMENT_PHASE_RUNNING`.
const ONA_PHASE_PREFIX: &str = "ENVIRONMENT_PHASE_";

/// Prints the cached inventory of every host, first discovering the host set and refreshing
/// every probed host when `refresh` is set. A host that cannot be refreshed keeps its cache and
/// fails the command once the rest are rendered; an unreachable host is recorded as such and
/// does not fail it, and neither does Ona being unavailable.
pub fn run(state: &StateDir, refresh: bool, json: bool) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let failed = if refresh {
        refresh_all(state, &config)?
    } else {
        Vec::new()
    };
    let inventories = inventory::load_all(state)?;
    if json {
        super::print_json(&inventories)?;
    } else if inventories.is_empty() {
        super::print("no inventory cached; run `flow inventory --refresh`\n")?;
    } else {
        let by_repo = config.repos.len() > 1;
        super::print(&render_all(&inventories, by_repo, Timestamp::now()))?;
    }
    if !failed.is_empty() {
        bail!("could not refresh {}", failed.join(", "));
    }
    Ok(())
}

/// Refreshes every host as `run --refresh` does and prints one summary line per host: its
/// name, environment id, Ona phase, and which configured repos it has a checkout of.
pub fn run_summary(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let failed = refresh_all(state, &config)?;
    let summary: String = inventory::load_all(state)?
        .iter()
        .map(|inventory| summary_line(inventory, &config))
        .collect();
    super::print(&summary)?;
    if !failed.is_empty() {
        bail!("could not refresh {}", failed.join(", "));
    }
    Ok(())
}

/// `init: host <name> (<env id>): <phase>, repos: <repos found>`.
fn summary_line(inventory: &HostInventory, config: &Config) -> String {
    let phase = if inventory.gone {
        "gone".to_owned()
    } else {
        match &inventory.ona_phase {
            Some(phase) => phase
                .strip_prefix(ONA_PHASE_PREFIX)
                .map_or_else(|| phase.clone(), str::to_lowercase),
            None => "no Ona phase".to_owned(),
        }
    };
    let repos = if !inventory.should_probe() {
        "unknown (not probed)".to_owned()
    } else if !inventory.reachable {
        "unknown (unreachable)".to_owned()
    } else {
        let found: Vec<&str> = config
            .repos
            .iter()
            .map(|repo| repo.name.as_str())
            .filter(|name| {
                !inventory
                    .missing_repos
                    .iter()
                    .any(|missing| missing == name)
            })
            .collect();
        if found.is_empty() {
            "none".to_owned()
        } else {
            found.join(", ")
        }
    };
    format!(
        "init: host {} ({}): {phase}, repos: {repos}\n",
        inventory.name, inventory.env_id
    )
}

/// Brings the host records in step with Ona or the static list, then refreshes every probed host
/// in parallel, joins the user's open PRs, listed once per repo, to each reachable host's
/// worktrees, and writes each inventory. A repo whose PRs cannot be listed keeps each host's
/// cached PRs of that repo. Returns the hosts that failed.
fn refresh_all(state: &StateDir, config: &Config) -> anyhow::Result<Vec<String>> {
    let now = Timestamp::now();
    let discovered = discovery::discover(state, &config.hosts, now)?;
    if let Some(warning) = &discovered.warning {
        eprintln!("{warning}");
    }
    let targets: Vec<(Host, HostInventory)> = discovered
        .records
        .into_iter()
        .filter(HostInventory::should_probe)
        .map(|record| (record.host(), record))
        .collect();
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let results: Vec<(&Host, anyhow::Result<HostInventory>)> = thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(host, record)| {
                let runner = &runner;
                scope.spawn(move || (host, refresh_one(runner, config, host, record, now)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("inventory refresh thread panicked"))
            .collect()
    });

    let mut failed = Vec::new();
    let mut refreshed = Vec::new();
    for (host, result) in results {
        match result {
            Ok(inventory) => refreshed.push((host, inventory)),
            Err(err) => {
                eprintln!("{}: {err:#}", host.name);
                failed.push(host.name.clone());
            }
        }
    }

    let listed = list_prs(runner.as_ref(), config, &refreshed);
    let several = config.repos.len() > 1;
    let errors: Vec<String> = listed
        .iter()
        .filter_map(|(repo, prs)| {
            let error = prs.as_ref().err()?;
            Some(if several {
                format!("{repo}: {error}")
            } else {
                error.clone()
            })
        })
        .collect();
    let pr_error = (!errors.is_empty()).then(|| errors.join("; "));
    for (_, inventory) in refreshed.iter_mut().filter(|(_, inv)| inv.reachable) {
        let mut prs: Vec<inventory::Pr> = Vec::new();
        let mut cached = None;
        for (repo, listed) in &listed {
            match listed {
                Ok(listed) => prs.extend(listed.iter().cloned()),
                Err(_) => {
                    if cached.is_none() {
                        cached = Some(inventory::load(state, &inventory.env_id)?);
                    }
                    let cached_prs = cached.iter().flatten().flat_map(|c| &c.prs);
                    prs.extend(cached_prs.filter(|pr| pr.repo == *repo).cloned());
                }
            }
        }
        inventory::join_prs(inventory, &prs);
        inventory.pr_error = pr_error.clone();
    }
    for (_, inventory) in &refreshed {
        inventory::save(state, inventory)?;
    }
    Ok(failed)
}

/// `host`'s fresh inventory keeping what discovery recorded in `record`, or `record` marked
/// unreachable when the host does not answer.
fn refresh_one(
    runner: &Arc<dyn Runner>,
    config: &Config,
    host: &Host,
    record: &HostInventory,
    now: Timestamp,
) -> anyhow::Result<HostInventory> {
    match inventory::refresh(runner, host, &config.repos, &config.herdr_sessions, now) {
        Ok(fresh) => Ok(HostInventory {
            ssh_alias: record.ssh_alias.clone(),
            ona_name: record.ona_name.clone(),
            ona_phase: record.ona_phase.clone(),
            ..fresh
        }),
        Err(err) if err.is::<Unreachable>() => {
            eprintln!("{err:#}");
            Ok(inventory::unreachable(host, Some(record.clone()), now))
        }
        Err(err) => Err(err),
    }
}

/// The user's open PRs per configured repo, `(repo name, PRs)` in config order. Each repo is
/// listed on this machine when it is a reachable host with the repo checked out, else
/// on the first reachable host that has it (gh's login is shared across hosts). A repo's entry
/// holds the reason when no reachable host has it or its listing fails.
fn list_prs(
    runner: &dyn Runner,
    config: &Config,
    refreshed: &[(&Host, HostInventory)],
) -> Vec<(String, Result<Vec<inventory::Pr>, String>)> {
    let local_env_id = host::local_env_id();
    config
        .repos
        .iter()
        .map(|repo| {
            let mut candidates = refreshed
                .iter()
                .filter(|(_, inv)| inv.reachable && !inv.missing_repos.contains(&repo.name))
                .map(|(host, _)| *host);
            let host = candidates
                .clone()
                .find(|host| host::is_local(host, local_env_id.as_deref()))
                .or_else(|| candidates.next());
            let prs = match host {
                Some(host) => {
                    inventory::open_prs(runner, host, repo).map_err(|err| format!("{err:#}"))
                }
                None => Err(format!("no reachable host has repo {}", repo.name)),
            };
            (repo.name.clone(), prs)
        })
        .collect()
}

/// Each live host's block, separated by blank lines, then a `gone: <names>` line naming the
/// hosts that left the host set; worktrees are grouped under a `repo <name>` line per repo when
/// `by_repo` is set.
fn render_all(inventories: &[HostInventory], by_repo: bool, now: Timestamp) -> String {
    let (gone, live): (Vec<&HostInventory>, Vec<&HostInventory>) =
        inventories.iter().partition(|inv| inv.gone);
    let mut out = live
        .iter()
        .map(|inv| render_block(inv, by_repo, now))
        .collect::<Vec<_>>()
        .join("\n");
    if !gone.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let names: Vec<&str> = gone.iter().map(|inv| inv.name.as_str()).collect();
        out.push_str(&format!("gone: {}\n", names.join(", ")));
    }
    out
}

/// The header `name  reachable|unreachable  load <load1>  <N> worktrees  <N> agents  (seen
/// <age>)`, any recorded errors, a `missing: <repo>` line per repo with no checkout there, then
/// `branch  path  [PR #n state]  [agent: <title> <status>]` per worktree, under a `repo <name>`
/// line per repo when `by_repo` is set.
fn render_block(inv: &HostInventory, by_repo: bool, now: Timestamp) -> String {
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
        ("gh", &inv.pr_error),
    ] {
        if let Some(error) = error {
            out.push_str(&format!(
                "  {what} failed: {}\n",
                render::truncate(error, ERROR_WIDTH)
            ));
        }
    }
    for repo in &inv.missing_repos {
        out.push_str(&format!("  missing: {repo}\n"));
    }
    if by_repo {
        let mut repos: Vec<&str> = Vec::new();
        for wt in &inv.worktrees {
            if !repos.contains(&wt.repo.as_str()) {
                repos.push(&wt.repo);
            }
        }
        for repo in repos {
            out.push_str(&format!("  repo {repo}\n"));
            let worktrees = inv.worktrees.iter().filter(|wt| wt.repo == repo);
            push_indented(&mut out, "    ", &worktree_rows(inv, worktrees));
        }
    } else {
        push_indented(&mut out, "  ", &worktree_rows(inv, inv.worktrees.iter()));
    }
    out
}

/// `branch  path  [PR #n state]  [agent: <title> <status>]` for each of `worktrees`.
fn worktree_rows<'a>(
    inv: &HostInventory,
    worktrees: impl Iterator<Item = &'a Worktree>,
) -> Vec<Vec<String>> {
    worktrees
        .map(|wt| {
            let mut row = vec![
                wt.branch.clone().unwrap_or_else(|| DETACHED.to_owned()),
                wt.path.display().to_string(),
                wt.branch
                    .as_deref()
                    .and_then(|branch| {
                        inv.prs
                            .iter()
                            .find(|pr| pr.repo == wt.repo && pr.branch == branch)
                    })
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
        .collect()
}

/// Appends `rows` as a table, each line prefixed with `indent`.
fn push_indented(out: &mut String, indent: &str, rows: &[Vec<String>]) {
    for line in render::table(rows).lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
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
