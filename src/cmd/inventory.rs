//! `flow inventory`: worktrees, agents, PRs, and load per host, from the cache or refreshed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde_json::Value;

use crate::herdr::Agent;
use crate::host;
use crate::inventory::{self, HostInventory, Unreachable, Worktree, discovery};
use crate::paths::StateDir;
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::{Config, Host, Repo};
use crate::supervisor;
use crate::worktree;

const DETACHED: &str = "(detached)";
/// The directory the worktree tool keeps its detached base checkout in, under the worktrees dir.
const BASE_DIR: &str = ".base";
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
        refresh_all(state, &config, &Arc::from(runner::from_env()?))?
    } else {
        Vec::new()
    };
    let inventories = inventory::load_all(state)?;
    let local = host::local_env_id();
    if json {
        let views = inventories
            .iter()
            .map(|inventory| json_view(inventory, local.as_deref()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        super::print_json(&views)?;
    } else if inventories.is_empty() {
        super::print("no inventory cached; run `flow inventory --refresh`\n")?;
    } else {
        let own = supervisor::herdr_session(state)?;
        super::print(&render_all(
            &inventories,
            &config.repos,
            tagged_session(&config, &own),
            local.as_deref(),
            Timestamp::now(),
        ))?;
    }
    if !failed.is_empty() {
        bail!("could not refresh {}", failed.join(", "));
    }
    Ok(())
}

/// Refreshes every host first when `refresh`, as `run` does, then syncs herdr's saved machines
/// with the hosts as `flow inventory --sync`; see `sync::run`. A host that could not be
/// refreshed fails the command once the sync is done.
pub fn run_sync(
    state: &StateDir,
    refresh: bool,
    json: bool,
    yes: bool,
) -> anyhow::Result<ExitCode> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let failed = if refresh {
        refresh_all(state, &config, &runner)?
    } else {
        Vec::new()
    };
    let code = super::sync::run(state, runner.as_ref(), json, yes)?;
    if !failed.is_empty() {
        bail!("could not refresh {}", failed.join(", "));
    }
    Ok(code)
}

/// The cached record as JSON, with `ssh_alias` resolved as flow reaches the host, `local` for
/// the host this runs on, and `phase` for Ona's phase.
fn json_view(inventory: &HostInventory, local_env_id: Option<&str>) -> anyhow::Result<Value> {
    let host = inventory.host();
    let local = host::is_local(&host, local_env_id);
    let mut value = serde_json::to_value(inventory)?;
    let fields = value
        .as_object_mut()
        .context("a host inventory serializes as an object")?;
    fields.insert("ssh_alias".to_owned(), host.ssh_alias.into());
    fields.insert("local".to_owned(), local.into());
    fields.insert("phase".to_owned(), inventory.ona_phase.clone().into());
    Ok(value)
}

