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
"#;

/// A temp dir holding `state/config.toml` and cached records of `host-a` (env `env-0001`,
/// running and reachable) and `host-b` (static, unreachable), both seen five minutes ago.
fn setup() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("config.toml"), CONFIG).unwrap();
    write_record(
        &state,
        &json!({
            "name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example",
            "reachable": true, "ona_phase": "ENVIRONMENT_PHASE_RUNNING",
        }),
    );
    write_record(
        &state,
        &json!({
            "name": "host-b", "env_id": "env-0002", "ssh_alias": "host-b.example",
            "reachable": false,
        }),
    );
    (dir, state)
}

/// Writes `fields` over a bare record seen five minutes ago to `state/hosts/<env_id>.json`.
fn write_record(state: &Path, fields: &serde_json::Value) {
    let seen = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(5);
    let mut record = json!({
        "seen": seen.to_string(), "load1": null, "worktrees": [], "agents": [], "prs": [],
    });
    for (key, value) in fields.as_object().unwrap() {
        record[key] = value.clone();
    }
    let hosts = state.join("hosts");
    fs::create_dir_all(&hosts).unwrap();
    let env_id = record["env_id"].as_str().unwrap().to_owned();
    fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
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
        .stdout(
            "host-a  env-0001  running  5m  reachable    local\n\
             host-b  env-0002  -        5m  unreachable\n",
        );
}

#[test]
fn host_list_shows_gone_last() {
    let (_dir, state) = setup();
    write_record(
        &state,
        &json!({
            "name": "aaa-retired", "env_id": "env-0000", "ssh_alias": "",
            "reachable": true, "gone": true, "ona_phase": "ENVIRONMENT_PHASE_RUNNING",
        }),
    );
    write_record(
        &state,
        &json!({
            "name": "host-c", "env_id": "env-0003", "ssh_alias": "",
            "reachable": false, "ona_phase": "ENVIRONMENT_PHASE_STOPPED",
        }),
    );

    flow(&state)
        .args(["host", "list"])
        .assert()
        .success()
        .stdout(
            "host-a       env-0001  running  5m  reachable    local\n\
             host-b       env-0002  -        5m  unreachable\n\
             host-c       env-0003  stopped  5m  unreachable\n\
             aaa-retired  env-0000  gone     5m  reachable\n",
        );
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
    let mut listed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    for view in listed.as_array_mut().unwrap() {
        assert!(view["seen"].is_string(), "{view}");
        view.as_object_mut().unwrap().remove("seen");
    }
    assert_eq!(
        listed,
        json!([
            {"name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example", "local": true,
             "phase": "ENVIRONMENT_PHASE_RUNNING", "reachable": true, "gone": false},
            {"name": "host-b", "env_id": "env-0002", "ssh_alias": "host-b.example", "local": false,
             "phase": null, "reachable": false, "gone": false},
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
