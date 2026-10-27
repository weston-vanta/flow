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

/// The records after Ona's `listed` environments: one per environment not excluded, named as
/// `assign_names` says. A new environment gets an empty record seen `now`; one not running is unreachable;
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
    // Excluded records do not count, so excluding a deleted environment ends the warning.
    let others_cached = cached.iter().any(|record| {
        !record.gone && !is_local(&record.env_id) && !config.exclude.contains(&record.env_id)
    });
    if only_local_listed && others_cached {
        return Discovered {
            records: cached,
            warning: Some(ONLY_LOCAL_VISIBLE.to_owned()),
        };
    }

    environments.retain(|env| !config.exclude.contains(&env.id));
    environments.sort_by(|a, b| a.id.cmp(&b.id));
    environments.dedup_by(|a, b| a.id == b.id);
    let mut by_id = by_env_id(cached);
    let names = assign_names(&environments, &config.names, &by_id);
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

/// Each environment's host name, in the order given (id order), chosen so that names stay put
/// while environments come and go: first every `overrides` entry, which always wins; then each
/// cached record keeps its current name unless an override or an earlier kept name took it;
/// the rest get the slug of their Ona name. A name already taken gets `-2`, `-3`, ... added.
fn assign_names(
    environments: &[OnaEnvironment],
    overrides: &BTreeMap<String, String>,
    cached: &BTreeMap<String, HostInventory>,
) -> Vec<String> {
    let mut taken = HashSet::new();
    let mut names: Vec<Option<String>> = environments
        .iter()
        .map(|env| {
            overrides
                .get(&env.id)
                .map(|name| take_free(&mut taken, name))
        })
        .collect();
    for (env, name) in environments.iter().zip(&mut names) {
        if name.is_none()
            && let Some(record) = cached.get(&env.id)
            && taken.insert(record.name.clone())
        {
            *name = Some(record.name.clone());
        }
    }
    environments
        .iter()
        .zip(names)
        .map(|(env, name)| name.unwrap_or_else(|| take_free(&mut taken, &ona::slug(&env.name))))
        .collect()
}

/// `base`, or `base-2`, `base-3`, ... when taken, marked taken.
fn take_free(taken: &mut HashSet<String>, base: &str) -> String {
    let mut name = base.to_owned();
    let mut suffix = 1;
    while !taken.insert(name.clone()) {
        suffix += 1;
        name = format!("{base}-{suffix}");
    }
    name
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

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "00000000-0000-4000-8000-000000000001";

    fn env(id_suffix: u32, name: &str) -> OnaEnvironment {
        OnaEnvironment {
            id: format!("00000000-0000-4000-8000-{id_suffix:012}"),
            name: name.to_owned(),
            phase: "ENVIRONMENT_PHASE_RUNNING".to_owned(),
            content_location: None,
        }
    }

    fn refresh(
        cached: Vec<HostInventory>,
        listed: Vec<OnaEnvironment>,
        config: &HostsConfig,
    ) -> Discovered {
        from_ona(
            cached,
            Ok(listed),
            config,
            Some(LOCAL),
            Timestamp::UNIX_EPOCH,
        )
    }

    /// `(env id suffix, name, gone)` of each record, by env id.
    fn names(records: &[HostInventory]) -> Vec<(String, String, bool)> {
        let mut names: Vec<_> = records
            .iter()
            .map(|r| (r.env_id[24..].to_owned(), r.name.clone(), r.gone))
            .collect();
        names.sort();
        names
    }

    fn row(id_suffix: u32, name: &str, gone: bool) -> (String, String, bool) {
        (format!("{id_suffix:012}"), name.to_owned(), gone)
    }

    #[test]
    fn names_stay_put_when_environments_come_and_go() {
        let config = HostsConfig::default();
        let first = refresh(
            Vec::new(),
            vec![
                env(1, "Scratch Box"),
                env(2, "Scratch Box"),
                env(3, "Scratch Box"),
            ],
            &config,
        );
        assert_eq!(
            names(&first.records),
            [
                row(1, "scratch-box", false),
                row(2, "scratch-box-2", false),
                row(3, "scratch-box-3", false),
            ]
        );

        let second = refresh(
            first.records,
            vec![
                env(2, "Scratch Box"),
                env(3, "Scratch Box"),
                env(4, "Scratch Box"),
            ],
            &config,
        );

        assert_eq!(second.warning, None);
        assert_eq!(
            names(&second.records),
            [
                row(1, "scratch-box", true),
                row(2, "scratch-box-2", false),
                row(3, "scratch-box-3", false),
                row(4, "scratch-box", false),
            ],
            "existing hosts keep their names; only the new one is named"
        );
    }

    #[test]
    fn an_override_wins_over_an_earlier_slug() {
        let mut config = HostsConfig::default();
        config.names.insert(env(2, "").id, "primary".to_owned());
        let listed = || vec![env(1, "Primary"), env(2, "Side Project")];

        let fresh = refresh(Vec::new(), listed(), &config);
        assert_eq!(
            names(&fresh.records),
            [row(1, "primary-2", false), row(2, "primary", false)]
        );

        let before = refresh(Vec::new(), listed(), &HostsConfig::default());
        assert_eq!(names(&before.records)[0], row(1, "primary", false));
        let after = refresh(before.records, listed(), &config);
        assert_eq!(
            names(&after.records),
            [row(1, "primary-2", false), row(2, "primary", false)],
            "an override takes its name even from a host that held it"
        );
    }

    #[test]
    fn excluded_hosts_do_not_count_against_a_local_only_listing() {
        let config = HostsConfig::default();
        let cached = refresh(
            Vec::new(),
            vec![env(1, "Primary"), env(2, "Side Project")],
            &config,
        )
        .records;

        let stuck = refresh(cached.clone(), vec![env(1, "Primary")], &config);
        assert_eq!(stuck.warning.as_deref(), Some(ONLY_LOCAL_VISIBLE));

        let excluded = HostsConfig {
            exclude: vec![env(2, "").id],
            ..HostsConfig::default()
        };
        let freed = refresh(cached, vec![env(1, "Primary")], &excluded);
        assert_eq!(freed.warning, None);
        assert_eq!(
            names(&freed.records),
            [row(1, "primary", false), row(2, "side-project", true)]
        );
    }
}