/// Refreshes every host as `run --refresh` does and prints one summary line per host: its
/// name, environment id, Ona phase, and which configured repos it has a checkout of.
pub fn run_summary(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let failed = refresh_all(state, &config, &Arc::from(runner::from_env()?))?;
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
                .unwrap_or(phase)
                .to_lowercase(),
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
fn refresh_all(
    state: &StateDir,
    config: &Config,
    runner: &Arc<dyn Runner>,
) -> anyhow::Result<Vec<String>> {
    let now = Timestamp::now();
    let tasks = &Store::open(state).list_tasks().unwrap_or_default();
    let own = &supervisor::herdr_session(state)?;
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
    let results: Vec<(&Host, anyhow::Result<HostInventory>)> = thread::scope(|scope| {
        let handles: Vec<_> = targets
            .iter()
            .map(|(host, record)| {
                scope.spawn(move || {
                    let fresh = inventory::refresh(runner, host, config, own, tasks, now);
                    (host, keep_discovered(host, record, fresh, now))
                })
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

/// `host`'s `fresh` inventory keeping what discovery recorded in `record`, or `record` marked
/// unreachable, with why, when the host did not answer.
fn keep_discovered(
    host: &Host,
    record: &HostInventory,
    fresh: anyhow::Result<HostInventory>,
    now: Timestamp,
) -> anyhow::Result<HostInventory> {
    match fresh {
        Ok(fresh) => Ok(HostInventory {
            ssh_alias: record.ssh_alias.clone(),
            ona_name: record.ona_name.clone(),
            ona_phase: record.ona_phase.clone(),
            ..fresh
        }),
        Err(err) if err.is::<Unreachable>() => {
            eprintln!("{err:#}");
            let probe_error = err
                .downcast_ref::<Unreachable>()
                .map(|unreachable| unreachable.detail.clone());
            Ok(HostInventory {
                probe_error,
                ..inventory::unreachable(host, Some(record.clone()), now)
            })
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

/// flow's own herdr session, `own`, when flow lists others too, so its agents are tagged with it.
fn tagged_session<'a>(config: &Config, own: &'a str) -> Option<&'a str> {
    (config.listed_sessions(own).len() > 1).then_some(own)
}

/// Each host's block, separated by blank lines: the local host first, then by name. Agents in
/// herdr session `tagged` are tagged with it.
fn render_all(
    inventories: &[HostInventory],
    repos: &[Repo],
    tagged: Option<&str>,
    local_env_id: Option<&str>,
    now: Timestamp,
) -> String {
    let mut sorted: Vec<&HostInventory> = inventories.iter().collect();
    sorted.sort_by_key(|inv| (Some(inv.env_id.as_str()) != local_env_id, inv.name.as_str()));
    let name_width = sorted
        .iter()
        .map(|inv| inv.name.chars().count())
        .max()
        .unwrap_or(0);
    sorted
        .iter()
        .map(|inv| render_block(inv, repos, tagged, name_width, now))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The host's header, its recorded errors, then its agents and repos. A gone or never-probed
/// host has only the header.
fn render_block(
    inv: &HostInventory,
    repos: &[Repo],
    tagged: Option<&str>,
    name_width: usize,
    now: Timestamp,
) -> String {
    let mut out = format!("{:<name_width$}  {}\n", inv.name, host_state(inv, now));
    if inv.gone || never_probed(inv) {
        return out;
    }
    for (what, error) in [
        ("probe", &inv.probe_error),
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
    out.push_str(&format!("  agents ({})\n", inv.agents.len()));
    push_indented(
        &mut out,
        "    ",
        &inv.agents
            .iter()
            .map(|agent| agent_row(agent, tagged))
            .collect::<Vec<_>>(),
    );
    out.push_str("  repos\n");
    push_repos(&mut out, inv, repos);
    out
}

/// `reachable · load <l1> · seen <age> ago`, `unreachable · seen <age> ago`, `gone · seen <age>
/// ago`, or `never probed`.
fn host_state(inv: &HostInventory, now: Timestamp) -> String {
    let seen = format!("seen {} ago", render::relative(inv.seen, now));
    if inv.gone {
        format!("gone · {seen}")
    } else if never_probed(inv) {
        "never probed".to_owned()
    } else if !inv.reachable {
        format!("unreachable · {seen}")
    } else {
        match inv.load1 {
            Some(load) => format!("reachable · load {load:.2} · {seen}"),
            None => format!("reachable · {seen}"),
        }
    }
}

/// Whether the record holds nothing a probe would have found: the host has never answered.
fn never_probed(inv: &HostInventory) -> bool {
    !inv.reachable
        && inv.load1.is_none()
        && inv.worktrees.is_empty()
        && inv.agents.is_empty()
        && inv.prs.is_empty()
        && inv.missing_repos.is_empty()
        && inv.probe_error.is_none()
        && inv.tool_error.is_none()
        && inv.herdr_error.is_none()
        && inv.pr_error.is_none()
}

/// `<status>[ [<session>]]  <title, or pane id when untitled>  <cwd>`; agents in herdr session
/// `tagged`, flow's own when it lists others, are tagged to tell them from the user's.
fn agent_row(agent: &Agent, tagged: Option<&str>) -> Vec<String> {
    let mut status = agent.status.as_str().to_owned();
    if tagged == Some(agent.session.as_str()) {
        status.push_str(&format!(" [{}]", agent.session));
    }
    let title = if agent.title.is_empty() {
        &agent.pane_id
    } else {
        &agent.title
    };
    vec![status, title.clone(), agent.cwd.display().to_string()]
}

/// A line per configured repo (`<name>  on <branch> · <n> worktrees`, `<name>  missing at
/// <path>`, or `<name>  unknown` when the inventory records neither) with its worktrees below,
/// then an `other worktrees` group for worktrees of no configured repo.
fn push_repos(out: &mut String, inv: &HostInventory, repos: &[Repo]) {
    let name_width = repos
        .iter()
        .map(|repo| repo.name.chars().count())
        .max()
        .unwrap_or(0);
    let dirs: Vec<Option<PathBuf>> = repos.iter().map(|repo| repo_dir(inv, repo)).collect();
    let owners: Vec<Option<&Repo>> = inv
        .worktrees
        .iter()
        .map(|wt| repos.iter().find(|repo| repo.name == wt.repo))
        .collect();
    for (repo, dir) in repos.iter().zip(&dirs) {
        let (summary, rows) = if inv.missing_repos.contains(&repo.name) {
            (format!("missing at {}", repo.path.display()), Vec::new())
        } else {
            let worktrees = inv
                .worktrees
                .iter()
                .zip(&owners)
                .filter(|(_, owner)| owner.is_some_and(|owner| owner.name == repo.name))
                .map(|(wt, _)| wt);
            repo_summary(inv, repo, dir.as_deref(), worktrees)
        };
        out.push_str(&format!("    {:<name_width$}  {summary}\n", repo.name));
        push_indented(out, "      ", &rows);
    }
    let others: Vec<Vec<String>> = inv
        .worktrees
        .iter()
        .zip(&owners)
        .filter(|(_, owner)| owner.is_none())
        .map(|(wt, _)| vec![branch_name(wt).to_owned(), wt.path.display().to_string()])
        .collect();
    if !others.is_empty() {
        out.push_str("    other worktrees\n");
        push_indented(out, "      ", &others);
    }
}

/// `on <branch of the main checkout> · <n> worktrees` (either part left out when there is
/// nothing to say, `unknown` when both are) and a row per worktree of `repo` other than its
/// main and base checkouts. Worktrees outside `dir`, the repo's worktrees dir, show their path.
fn repo_summary<'a>(
    inv: &HostInventory,
    repo: &Repo,
    dir: Option<&Path>,
    worktrees: impl Iterator<Item = &'a Worktree>,
) -> (String, Vec<Vec<String>>) {
    let mut main = None;
    let mut rows = Vec::new();
    for wt in worktrees {
        if wt.path == repo.path {
            main = Some(wt);
        } else if !is_base(wt, dir) {
            rows.push(worktree_row(inv, wt, dir));
        }
    }
    let mut parts = Vec::new();
    if let Some(main) = main {
        parts.push(format!("on {}", branch_name(main)));
    }
    match rows.len() {
        0 => {}
        1 => parts.push("1 worktree".to_owned()),
        n => parts.push(format!("{n} worktrees")),
    }
    if parts.is_empty() {
        parts.push("unknown".to_owned());
    }
    (parts.join(" · "), rows)
}

/// `repo`'s worktrees dir on this host: the configured one, else the one inferred from the
/// worktrees naming the repo, else the sibling default.
fn repo_dir(inv: &HostInventory, repo: &Repo) -> Option<PathBuf> {
    if repo.worktrees_dir.is_some() {
        return worktree::worktrees_dir(repo);
    }
    let worktrees = inv.worktrees.iter().filter(|wt| wt.repo == repo.name);
    inferred_worktrees_dir(&repo.path, worktrees).or_else(|| worktree::worktrees_dir(repo))
}

/// Whether `wt` is the detached base checkout the worktree tool keeps: `<worktrees dir>/.base`,
/// or a detached head in a directory named `.base`.
fn is_base(wt: &Worktree, dir: Option<&Path>) -> bool {
    let at_base = dir.is_some_and(|dir| wt.path == dir.join(BASE_DIR));
    let detached_base =
        wt.branch.is_none() && wt.path.file_name().is_some_and(|name| name == BASE_DIR);
    at_base || detached_base
}

/// `<branch>  [PR #<n> <state>]  [<path>]`: the path only when it is not `<dir>/<branch>`.
fn worktree_row(inv: &HostInventory, wt: &Worktree, dir: Option<&Path>) -> Vec<String> {
    let pr = wt
        .branch
        .as_deref()
        .and_then(|branch| {
            inv.prs
                .iter()
                .find(|pr| pr.repo == wt.repo && pr.branch == branch)
        })
        .map_or_else(String::new, |pr| {
            format!("PR #{} {}", pr.number, pr.state.to_lowercase())
        });
    let conventional = match (&wt.branch, dir) {
        (Some(branch), Some(dir)) => wt.path == dir.join(branch),
        _ => false,
    };
    let path = if conventional {
        String::new()
    } else {
        wt.path.display().to_string()
    };
    vec![branch_name(wt).to_owned(), pr, path]
}

/// Where the worktrees of a repo at `repo_path` live, judged from the worktrees other than the
/// main checkout: the directory holding a `.base` checkout when there is one, else the directory
/// more than half of them sit in (`<dir>` of a worktree at `<dir>/<branch>`, else the worktree's
/// parent). `None` when neither exists.
fn inferred_worktrees_dir<'a>(
    repo_path: &Path,
    worktrees: impl Iterator<Item = &'a Worktree>,
) -> Option<PathBuf> {
    let worktrees: Vec<&Worktree> = worktrees.filter(|wt| wt.path != repo_path).collect();
    let base = worktrees
        .iter()
        .find(|wt| wt.path.file_name().is_some_and(|name| name == BASE_DIR));
    if let Some(base) = base {
        return base.path.parent().map(Path::to_path_buf);
    }
    let dirs: Vec<&Path> = worktrees
        .iter()
        .filter_map(|wt| match &wt.branch {
            Some(branch) if wt.path.ends_with(branch) => wt
                .path
                .ancestors()
                .nth(Path::new(branch).components().count()),
            _ => wt.path.parent(),
        })
        .collect();
    dirs.iter()
        .find(|dir| dirs.iter().filter(|other| other == dir).count() * 2 > worktrees.len())
        .map(|dir| dir.to_path_buf())
}

fn branch_name(wt: &Worktree) -> &str {
    wt.branch.as_deref().unwrap_or(DETACHED)
}

/// `rows` without the columns that are empty in every row, and without each row's trailing
/// empty cells, so the table prints nothing for them.
fn drop_empty_columns(rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let used: Vec<bool> = (0..columns)
        .map(|i| {
            rows.iter()
                .any(|row| row.get(i).is_some_and(|cell| !cell.is_empty()))
        })
        .collect();
    rows.into_iter()
        .map(|row| {
            let mut kept: Vec<String> = row
                .into_iter()
                .zip(&used)
                .filter(|(_, used)| **used)
                .map(|(cell, _)| cell)
                .collect();
            while kept.last().is_some_and(String::is_empty) {
                kept.pop();
            }
            kept
        })
        .collect()
}

/// Appends `rows` as a table, each line prefixed with `indent`, leaving out empty columns.
fn push_indented(out: &mut String, indent: &str, rows: &[Vec<String>]) {
    for line in render::table(&drop_empty_columns(rows.to_vec())).lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::AgentStatus;

    fn config() -> Config {
        toml::from_str(
            r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
"#,
        )
        .unwrap()
    }

    fn running(phase: &str) -> HostInventory {
        let host = Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        };
        HostInventory {
            ona_phase: Some(phase.to_owned()),
            reachable: true,
            ..HostInventory::new(&host, Timestamp::now())
        }
    }

    const NOW: &str = "2026-10-26T12:00:00Z";

    fn repos() -> Vec<Repo> {
        toml::from_str::<Config>(
            r#"
[[repos]]
name = "app"
path = "/work/app"
default_branch = "main"

[[repos]]
name = "lib"
path = "/work/lib"
default_branch = "main"
worktrees_dir = "/work/trees/lib"

[[repos]]
name = "docs"
path = "/work/docs"
default_branch = "main"
"#,
        )
        .unwrap()
        .repos
    }

    fn ago(minutes: i64) -> Timestamp {
        NOW.parse::<Timestamp>().unwrap() - jiff::SignedDuration::from_mins(minutes)
    }

    fn host(name: &str, seen_minutes_ago: i64) -> HostInventory {
        let host = Host {
            name: name.to_owned(),
            env_id: format!("env-{name}"),
            ssh_alias: format!("{name}.example"),
            local_marker_env: None,
        };
        HostInventory {
            reachable: true,
            ..HostInventory::new(&host, ago(seen_minutes_ago))
        }
    }

    fn wt(repo: &str, path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            repo: repo.to_owned(),
            path: path.into(),
            branch: branch.map(str::to_owned),
            busy: None,
            dirty_files: None,
        }
    }

    fn agent(status: AgentStatus, title: &str, cwd: &str, session: &str) -> Agent {
        Agent {
            pane_id: "w1:p2".to_owned(),
            workspace_id: "w1".to_owned(),
            agent: "claude".to_owned(),
            status,
            cwd: cwd.into(),
            title: title.to_owned(),
            session: session.to_owned(),
            agent_session_id: None,
            workspace_label: None,
        }
    }

    fn pr(repo: &str, branch: &str, number: u64) -> inventory::Pr {
        inventory::Pr {
            repo: repo.to_owned(),
            branch: branch.to_owned(),
            number,
            url: format!("https://example.com/pull/{number}"),
            state: "OPEN".to_owned(),
        }
    }

    /// The local host: agents from flow's own and another herdr session, a repo with the base
    /// checkout, a PR, a worktree outside the convention and a detached one, a repo on a non-main
    /// branch with no worktrees, a missing repo, and a worktree of no configured repo.
    fn local_host() -> HostInventory {
        HostInventory {
            load1: Some(1.5),
            agents: vec![
                agent(
                    AgentStatus::Idle,
                    "Build feature x",
                    "/work/app-worktrees/feature-x",
                    "flow",
                ),
                agent(AgentStatus::Working, "", "/work/app", "default"),
            ],
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("app", "/work/app-worktrees/.base", None),
                wt("app", "/work/app-worktrees/feature-x", Some("feature-x")),
                wt("app", "/work/elsewhere/fix-y", Some("fix-y")),
                wt("app", "/work/app-worktrees/bisect", None),
                wt("lib", "/work/lib", Some("release")),
                wt("retired", "/work/old/spike", Some("spike")),
            ],
            prs: vec![pr("app", "feature-x", 12)],
            missing_repos: vec!["docs".to_owned()],
            ..host("host-b", 2)
        }
    }

    /// A remote host whose inventory predates the `docs` repo: one worktree of `lib`.
    fn remote_host() -> HostInventory {
        HostInventory {
            load1: Some(0.2),
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("lib", "/work/lib", Some("main")),
                wt("lib", "/work/trees/lib/fix-z", Some("fix-z")),
            ],
            ..host("host-a", 1)
        }
    }

    fn unreachable_host() -> HostInventory {
        HostInventory {
            reachable: false,
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("lib", "/work/lib", Some("main")),
            ],
            probe_error: Some("ssh: connect to host host-c.example: timed out".to_owned()),
            pr_error: Some("gh: not logged in".to_owned()),
            missing_repos: vec!["docs".to_owned()],
            ..host("host-c", 180)
        }
    }

    fn gone_host() -> HostInventory {
        HostInventory {
            reachable: false,
            gone: true,
            worktrees: vec![wt("app", "/work/app", Some("main"))],
            ..host("host-d", 5 * 24 * 60)
        }
    }

    fn never_probed_host() -> HostInventory {
        HostInventory {
            reachable: false,
            ..host("host-long-name", 0)
        }
    }

    fn render(inventories: &[HostInventory]) -> String {
        render_all(
            inventories,
            &repos(),
            Some("flow"),
            Some("env-host-b"),
            NOW.parse().unwrap(),
        )
    }

    #[test]
    fn only_flows_own_session_is_tagged_and_only_beside_others() {
        let alone = config();
        assert_eq!(tagged_session(&alone, "flow"), None);
        let beside: Config = toml::from_str("herdr_sessions = [\"default\"]\n").unwrap();
        assert_eq!(tagged_session(&beside, "flow"), Some("flow"));
        assert_eq!(tagged_session(&beside, "default"), None);
    }

    #[test]
    fn render_lays_out_hosts_agents_and_repos() {
        let inventories = [
            never_probed_host(),
            gone_host(),
            unreachable_host(),
            remote_host(),
            local_host(),
        ];
        assert_eq!(
            render(&inventories),
            "\
host-b          reachable · load 1.50 · seen 2m ago
  agents (2)
    idle [flow]  Build feature x  /work/app-worktrees/feature-x
    working      w1:p2            /work/app
  repos
    app   on main · 3 worktrees
      feature-x   PR #12 open
      fix-y                    /work/elsewhere/fix-y
      (detached)               /work/app-worktrees/bisect
    lib   on release
    docs  missing at /work/docs
    other worktrees
      spike  /work/old/spike

host-a          reachable · load 0.20 · seen 1m ago
  agents (0)
  repos
    app   on main
    lib   on main · 1 worktree
      fix-z
    docs  unknown

host-c          unreachable · seen 3h ago
  probe failed: ssh: connect to host host-c.example: timed out
  gh failed: gh: not logged in
  agents (0)
  repos
    app   on main
    lib   on main
    docs  missing at /work/docs

host-d          gone · seen 5d ago

host-long-name  never probed
"
        );
    }

    #[test]
    fn render_counts_several_worktrees_and_pads_path_column_only_when_needed() {
        let inventory = HostInventory {
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("app", "/work/app-worktrees/a", Some("a")),
                wt("app", "/work/app-worktrees/longer-b", Some("longer-b")),
            ],
            prs: vec![pr("app", "a", 7)],
            ..host("host-a", 0)
        };
        assert_eq!(
            render(&[inventory]),
            "\
host-a  reachable · seen 0s ago
  agents (0)
  repos
    app   on main · 2 worktrees
      a         PR #7 open
      longer-b
    lib   unknown
    docs  unknown
"
        );
    }

    #[test]
    fn render_takes_the_worktrees_dir_from_the_worktrees_when_unset() {
        let inventory = HostInventory {
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("app", "/work/app.wt/.base", None),
                wt("app", "/work/app.wt/user/a", Some("user/a")),
                wt("app", "/work/app.wt/b", Some("b")),
                wt("app", "/work/app.wt/renamed", Some("c")),
            ],
            ..host("host-a", 0)
        };
        assert_eq!(
            render(&[inventory]),
            "\
host-a  reachable · seen 0s ago
  agents (0)
  repos
    app   on main · 3 worktrees
      user/a
      b
      c       /work/app.wt/renamed
    lib   unknown
    docs  unknown
"
        );
    }

    #[test]
    fn render_reads_a_host_whose_first_probe_failed_as_unreachable() {
        let inventory = HostInventory {
            reachable: false,
            probe_error: Some("ssh: Could not resolve hostname".to_owned()),
            ..host("host-a", 0)
        };
        assert_eq!(
            render(&[inventory]),
            "\
host-a  unreachable · seen 0s ago
  probe failed: ssh: Could not resolve hostname
  agents (0)
  repos
    app   unknown
    lib   unknown
    docs  unknown
"
        );
    }

    #[test]
    fn render_prints_the_path_of_a_worktree_outside_the_majority_dir() {
        let inventory = HostInventory {
            worktrees: vec![
                wt("app", "/work/app", Some("main")),
                wt("app", "/work/app.wt/a", Some("a")),
                wt("app", "/work/app.wt/b", Some("b")),
                wt("app", "/work/elsewhere/c", Some("c")),
            ],
            ..host("host-a", 0)
        };
        assert_eq!(
            render(&[inventory]),
            "\
host-a  reachable · seen 0s ago
  agents (0)
  repos
    app   on main · 3 worktrees
      a
      b
      c  /work/elsewhere/c
    lib   unknown
    docs  unknown
"
        );
    }

    fn infer(worktrees: &[Worktree]) -> Option<PathBuf> {
        inferred_worktrees_dir(Path::new("/work/app"), worktrees.iter())
    }

    #[test]
    fn inferred_worktrees_dir_is_the_dir_every_worktree_sits_in() {
        let worktrees = [
            wt("app", "/work/app", Some("main")),
            wt("app", "/work/app.wt/user/a", Some("user/a")),
            wt("app", "/work/app.wt/b", Some("b")),
            wt("app", "/work/app.wt/renamed", Some("c")),
            wt("app", "/work/app.wt/bisect", None),
        ];
        assert_eq!(infer(&worktrees), Some(PathBuf::from("/work/app.wt")));
    }

    #[test]
    fn inferred_worktrees_dir_counts_the_base_checkout() {
        let worktrees = [
            wt("app", "/work/app", Some("main")),
            wt("app", "/work/app.wt/.base", None),
        ];
        assert_eq!(infer(&worktrees), Some(PathBuf::from("/work/app.wt")));
    }

    #[test]
    fn inferred_worktrees_dir_is_none_for_mixed_dirs() {
        let worktrees = [
            wt("app", "/work/app.wt/a", Some("a")),
            wt("app", "/work/elsewhere/b", Some("b")),
        ];
        assert_eq!(infer(&worktrees), None);
    }

    #[test]
    fn inferred_worktrees_dir_prefers_the_base_checkouts_dir() {
        let worktrees = [
            wt("app", "/work/app.wt/.base", None),
            wt("app", "/work/other/a", Some("a")),
            wt("app", "/work/other/b", Some("b")),
        ];
        assert_eq!(infer(&worktrees), Some(PathBuf::from("/work/app.wt")));
    }

    #[test]
    fn inferred_worktrees_dir_takes_a_strict_majority() {
        let majority = [
            wt("app", "/work/app.wt/a", Some("a")),
            wt("app", "/work/app.wt/b", Some("b")),
            wt("app", "/work/elsewhere/c", Some("c")),
        ];
        assert_eq!(infer(&majority), Some(PathBuf::from("/work/app.wt")));
        let half = [
            wt("app", "/work/app.wt/a", Some("a")),
            wt("app", "/work/app.wt/b", Some("b")),
            wt("app", "/work/elsewhere/c", Some("c")),
            wt("app", "/work/elsewhere/d", Some("d")),
        ];
        assert_eq!(infer(&half), None);
    }

    #[test]
    fn inferred_worktrees_dir_is_none_without_worktrees() {
        assert_eq!(infer(&[wt("app", "/work/app", Some("main"))]), None);
        assert_eq!(infer(&[]), None);
    }

    #[test]
    fn render_puts_the_local_host_first_then_sorts_by_name() {
        let rendered = render(&[host("zeta", 0), host("host-b", 0), host("alpha", 0)]);
        let headers: Vec<&str> = rendered
            .lines()
            .filter(|line| !line.starts_with(' '))
            .filter(|line| !line.is_empty())
            .map(|line| line.split(' ').next().unwrap())
            .collect();
        assert_eq!(headers, ["host-b", "alpha", "zeta"]);
    }

    #[test]
    fn render_has_no_trailing_whitespace() {
        let rendered = render(&[local_host(), remote_host(), unreachable_host()]);
        for line in rendered.lines() {
            assert_eq!(line, line.trim_end(), "{rendered}");
        }
    }

    #[test]
    fn summary_line_lowercases_the_phase_with_or_without_ona_prefix() {
        let config = config();
        assert_eq!(
            summary_line(&running("ENVIRONMENT_PHASE_RUNNING"), &config),
            "init: host host-a (env-0001): running, repos: example-repo\n"
        );
        assert_eq!(
            summary_line(&running("PHASE_UNKNOWN"), &config),
            "init: host host-a (env-0001): phase_unknown, repos: unknown (not probed)\n"
        );
    }
}
