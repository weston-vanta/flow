use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::json;

const CONFIG: &str = r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/workspaces/example-repo"
default_branch = "main"

[[cdes]]
name = "cde-a"
env_id = "env-0001"
ssh_alias = "cde-a.example"

[[cdes]]
name = "cde-b"
env_id = "env-0002"
ssh_alias = "cde-b.example"
"#;

/// A temp dir holding `state/config.toml` with `cde-a` (env `env-0001`) and `cde-b`.
fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("config.toml"), CONFIG).unwrap();
    (dir, state)
}

/// `flow` on `state`, running as the CDE whose env id is `env-0001` (`cde-a`).
fn flow(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
    cmd.env("FLOW_STATE", state)
        .env("ONA_ID", "env-0001")
        .env_remove("FLOW_FAKE_RUNNER");
    cmd
}

#[test]
fn cde_list_marks_the_local_cde() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["cde", "list"])
        .assert()
        .success()
        .stdout("cde-a  env-0001  local\ncde-b  env-0002\n");
}

#[test]
fn cde_list_json() {
    let (_dir, state) = setup();
    let out = flow(&state)
        .args(["cde", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        listed,
        json!([
            {"name": "cde-a", "env_id": "env-0001", "ssh_alias": "cde-a.example", "local": true},
            {"name": "cde-b", "env_id": "env-0002", "ssh_alias": "cde-b.example", "local": false},
        ])
    );
}

#[test]
fn cde_run_local_echo() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["cde", "run", "cde-a", "--", "echo", "hi"])
        .assert()
        .success()
        .stdout("hi\n");
}

#[test]
fn cde_run_passes_stderr_and_exit_code_through() {
    let (_dir, state) = setup();
    flow(&state)
        .args([
            "cde",
            "run",
            "cde-a",
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
fn cde_run_unknown_cde_fails() {
    let (_dir, state) = setup();
    flow(&state)
        .args(["cde", "run", "cde-z", "--", "true"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cde-z"));
}

#[test]
fn cde_run_uses_fake_runner_when_set() {
    let (dir, state) = setup();
    let script = dir.path().join("runner.json");
    fs::write(
        &script,
        json!([{"cde": "cde-b", "status": 4, "stdout": "scripted\n", "stderr": "warn\n"}])
            .to_string(),
    )
    .unwrap();

    flow(&state)
        .env("FLOW_FAKE_RUNNER", &script)
        .args(["cde", "run", "cde-b", "--", "herdr", "status"])
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
        [json!({"cde": "cde-b", "argv": ["herdr", "status"]})]
    );
}
