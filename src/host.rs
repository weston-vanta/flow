//! Which host this process runs on, and lookups over the known hosts (`inventory::hosts`).

use anyhow::anyhow;

use crate::state::config::Host;

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

/// The host named `name` in `hosts`; the first one when several share it.
pub fn find<'a>(hosts: &'a [Host], name: &str) -> anyhow::Result<&'a Host> {
    hosts.iter().find(|host| host.name == name).ok_or_else(|| {
        anyhow!("no host named {name:?}; run `flow inventory --refresh` to discover hosts")
    })
}

/// The host in `hosts` this process runs on, found by `$ONA_ID`.
pub fn local(hosts: &[Host]) -> anyhow::Result<&Host> {
    let env_id = local_env_id().ok_or_else(|| {
        anyhow!("${LOCAL_ENV_ID_VAR} is not set, so this machine is unknown; pass --host")
    })?;
    hosts
        .iter()
        .find(|host| is_local(host, Some(&env_id)))
        .ok_or_else(|| {
            anyhow!(
                "this machine ({env_id}) is not a known host; run `flow inventory --refresh` \
                 to discover hosts"
            )
        })
}
