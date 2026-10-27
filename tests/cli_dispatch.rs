use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::task::{HerdrTarget, HoldOn, State};
use serde_json::{Value, json};

const CONFIG: &str = r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/workspaces/acme"
default_branch = "main"

[[cdes]]
name = "cde-a"
env_id = "env-0001"
ssh_alias = "cde-a.example"
"#;

const AGENT_LIST: &str = include_str!("fixtures/herdr/agent_list.json");
const AGENT_GET: &str = include_str!("fixtures/herdr/agent_get.json");
const AGENT_WAIT_TIMEOUT: &str = include_str!("fixtures/herdr/agent_wait_timeout.json");
const WORKSPACE_CREATE: &str = include_str!("fixtures/herdr/workspace_create.json");
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const PANE_RUN_OK: &str = r#"{"id":"cli:pane:run","result":{"type":"ok"}}"#;
const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";

/// The worktree a scripted `workspace create` reports, and the one most tests dispatch into.
const WORKTREE: &str = "/workspaces/acme-worktrees/search-index";
/// The pane and workspace a scripted `workspace create` returns.
const PANE: &str = "w3:p1";
const WORKSPACE: &str = "w3";

const TRUST_DIALOG: &str = "\
 Accessing workspace:

 /workspaces/acme-worktrees/search-index

 Quick safety check: Is this a project you created or one you trust?

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        Fixture { dir, state }
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on this state dir, answering runner calls from `script` in order.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path());
        cmd
    }

    /// The argv of every runner call, in order.
    fn calls(&self) -> Vec<Vec<String>> {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                assert_eq!(call["cde"], "cde-a", "{call}");
                serde_json::from_value(call["argv"].clone()).unwrap()
            })
            .collect()
    }

    fn store(&self) -> Store {
        Store::open(&StateDir::resolve(self.dir.path()).unwrap())
    }

    /// The id of the only task in the store.
    fn only_task(&self) -> String {
        let tasks = self.store().list_tasks().unwrap();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        tasks[0].0.id.clone()
    }

    fn events(&self, id: &str) -> Vec<Event> {
        events::read_all(&self.store().task_dir(id).unwrap()).unwrap()
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
}

fn failed(stderr: &str) -> Value {
    json!({"status": 1, "stdout": "", "stderr": stderr})
}

/// `agent wait` returning an agent in `status`.
fn wait_reports(status: &str) -> Value {
    let fixture = AGENT_GET.replace(
        r#""agent_status":"working""#,
        &format!(r#""agent_status":"{status}""#),
    );
    assert!(AGENT_GET.contains(r#""agent_status":"working""#));
    ok(&fixture)
}

/// Runner answers for a dispatch whose worker starts working right away, after `before`.
fn starts_working(before: Vec<Value>) -> Vec<Value> {
    let mut script = before;
    script.extend([
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("working"),
    ]);
    script
}

fn dispatch_args(worktree: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "dispatch",
        "--cde",
        "cde-a",
        "--title",
        "Rebuild the search index",
        "--brief",
        "Rebuild the search index nightly.",
        "--criteria",
        "- A draft PR is open",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(worktree.iter().map(|s| (*s).to_owned()));
    args
}

/// The herdr calls whose subcommand, after `herdr --session flow`, starts with `subcommand`.
fn herdr_calls<'a>(calls: &'a [Vec<String>], subcommand: &[&str]) -> Vec<&'a Vec<String>> {
    calls
        .iter()
        .filter(|argv| {
            argv.first().is_some_and(|program| program == "herdr")
                && argv.len() >= 3 + subcommand.len()
                && argv[3..3 + subcommand.len()] == *subcommand
        })
        .collect()
}

#[test]
fn dispatch_refuses_shared_worktree() {
    let fx = Fixture::new();
    let shared = "/workspaces/acme-worktrees/login-form";

    let out = fx
        .flow(&[ok(SERVER_RUNNING), ok(AGENT_LIST)])
        .args(dispatch_args(&["--worktree", shared]))
        .assert()
        .code(2)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("Refactor login form"), "{stderr}");
    assert!(stderr.contains("w1:p1"), "{stderr}");
    assert!(stderr.contains("--allow-shared-worktree"), "{stderr}");
    let tasks = fx.state.join("tasks");
    assert!(
        !tasks.exists() || fs::read_dir(&tasks).unwrap().next().is_none(),
        "a task was created"
    );
    assert!(herdr_calls(&fx.calls(), &["workspace", "create"]).is_empty());
}

