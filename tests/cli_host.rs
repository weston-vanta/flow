use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::json;

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/workspaces/example-repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[[hosts]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"

[[hosts]]
name = "host-b"
env_id = "env-0002"
ssh_alias = "host-b.example"
"#;

/// A temp dir holding `state/config.toml` with `host-a` (env `env-0001`) and `host-b`.
fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("config.toml"), CONFIG).unwrap();
    (dir, state)
}

/// `flow` on `state`, running as the host whose env id is `env-0001` (`host-a`).
fn flow(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
    cmd.env("FLOW_STATE", state)
        .env("ONA_ID", "env-0001")
        .env_remove("FLOW_FAKE_RUNNER");
    cmd
}

#[test]
fn host_list_marks_the_local_host() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["host", "list"])
        .assert()
        .success()
        .stdout("host-a  env-0001  local\nhost-b  env-0002\n");
}

#[test]
fn host_list_json() {
    let (_dir, state) = setup();
    let out = flow(&state)
        .args(["host", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        listed,
        json!([
            {"name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example", "local": true},
            {"name": "host-b", "env_id": "env-0002", "ssh_alias": "host-b.example", "local": false},
        ])
    );
}

#[test]
fn host_run_local_echo() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["host", "run", "host-a", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout("hi\n");
}

#[test]
fn host_run_passes_stderr_and_exit_code_through() {
    let (_dir, state) = setup();
    flow(&state)
        .args([
            "host",
            "run",
            "host-a",
            "--",
            "sh",
            "-c",
            "echo oops >&2; exit 7",
        ])
        .assert()
        .code(7)
        .stdout("")
        .stderr("oops\n");
}

#[test]
fn host_run_unknown_host_fails() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["host", "run", "host-z", "--", "true"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("host-z"));
}

#[test]
fn host_run_uses_fake_runner_when_set() {
    let (dir, state) = setup();
    let script = dir.path().join("runner.json");
    fs::write(
        &script,
        json!([{"host": "host-b", "status": 4, "stdout": "scripted\n", "stderr": "warn\n"}])
            .to_string(),
    )
    .unwrap();

    flow(&state)
        .env("FLOW_FAKE_RUNNER", &script)
        .args(["host", "run", "host-b", "--", "herdr", "status"])
        .assert()
        .code(4)
        .stdout("scripted\n")
        .stderr("warn\n");

    let calls = fs::read_to_string(dir.path().join("runner.json.calls.jsonl")).unwrap();
    let logged: Vec<serde_json::Value> = calls
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        logged,
        [json!({"host": "host-b", "argv": ["herdr", "status"]})]
    );
}
