use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

use crate::paths::StateDir;

/// Operator configuration from `state/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    pub hosts: Vec<Host>,
    pub monorepo: Monorepo,
    pub worktree_tool: WorktreeTool,
    /// The herdr sessions whose agents flow lists: its own session, where workers run, and the
    /// sessions the user starts sessions in by hand.
    #[serde(default = "default_herdr_sessions")]
    pub herdr_sessions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Host {
    pub name: String,
    pub env_id: String,
    pub ssh_alias: String,
    /// `(VAR, VALUE)`: this host is the local one when `$VAR` equals `VALUE`.
    #[serde(default)]
    pub local_marker_env: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Monorepo {
    pub path: PathBuf,
    pub default_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeTool {
    /// Argv template; `{branch}` is replaced with the branch name.
    pub create: Vec<String>,
    /// Argv that prints the tool's worktrees as JSON: `{"worktrees": [{"path", "busy",
    /// "dirty_files"}, ...]}`.
    #[serde(default = "default_list")]
    pub list: Vec<String>,
}

fn default_herdr_sessions() -> Vec<String> {
    ["flow", "default"].map(str::to_owned).to_vec()
}

fn default_list() -> Vec<String> {
    ["wt", "--json", "list"].map(str::to_owned).to_vec()
}

impl Config {
    pub fn load(state: &StateDir) -> anyhow::Result<Config> {
        let path = state.root().join("config.toml");
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}