#[test]
fn dispatch_allows_shared_worktree_when_asked() {
    let fx = Fixture::new();
    let shared = "/workspaces/acme-worktrees/login-form";
    let script = [
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("idle"),
    ];

    fx.flow(&script)
        .args(dispatch_args(&[
            "--worktree",
            shared,
            "--allow-shared-worktree",
        ]))
        .assert()
        .success();

    assert!(herdr_calls(&fx.calls(), &["agent", "list"]).is_empty());
    let (rec, status) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(rec.worktree, Path::new(shared));
    assert_eq!(status.state, State::Working);
}

#[test]
fn dispatch_creates_task_and_starts_agent() {
    let fx = Fixture::new();

    let out = fx
        .flow(&starts_working(Vec::new()))
        .args(dispatch_args(&["--worktree", WORKTREE]))
        .assert()
        .success()
        .get_output()
        .clone();

    let id = fx.only_task();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("dispatched {id} on cde-a ({PANE})\n")
    );
    let calls = fx.calls();
    for argv in &calls {
        assert_eq!(argv[..3], ["herdr", "--session", "flow"], "{argv:?}");
    }
    assert_eq!(
        *herdr_calls(&calls, &["workspace", "create"])[0],
        [
            "herdr",
            "--session",
            "flow",
            "workspace",
            "create",
            "--cwd",
            WORKTREE,
            "--label",
            &id,
            "--no-focus"
        ]
    );
    let runs = herdr_calls(&calls, &["pane", "run"]);
    assert_eq!(runs.len(), 1, "{calls:?}");
    assert_eq!(runs[0][5], PANE);
    assert!(
        runs[0][6].starts_with("claude --settings "),
        "{:?}",
        runs[0][6]
    );
    assert!(
        runs[0][6].contains(&format!("--name {id} ")),
        "{:?}",
        runs[0][6]
    );
    let waits = herdr_calls(&calls, &["agent", "wait"]);
    assert_eq!(
        waits[0][5..],
        [
            PANE,
            "--until",
            "blocked",
            "--until",
            "idle",
            "--until",
            "working",
            "--timeout",
            "60000"
        ]
    );

    let store = fx.store();
    let (rec, status) = store.task(&id).unwrap();
    assert_eq!(rec.title, "Rebuild the search index");
    assert_eq!(rec.cde, "cde-a");
    assert_eq!(rec.worktree, Path::new(WORKTREE));
    assert_eq!(rec.owner, "supervisor");
    assert_eq!(rec.parent, None);
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert_eq!(
        status.herdr,
        Some(HerdrTarget {
            cde: "cde-a".to_owned(),
            workspace_id: WORKSPACE.to_owned(),
            pane_id: PANE.to_owned(),
            agent: "claude".to_owned(),
        })
    );
    let brief = store.brief(&id).unwrap();
    assert!(
        brief.contains("Rebuild the search index nightly."),
        "{brief}"
    );
    assert!(brief.contains("- A draft PR is open"), "{brief}");
    assert!(
        store
            .task_dir(&id)
            .unwrap()
            .join("claude-settings.json")
            .is_file()
    );

    let events = fx.events(&id);
    let started = events
        .iter()
        .find(|ev| ev.kind == "worker.started")
        .expect("worker.started event");
    assert_eq!(started.data["pane_id"], PANE);
    assert_eq!(started.data["workspace_id"], WORKSPACE);
    assert_eq!(started.data["argv"][0], "claude");
    assert_eq!(started.data["argv"][1], "--settings");
    assert!(!events.iter().any(|ev| ev.kind == "dispatch.failed"));

    let status_out = fx
        .flow(&[])
        .arg("status")
        .assert()
        .success()
        .get_output()
        .clone();
    let status_out = String::from_utf8(status_out.stdout).unwrap();
    assert!(
        status_out.starts_with(&format!("{id}  working  cde-a  {PANE}  ")),
        "{status_out}"
    );
}

#[test]
fn dispatch_with_branch_runs_worktree_tool() {
    let fx = Fixture::new();
    let created = "/workspaces/acme-worktrees/feature-x";
    let tool_output = format!("Preparing worktree (new branch 'feature-x')\n{created}\n\n");

    fx.flow(&starts_working(vec![ok(&tool_output)]))
        .args(dispatch_args(&["--branch", "feature-x"]))
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(
        calls[0],
        [
            "sh",
            "-c",
            r#"cd "$1" && shift && exec "$@""#,
            "sh",
            "/workspaces/acme",
            "wt",
            "create",
            "feature-x"
        ]
    );
    let (rec, _) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(rec.worktree, Path::new(created));
    let create = herdr_calls(&calls, &["workspace", "create"]);
    assert_eq!(create[0][6], created);
}

