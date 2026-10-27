//! `flow attach`: replaces this process with a terminal attached to the supervisor's pane.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{Context, anyhow};

use crate::host;
use crate::inventory;
use crate::paths::StateDir;
use crate::runner;
use crate::state::config::Host;
use crate::supervisor::{self, Lock};

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let lock = supervisor::load(state)?.ok_or_else(|| {
        anyhow!("no supervisor recorded in state/supervisor/lock.json; run `flow up` to start one")
    })?;
    let hosts = inventory::hosts(state)?;
    let host = host(&hosts, &lock).ok_or_else(|| {
        anyhow!(
            "supervisor host {} is not a known host; run `flow up` to start a new supervisor",
            lock.host_env_id
        )
    })?;
    exec(&lock, host)
}

/// The host in `hosts` the lock says the supervisor runs on.
pub(crate) fn host<'a>(hosts: &'a [Host], lock: &Lock) -> Option<&'a Host> {
    hosts.iter().find(|host| host.env_id == lock.host_env_id)
}

/// Runs the attach command in place of this process, so it owns the terminal. Under the fake
/// runner it prints `attach: <argv>` instead.
pub(crate) fn exec(lock: &Lock, host: &Host) -> anyhow::Result<()> {
    let local = host::is_local(host, host::local_env_id().as_deref());
    let control_dir = runner::default_control_dir();
    let argv = supervisor::attach_argv(lock, host, local, &control_dir);
    if runner::fake_runner_active() {
        return super::print(&format!("attach: {}\n", argv.join(" ")));
    }
    if !local {
        runner::ensure_control_dir(&control_dir)?;
    }
    let (program, args) = argv.split_first().expect("the attach argv names a program");
    let err = Command::new(program).args(args).exec();
    Err(err).with_context(|| format!("running `{}`", argv.join(" ")))
}
