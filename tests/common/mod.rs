//! Runs flow's binaries against a Store of their own, so no test touches the real one.
#![allow(dead_code)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};

use assert_cmd::Command;
use assert_cmd::cargo::CommandCargoExt as _;
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
/// it, and a `bin` directory put first on PATH for fake executables (holding a `gh` that
/// always fails until a test replaces it); removed on drop.
pub struct TestStore {
    dir: TempDir,
}

impl TestStore {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        fs::create_dir(dir.path().join("bin")).expect("create bin dir");
        let ts = Self { dir };
        // `flowd` polls GitHub through `gh`; no test reaches the real one.
        ts.fake_bin("gh", "echo 'no fake gh in this test' >&2; exit 1");
        ts
    }

    /// The `flow` CLI with `FLOW_STORE` set to this Store.
    pub fn flow(&self) -> Command {
        self.command("flow")
    }

    /// Starts `flowd` on a free loopback port over this Store; it is killed on drop.
    pub fn flowd(&self) -> Flowd {
        self.flowd_with_env(&[])
    }

    /// Starts `flowd` like [`Self::flowd`], with extra environment variables.
    pub fn flowd_with_env(&self, env: &[(&str, &str)]) -> Flowd {
        let mut child = self
            .std_command("flowd")
            .args(["--port", "0"])
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start flowd");
        let mut line = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout
            .read_line(&mut line)
            .expect("read flowd's first line");
        let addr = line
            .trim()
            .rsplit("http://")
            .next()
            .and_then(|addr| addr.parse().ok())
            .unwrap_or_else(|| panic!("flowd printed no address: {line:?}"));
        Flowd {
            child,
            stdout,
            addr,
        }
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
        Command::from_std(self.std_command(bin))
    }

    fn std_command(&self, bin: &str) -> std::process::Command {
        let mut cmd = std::process::Command::cargo_bin(bin).expect("binary built by cargo");
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

/// A running `flowd`, killed on drop.
pub struct Flowd {
    child: Child,
    /// Held open so a later line from `flowd` does not fail on a closed pipe.
    stdout: BufReader<ChildStdout>,
    pub addr: SocketAddr,
}

impl Flowd {
    /// Sends `GET path` and returns the status code and body.
    pub fn get(&self, path: &str) -> (u16, String) {
        get(self.addr, path).expect("connect to flowd")
    }

    /// Blocks until `flowd` prints a stdout line starting with `prefix`, and returns it.
    pub fn wait_for_line(&mut self, prefix: &str) -> String {
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("read flowd's stdout");
            assert!(read > 0, "flowd exited before printing {prefix:?}");
            if line.starts_with(prefix) {
                return line;
            }
        }
    }
}

impl Drop for Flowd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sends `GET path` to `addr` over plain HTTP/1.1 and returns the status code and body.
pub fn get(addr: SocketAddr, path: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(addr)?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response.split_once("\r\n\r\n").expect("HTTP response");
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    Ok((status, body.to_owned()))
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
