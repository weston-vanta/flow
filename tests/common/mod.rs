//! What every integration test builds its state and its child processes through, so that no
//! test reads or writes the state `cargo test`'s own environment names. A worker session runs
//! with `FLOW_STATE` set to the shared state, and the binary, when `FLOW_STATE` is unset, falls
//! back to the repo's `state` link, which points at the same place.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use flow::paths::{STATE_ENV, StateDir};

/// The `FLOW_STATE` a flow child gets until its test names its own state: a path that never
/// exists, so a child the test forgot to point at a fixture fails instead of falling back to
/// the repo's state link.
pub const NO_STATE: &str = "/nonexistent/flow-test-state";

/// The state directory `<root>/state`, read without the environment.
pub fn state(root: &Path) -> StateDir {
    StateDir::at(root.join("state")).unwrap()
}

/// `program` with `FLOW_STATE` removed from its environment.
pub fn command(program: impl AsRef<OsStr>) -> StdCommand {
    let mut cmd = StdCommand::new(program);
    cmd.env_remove(STATE_ENV);
    cmd
}

/// The flow binary, with every variable flow reads removed and `FLOW_STATE` set to
/// [`NO_STATE`]; the test sets the state it means after.
pub fn flow_std() -> StdCommand {
    let mut cmd = command(flow_bin());
    for name in flow::vars::ALL {
        cmd.env_remove(name);
    }
    cmd.env(STATE_ENV, NO_STATE);
    cmd
}

/// [`flow_std`] as an `assert_cmd` command.
pub fn flow() -> assert_cmd::Command {
    assert_cmd::Command::from_std(flow_std())
}

pub fn flow_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("flow")
}
