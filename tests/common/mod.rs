//! Runs flow's binaries against a Store of their own, so no test touches the real one.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::store::Store;
use serde_json::Value;
use tempfile::TempDir;

/// Variables the binaries read that a test run inside a Claude Code Session would inherit.
const INHERITED_ENV: &[&str] = &[
    "CLAUDE_CODE_SESSION_ID",
    "ONA_ID",
    "HERDR_PANE_ID",
    "HERDR_SOCKET_PATH",
];

/// A temp directory holding the binaries' Store (through `FLOW_STORE`), the hook log beside
/// it, and a `bin` directory put first on PATH for fake executables; removed on drop.
pub struct TestStore {
    dir: TempDir,
}

impl TestStore {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        fs::create_dir(dir.path().join("bin")).expect("create bin dir");
        Self { dir }
    }

    /// The `flow` CLI with `FLOW_STORE` set to this Store.
    pub fn flow(&self) -> Command {
        self.command("flow")
    }

    /// The `flowd` daemon with `FLOW_STORE` set to this Store.
    pub fn flowd(&self) -> Command {
        self.command("flowd")
    }

    /// The library Store the binaries use, for writing backdated state with an explicit `at`.
    pub fn store(&self) -> Store {
        Store::new(self.store_path())
    }

    /// The Store's root directory, which the binaries get as `FLOW_STORE`.
    pub fn store_path(&self) -> PathBuf {
        self.dir.path().join("store")
    }

    /// A scratch directory inside this test's temp dir.
    pub fn scratch(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::create_dir_all(&path).expect("create scratch dir");
        path
    }

    /// Puts an executable shell script named `name` first on the binaries' PATH.
    pub fn fake_bin(&self, name: &str, script: &str) {
        let path = self.dir.path().join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write fake bin");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake bin");
    }

    /// The lines `flow hook` has logged so far.
    pub fn hook_log(&self) -> Vec<String> {
        fs::read_to_string(self.dir.path().join("hook.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Runs `flow session get <id>` and returns its JSON, asserting it succeeded.
    pub fn session(&self, id: &str) -> Value {
        let out = self.flow().args(["session", "get", id]).assert().success();
        serde_json::from_slice(&out.get_output().stdout).expect("session get prints JSON")
    }

    /// Runs `flow session list` and returns its JSON array, asserting it succeeded.
    pub fn sessions(&self) -> Vec<Value> {
        let out = self.flow().args(["session", "list"]).assert().success();
        serde_json::from_slice(&out.get_output().stdout).expect("session list prints JSON")
    }

    fn command(&self, bin: &str) -> Command {
        let mut cmd = Command::cargo_bin(bin).expect("binary built by cargo");
        cmd.env("FLOW_STORE", self.store_path());
        for name in INHERITED_ENV {
            cmd.env_remove(name);
        }
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env(
            "PATH",
            format!("{}:{path}", self.dir.path().join("bin").display()),
        );
        cmd
    }
}

/// Makes `dir` a git repository with `branch` checked out (unborn: no commit needed).
pub fn git_repo(dir: &Path, branch: &str) {
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", branch])
        .current_dir(dir)
        .status()
        .expect("run git init");
    assert!(status.success(), "git init failed");
}
