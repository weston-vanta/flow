use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

use crate::paths::StateDir;

/// Operator configuration from `state/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    /// Where the host set comes from, and overrides for discovered hosts. The hosts themselves
    /// are the cached records in `state/hosts/`.
    #[serde(default)]
    pub hosts: HostsConfig,
    /// The repositories tasks can get worktrees of, checked out at the same path on every host.
    #[serde(default)]
    pub repos: Vec<Repo>,
    /// The herdr sessions whose agents flow lists: its own session, where workers run, and the
    /// sessions the user starts sessions in by hand.
    #[serde(default = "default_herdr_sessions")]
    pub herdr_sessions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HostsConfig {
    pub source: HostSource,
    /// Ona environment ids that are never hosts.
    pub exclude: Vec<String>,
    /// Ona environment id to host name, in place of the slug of the environment's name.
    pub names: BTreeMap<String, String>,
    /// The host set when `source = "static"`; unused otherwise.
    #[serde(rename = "static")]
    pub static_hosts: Vec<Host>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostSource {
    /// Every environment the Ona CLI lists is a host.
    #[default]
    Ona,
    /// The hosts are `static_hosts`.
    Static,
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
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub default_branch: String,
    /// Where `git worktree add` puts worktrees when there is no worktree tool; the sibling
    /// `<path>-worktrees` when unset.
    #[serde(default)]
    pub worktrees_dir: Option<PathBuf>,
    /// Creates and lists the repo's worktrees; plain `git worktree` when unset.
    #[serde(default)]
    pub worktree_tool: Option<WorktreeTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeTool {
    /// Argv template; `{branch}` is replaced with the branch name. It prints the new
    /// worktree's path: a JSON object with a `path` field, or a last line whose first word is
    /// the path.
    pub create: Vec<String>,
    /// Argv that prints the tool's worktrees as JSON: `{"worktrees": [{"path", "busy",
    /// "dirty_files"}, ...]}`.
    #[serde(default = "default_list")]
    pub list: Vec<String>,
}

const HOSTS_ARRAY_MOVED: &str = "the [[hosts]] list is now [[hosts.static]] under [hosts] with \
     source = \"static\"; remove it to discover hosts from Ona";

fn parse(text: &str) -> anyhow::Result<Config> {
    match toml::from_str(text) {
        Ok(config) => Ok(config),
        Err(err) => {
            let hosts_is_array = toml::from_str::<toml::Table>(text)
                .is_ok_and(|table| table.get("hosts").is_some_and(toml::Value::is_array));
            if hosts_is_array {
                bail!(HOSTS_ARRAY_MOVED);
            }
            Err(err.into())
        }
    }
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
        parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// The repo named `name`, or with no name the only configured repo.
    pub fn repo(&self, name: Option<&str>) -> anyhow::Result<&Repo> {
        match name {
            Some(name) => self
                .repos
                .iter()
                .find(|repo| repo.name == name)
                .ok_or_else(|| anyhow!("no repo named {name}")),
            None => match self.repos.as_slice() {
                [repo] => Ok(repo),
                [] => bail!("no repos configured in config.toml"),
                repos => {
                    let names: Vec<&str> = repos.iter().map(|repo| repo.name.as_str()).collect();
                    bail!(
                        "several repos configured ({}); pass --repo",
                        names.join(", ")
                    )
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn parse(repos: &str) -> Config {
        super::parse(repos).unwrap()
    }

    #[test]
    fn config_single_repo_is_the_default() {
        let config = parse(
            r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
"#,
        );

        let repo = config.repo(None).unwrap();
        assert_eq!(repo.name, "example-repo");
        assert_eq!(repo.path, Path::new("/work/repo"));
        assert_eq!(repo.default_branch, "main");
        assert_eq!(repo.worktrees_dir, None);
        assert_eq!(repo.worktree_tool, None);
        assert_eq!(config.repo(Some("example-repo")).unwrap(), repo);
        assert_eq!(
            format!("{:#}", config.repo(Some("other-repo")).unwrap_err()),
            "no repo named other-repo"
        );
    }

    #[test]
    fn config_requires_repo_when_several() {
        let config = parse(
            r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[[repos]]
name = "other-repo"
path = "/work/other"
default_branch = "trunk"
worktrees_dir = "/work/other.wt"
"#,
        );

        assert_eq!(
            format!("{:#}", config.repo(None).unwrap_err()),
            "several repos configured (example-repo, other-repo); pass --repo"
        );
        let other = config.repo(Some("other-repo")).unwrap();
        assert_eq!(
            other.worktrees_dir.as_deref(),
            Some(Path::new("/work/other.wt"))
        );
        let tool = config
            .repo(Some("example-repo"))
            .unwrap()
            .worktree_tool
            .as_ref();
        assert_eq!(
            tool.map(|tool| tool.list.clone()),
            Some(["wt", "--json", "list"].map(str::to_owned).to_vec()),
            "the tool's list command defaults to wt's"
        );
    }

    #[test]
    fn config_hosts_default_to_ona_discovery() {
        let config = parse("");
        assert_eq!(config.hosts, HostsConfig::default());
        assert_eq!(config.hosts.source, HostSource::Ona);

        let config = parse(
            r#"
[hosts]
source = "static"
exclude = ["env-0009"]
names = { env-0001 = "main-box" }

[[hosts.static]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"
"#,
        );
        assert_eq!(config.hosts.source, HostSource::Static);
        assert_eq!(config.hosts.exclude, ["env-0009"]);
        assert_eq!(config.hosts.names["env-0001"], "main-box");
        assert_eq!(config.hosts.static_hosts[0].ssh_alias, "host-a.example");

        let err = super::parse(
            r#"
[[hosts]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"
"#,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("[[hosts.static]]"), "{err:#}");
    }
}
