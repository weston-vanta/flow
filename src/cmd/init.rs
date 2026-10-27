//! `flow init`: writes `state/config.toml` from what this machine can tell, refreshes the
//! inventory, and lists what the user still provides.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, anyhow};

use super::Refused;
use crate::host;
use crate::ona;
use crate::paths::StateDir;
use crate::state::fs::write_atomic;

const CONFIG_FILE: &str = "config.toml";
/// The directory searched for the repository when Ona does not name one.
const WORKSPACES_VAR: &str = "FLOW_INIT_WORKSPACES";
const DEFAULT_WORKSPACES: &str = "/workspaces";
/// The search path for the worktree tool, in place of `$PATH`.
const SEARCH_PATH_VAR: &str = "FLOW_INIT_PATH";
const WORKTREE_TOOL: &str = "wt";
const FALLBACK_BRANCH: &str = "main";

const HEADER: &str = "# flow operator config, written by `flow init`.\n";
const TOOL_LINE: &str =
    r#"worktree_tool = { create = ["wt", "create", "{branch}"], list = ["wt", "--json", "list"] }"#;
const NO_TOOL_COMMENT: &str = "# no worktree tool found; flow will use git worktree\n";
const REPO_EXAMPLE: &str = r#"# No repository found. Describe each one tasks can get worktrees of:
# [[repos]]
# name = "example-repo"
# path = "/workspaces/example-repo"
# default_branch = "main"
"#;
const HOSTS_BLOCK: &str = r#"
[hosts]
source = "ona"
# Ona environment ids that are never hosts:
# exclude = ["<environment id>"]
# Host names to use in place of the slug of an environment's Ona name:
# names = { "<environment id>" = "<host name>" }
"#;

const CHECKLIST: &str = "\
init: still yours to provide:
  - herdr installed, with `herdr integration install claude`
  - Claude Code installed and logged in on every host
  - `gitpod login` and `gitpod environment ssh-config` on the supervisor's machine
  - `gh auth login` on hosts where PR listing matters (optional)
";

/// The repository `flow init` configures.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedRepo {
    name: String,
    path: String,
    /// How the path was found.
    found_by: String,
    default_branch: String,
    /// Whether `default_branch` came from `origin/HEAD` rather than the fallback.
    branch_from_origin: bool,
    /// Whether `wt` is on the search path.
    worktree_tool: bool,
}

/// Writes `state/config.toml`, refusing to replace an existing one unless `force`; then runs the
/// inventory refresh and prints the checklist. `flow_checkout` is never taken for the repo.
pub fn run(state: &StateDir, flow_checkout: &Path, force: bool) -> anyhow::Result<()> {
    let config_path = state.root().join(CONFIG_FILE);
    if fs::symlink_metadata(&config_path).is_ok() && !force {
        return Err(
            Refused("state/config.toml exists; pass --force to overwrite".to_owned()).into(),
        );
    }
    let repo = find_repo(flow_checkout)
        .map(|(path, found_by)| detect_repo(&path, found_by))
        .transpose()?;
    write_atomic(&config_path, render(repo.as_ref()).as_bytes())
        .with_context(|| format!("writing {}", config_path.display()))?;
    super::print(&format!(
        "{}init: wrote {}\n",
        report(repo.as_ref()),
        config_path.display()
    ))?;
    let refreshed = super::inventory::run(state, true, false);
    super::print(CHECKLIST)?;
    refreshed
}

/// One `init: <field> = <value> (<how it was found>)` line per decision.
fn report(repo: Option<&DetectedRepo>) -> String {
    let mut out = String::new();
    match repo {
        Some(repo) => {
            let branch_how = if repo.branch_from_origin {
                "origin/HEAD"
            } else {
                "fallback; origin/HEAD is not set"
            };
            let tool = if repo.worktree_tool {
                "wt (wt is on PATH)"
            } else {
                "none (wt is not on PATH; flow will use git worktree)"
            };
            out.push_str(&format!(
                "init: repos.path = {} ({})\n\
                 init: repos.name = {} (the path's directory name)\n\
                 init: repos.default_branch = {} ({branch_how})\n\
                 init: repos.worktree_tool = {tool}\n",
                repo.path, repo.found_by, repo.name, repo.default_branch
            ));
        }
        None => out.push_str("init: repos = none found; edit state/config.toml\n"),
    }
    out.push_str("init: hosts.source = ona (hosts are discovered from Ona)\n");
    out
}

