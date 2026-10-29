use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::herdr::SESSION;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::requests::NewRequest;
use flow::state::task::{Harness, HerdrTarget, HoldOn, Origin, State, TaskRecord};
use jiff::Timestamp;
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }
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

/// The directory a scripted `workspace create` reports, and the one most tests dispatch into.
const WORKTREE: &str = "/workspaces/acme-worktrees/search-index";
/// The pane and workspace a scripted `workspace create` returns.
const PANE: &str = "w3:p1";
const WORKSPACE: &str = "w3";

/// What `flow dispatch` adds when run outside the supervisor's session.
const HAND_DISPATCH_NOTE: &str = "note: no supervisor turn is watching; the supervisor hears of \
     this task when it next ends a turn\n";

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
        let fx = Fixture { dir, state };
        fx.write_host("host-a", "env-0001", false);
        fx
    }

    /// A cached record of `name` (env `env_id`) as discovery leaves it, marked gone when `gone`.
    fn write_host(&self, name: &str, env_id: &str, gone: bool) {
        let hosts = self.state.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        let record = json!({
            "name": name, "env_id": env_id, "ssh_alias": format!("{name}.example"),
            "gone": gone, "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
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
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env_remove("FLOW_SUPERVISOR");
        cmd
    }

    fn with_config(config: &str) -> Fixture {
        let fx = Fixture::new();
        fs::write(fx.state.join("config.toml"), config).unwrap();
        fx
    }

    /// The argv of every runner call, in order, each of them made on `host-a`.
    fn calls(&self) -> Vec<Vec<String>> {
        self.calls_on("host-a")
    }

    /// The argv of every runner call, in order, each of them made on `host`.
    fn calls_on(&self, host: &str) -> Vec<Vec<String>> {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                assert_eq!(call["host"], host, "{call}");
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

    /// Creates task `REQUESTER` and files a dispatch request from it for `host`; returns the
    /// request id.
    fn file_request(&self, host: Option<&str>) -> String {
        let store = self.store();
        store
            .create_task(task_record(REQUESTER, "Split the indexer"), "brief")
            .unwrap();
        let new = NewRequest {
            title: "Rebuild the search index",
            host,
            brief: REQUEST_BRIEF.as_bytes(),
        };
        store.add_request(REQUESTER, &new).unwrap().0
    }

    /// The id of the only task other than `REQUESTER` and `OTHER_PARENT`.
    fn dispatched_task(&self) -> String {
        let ids: Vec<String> = self
            .store()
            .list_tasks()
            .unwrap()
            .into_iter()
            .map(|(rec, _)| rec.id)
            .filter(|id| id != REQUESTER && id != OTHER_PARENT)
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        ids[0].clone()
    }

    fn pending_request_ids(&self) -> Vec<String> {
        self.store()
            .pending_requests()
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }
}

/// The task that files dispatch requests in the request tests.
const REQUESTER: &str = "aaaaaaab";
/// A task other than `REQUESTER` that a request's dispatch may name as its parent.
const OTHER_PARENT: &str = "aaaaaaac";

fn task_record(id: &str, title: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_owned(),
        parent: None,
        title: title.to_owned(),
        host: "host-a".to_owned(),
        cwd: PathBuf::from(WORKTREE),
        repo: None,
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: "supervisor".to_owned(),
        origin: Origin::Dispatched,
    }
}
const REQUEST_BRIEF: &str = "Rebuild the search index from the request.\n";

/// `flow dispatch --request <req_id>` with criteria and `--dir WORKTREE`, then `extra`.
fn request_dispatch_args(req_id: &str, extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "dispatch",
        "--request",
        req_id,
        "--criteria",
        "- A draft PR is open",
        "--dir",
        WORKTREE,
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    args
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

fn dispatch_args(dir: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "dispatch",
        "--host",
        "host-a",
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
    args.extend(dir.iter().map(|s| (*s).to_owned()));
    args
}

/// `dispatch_args` without `--host`.
fn local_dispatch_args(dir: &[&str]) -> Vec<String> {
    let mut args = dispatch_args(dir);
    assert_eq!(args[1..3], ["--host", "host-a"]);
    args.drain(1..3);
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
fn dispatch_refuses_shared_dir() {
    let fx = Fixture::new();
    let shared = "/workspaces/acme-worktrees/login-form";

    let out = fx
        .flow(&[ok(SERVER_RUNNING), ok(AGENT_LIST)])
        .args(dispatch_args(&["--dir", shared]))
        .assert()
        .code(2)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("Refactor login form"), "{stderr}");
    assert!(stderr.contains("w1:p1"), "{stderr}");
    assert!(stderr.contains("--allow-shared-dir"), "{stderr}");
    assert!(
        stderr.contains("to dispatch into that directory anyway"),
        "{stderr}"
    );
    let tasks = fx.state.join("tasks");
    assert!(
        !tasks.exists() || fs::read_dir(&tasks).unwrap().next().is_none(),
        "a task was created"
    );
    assert!(herdr_calls(&fx.calls(), &["workspace", "create"]).is_empty());
}

#[test]
fn dispatch_allows_shared_dir_when_asked() {
    let fx = Fixture::new();
    let shared = "/workspaces/acme-worktrees/login-form";
    let script = [
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("idle"),
    ];

    fx.flow(&script)
        .args(dispatch_args(&["--dir", shared, "--allow-shared-dir"]))
        .assert()
        .success();

    assert!(herdr_calls(&fx.calls(), &["agent", "list"]).is_empty());
    let (rec, status) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(rec.cwd, Path::new(shared));
    assert_eq!(status.state, State::Working);
}

#[test]
fn dispatch_creates_task_and_starts_agent() {
    let fx = Fixture::new();

    let out = fx
        .flow(&starts_working(Vec::new()))
        .args(dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success()
        .get_output()
        .clone();

    let id = fx.only_task();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("dispatched {id} on host-a ({PANE})\n{HAND_DISPATCH_NOTE}")
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
    let state = fs::canonicalize(&fx.state).unwrap();
    assert!(
        runs[0][6].starts_with(&format!(
            "env 'FLOW_STATE={}' claude --settings ",
            state.display()
        )),
        "the worker's hooks get the state path whatever herdr's environment holds: {:?}",
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
    assert_eq!(rec.host, "host-a");
    assert_eq!(rec.cwd, Path::new(WORKTREE));
    assert_eq!(rec.owner, "supervisor");
    assert_eq!(rec.parent, None);
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert_eq!(
        status.herdr,
        Some(HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: WORKSPACE.to_owned(),
            pane_id: PANE.to_owned(),
            agent: "claude".to_owned(),
            session: SESSION.to_owned(),
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
    assert_eq!(started.data["argv"][0], "env");
    assert_eq!(
        started.data["argv"][1],
        format!("FLOW_STATE={}", state.display())
    );
    assert_eq!(started.data["argv"][2], "claude");
    assert_eq!(started.data["argv"][3], "--settings");
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
        status_out.starts_with(&format!("{id}  working  host-a  {PANE}  ")),
        "{status_out}"
    );
}

#[test]
fn dispatch_launch_line_names_the_state_dir_behind_a_symlink() {
    let fx = Fixture::new();
    let link = fx.dir.path().join("state-link");
    std::os::unix::fs::symlink(&fx.state, &link).unwrap();

    fx.flow(&starts_working(Vec::new()))
        .env("FLOW_STATE", &link)
        .args(dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success();

    let calls = fx.calls();
    let runs = herdr_calls(&calls, &["pane", "run"]);
    let target = fs::canonicalize(&fx.state).unwrap();
    assert!(
        runs[0][6].starts_with(&format!("env 'FLOW_STATE={}' claude ", target.display())),
        "{:?}",
        runs[0][6]
    );
    assert!(!runs[0][6].contains("state-link"), "{:?}", runs[0][6]);
}

#[test]
fn dispatch_with_branch_runs_worktree_tool() {
    let fx = Fixture::new();
    let created = "/workspaces/acme-worktrees/feature-x";
    let tool_output = format!("{created}  (new branch feature-x, 1.5s)\n");

    fx.flow(&starts_working(vec![ok(&tool_output), ok("")]))
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
    assert_eq!(calls[1], ["test", "-d", created]);
    let (rec, _) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(rec.cwd, Path::new(created));
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
        .args(dispatch_args(&["--dir", WORKTREE]))
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
        waits[1][5..10],
        [PANE, "--until", "idle", "--until", "working"]
    );
    assert!(
        wait_timeout(waits[1]) <= 60_000,
        "the whole start shares one timeout: {:?}",
        waits[1]
    );
    let (_, status) = fx.store().task(&fx.only_task()).unwrap();
    assert_eq!(status.state, State::Working);
}

#[test]
fn dispatch_says_once_that_it_is_waiting_for_a_slow_worker() {
    let fx = Fixture::new();
    let not_found = r#"{"error":{"code":"agent_not_found","message":"agent target w3:p1 not found"},"id":"cli:agent:wait"}"#;
    let mut script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
    ];
    // About 4 s of polling at 400 ms, past the 3 s after which the wait is announced.
    script.extend((0..10).map(|_| failed(not_found)));
    script.push(wait_reports("working"));

    let out = fx
        .flow(&script)
        .env("FLOW_HANDSHAKE_POLL_MS", "400")
        .args(dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    let notice = format!("waiting for Claude Code in pane {PANE} on host-a…\n");
    assert_eq!(stderr.matches(&notice).count(), 1, "{stderr}");
}

/// The `--timeout` an `agent wait` call passes, in milliseconds.
fn wait_timeout(argv: &[String]) -> u64 {
    let at = argv.iter().position(|word| word == "--timeout").unwrap();
    argv[at + 1].parse().unwrap()
}

#[test]
fn dispatch_waits_until_herdr_sees_the_worker() {
    let fx = Fixture::new();
    let not_found = r#"{"error":{"code":"agent_not_found","message":"agent target w3:p1 not found"},"id":"cli:agent:wait"}"#;
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(not_found),
        failed(not_found),
        wait_reports("blocked"),
        ok(TRUST_DIALOG),
        ok(""),
        wait_reports("idle"),
    ];

    fx.flow(&script)
        .env("FLOW_HANDSHAKE_POLL_MS", "10")
        .args(dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success()
        .stderr("");

    let calls = fx.calls();
    let waits = herdr_calls(&calls, &["agent", "wait"]);
    assert_eq!(waits.len(), 4, "{calls:?}");
    assert_eq!(wait_timeout(waits[0]), 60_000);
    assert!(wait_timeout(waits[2]) < 60_000, "{:?}", waits[2]);
    let keys = herdr_calls(&calls, &["pane", "send-keys"]);
    assert_eq!(keys.len(), 1, "{calls:?}");
    assert_eq!(keys[0][5..], [PANE, "down", "enter"]);
    let id = fx.only_task();
    let (_, status) = fx.store().task(&id).unwrap();
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert!(!fx.events(&id).iter().any(|ev| ev.kind == "dispatch.failed"));
}

#[test]
fn dispatch_retries_a_wait_that_gives_no_answer() {
    let fx = Fixture::new();
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(""),
        failed(""),
        wait_reports("idle"),
    ];

    let out = fx
        .flow(&script)
        .env("FLOW_HANDSHAKE_POLL_MS", "10")
        .args(dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr,
        format!("herdr gave no answer for pane {PANE} on host-a; retrying\n"),
    );
    let calls = fx.calls();
    assert_eq!(
        herdr_calls(&calls, &["agent", "wait"]).len(),
        3,
        "{calls:?}"
    );
    let id = fx.only_task();
    let (_, status) = fx.store().task(&id).unwrap();
    assert_eq!(status.state, State::Working);
    assert!(!fx.events(&id).iter().any(|ev| ev.kind == "dispatch.failed"));
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
        .args(dispatch_args(&["--dir", WORKTREE]))
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
        .args(dispatch_args(&["--dir", WORKTREE]))
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
        .args(dispatch_args(&["--dir", WORKTREE]))
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
fn dispatch_unknown_host_exits_2() {
    let fx = Fixture::new();
    let mut args = dispatch_args(&["--dir", WORKTREE]);
    args[2] = "host-z".to_owned();

    fx.flow(&[])
        .args(args)
        .assert()
        .code(2)
        .stderr(predicates::str::contains("host-z"));

    assert!(fx.calls().is_empty());
}

const TWO_REPOS: &str = r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[[repos]]
name = "other-repo"
path = "/work/other"
default_branch = "trunk"
"#;

const CHECKOUT_SENTENCE: &str =
    "Work only inside this directory; never touch the main checkout or other worktrees.";

#[test]
fn dispatch_defaults_to_the_local_host() {
    let fx = Fixture::new();
    fx.write_host("host-b", "env-0002", false);

    let out = fx
        .flow(&starts_working(Vec::new()))
        .env("ONA_ID", "env-0002")
        // From the supervisor's own session, which hears of the task, there is no note.
        .env("FLOW_SUPERVISOR", "1")
        .args(local_dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .success()
        .get_output()
        .clone();

    let id = fx.only_task();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("dispatched {id} on host-b ({PANE})\n")
    );
    assert!(!fx.calls_on("host-b").is_empty());
    let (rec, _) = fx.store().task(&id).unwrap();
    assert_eq!(rec.host, "host-b");
    assert_eq!(rec.cwd, Path::new(WORKTREE));
    assert_eq!(rec.repo, None);

    let unset = Fixture::new();
    unset.write_host("host-b", "env-0002", false);
    let out = unset
        .flow(&[])
        .env_remove("ONA_ID")
        .args(local_dispatch_args(&["--dir", WORKTREE]))
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--host"), "{stderr}");
    assert!(unset.calls_on("host-a").is_empty());
}

#[test]
fn dispatch_refuses_unknown_local_host() {
    let fx = Fixture::new();

    let out = fx
        .flow(&[])
        .env("ONA_ID", "env-9999")
        .args(local_dispatch_args(&[]))
        .assert()
        .code(2)
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "this machine (env-9999) is not a known host; run `flow inventory --refresh` to \
         discover hosts\n"
    );
    assert!(fx.calls().is_empty());
    assert!(!fx.state.join("tasks").exists());
}

#[test]
fn dispatch_refuses_a_gone_host() {
    let fx = Fixture::new();
    fx.write_host("host-b", "env-0002", true);

    let out = fx
        .flow(&[])
        .args(local_dispatch_args(&["--host", "host-b"]))
        .assert()
        .code(2)
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "host host-b is gone\n"
    );
    assert!(fx.calls_on("host-b").is_empty());
    assert!(!fx.state.join("tasks").exists());
}

#[test]
fn dispatch_without_dir_creates_the_task_work_dir() {
    let fx = Fixture::new();
    let linked = fx.dir.path().join("linked-state");
    std::os::unix::fs::symlink(&fx.state, &linked).unwrap();

    fx.flow(&starts_working(Vec::new()))
        .env("FLOW_STATE", &linked)
        .args(dispatch_args(&[]))
        .assert()
        .success();

    let id = fx.only_task();
    let store = fx.store();
    let work = fs::canonicalize(&fx.state)
        .unwrap()
        .join("tasks")
        .join(&id)
        .join("work");
    assert!(work.is_dir(), "{} was not created", work.display());
    let (rec, _) = store.task(&id).unwrap();
    assert_eq!(rec.cwd, work);
    assert_eq!(rec.repo, None);
    let calls = fx.calls();
    let create = herdr_calls(&calls, &["workspace", "create"]);
    assert_eq!(create[0][5..7], ["--cwd", work.to_str().unwrap()]);
    let task_dir = work.parent().unwrap().display().to_string();
    let run = &herdr_calls(&calls, &["pane", "run"])[0][6];
    assert!(
        run.contains(&format!("--settings {task_dir}/claude-settings.json ")),
        "{run}"
    );
    assert!(run.contains(&format!("Read {task_dir}/brief.md")), "{run}");
    assert!(!run.contains("linked-state"), "{run}");

    let brief = store.brief(&id).unwrap();
    assert!(
        brief.contains(&format!(
            "## Working directory\n\n`{}`\n\n## What is asked\n",
            work.display()
        )),
        "{brief}"
    );
    assert!(!brief.contains(CHECKOUT_SENTENCE), "{brief}");
    assert!(!brief.contains("\n\n\n"), "{brief}");
}

#[test]
fn dispatch_branch_uses_the_named_repo() {
    let fx = Fixture::with_config(TWO_REPOS);
    let created = "/work/other-worktrees/feature-x";

    fx.flow(&starts_working(vec![ok("")]))
        .args(dispatch_args(&[
            "--branch",
            "feature-x",
            "--repo",
            "other-repo",
        ]))
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(
        calls[0],
        [
            "git",
            "-C",
            "/work/other",
            "worktree",
            "add",
            created,
            "-b",
            "feature-x",
            "trunk"
        ]
    );
    let id = fx.only_task();
    let (rec, _) = fx.store().task(&id).unwrap();
    assert_eq!(rec.cwd, Path::new(created));
    assert_eq!(rec.repo.as_deref(), Some("other-repo"));
    assert_eq!(herdr_calls(&calls, &["workspace", "create"])[0][6], created);
    let brief = fx.store().brief(&id).unwrap();
    assert!(
        brief.contains(&format!(
            "`{created}`\n\n{CHECKOUT_SENTENCE}\n\n## What is asked\n"
        )),
        "{brief}"
    );

    for (args, error) in [
        (
            vec!["--branch", "feature-y"],
            "several repos configured (example-repo, other-repo); pass --repo",
        ),
        (
            vec!["--branch", "feature-y", "--repo", "nope"],
            "no repo named nope",
        ),
        (
            vec!["--repo", "other-repo"],
            "--repo only applies with --branch",
        ),
        (
            vec!["--dir", WORKTREE, "--repo", "other-repo"],
            "--repo only applies with --branch",
        ),
    ] {
        let refused = Fixture::with_config(TWO_REPOS);
        let out = refused
            .flow(&[])
            .args(dispatch_args(&args))
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            format!("{error}\n"),
            "{args:?}"
        );
        assert!(refused.calls().is_empty(), "{args:?}");
    }
}

#[test]
fn dispatch_from_a_request_fills_its_fields_and_consumes_it() {
    let fx = Fixture::new();
    let req_id = fx.file_request(Some("host-a"));

    let out = fx
        .flow(&starts_working(Vec::new()))
        .args(request_dispatch_args(&req_id, &[]))
        .assert()
        .success()
        .get_output()
        .clone();

    let id = fx.dispatched_task();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("dispatched {id} on host-a ({PANE})\n{HAND_DISPATCH_NOTE}")
    );
    let store = fx.store();
    let (rec, status) = store.task(&id).unwrap();
    assert_eq!(rec.title, "Rebuild the search index");
    assert_eq!(rec.host, "host-a");
    assert_eq!(rec.parent.as_deref(), Some(REQUESTER));
    assert_eq!(status.state, State::Working);
    let brief = store.brief(&id).unwrap();
    assert!(
        brief.contains("Rebuild the search index from the request."),
        "{brief}"
    );
    assert!(brief.contains("- A draft PR is open"), "{brief}");
    assert!(fx.pending_request_ids().is_empty());
    let consumed = fx
        .events(REQUESTER)
        .into_iter()
        .find(|ev| ev.kind == "request.consumed")
        .expect("request.consumed event");
    assert_eq!(consumed.data, json!({"req_id": req_id, "task": id}));
}

#[test]
fn dispatch_flags_override_the_request() {
    let fx = Fixture::new();
    let req_id = fx.file_request(Some("host-z"));

    fx.flow(&starts_working(Vec::new()))
        .args(request_dispatch_args(
            &req_id,
            &["--host", "host-a", "--title", "Rebuild it nightly"],
        ))
        .assert()
        .success();

    let (rec, _) = fx.store().task(&fx.dispatched_task()).unwrap();
    assert_eq!(rec.host, "host-a");
    assert_eq!(rec.title, "Rebuild it nightly");
    assert!(fx.pending_request_ids().is_empty());
}

#[test]
fn dispatch_explicit_parent_overrides_the_requester() {
    let fx = Fixture::new();
    let req_id = fx.file_request(None);
    fx.store()
        .create_task(task_record(OTHER_PARENT, "Plan the indexer"), "brief")
        .unwrap();

    fx.flow(&starts_working(Vec::new()))
        .args(request_dispatch_args(&req_id, &["--parent", OTHER_PARENT]))
        .assert()
        .success();

    let (rec, _) = fx.store().task(&fx.dispatched_task()).unwrap();
    assert_eq!(rec.parent.as_deref(), Some(OTHER_PARENT));
    assert!(fx.pending_request_ids().is_empty());
}

#[test]
fn dispatch_says_the_worker_started_when_its_request_cannot_be_consumed() {
    let fx = Fixture::new();
    let req_id = fx.file_request(None);
    // A directory where the consumed request would go makes the rename fail.
    let requests = fx.store().task_dir(REQUESTER).unwrap().join("requests");
    fs::create_dir(requests.join(format!("{req_id}.json.consumed"))).unwrap();

    let out = fx
        .flow(&starts_working(Vec::new()))
        .args(request_dispatch_args(&req_id, &[]))
        .assert()
        .code(1)
        .get_output()
        .clone();

    let id = fx.dispatched_task();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("dispatched {id} on host-a ({PANE})\n{HAND_DISPATCH_NOTE}")
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with(&format!(
            "task {id} is dispatched and its worker started, but request {req_id} was not \
             consumed: "
        )),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    let (_, status) = fx.store().task(&id).unwrap();
    assert_eq!(status.state, State::Working);
    assert_eq!(fx.pending_request_ids(), [req_id]);
}

#[test]
fn dispatch_from_a_request_leaves_it_pending_when_the_start_fails() {
    let fx = Fixture::new();
    let req_id = fx.file_request(None);
    let script = vec![
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(AGENT_WAIT_TIMEOUT),
    ];

    fx.flow(&script)
        .args(request_dispatch_args(&req_id, &[]))
        .assert()
        .code(1);

    let (_, status) = fx.store().task(&fx.dispatched_task()).unwrap();
    assert_eq!(status.state, State::Blocked);
    assert_eq!(fx.pending_request_ids(), [req_id]);
    assert!(
        !fx.events(REQUESTER)
            .iter()
            .any(|ev| ev.kind == "request.consumed")
    );
}

#[test]
fn dispatch_refuses_an_unknown_or_consumed_request() {
    let fx = Fixture::new();
    let req_id = fx.file_request(None);
    fx.store().consume_request(&req_id).unwrap();

    for id in ["aaaaaaaz", req_id.as_str()] {
        fx.flow(&[])
            .args(request_dispatch_args(id, &[]))
            .assert()
            .code(2)
            .stderr(predicates::str::contains(format!(
                "no pending request: {id}"
            )));
    }

    assert!(fx.calls().is_empty());
    assert_eq!(fx.store().list_tasks().unwrap().len(), 1);
}

#[test]
fn dispatch_refuses_a_brief_alongside_a_request() {
    let fx = Fixture::new();
    let req_id = fx.file_request(None);

    for brief in [
        ["--brief", "Other work."],
        ["--brief-file", "/tmp/brief.md"],
    ] {
        fx.flow(&[])
            .args(request_dispatch_args(&req_id, &brief))
            .assert()
            .code(2)
            .stderr(predicates::str::contains("the request supplies the brief"));
    }

    assert!(fx.calls().is_empty());
    assert_eq!(fx.pending_request_ids(), [req_id]);
}

#[test]
fn dispatch_needs_exactly_one_brief_source_and_a_title() {
    let fx = Fixture::new();
    let mut without_brief = dispatch_args(&["--dir", WORKTREE]);
    without_brief.drain(5..7);
    let mut without_title = dispatch_args(&["--dir", WORKTREE]);
    without_title.drain(3..5);
    let mut both_briefs = dispatch_args(&["--dir", WORKTREE]);
    both_briefs.extend(["--brief-file".to_owned(), "/tmp/brief.md".to_owned()]);

    for (args, message) in [
        (
            without_brief,
            "pass exactly one of --request, --brief-file, and --brief",
        ),
        (
            both_briefs,
            "pass exactly one of --request, --brief-file, and --brief",
        ),
        (
            without_title,
            "--title is required unless --request supplies it",
        ),
    ] {
        fx.flow(&[])
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicates::str::contains(message));
    }

    assert!(fx.calls().is_empty());
}