#[test]
fn dispatch_answers_the_trust_dialog() {
    let fx = Fixture::new();
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("blocked"),
        ok(TRUST_DIALOG),
        ok(""),
        wait_reports("idle"),
    ];

    fx.flow(&script)
        .args(dispatch_args(&["--worktree", WORKTREE]))
        .assert()
        .success();

    let calls = fx.calls();
    let reads = herdr_calls(&calls, &["agent", "read"]);
    assert_eq!(reads[0][5..], [PANE, "--lines", "40", "--format", "text"]);
    let keys = herdr_calls(&calls, &["pane", "send-keys"]);
    assert_eq!(keys.len(), 1, "{calls:?}");
    assert_eq!(keys[0][5..], [PANE, "down", "enter"]);
    let waits = herdr_calls(&calls, &["agent", "wait"]);
    assert_eq!(waits.len(), 2, "{calls:?}");
    assert_eq!(
        waits[1][5..],
        [
            PANE,
            "--until",
            "idle",
            "--until",
            "working",
            "--timeout",
            "60000"
        ]
    );
    let (_, status) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(status.state, State::Working);
}

#[test]
fn dispatch_records_an_unexpected_block_without_sending_keys() {
    let fx = Fixture::new();
    let pane_text = "Choose a theme:\n ❯ Dark\n   Light\n";
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("blocked"),
        ok(pane_text),
    ];

    fx.flow(&script)
        .args(dispatch_args(&["--worktree", WORKTREE]))
        .assert()
        .success();

    assert!(herdr_calls(&fx.calls(), &["pane", "send-keys"]).is_empty());
    let id = fx.only_task();
    let (_, status) = fx.store().task(&id).unwrap();
    assert_eq!(status.state, State::Working);
    let events = fx.events(&id);
    let blocked = events
        .iter()
        .find(|ev| ev.kind == "worker.blocked_at_start")
        .expect("worker.blocked_at_start event");
    assert_eq!(blocked.data["pane_text"], pane_text);
}

#[test]
fn dispatch_marks_task_blocked_when_launch_fails() {
    let fx = Fixture::new();
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        failed(
            r#"{"error":{"code":"pane_not_found","message":"no such pane"},"id":"cli:pane:run"}"#,
        ),
    ];

    let out = fx
        .flow(&script)
        .args(dispatch_args(&["--worktree", WORKTREE]))
        .assert()
        .code(1)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("pane_not_found"), "{stderr}");
    let id = fx.only_task();
    let (_, status) = fx.store().task(&id).unwrap();
    assert_eq!(status.state, State::Blocked);
    let hold = status.hold.expect("hold is set");
    assert_eq!(hold.on, HoldOn::Supervisor);
    assert!(hold.what.starts_with("dispatch failed: "), "{}", hold.what);
    assert!(hold.what.contains("pane_not_found"), "{}", hold.what);
    assert_eq!(status.herdr.map(|t| t.pane_id).as_deref(), Some(PANE));
    let events = fx.events(&id);
    let failed = events
        .iter()
        .find(|ev| ev.kind == "dispatch.failed")
        .expect("dispatch.failed event");
    assert!(
        failed.data["error"]
            .as_str()
            .unwrap()
            .contains("pane_not_found"),
        "{}",
        failed.data
    );
}

#[test]
fn dispatch_fails_naming_the_pane_when_the_worker_never_starts() {
    let fx = Fixture::new();
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(AGENT_WAIT_TIMEOUT),
    ];

    let out = fx
        .flow(&script)
        .args(dispatch_args(&["--worktree", WORKTREE]))
        .assert()
        .code(1)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(PANE), "{stderr}");
    assert!(herdr_calls(&fx.calls(), &["workspace", "close"]).is_empty());
    let (_, status) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(status.state, State::Blocked);
}

#[test]
fn dispatch_unknown_cde_exits_2() {
    let fx = Fixture::new();
    let mut args = dispatch_args(&["--worktree", WORKTREE]);
    args[2] = "cde-z".to_owned();

    fx.flow(&[])
        .args(args)
        .assert()
        .code(2)
        .stderr(predicates::str::contains("cde-z"));

    assert!(fx.calls().is_empty());
}
