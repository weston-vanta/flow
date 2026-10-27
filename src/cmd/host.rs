use std::io::{self, Write};

use jiff::Timestamp;
use serde::Serialize;

use crate::host;
use crate::inventory::{self, HostInventory};
use crate::paths::StateDir;
use crate::render;
use crate::runner;

const LOCAL_MARKER: &str = "local";
const GONE: &str = "gone";
/// The phase column of a host Ona does not manage.
const NO_PHASE: &str = "-";
const ONA_PHASE_PREFIX: &str = "ENVIRONMENT_PHASE_";

#[derive(Debug, Serialize)]
struct HostView<'a> {
    name: &'a str,
    env_id: &'a str,
    ssh_alias: String,
    local: bool,
    /// Ona's phase name at the last listing; `null` for a host Ona does not manage.
    phase: Option<&'a str>,
    seen: Timestamp,
    reachable: bool,
    gone: bool,
}

/// Known hosts from the cache: `name  env_id  phase  seen  reachable|unreachable  [local]`, gone
/// hosts last with `gone` as their phase.
pub fn run_list(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let mut records = inventory::load_all(state)?;
    records.sort_by_key(|record| record.gone);
    let local_env_id = host::local_env_id();
    let views: Vec<HostView> = records
        .iter()
        .map(|record| view(record, local_env_id.as_deref()))
        .collect();
    if json {
        return super::print_json(&views);
    }
    let now = Timestamp::now();
    let rows: Vec<Vec<String>> = views
        .iter()
        .map(|view| {
            let mut row = vec![
                view.name.to_owned(),
                view.env_id.to_owned(),
                phase_cell(view),
                render::relative(view.seen, now),
                if view.reachable {
                    "reachable"
                } else {
                    "unreachable"
                }
                .to_owned(),
            ];
            if view.local {
                row.push(LOCAL_MARKER.to_owned());
            }
            row
        })
        .collect();
    super::print(&render::table(&rows))
}

fn view<'a>(record: &'a HostInventory, local_env_id: Option<&str>) -> HostView<'a> {
    let host = record.host();
    HostView {
        name: &record.name,
        env_id: &record.env_id,
        local: host::is_local(&host, local_env_id),
        ssh_alias: host.ssh_alias,
        phase: record.ona_phase.as_deref(),
        seen: record.seen,
        reachable: record.reachable,
        gone: record.gone,
    }
}

/// `gone`, Ona's phase without its `ENVIRONMENT_PHASE_` prefix in lower case, or `-`.
fn phase_cell(view: &HostView) -> String {
    if view.gone {
        return GONE.to_owned();
    }
    view.phase.map_or_else(
        || NO_PHASE.to_owned(),
        |phase| {
            phase
                .strip_prefix(ONA_PHASE_PREFIX)
                .unwrap_or(phase)
                .to_lowercase()
        },
    )
}

/// Runs `argv` on the host named `name`, passing its stdout and stderr through, and returns its
/// exit status.
pub fn run_command(state: &StateDir, name: &str, argv: &[String]) -> anyhow::Result<i32> {
    let hosts = inventory::hosts(state)?;
    let target = host::find(&hosts, name)?;
    let output = runner::from_env()?.run(target, argv, None)?;
    super::print(&output.stdout)?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(output.stderr.as_bytes())?;
    stderr.flush()?;
    Ok(output.status)
}
