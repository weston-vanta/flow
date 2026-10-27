//! Which host this process runs on, and lookups over the configured hosts.

use anyhow::anyhow;

use crate::state::config::{Config, Host};

const LOCAL_ENV_ID_VAR: &str = "ONA_ID";

/// The Ona environment id of the host this process runs on, from `$ONA_ID`.
pub fn local_env_id() -> Option<String> {
    std::env::var(LOCAL_ENV_ID_VAR)
        .ok()
        .filter(|id| !id.is_empty())
}

/// Whether `host` is the machine whose environment id is `local_env_id`.
pub fn is_local(host: &Host, local_env_id: Option<&str>) -> bool {
    local_env_id == Some(host.env_id.as_str())
}

/// The configured host named `name`.
pub fn find<'a>(config: &'a Config, name: &str) -> anyhow::Result<&'a Host> {
    config
        .hosts
        .iter()
        .find(|host| host.name == name)
        .ok_or_else(|| anyhow!("no host named {name:?} in config.toml"))
}
