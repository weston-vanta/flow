//! Keeps the cached host records in step with the host set: the environments Ona lists, or the
//! static list in config.

use std::collections::{BTreeMap, HashSet};

use jiff::Timestamp;

use super::{HostInventory, load_all, save, unreachable};
use crate::host;
use crate::ona::{self, OnaEnvironment};
use crate::paths::StateDir;
use crate::state::config::{Host, HostSource, HostsConfig};

const ONLY_LOCAL_VISIBLE: &str = "ona: only the current environment is visible; run gitpod login";

/// Every host record after discovery, gone ones included, and the line to print when Ona could
/// not be used and the cached records were kept as they were.
#[derive(Debug)]
pub struct Discovered {
    pub records: Vec<HostInventory>,
    pub warning: Option<String>,
}

/// Brings the cached host records in step with the host set `config` names, saving the ones that
/// changed. With `source = "ona"` this lists environments through the Ona CLI.
pub fn discover(
    state: &StateDir,
    config: &HostsConfig,
    now: Timestamp,
) -> anyhow::Result<Discovered> {
    let cached = load_all(state)?;
    let discovered = match config.source {
        HostSource::Ona => from_ona(
            cached.clone(),
            ona::list_environments(),
            config,
            host::local_env_id().as_deref(),
            now,
        ),
        HostSource::Static => Discovered {
            records: from_static(cached.clone(), &config.static_hosts, now),
            warning: None,
        },
    };
    for record in &discovered.records {
        if !cached.contains(record) {
            save(state, record)?;
        }
    }
    Ok(discovered)
}

/// The records after Ona's `listed` environments: one per environment not excluded, named by
/// `config.names` or the slug of its Ona name, with `-2`, `-3`, ... added in id order where names
/// collide. A new environment gets an empty record seen `now`; one not running is unreachable;
/// a cached record whose environment is not listed is marked gone. When the listing failed, or
/// shows only the local environment while other hosts are cached (an environment-scoped token),
/// the cached records are returned unchanged with a warning.
pub fn from_ona(
    cached: Vec<HostInventory>,
    listed: anyhow::Result<Vec<OnaEnvironment>>,
    config: &HostsConfig,
    local_env_id: Option<&str>,
    now: Timestamp,
) -> Discovered {
    let mut environments = match listed {
        Ok(environments) => environments,
        Err(err) => {
            let live = cached.iter().filter(|record| !record.gone).count();
            return Discovered {
                records: cached,
                warning: Some(format!("ona: {err:#}; using {live} cached hosts")),
            };
        }
    };
    let is_local = |env_id: &str| local_env_id == Some(env_id);
    let only_local_listed = environments.iter().all(|env| is_local(&env.id));
    let others_cached = cached
        .iter()
        .any(|record| !record.gone && !is_local(&record.env_id));
    if only_local_listed && others_cached {
        return Discovered {
            records: cached,
            warning: Some(ONLY_LOCAL_VISIBLE.to_owned()),
        };
    }

    environments.retain(|env| !config.exclude.contains(&env.id));
    environments.sort_by(|a, b| a.id.cmp(&b.id));
    environments.dedup_by(|a, b| a.id == b.id);
    let names = assign_names(&environments, &config.names);
    let mut by_id = by_env_id(cached);
    let mut records: Vec<HostInventory> = environments
        .iter()
        .zip(names)
        .map(|(env, name)| {
            let host = Host {
                name,
                env_id: env.id.clone(),
                ssh_alias: ona::ssh_alias(&env.id),
                local_marker_env: None,
            };
            let record = by_id
                .remove(&env.id)
                .unwrap_or_else(|| HostInventory::new(&host, now));
            let record = HostInventory {
                name: host.name.clone(),
                ssh_alias: host.ssh_alias.clone(),
                ona_name: Some(env.name.clone()),
                ona_phase: Some(env.phase.clone()),
                gone: false,
                ..record
            };
            if env.running() {
                record
            } else {
                unreachable(&host, Some(record), now)
            }
        })
        .collect();
    records.extend(mark_gone(by_id));
    records.sort_by(|a, b| a.name.cmp(&b.name));
    Discovered {
        records,
        warning: None,
    }
}

/// The records after the static host list: one per listed host, named and aliased as config
/// says, and every other cached record marked gone.
pub fn from_static(
    cached: Vec<HostInventory>,
    hosts: &[Host],
    now: Timestamp,
) -> Vec<HostInventory> {
    let mut by_id = by_env_id(cached);
    let mut records: Vec<HostInventory> = hosts
        .iter()
        .map(|host| {
            let record = by_id
                .remove(&host.env_id)
                .unwrap_or_else(|| HostInventory::new(host, now));
            HostInventory {
                name: host.name.clone(),
                ssh_alias: host.ssh_alias.clone(),
                ona_name: None,
                ona_phase: None,
                gone: false,
                ..record
            }
        })
        .collect();
    records.extend(mark_gone(by_id));
    records.sort_by(|a, b| a.name.cmp(&b.name));
    records
}

/// Each environment's host name, in the order given: its `overrides` entry or the slug of its
/// name, with `-2`, `-3`, ... added when an earlier environment took it.
fn assign_names(
    environments: &[OnaEnvironment],
    overrides: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut taken = HashSet::new();
    environments
        .iter()
        .map(|env| {
            let base = overrides
                .get(&env.id)
                .cloned()
                .unwrap_or_else(|| ona::slug(&env.name));
            let mut name = base.clone();
            let mut suffix = 1;
            while !taken.insert(name.clone()) {
                suffix += 1;
                name = format!("{base}-{suffix}");
            }
            name
        })
        .collect()
}

fn by_env_id(records: Vec<HostInventory>) -> BTreeMap<String, HostInventory> {
    records
        .into_iter()
        .map(|record| (record.env_id.clone(), record))
        .collect()
}

fn mark_gone(records: BTreeMap<String, HostInventory>) -> impl Iterator<Item = HostInventory> {
    records.into_values().map(|record| HostInventory {
        gone: true,
        ..record
    })
}
