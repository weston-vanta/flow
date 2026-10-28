//! `flow run`: runs a command on a host, passing its output and exit code through.

use std::io::{self, Write};

use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner;

/// Runs `argv` on the host named `name`, passing its stdout and stderr through, and returns its
/// exit status.
pub fn run(state: &StateDir, name: &str, argv: &[String]) -> anyhow::Result<i32> {
    let hosts = inventory::hosts(state)?;
    let target = host::find(&hosts, name)?;
    let output = runner::from_env()?.run(target, argv, None)?;
    super::print(&output.stdout)?;
    let mut stderr = io::stderr().lock();
    stderr.write_all(output.stderr.as_bytes())?;
    stderr.flush()?;
    Ok(output.status)
}
