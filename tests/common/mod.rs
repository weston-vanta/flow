//! Runs flow's binaries against a Store of their own, so no test touches the real one.

use assert_cmd::Command;
use tempfile::TempDir;

/// A temp directory the binaries use as their Store through `FLOW_STORE`; removed on drop.
pub struct TestStore {
    dir: TempDir,
}

impl TestStore {
    pub fn new() -> Self {
        Self {
            dir: TempDir::new().expect("create temp Store"),
        }
    }

    /// The `flow` CLI with `FLOW_STORE` set to this Store.
    pub fn flow(&self) -> Command {
        self.command("flow")
    }

    /// The `flowd` daemon with `FLOW_STORE` set to this Store.
    pub fn flowd(&self) -> Command {
        self.command("flowd")
    }

    fn command(&self, bin: &str) -> Command {
        let mut cmd = Command::cargo_bin(bin).expect("binary built by cargo");
        cmd.env("FLOW_STORE", self.dir.path());
        cmd
    }
}
