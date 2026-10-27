//! `flow attach`: replaces this process with a terminal attached to the supervisor's pane.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{Context, anyhow};

use crate::cde;
use crate::paths::StateDir;
use crate::runner;
use crate::state::config::{Cde, Config};
use crate::supervisor::{self, Lock};

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let lock = supervisor::load(state)?.ok_or_else(|| {
        anyhow!("no supervisor recorded in state/supervisor/lock.json; run `flow up` to start one")
    })?;
    let host = host(&config, &lock).ok_or_else(|| {
        anyhow!(
            "supervisor host {} is not a CDE in config.toml; run `flow up` to start a new \
             supervisor",
            lock.host_env_id
        )
    })?;
    exec(&lock, host)
}

/// The configured CDE the lock says the supervisor runs on.
pub(crate) fn host<'a>(config: &'a Config, lock: &Lock) -> Option<&'a Cde> {
    config
        .cdes
        .iter()
        .find(|cde| cde.env_id == lock.host_env_id)
}

/// Runs the attach command in place of this process, so it owns the terminal. Under the fake
/// runner it prints `attach: <argv>` instead.
pub(crate) fn exec(lock: &Lock, host: &Cde) -> anyhow::Result<()> {
    let local = cde::is_local(host, cde::local_env_id().as_deref());
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
