//! Per-host inventory: worktrees, load, herdr agents, and open PRs, cached in
//! `state/hosts/<env_id>.json` so the supervisor can pick a task's natural home. The cached
//! records are also the host set: every host flow knows has one.

pub mod discovery;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::{self, Agent, Herdr};
use crate::ona;
use crate::paths::StateDir;
use crate::runner::{Output, Runner};
use crate::state::config::{Host, Repo, WorktreeTool};
use crate::state::fs::write_atomic;
use crate::worktree;

/// The exit code ssh uses for its own failures, as opposed to the remote command's.
const SSH_FAILURE_STATUS: i32 = 255;
/// The exit code git uses for `fatal:` errors.
const GIT_FATAL_STATUS: i32 = 128;
/// What git prints when `-C <path>` names no directory: both parts appear on the same line.
const NO_DIRECTORY_MESSAGE: [&str; 2] = ["cannot change to", "No such file or directory"];
/// What git prints when `-C <path>` names a directory that is not a repository.
const NOT_A_REPO_MESSAGE: &str = "not a git repository";
const LOADAVG_ARGV: [&str; 2] = ["cat", "/proc/loadavg"];
const PR_LIST_ARGV: [&str; 11] = [
    "gh",
    "pr",
    "list",
    "--author",
    "@me",
    "--state",
    "open",
    "--json",
    "number,url,headRefName,state",
    "--limit",
    "100",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInventory {
    pub name: String,
    pub env_id: String,
    /// The ssh alias the host is reached by. Records cached before hosts were discovered leave
    /// it empty, which means the Ona alias of `env_id`.
    #[serde(default)]
    pub ssh_alias: String,
    /// The environment's free-text name in Ona, for a discovered host.
    #[serde(default)]
    pub ona_name: Option<String>,
    /// The environment's Ona phase at the last listing, for a discovered host. Only running
    /// hosts are probed.
    #[serde(default)]
    pub ona_phase: Option<String>,
    /// The host has left the host set: Ona no longer lists it, or the static list dropped it.
    /// Its record is kept for the tasks that ran there.
    #[serde(default)]
    pub gone: bool,
    /// When the host last answered. An unreachable host keeps the time of its cached inventory.
    pub seen: Timestamp,
    pub reachable: bool,
    /// The one-minute load average.
    pub load1: Option<f32>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    /// Open PRs whose head branch is checked out in one of `worktrees` of the same repo.
    pub prs: Vec<Pr>,
    /// Why the last probe could not reach the host; cleared once a probe or herdr reaches it.
    #[serde(default)]
    pub probe_error: Option<String>,
    /// Why the worktree tool's list failed; `busy` and `dirty_files` are then unknown.
    #[serde(default)]
    pub tool_error: Option<String>,
    /// Why herdr could not be asked for some session's agents; `agents` then lacks them.
    #[serde(default)]
    pub herdr_error: Option<String>,
    /// Why open PRs could not be listed; `prs` then holds the previously cached PRs.
    #[serde(default)]
    pub pr_error: Option<String>,
    /// Configured repos with no checkout on this host; `worktrees` has none of theirs.
    #[serde(default)]
    pub missing_repos: Vec<String>,
}

impl HostInventory {
    /// A record of `host` with nothing probed yet: unreachable, seen `now`, empty.
    pub fn new(host: &Host, now: Timestamp) -> HostInventory {
        HostInventory {
            name: host.name.clone(),
            env_id: host.env_id.clone(),
            ssh_alias: host.ssh_alias.clone(),
            ona_name: None,
            ona_phase: None,
            gone: false,
            seen: now,
            reachable: false,
            load1: None,
            worktrees: Vec::new(),
            agents: Vec::new(),
            prs: Vec::new(),
            probe_error: None,
            tool_error: None,
            herdr_error: None,
            pr_error: None,
            missing_repos: Vec::new(),
        }
    }

    /// The host this record describes.
    pub fn host(&self) -> Host {
        Host {
            name: self.name.clone(),
            env_id: self.env_id.clone(),
            ssh_alias: if self.ssh_alias.is_empty() {
                ona::ssh_alias(&self.env_id)
            } else {
                self.ssh_alias.clone()
            },
            local_marker_env: None,
        }
    }

    /// Whether `inventory --refresh` probes this host: it is not gone, and Ona last listed it
    /// running when Ona lists it at all.
    pub fn should_probe(&self) -> bool {
        !self.gone && self.ona_phase.as_deref().is_none_or(ona::is_running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// The configured repo this is a worktree of. Inventories cached before repos were named
    /// leave it empty.
    #[serde(default)]
    pub repo: String,
    pub path: PathBuf,
    /// `None` for a detached HEAD.
    pub branch: Option<String>,
    /// From the worktree tool; `None` when it does not know the worktree.
    pub busy: Option<bool>,
    pub dirty_files: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    /// The configured repo the PR was listed in. Inventories cached before repos were named leave
    /// it empty.
    #[serde(default)]
    pub repo: String,
    /// The PR's head branch.
    pub branch: String,
    pub number: u64,
    pub url: String,
    /// GitHub's state name, such as `OPEN`.
    pub state: String,
}

/// The agents herdr reports on one host across sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrAgents {
    pub agents: Vec<Agent>,
    /// Why some session could not be asked; its agents are missing from `agents`.
    pub error: Option<String>,
    /// ssh could not reach the host, so the sessions after the failing one were not asked.
    pub unreachable: bool,
}

/// The host did not answer: ssh itself failed, or no probe could run at all.
#[derive(Debug, thiserror::Error)]
#[error("host unreachable: {host}: {detail}")]
pub struct Unreachable {
    pub host: String,
    pub detail: String,
}

/// The worktree tool's view of one worktree. Only the fields flow reads are declared.
#[derive(Debug, Deserialize)]
struct ToolWorktree {
    path: PathBuf,
    busy: Option<bool>,
    dirty_files: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ToolList {
    worktrees: Vec<ToolWorktree>,
}

/// One entry of `gh pr list --json number,url,headRefName,state`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPr {
    head_ref_name: String,
    number: u64,
    url: String,
    state: String,
}

/// One repo's worktree probes on a host: git's listing, and the worktree tool's when the repo
/// has one.
struct RepoProbe<'a> {
    repo: &'a Repo,
    git_argv: Vec<String>,
    git: anyhow::Result<Output>,
    tool: Option<(&'a WorktreeTool, anyhow::Result<Output>)>,
}

impl RepoProbe<'_> {
    /// Whether no call for this repo ran at all.
    fn failed(&self) -> bool {
        self.git.is_err() && self.tool.as_ref().is_none_or(|(_, listed)| listed.is_err())
    }
}

/// Probes `host` for the worktrees of each of `repos`, its load, and the agents in
/// `herdr_sessions`. Fails with `Unreachable` when ssh fails on the first call or no probe runs at
/// all, and with another error when git cannot list the worktrees of a repo checked out there. A
/// repo with no checkout on the host is recorded in `missing_repos`; the worktree tool and herdr
/// failing are recorded on the inventory too. `prs` is left empty for `join_prs`.
pub fn refresh(
    runner: &Arc<dyn Runner>,
    host: &Host,
    repos: &[Repo],
    herdr_sessions: &[String],
    now: Timestamp,
) -> anyhow::Result<HostInventory> {
    let mut probes = Vec::new();
    for repo in repos {
        let git_argv = words(&["git", "-C", &repo.path.display().to_string()])
            .into_iter()
            .chain(words(&["worktree", "list", "--porcelain"]))
            .collect::<Vec<_>>();
        let git = runner.run(host, &git_argv, None);
        if probes.is_empty()
            && let Ok(output) = &git
            && is_ssh_failure(output)
        {
            return Err(unreachable_error(host, output));
        }
        let tool = repo.worktree_tool.as_ref().map(|tool| {
            let listed = if tool.list.is_empty() {
                Err(anyhow!("worktree_tool.list in config.toml is empty"))
            } else {
                runner.run(host, &worktree::wrapped_argv(&repo.path, &tool.list), None)
            };
            (tool, listed)
        });
        probes.push(RepoProbe {
            repo,
            git_argv,
            git,
            tool,
        });
    }
    let load = runner.run(host, &words(&LOADAVG_ARGV), None);
    if let Err(load_err) = &load
        && probes.iter().all(RepoProbe::failed)
    {
        let first = probes.first().and_then(|probe| probe.git.as_ref().err());
        return Err(Unreachable {
            host: host.name.clone(),
            detail: format!("{:#}", first.unwrap_or(load_err)),
        }
        .into());
    }

    let several = repos.len() > 1;
    let mut worktrees = Vec::new();
    let mut tool_errors = Vec::new();
    let mut missing_repos = Vec::new();
    for probe in probes {
        let repo = probe.repo;
        let git = probe
            .git
            .with_context(|| format!("listing worktrees of {} on {}", repo.name, host.name))?;
        if is_missing_repo(&git) {
            missing_repos.push(repo.name.clone());
            continue;
        }
        check_status(host, &probe.git_argv, &git)?;
        let mut listed = parse_porcelain(&repo.name, &git.stdout);
        if let Some((tool, output)) = probe.tool {
            match output.and_then(|output| {
                check_status(host, &tool.list, &output)?;
                parse_tool_list(&output.stdout)
            }) {
                Ok(known) => merge_tool(&mut listed, &known),
                Err(err) if several => tool_errors.push(format!("{}: {err:#}", repo.name)),
                Err(err) => tool_errors.push(format!("{err:#}")),
            }
        }
        worktrees.extend(listed);
    }
    let load1 = load
        .ok()
        .filter(|output| output.status == 0)
        .and_then(|output| parse_load1(&output.stdout));

    let HerdrAgents {
        agents,
        error: herdr_error,
        ..
    } = herdr_agents(runner, host, herdr_sessions);

    Ok(HostInventory {
        reachable: true,
        load1,
        worktrees,
        agents,
        tool_error: (!tool_errors.is_empty()).then(|| tool_errors.join("; ")),
        herdr_error,
        missing_repos,
        ..HostInventory::new(host, now)
    })
}

/// The agents in each of `sessions` on `host`, in session order. flow's own session is started
/// when it is not running; another session without a server has no agents. A session whose
/// server cannot be asked is recorded in `error`, and asking stops at the first session ssh
/// cannot reach.
pub fn herdr_agents(runner: &Arc<dyn Runner>, host: &Host, sessions: &[String]) -> HerdrAgents {
    let mut agents = Vec::new();
    let mut errors = Vec::new();
    let mut unreachable = false;
    for session in sessions {
        let herdr = Herdr::with_session(runner.clone(), session);
        let found = if session == herdr::SESSION {
            herdr.ensure_server(host).and_then(|()| herdr.agents(host))
        } else {
            herdr
                .agents_if_running(host)
                .map(Option::unwrap_or_default)
                .with_context(|| format!("herdr session {session}"))
        };
        match found {
            Ok(found) => agents.extend(found),
            Err(err) => {
                errors.push(format!("{err:#}"));
                if herdr::is_unreachable(&err) {
                    unreachable = true;
                    break;
                }
            }
        }
    }
    HerdrAgents {
        agents,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        unreachable,
    }
}

/// `cached` with its agents and herdr error replaced by `found` as of `now`, or, with nothing
/// cached, an inventory of only those agents. herdr answering means the host was reached, so a
/// cached inventory is marked reachable and its probe error cleared; nothing else changes.
pub fn with_agents(
    host: &Host,
    cached: Option<HostInventory>,
    found: HerdrAgents,
    now: Timestamp,
) -> HostInventory {
    match cached {
        Some(cached) => HostInventory {
            seen: now,
            reachable: true,
            agents: found.agents,
            herdr_error: found.error,
            probe_error: None,
            ..cached
        },
        None => HostInventory {
            reachable: true,
            agents: found.agents,
            herdr_error: found.error,
            ..HostInventory::new(host, now)
        },
    }
}

/// The inventory of a host that did not answer: the cached record as of its `seen`, without its
/// load and its worktree tool and herdr errors, or an empty one seen `now` when nothing is cached.
pub fn unreachable(host: &Host, cached: Option<HostInventory>, now: Timestamp) -> HostInventory {
    match cached {
        Some(cached) => HostInventory {
            name: host.name.clone(),
            env_id: host.env_id.clone(),
            reachable: false,
            load1: None,
            tool_error: None,
            herdr_error: None,
            ..cached
        },
        None => HostInventory::new(host, now),
    }
}

/// The user's open PRs in `repo`, listed by `gh` run inside its checkout on `host`.
pub fn open_prs(runner: &dyn Runner, host: &Host, repo: &Repo) -> anyhow::Result<Vec<Pr>> {
    let argv = words(&PR_LIST_ARGV);
    let output = runner.run(host, &worktree::wrapped_argv(&repo.path, &argv), None)?;
    if output.status != 0 {
        bail!(
            "`gh pr list` in {} on {} exited {}: {}",
            repo.name,
            host.name,
            output.status,
            output.stderr.trim()
        );
    }
    parse_prs(&repo.name, &output.stdout)
}

/// Sets `inventory.prs` to the PRs in `prs` whose head branch one of its worktrees of the same
/// repo has checked out.
pub fn join_prs(inventory: &mut HostInventory, prs: &[Pr]) {
    inventory.prs =
        prs.iter()
            .filter(|pr| {
                inventory.worktrees.iter().any(|wt| {
                    wt.repo == pr.repo && wt.branch.as_deref() == Some(pr.branch.as_str())
                })
            })
            .cloned()
            .collect();
}

fn hosts_dir(state: &StateDir) -> PathBuf {
    state.root().join("hosts")
}

/// The cached inventory of the host with `env_id`, if there is one.
pub fn load(state: &StateDir, env_id: &str) -> anyhow::Result<Option<HostInventory>> {
    let path = hosts_dir(state).join(format!("{env_id}.json"));
    match fs::read_to_string(&path) {
        Ok(text) => parse_cached(&path, &text).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// The host of every cached record, sorted by name with the gone ones after the live ones, so a
/// lookup by name finds a live host before a gone one of the same name.
pub fn hosts(state: &StateDir) -> anyhow::Result<Vec<Host>> {
    let mut records = load_all(state)?;
    records.sort_by_key(|record| record.gone);
    Ok(records.iter().map(HostInventory::host).collect())
}

/// The host of every cached record not marked gone, sorted by name.
pub fn live_hosts(state: &StateDir) -> anyhow::Result<Vec<Host>> {
    Ok(load_all(state)?
        .iter()
        .filter(|record| !record.gone)
        .map(HostInventory::host)
        .collect())
}

/// Every cached inventory, sorted by host name.
pub fn load_all(state: &StateDir) -> anyhow::Result<Vec<HostInventory>> {
    let dir = hosts_dir(state);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    let mut inventories = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        inventories.push(parse_cached(&path, &text)?);
    }
    inventories.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(inventories)
}

/// Writes `inventory` to `state/hosts/<env_id>.json` atomically.
pub fn save(state: &StateDir, inventory: &HostInventory) -> anyhow::Result<()> {
    let dir = hosts_dir(state);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.json", inventory.env_id));
    let mut bytes = serde_json::to_vec_pretty(inventory)?;
    bytes.push(b'\n');
    write_atomic(&path, &bytes).with_context(|| format!("writing {}", path.display()))
}

fn parse_cached(path: &Path, text: &str) -> anyhow::Result<HostInventory> {
    serde_json::from_str(text).with_context(|| format!("parsing {}", path.display()))
}

/// Parses `git worktree list --porcelain` of `repo`: blank-line separated blocks of `worktree <path>`,
/// `HEAD <sha>`, and `branch refs/heads/<name>` or `detached`.
fn parse_porcelain(repo: &str, text: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            worktrees.push(Worktree {
                repo: repo.to_owned(),
                path: PathBuf::from(path),
                branch: None,
                busy: None,
                dirty_files: None,
            });
        } else if let (Some(branch), Some(current)) =
            (line.strip_prefix("branch "), worktrees.last_mut())
        {
            let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            current.branch = Some(branch.to_owned());
        }
    }
    worktrees
}

