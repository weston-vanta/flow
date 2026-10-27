//! Per-CDE inventory: worktrees, load, herdr agents, and open PRs, cached in
//! `state/cdes/<env_id>.json` so the supervisor can pick a task's natural home.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::{Agent, Herdr};
use crate::paths::StateDir;
use crate::runner::{Output, Runner};
use crate::state::config::{Cde, WorktreeTool};
use crate::state::fs::write_atomic;
use crate::worktree;

/// The exit code ssh uses for its own failures, as opposed to the remote command's.
const SSH_FAILURE_STATUS: i32 = 255;
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
pub struct CdeInventory {
    pub name: String,
    pub env_id: String,
    /// When the CDE last answered. An unreachable CDE keeps the time of its cached inventory.
    pub seen: Timestamp,
    pub reachable: bool,
    /// The one-minute load average.
    pub load1: Option<f32>,
    pub worktrees: Vec<Worktree>,
    pub agents: Vec<Agent>,
    /// Open PRs whose head branch is checked out in one of `worktrees`.
    pub prs: Vec<Pr>,
    /// Why the worktree tool's list failed; `busy` and `dirty_files` are then unknown.
    #[serde(default)]
    pub tool_error: Option<String>,
    /// Why herdr could not be asked for agents; `agents` is then empty.
    #[serde(default)]
    pub herdr_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` for a detached HEAD.
    pub branch: Option<String>,
    /// From the worktree tool; `None` when it does not know the worktree.
    pub busy: Option<bool>,
    pub dirty_files: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    /// The PR's head branch.
    pub branch: String,
    pub number: u64,
    pub url: String,
    /// GitHub's state name, such as `OPEN`.
    pub state: String,
}

/// The CDE did not answer: ssh itself failed, or no probe could run at all.
#[derive(Debug, thiserror::Error)]
#[error("cde unreachable: {cde}: {detail}")]
pub struct Unreachable {
    pub cde: String,
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

/// Probes `cde` for its worktrees, load, and herdr agents. Fails with `Unreachable` when ssh
/// fails on the first call or no probe runs at all, and with another error when git cannot list
/// the worktrees. The worktree tool and herdr failing are recorded on the inventory instead.
/// `prs` is left empty for `join_prs`.
pub fn refresh(
    runner: &Arc<dyn Runner>,
    cde: &Cde,
    monorepo: &Path,
    tool: &WorktreeTool,
    now: Timestamp,
) -> anyhow::Result<CdeInventory> {
    let git_argv = words(&["git", "-C", &monorepo.display().to_string()])
        .into_iter()
        .chain(words(&["worktree", "list", "--porcelain"]))
        .collect::<Vec<_>>();
    let git = runner.run(cde, &git_argv, None);
    if let Ok(output) = &git
        && is_ssh_failure(output)
    {
        return Err(unreachable_error(cde, output));
    }
    let listed = if tool.list.is_empty() {
        Err(anyhow!("worktree_tool.list in config.toml is empty"))
    } else {
        runner.run(cde, &worktree::wrapped_argv(monorepo, &tool.list), None)
    };
    let load = runner.run(cde, &words(&LOADAVG_ARGV), None);
    if let (Err(err), Err(_), Err(_)) = (&git, &listed, &load) {
        return Err(Unreachable {
            cde: cde.name.clone(),
            detail: format!("{err:#}"),
        }
        .into());
    }

    let git = git.with_context(|| format!("listing worktrees on {}", cde.name))?;
    check_status(cde, &git_argv, &git)?;
    let mut worktrees = parse_porcelain(&git.stdout);
    let tool_error = match listed.and_then(|output| {
        check_status(cde, &tool.list, &output)?;
        parse_tool_list(&output.stdout)
    }) {
        Ok(listed) => {
            merge_tool(&mut worktrees, &listed);
            None
        }
        Err(err) => Some(format!("{err:#}")),
    };
    let load1 = load
        .ok()
        .filter(|output| output.status == 0)
        .and_then(|output| parse_load1(&output.stdout));

    let herdr = Herdr::new(runner.clone());
    let (agents, herdr_error) = match herdr.ensure_server(cde).and_then(|()| herdr.agents(cde)) {
        Ok(agents) => (agents, None),
        Err(err) => (Vec::new(), Some(format!("{err:#}"))),
    };

    Ok(CdeInventory {
        name: cde.name.clone(),
        env_id: cde.env_id.clone(),
        seen: now,
        reachable: true,
        load1,
        worktrees,
        agents,
        prs: Vec::new(),
        tool_error,
        herdr_error,
    })
}

/// The inventory of a CDE that did not answer: the cached worktrees, agents, and PRs as of the
/// cached `seen`, or an empty inventory seen `now` when nothing is cached.
pub fn unreachable(cde: &Cde, cached: Option<CdeInventory>, now: Timestamp) -> CdeInventory {
    let (seen, worktrees, agents, prs) = match cached {
        Some(cached) => (cached.seen, cached.worktrees, cached.agents, cached.prs),
        None => (now, Vec::new(), Vec::new(), Vec::new()),
    };
    CdeInventory {
        name: cde.name.clone(),
        env_id: cde.env_id.clone(),
        seen,
        reachable: false,
        load1: None,
        worktrees,
        agents,
        prs,
        tool_error: None,
        herdr_error: None,
    }
}

/// The user's open PRs, listed by `gh` run inside `monorepo` on `cde`.
pub fn open_prs(runner: &dyn Runner, cde: &Cde, monorepo: &Path) -> anyhow::Result<Vec<Pr>> {
    let argv = words(&PR_LIST_ARGV);
    let output = runner.run(cde, &worktree::wrapped_argv(monorepo, &argv), None)?;
    check_status(cde, &argv, &output)?;
    parse_prs(&output.stdout)
}

/// Sets `inventory.prs` to the PRs in `prs` whose head branch one of its worktrees has checked
/// out.
pub fn join_prs(inventory: &mut CdeInventory, prs: &[Pr]) {
    inventory.prs = prs
        .iter()
        .filter(|pr| {
            inventory
                .worktrees
                .iter()
                .any(|wt| wt.branch.as_deref() == Some(pr.branch.as_str()))
        })
        .cloned()
        .collect();
}

fn cdes_dir(state: &StateDir) -> PathBuf {
    state.root().join("cdes")
}

/// The cached inventory of the CDE with `env_id`, if there is one.
pub fn load(state: &StateDir, env_id: &str) -> anyhow::Result<Option<CdeInventory>> {
    let path = cdes_dir(state).join(format!("{env_id}.json"));
    match fs::read_to_string(&path) {
        Ok(text) => parse_cached(&path, &text).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// Every cached inventory, sorted by CDE name.
pub fn load_all(state: &StateDir) -> anyhow::Result<Vec<CdeInventory>> {
    let dir = cdes_dir(state);
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

/// Writes `inventory` to `state/cdes/<env_id>.json` atomically.
pub fn save(state: &StateDir, inventory: &CdeInventory) -> anyhow::Result<()> {
    let dir = cdes_dir(state);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{}.json", inventory.env_id));
    let mut bytes = serde_json::to_vec_pretty(inventory)?;
    bytes.push(b'\n');
    write_atomic(&path, &bytes).with_context(|| format!("writing {}", path.display()))
}

fn parse_cached(path: &Path, text: &str) -> anyhow::Result<CdeInventory> {
    serde_json::from_str(text).with_context(|| format!("parsing {}", path.display()))
}

/// Parses `git worktree list --porcelain`: blank-line separated blocks of `worktree <path>`,
/// `HEAD <sha>`, and `branch refs/heads/<name>` or `detached`.
fn parse_porcelain(text: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            worktrees.push(Worktree {
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

fn parse_prs(text: &str) -> anyhow::Result<Vec<Pr>> {
    let prs: Vec<GhPr> = serde_json::from_str(text).context("parsing `gh pr list` output")?;
    Ok(prs
        .into_iter()
        .map(|pr| Pr {
            branch: pr.head_ref_name,
            number: pr.number,
            url: pr.url,
            state: pr.state,
        })
        .collect())
}

fn is_ssh_failure(output: &Output) -> bool {
    output.status == SSH_FAILURE_STATUS && output.stdout.is_empty()
}

fn unreachable_error(cde: &Cde, output: &Output) -> anyhow::Error {
    Unreachable {
        cde: cde.name.clone(),
        detail: output.stderr.trim().to_owned(),
    }
    .into()
}

fn check_status(cde: &Cde, argv: &[String], output: &Output) -> anyhow::Result<()> {
    if output.status != 0 {
        bail!(
            "`{}` on {} exited {}: {}",
            argv.join(" "),
            cde.name,
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

    fn worktree(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            busy: None,
            dirty_files: None,
        }
    }

    #[test]
    fn porcelain_parses_branches_and_detached() {
        assert_eq!(
            parse_porcelain(PORCELAIN),
            [
                worktree("/workspaces/acme", Some("main")),
                worktree("/workspaces/acme-worktrees/feature-x", Some("feature-x")),
                worktree("/workspaces/acme-worktrees/bisect", None),
            ]
        );
        assert_eq!(parse_porcelain(""), []);
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
        let mut worktrees = parse_porcelain(PORCELAIN);

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

    fn cde() -> Cde {
        Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn tool() -> WorktreeTool {
        WorktreeTool {
            create: Vec::new(),
            list: words(&["wt", "--json", "list"]),
        }
    }

    fn refresh_with(script: Vec<Scripted>) -> anyhow::Result<CdeInventory> {
        let runner: Arc<dyn Runner> = Arc::new(FakeRunner::new(script));
        refresh(
            &runner,
            &cde(),
            Path::new("/workspaces/acme"),
            &tool(),
            Timestamp::UNIX_EPOCH,
        )
    }

    #[test]
    fn no_probe_running_means_unreachable() {
        let err = refresh_with(Vec::new()).unwrap_err();
        assert!(err.is::<Unreachable>(), "{err:#}");
    }

    #[test]
    fn tool_and_herdr_failures_are_recorded() {
        let ok = |stdout: &str| Scripted {
            stdout: stdout.to_owned(),
            ..Scripted::default()
        };
        let inventory = refresh_with(vec![
            ok(PORCELAIN),
            Scripted {
                status: 127,
                stderr: "zsh: command not found: wt\n".to_owned(),
                ..Scripted::default()
            },
            ok("1.75 0.38 0.68 2/1501 1003799\n"),
            ok("garbage\n"),
        ])
        .unwrap();

        assert!(inventory.reachable);
        assert_eq!(inventory.load1, Some(1.75));
        assert_eq!(inventory.worktrees, parse_porcelain(PORCELAIN));
        let tool_error = inventory.tool_error.unwrap();
        assert!(tool_error.contains("command not found: wt"), "{tool_error}");
        let herdr_error = inventory.herdr_error.unwrap();
        assert!(herdr_error.contains("herdr server status"), "{herdr_error}");
        assert!(inventory.agents.is_empty());
    }
}
