//! `flow attach`: on a terminal, replaces this process with one attached to the supervisor's
//! pane; otherwise prints the command that attaches.

use std::io::{self, IsTerminal};
use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{Context, anyhow};

use crate::herdr;
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
    exec(&lock, host, Instead::Command)
}

/// Under the fake runner, makes `exec` treat stdout as a terminal when set.
const FAKE_TTY_VAR: &str = "FLOW_FAKE_TTY";

/// What `exec` tells the user to run when stdout is not a terminal to attach.
pub(crate) enum Instead {
    /// `flow attach`.
    FlowAttach,
    /// The attach command itself.
    Command,
}

/// The host in `hosts` the lock says the supervisor runs on.
pub(crate) fn host<'a>(hosts: &'a [Host], lock: &Lock) -> Option<&'a Host> {
    hosts.iter().find(|host| host.env_id == lock.host_env_id)
}

/// Runs the attach command in place of this process, so it owns the terminal. When stdout is
/// not a terminal, which the attach command needs, it prints what to run from one, as
/// `instead` says, and returns. Under the fake runner it prints `attach: <argv>` in place of
/// running it.
pub(crate) fn exec(lock: &Lock, host: &Host, instead: Instead) -> anyhow::Result<()> {
    let local = host::is_local(host, host::local_env_id().as_deref());
    let control_dir = runner::default_control_dir();
    let argv = supervisor::attach_argv(lock, host, local, &control_dir);
    if !stdout_is_terminal() {
        let command = match instead {
            Instead::FlowAttach => "flow attach".to_owned(),
            Instead::Command => {
                // The printed ssh command names a socket in the control dir, which is gone after
                // a reboot.
                if !local {
                    runner::ensure_control_dir(&control_dir)?;
                }
                argv.iter()
                    .map(|word| herdr::shell_word(word))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        return super::print(&format!(
            "stdout is not a terminal, so flow did not attach; to attach, run: {command}\n"
        ));
    }
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

fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
        || (runner::fake_runner_active() && std::env::var_os(FAKE_TTY_VAR).is_some())
}