fn parse_tool_list(text: &str) -> anyhow::Result<Vec<ToolWorktree>> {
    let list: ToolList =
        serde_json::from_str(text).context("parsing the worktree tool's JSON list")?;
    Ok(list.worktrees)
}

/// Adds the tool's `busy` and `dirty_files` to the worktrees it reports by the same path.
fn merge_tool(worktrees: &mut [Worktree], tool: &[ToolWorktree]) {
    for wt in worktrees {
        if let Some(known) = tool.iter().find(|known| known.path == wt.path) {
            wt.busy = known.busy;
            wt.dirty_files = known.dirty_files;
        }
    }
}

/// The first field of `/proc/loadavg`.
fn parse_load1(text: &str) -> Option<f32> {
    text.split_whitespace().next()?.parse().ok()
}

fn parse_prs(repo: &str, text: &str) -> anyhow::Result<Vec<Pr>> {
    let prs: Vec<GhPr> = serde_json::from_str(text).context("parsing `gh pr list` output")?;
    Ok(prs
        .into_iter()
        .map(|pr| Pr {
            repo: repo.to_owned(),
            branch: pr.head_ref_name,
            number: pr.number,
            url: pr.url,
            state: pr.state,
        })
        .collect())
}

/// Whether `git -C <path> worktree list` failed because `<path>` is not a checkout on the host:
/// the directory does not exist or is not a git repository.
fn is_missing_repo(output: &Output) -> bool {
    output.status == GIT_FATAL_STATUS
        && output.stderr.lines().any(|line| {
            NO_DIRECTORY_MESSAGE.iter().all(|part| line.contains(part))
                || line.contains(NOT_A_REPO_MESSAGE)
        })
}

