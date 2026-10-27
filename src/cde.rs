//! Which CDE this process runs on, and lookups over the configured CDEs.

use anyhow::anyhow;

use crate::state::config::{Cde, Config};

const LOCAL_ENV_ID_VAR: &str = "ONA_ID";

/// The Ona environment id of the CDE this process runs on, from `$ONA_ID`.
pub fn local_env_id() -> Option<String> {
    std::env::var(LOCAL_ENV_ID_VAR)
        .ok()
        .filter(|id| !id.is_empty())
}

/// Whether `cde` is the machine whose environment id is `local_env_id`.
pub fn is_local(cde: &Cde, local_env_id: Option<&str>) -> bool {
    local_env_id == Some(cde.env_id.as_str())
}

/// The configured CDE named `name`.
pub fn find<'a>(config: &'a Config, name: &str) -> anyhow::Result<&'a Cde> {
    config
        .cdes
        .iter()
        .find(|cde| cde.name == name)
        .ok_or_else(|| anyhow!("no CDE named {name:?} in config.toml"))
}
