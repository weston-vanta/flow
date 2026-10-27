use std::io::{self, Write};

use serde::Serialize;

use crate::cde;
use crate::paths::StateDir;
use crate::runner;
use crate::state::config::Config;

const LOCAL_MARKER: &str = "local";

#[derive(Debug, Serialize)]
struct CdeView<'a> {
    name: &'a str,
    env_id: &'a str,
    ssh_alias: &'a str,
    local: bool,
}

/// Configured CDEs: `name  env-id  local?`.
pub fn run_list(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let local_env_id = cde::local_env_id();
    let views: Vec<CdeView> = config
        .cdes
        .iter()
        .map(|c| CdeView {
            name: &c.name,
            env_id: &c.env_id,
            ssh_alias: &c.ssh_alias,
            local: cde::is_local(c, local_env_id.as_deref()),
        })
        .collect();
    if json {
        return super::print_json(&views);
    }
    let rows: Vec<Vec<String>> = views
        .iter()
        .map(|view| {
            let mut row = vec![view.name.to_owned(), view.env_id.to_owned()];
            if view.local {
                row.push(LOCAL_MARKER.to_owned());
            }
            row
        })
        .collect();
    super::print(&crate::render::table(&rows))
}

/// Runs `argv` on the CDE named `name`, passing its stdout and stderr through, and returns its
/// exit status.
pub fn run_command(state: &StateDir, name: &str, argv: &[String]) -> anyhow::Result<i32> {
    let config = Config::load(state)?;
    let target = cde::find(&config, name)?;
    let output = runner::from_env()?.run(target, argv, None)?;
    super::print(&output.stdout)?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(output.stderr.as_bytes())?;
    stderr.flush()?;
    Ok(output.status)
}