fn is_ssh_failure(output: &Output) -> bool {
    output.status == SSH_FAILURE_STATUS && output.stdout.is_empty()
}

fn unreachable_error(host: &Host, output: &Output) -> anyhow::Error {
    Unreachable {
        host: host.name.clone(),
        detail: output.stderr.trim().to_owned(),
    }
    .into()
}

fn check_status(host: &Host, argv: &[String], output: &Output) -> anyhow::Result<()> {
    if output.status != 0 {
        bail!(
            "`{}` on {} exited {}: {}",
            argv.join(" "),
            host.name,
            output.status,
            output.stderr.trim()
        );
    }
    Ok(())
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|word| (*word).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{FakeRunner, Scripted};

    const PORCELAIN: &str = "\
worktree /workspaces/acme
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /workspaces/acme-worktrees/feature-x
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature-x

worktree /workspaces/acme-worktrees/bisect
HEAD 3333333333333333333333333333333333333333
detached
";

    const REPO: &str = "example-repo";

    fn worktree(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            repo: REPO.to_owned(),
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            busy: None,
            dirty_files: None,
        }
    }

    #[test]
    fn porcelain_parses_branches_and_detached() {
        assert_eq!(
            parse_porcelain(REPO, PORCELAIN),
            [
                worktree("/workspaces/acme", Some("main")),
                worktree("/workspaces/acme-worktrees/feature-x", Some("feature-x")),
                worktree("/workspaces/acme-worktrees/bisect", None),
            ]
        );
        assert_eq!(parse_porcelain(REPO, ""), []);
    }

    #[test]
    fn tool_output_adds_busy_and_dirty() {
        let tool = parse_tool_list(
            r#"{"repo": "/workspaces/acme", "worktrees": [
                {"name": "feature-x", "path": "/workspaces/acme-worktrees/feature-x",
                 "mounted": true, "branch": "feature-x", "dirty_files": 3, "busy": true,
                 "upper_bytes": 40960},
                {"name": "gone", "path": "/workspaces/acme-worktrees/gone", "mounted": false,
                 "branch": "gone", "dirty_files": 0, "busy": false, "upper_bytes": 0}
            ]}"#,
        )
        .unwrap();
        let mut worktrees = parse_porcelain(REPO, PORCELAIN);

        merge_tool(&mut worktrees, &tool);

        let feature_x = Worktree {
            busy: Some(true),
            dirty_files: Some(3),
            ..worktree("/workspaces/acme-worktrees/feature-x", Some("feature-x"))
        };
        assert_eq!(
            worktrees,
            [
                worktree("/workspaces/acme", Some("main")),
                feature_x,
                worktree("/workspaces/acme-worktrees/bisect", None),
            ],
            "git decides which worktrees exist; the tool only annotates them"
        );
        assert!(parse_tool_list("not json").is_err());
    }

    fn host() -> Host {
        Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn repos() -> Vec<Repo> {
        vec![Repo {
            name: REPO.to_owned(),
            path: PathBuf::from("/workspaces/acme"),
            default_branch: "main".to_owned(),
            worktrees_dir: None,
            worktree_tool: Some(WorktreeTool {
                create: Vec::new(),
                list: words(&["wt", "--json", "list"]),
            }),
        }]
    }

    const SERVER_RUNNING: &str = "status: running\nprivate_protocol: 22\n";
    const AGENT_LIST: &str = include_str!("../tests/fixtures/herdr/agent_list.json");
    const AGENT_LIST_SERVER_NOT_RUNNING: &str =
        include_str!("../tests/fixtures/herdr/agent_list_server_not_running.json");

    fn sessions() -> Vec<String> {
        words(&["flow", "default"])
    }

    fn ok(stdout: &str) -> Scripted {
        Scripted {
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    fn failed(status: i32, stderr: &str) -> Scripted {
        Scripted {
            status,
            stderr: stderr.to_owned(),
            ..Scripted::default()
        }
    }

    fn refresh_with(script: Vec<Scripted>) -> anyhow::Result<HostInventory> {
        let runner: Arc<dyn Runner> = Arc::new(FakeRunner::new(script));
        refresh(
            &runner,
            &host(),
            &repos(),
            &sessions(),
            Timestamp::UNIX_EPOCH,
        )
    }

    fn herdr_agents_with(script: Vec<Scripted>) -> HerdrAgents {
        let runner: Arc<dyn Runner> = Arc::new(FakeRunner::new(script));
        herdr_agents(&runner, &host(), &sessions())
    }

    #[test]
    fn herdr_agents_span_sessions_and_skip_a_stopped_one() {
        let both = herdr_agents_with(vec![ok(SERVER_RUNNING), ok(AGENT_LIST), ok(AGENT_LIST)]);
        let sessions: Vec<&str> = both.agents.iter().map(|a| a.session.as_str()).collect();
        assert_eq!(sessions, ["flow", "flow", "default", "default"]);
        assert_eq!(both.error, None);

        let stopped = herdr_agents_with(vec![
            ok(SERVER_RUNNING),
            ok(AGENT_LIST),
            failed(1, AGENT_LIST_SERVER_NOT_RUNNING),
        ]);
        assert_eq!(stopped.agents.len(), 2);
        assert_eq!(stopped.error, None);
        assert!(!stopped.unreachable);
    }

    #[test]
    fn herdr_agents_stop_at_an_unreachable_host() {
        let found = herdr_agents_with(vec![failed(
            255,
            "ssh: connect to host: Connection refused\n",
        )]);
        assert!(found.unreachable);
        assert!(found.agents.is_empty());
        let error = found.error.unwrap();
        assert!(error.starts_with("host unreachable: host-a"), "{error}");
        assert!(!error.contains("session default"), "{error}");
    }

    #[test]
    fn no_probe_running_means_unreachable() {
        let err = refresh_with(Vec::new()).unwrap_err();
        assert!(err.is::<Unreachable>(), "{err:#}");
    }

    #[test]
    fn tool_and_herdr_failures_are_recorded() {
        let inventory = refresh_with(vec![
            ok(PORCELAIN),
            failed(127, "zsh: command not found: wt\n"),
            ok("1.75 0.38 0.68 2/1501 1003799\n"),
            ok("garbage\n"),
            failed(1, AGENT_LIST_SERVER_NOT_RUNNING),
        ])
        .unwrap();

        assert!(inventory.reachable);
        assert_eq!(inventory.load1, Some(1.75));
        assert_eq!(inventory.worktrees, parse_porcelain(REPO, PORCELAIN));
        let tool_error = inventory.tool_error.unwrap();
        assert!(tool_error.contains("command not found: wt"), "{tool_error}");
        let herdr_error = inventory.herdr_error.unwrap();
        assert!(herdr_error.contains("herdr server status"), "{herdr_error}");
        assert!(!herdr_error.contains("session default"), "{herdr_error}");
        assert!(inventory.agents.is_empty());
    }
}