/// The repository's path and how it was found: Ona's content location for this environment,
/// else the only git repository directly under the workspaces directory; never `flow_checkout`.
fn find_repo(flow_checkout: &Path) -> Option<(PathBuf, String)> {
    let checkout = fs::canonicalize(flow_checkout).ok();
    let is_checkout = |path: &Path| fs::canonicalize(path).ok() == checkout;
    if let Some(path) = ona_content_location().filter(|path| !is_checkout(path)) {
        return Some((path, "Ona content location of $ONA_ID".to_owned()));
    }
    let workspaces = std::env::var_os(WORKSPACES_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKSPACES));
    let repos: Vec<PathBuf> = fs::read_dir(&workspaces)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_dir() && path.join(".git").exists() && !is_checkout(path))
        .collect();
    let [path] = <[PathBuf; 1]>::try_from(repos).ok()?;
    let how = format!("the only git repository in {}", workspaces.display());
    Some((path, how))
}

/// Where Ona checked out this environment's repository; `None` when Ona cannot say.
fn ona_content_location() -> Option<PathBuf> {
    let env_id = host::local_env_id()?;
    ona::list_environments()
        .ok()?
        .into_iter()
        .find(|env| env.id == env_id)?
        .content_location
}

/// What `flow init` writes for the repository at `path`, found as `found_by` says.
fn detect_repo(path: &Path, found_by: String) -> anyhow::Result<DetectedRepo> {
    let text = path
        .to_str()
        .ok_or_else(|| anyhow!("{} is not valid UTF-8", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{text} has no directory name"))?;
    let origin = origin_head(path);
    Ok(DetectedRepo {
        name: name.to_owned(),
        path: text.to_owned(),
        found_by,
        branch_from_origin: origin.is_some(),
        default_branch: origin.unwrap_or_else(|| FALLBACK_BRANCH.to_owned()),
        worktree_tool: on_search_path(WORKTREE_TOOL),
    })
}

/// The branch `origin/HEAD` points at in the repository at `path`.
fn origin_head(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let branch = stdout.trim();
    let branch = branch.strip_prefix("origin/").unwrap_or(branch);
    (output.status.success() && !branch.is_empty()).then(|| branch.to_owned())
}

/// Whether an executable `program` is in `$FLOW_INIT_PATH`, else `$PATH`.
fn on_search_path(program: &str) -> bool {
    let search: OsString = std::env::var_os(SEARCH_PATH_VAR)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    std::env::split_paths(&search).any(|dir| {
        fs::metadata(dir.join(program))
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

/// The config file for `repo`, or with a commented example in its place when there is none.
fn render(repo: Option<&DetectedRepo>) -> String {
    let mut out = format!("{HEADER}\n");
    match repo {
        Some(repo) => {
            if !repo.worktree_tool {
                out.push_str(NO_TOOL_COMMENT);
            }
            out.push_str("[[repos]]\n");
            for (key, value) in [
                ("name", &repo.name),
                ("path", &repo.path),
                ("default_branch", &repo.default_branch),
            ] {
                out.push_str(&format!("{key} = {}\n", toml_string(value)));
            }
            if repo.worktree_tool {
                out.push_str(TOOL_LINE);
                out.push('\n');
            }
        }
        None => {
            out.push_str(REPO_EXAMPLE);
            out.push_str("# ");
            out.push_str(TOOL_LINE);
            out.push('\n');
        }
    }
    out.push_str(HOSTS_BLOCK);
    out
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}
