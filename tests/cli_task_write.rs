use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::herdr::SESSION;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::task::{Harness, Hold, HoldOn, Origin, State, TaskRecord};
use jiff::Timestamp;
use serde_json::{Value, json};

const TASK: &str = "aaaaaaab";

fn flow(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
    cmd.env("FLOW_STATE", state);
    cmd
}

/// A temp repo root with an empty `state/` holding one pending task, and a `Store` over it. The
/// CLI gets the state dir through `FLOW_STATE` on each command.
fn setup() -> (tempfile::TempDir, PathBuf, Store) {
    let repo = tempfile::tempdir().unwrap();
    let state = repo.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let store = Store::open(&StateDir::resolve(repo.path()).unwrap());
    store
        .create_task(record(TASK, "Add retry to the example client"), "brief")
        .unwrap();
    (repo, state, store)
}

fn record(id: &str, title: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_owned(),
        parent: None,
        title: title.to_owned(),
        host: "host-a".to_owned(),
        cwd: PathBuf::from("/workspaces/example-repo.wt/retry"),
        repo: Some("example-repo".to_owned()),
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: "supervisor".to_owned(),
        origin: Origin::Dispatched,
    }
}

fn stdout(cmd: &mut Command) -> String {
    let out = cmd.assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

fn last_event(store: &Store, id: &str) -> Event {
    events::read_all(&store.task_dir(id).unwrap())
        .unwrap()
        .pop()
        .expect("task has events")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn needs_decision_sets_supervisor_hold() {
    let (_repo, state, store) = setup();

    flow(&state)
        .args(["task", "status", TASK, "needs-decision"])
        .args(["--message", "Retry forever or give up after five tries?"])
        .assert()
        .success();

    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.state, State::NeedsDecision);
    let hold = status.hold.expect("hold is set");
    assert_eq!(hold.on, HoldOn::Supervisor);
    assert_eq!(hold.what, "Retry forever or give up after five tries?");
    let ev = last_event(&store, TASK);
    assert_eq!(ev.source, "cli");
    assert_eq!(ev.kind, "status.report");
    assert_eq!(
        ev.data,
        json!({"message": "Retry forever or give up after five tries?"})
    );
}

#[test]
fn working_report_clears_supervisor_hold() {
    let (_repo, state, store) = setup();
    flow(&state)
        .args(["task", "status", TASK, "blocked", "--message", "CI is down"])
        .assert()
        .success();

    flow(&state)
        .args(["task", "status", TASK, "working"])
        .assert()
        .success();

    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert_eq!(last_event(&store, TASK).data, json!({"message": null}));
}

#[test]
fn working_report_keeps_user_hold() {
    let (_repo, state, store) = setup();
    flow(&state)
        .args([
            "task",
            "hold",
            TASK,
            "--on",
            "user",
            "--what",
            "Approve the schema change",
        ])
        .assert()
        .success();

    flow(&state)
        .args(["task", "status", TASK, "working"])
        .assert()
        .success();

    let hold = store.task(TASK).unwrap().1.hold.expect("user hold stays");
    assert_eq!(hold.on, HoldOn::User);
}

#[test]
fn blocked_without_message_is_rejected() {
    let (_repo, state, store) = setup();

    let out = flow(&state)
        .args(["task", "status", TASK, "blocked"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();

    assert!(String::from_utf8(out).unwrap().contains("--message"));

    assert_eq!(store.task(TASK).unwrap().1.state, State::Pending);
}

#[test]
fn invalid_state_is_rejected() {
    let (_repo, state, store) = setup();

    let out = flow(&state)
        .args(["task", "status", TASK, "finished"])
        .assert()
        .code(2)
        .get_output()
        .stderr
        .clone();

    let err = String::from_utf8(out).unwrap();
    for accepted in ["working", "blocked", "needs-decision"] {
        assert!(err.contains(accepted), "{err}");
    }
    assert!(err.contains("flow task done"), "{err}");
    assert!(err.contains("flow task orphan"), "{err}");
    assert_eq!(store.task(TASK).unwrap().1.state, State::Pending);
}

#[test]
fn done_copies_report_and_transitions() {
    let (repo, state, store) = setup();
    store
        .set_status(TASK, |s| {
            s.hold = Some(flow::state::task::Hold {
                on: HoldOn::Supervisor,
                what: "Which endpoint?".to_owned(),
                since: Timestamp::now(),
            })
        })
        .unwrap();
    let report = repo.path().join("report-draft.md");
    std::fs::write(&report, "# Retry added\n\nAll tests pass.\n").unwrap();

    flow(&state)
        .args(["task", "done", TASK, "--report"])
        .arg(&report)
        .assert()
        .success();

    let dir = store.task_dir(TASK).unwrap();
    let copied = dir.join("report.md");
    assert_eq!(
        std::fs::read_to_string(&copied).unwrap(),
        "# Retry added\n\nAll tests pass.\n"
    );
    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.state, State::Done);
    assert_eq!(status.hold, None);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "report");
    assert_eq!(ev.data, json!({"path": copied}));
}

#[test]
fn request_dispatch_writes_request_file() {
    let (repo, state, store) = setup();
    let brief = repo.path().join("sibling-brief.md");
    std::fs::write(&brief, "Update the example docs.\n").unwrap();

    let out = stdout(
        flow(&state)
            .args(["task", "request-dispatch", TASK])
            .args(["--title", "Update docs", "--host", "host-b", "--brief"])
            .arg(&brief),
    );

    let req_id = out.trim();
    let requests = store.task_dir(TASK).unwrap().join("requests");
    let path = requests.join(format!("{req_id}.json"));
    assert!(path.is_file(), "{} missing", path.display());
    let copied = requests.join(format!("{req_id}.brief.md"));
    assert_eq!(
        std::fs::read_to_string(&copied).unwrap(),
        "Update the example docs.\n"
    );
    let expected = json!({
        "title": "Update docs",
        "brief_path": copied,
        "host": "host-b",
        "requested_by": TASK,
    });
    assert_eq!(read_json(&path), expected);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "request.dispatch");
    assert_eq!(ev.data, expected);
}

#[test]
fn decide_clears_hold_and_logs() {
    let (_repo, state, store) = setup();
    flow(&state)
        .args(["task", "status", TASK, "needs-decision"])
        .args(["--message", "Which endpoint?"])
        .assert()
        .success();

    flow(&state)
        .args(["task", "decide", TASK, "--by", "supervisor"])
        .args(["--answer", "Use the v2 endpoint"])
        .assert()
        .success();

    let dir = store.task_dir(TASK).unwrap();
    let log = std::fs::read_to_string(dir.join("decisions.jsonl")).unwrap();
    let lines: Vec<_> = log.lines().collect();
    assert_eq!(lines.len(), 1, "{log}");
    let decision: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(decision["by"], "supervisor");
    assert_eq!(decision["answer"], "Use the v2 endpoint");
    assert_eq!(decision["question"], "Which endpoint?");
    let status = read_json(&dir.join("status.json"));
    assert_eq!(status["hold"], Value::Null);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "decision");
    assert_eq!(
        ev.data,
        json!({"by": "supervisor", "answer": "Use the v2 endpoint", "question": "Which endpoint?"})
    );
}

#[test]
fn hold_sets_hold_and_logs() {
    let (_repo, state, store) = setup();

    flow(&state)
        .args(["task", "hold", TASK, "--on", "user"])
        .args(["--what", "Approve the schema change"])
        .assert()
        .success();

    let (_, status) = store.task(TASK).unwrap();
    let hold = status.hold.expect("hold is set");
    assert_eq!(hold.on, HoldOn::User);
    assert_eq!(hold.what, "Approve the schema change");
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "hold.set");
    assert_eq!(
        ev.data,
        json!({"on": "user", "what": "Approve the schema change"})
    );
}

#[test]
fn status_lists_pending_requests() {
    let (repo, state, store) = setup();
    store
        .create_task(record("aaaaaaac", "Second task"), "brief")
        .unwrap();
    let brief = repo.path().join("sibling-brief.md");
    std::fs::write(&brief, "Sibling work.\n").unwrap();
    let with_host = stdout(
        flow(&state)
            .args(["task", "request-dispatch", TASK])
            .args(["--title", "Update docs", "--host", "host-b", "--brief"])
            .arg(&brief),
    );
    let without_host = stdout(
        flow(&state)
            .args(["task", "request-dispatch", "aaaaaaac"])
            .args(["--title", "Bump the client", "--brief"])
            .arg(&brief),
    );
    let (with_host, without_host) = (with_host.trim(), without_host.trim());

    let listed = stdout(flow(&state).arg("status"));

    let lines: Vec<_> = listed.lines().collect();
    assert_eq!(
        lines[2..],
        [
            "requests:".to_owned(),
            format!("{with_host}  {TASK}  host-b  Update docs"),
            format!("{without_host}  aaaaaaac  -       Bump the client"),
            "observed: 0 sessions".to_owned(),
        ],
        "{listed}"
    );

    store.consume_request(with_host).unwrap();
    let json = stdout(flow(&state).args(["status", "--json"]));
    let json: Value = serde_json::from_str(&json).unwrap();
    let copied = store
        .task_dir("aaaaaaac")
        .unwrap()
        .join("requests")
        .join(format!("{without_host}.brief.md"));
    assert_eq!(
        json["requests"],
        json!([{
            "id": without_host,
            "title": "Bump the client",
            "brief_path": copied,
            "host": null,
            "requested_by": "aaaaaaac",
        }])
    );
}

#[test]
fn status_lists_holds_then_requests_then_observed() {
    let (repo, state, store) = setup();
    store
        .set_status(TASK, |s| {
            s.state = State::NeedsDecision;
            s.hold = Some(Hold {
                on: HoldOn::Supervisor,
                what: "Which backoff schedule?".to_owned(),
                since: Timestamp::now(),
            });
        })
        .unwrap();
    let brief = repo.path().join("sibling-brief.md");
    std::fs::write(&brief, "Sibling work.\n").unwrap();
    let req_id = stdout(
        flow(&state)
            .args(["task", "request-dispatch", TASK])
            .args(["--title", "Update docs", "--brief"])
            .arg(&brief),
    );

    let listed = stdout(flow(&state).arg("status"));

    let lines: Vec<_> = listed.lines().collect();
    assert_eq!(lines.len(), 6, "{listed}");
    assert!(
        lines[0].starts_with(&format!("{TASK}  needs-decision  host-a  ")),
        "{listed}"
    );
    assert_eq!(lines[1], "holds:", "{listed}");
    assert!(
        lines[2].starts_with(&format!("{TASK}  supervisor  Which backoff schedule?  ")),
        "{listed}"
    );
    assert_eq!(lines[3], "requests:", "{listed}");
    assert_eq!(
        lines[4],
        format!("{}  {TASK}  -  Update docs", req_id.trim())
    );
    assert_eq!(lines[5], "observed: 0 sessions", "{listed}");
}

#[test]
fn status_skips_an_unreadable_request_with_a_warning() {
    let (repo, state, store) = setup();
    let brief = repo.path().join("sibling-brief.md");
    std::fs::write(&brief, "Sibling work.\n").unwrap();
    let req_id = stdout(
        flow(&state)
            .args(["task", "request-dispatch", TASK])
            .args(["--title", "Update docs", "--brief"])
            .arg(&brief),
    );
    let req_id = req_id.trim();
    let broken = store
        .task_dir(TASK)
        .unwrap()
        .join("requests")
        .join("aaaaaaaz.json");
    std::fs::write(&broken, "{not json").unwrap();

    for args in [&["status"][..], &["status", "--json"][..]] {
        let out = flow(&state)
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(out.stdout).unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stdout.contains(req_id), "{stdout}");
        assert!(!stdout.contains("aaaaaaaz"), "{stdout}");
        assert!(
            stderr.contains(&broken.display().to_string()),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn requests_is_not_a_command() {
    let (_repo, state, _store) = setup();

    flow(&state)
        .arg("requests")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unrecognized subcommand"));
}

#[test]
fn orphan_transitions_with_reason_event() {
    let (_repo, state, store) = setup();

    flow(&state)
        .args(["task", "orphan", TASK, "--reason", "host was deleted"])
        .assert()
        .success()
        .stdout(format!("orphaned {TASK}\n"));

    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.state, State::Orphaned);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.source, "cli");
    assert_eq!(ev.kind, "orphaned");
    assert_eq!(ev.data, json!({"reason": "host was deleted"}));
}

#[test]
fn write_commands_reject_unknown_task() {
    let (_repo, state, _store) = setup();

    let out = flow(&state)
        .args(["task", "orphan", "aaaaaaaz", "--reason", "gone"])
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();

    assert!(String::from_utf8(out).unwrap().contains("no such task"));
}

const WORKSPACE_CLOSED: &str = r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#;

/// `setup()` with the task `state`, its worker in workspace `w2` on `host-a`, the host's cached
/// record, and a fake runner answering `script`; returns the runner's script path.
fn setup_close(
    state_name: State,
    script: &[Value],
) -> (tempfile::TempDir, PathBuf, Store, PathBuf) {
    let (repo, state, store) = setup();
    store
        .set_status(TASK, |status| {
            status.state = state_name;
            status.herdr = Some(flow::state::task::HerdrTarget {
                host: "host-a".to_owned(),
                workspace_id: "w2".to_owned(),
                pane_id: "w2:p1".to_owned(),
                agent: "claude".to_owned(),
                session: SESSION.to_owned(),
            });
        })
        .unwrap();
    let hosts = state.join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    let record = json!({
        "name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example",
        "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
        "worktrees": [], "agents": [], "prs": [],
    });
    std::fs::write(hosts.join("env-0001.json"), record.to_string()).unwrap();
    let script_path = repo.path().join("runner.json");
    std::fs::write(&script_path, Value::from(script.to_vec()).to_string()).unwrap();
    (repo, state, store, script_path)
}

fn runner_calls(script_path: &Path) -> Vec<Value> {
    let mut log = script_path.as_os_str().to_owned();
    log.push(".calls.jsonl");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn close_closes_a_done_task_workspace_and_keeps_the_record() {
    let ok = json!({"status": 0, "stdout": WORKSPACE_CLOSED, "stderr": ""});
    let (_repo, state, store, script) = setup_close(State::Done, &[ok]);
    let (rec_before, _) = store.task(TASK).unwrap();

    let out = stdout(
        flow(&state)
            .env("FLOW_FAKE_RUNNER", &script)
            .args(["task", "close", TASK]),
    );

    assert_eq!(out, format!("closed {TASK} (workspace w2 on host-a)\n"));
    let calls = runner_calls(&script);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0]["host"], "host-a");
    assert_eq!(
        calls[0]["argv"],
        json!(["herdr", "--session", "flow", "workspace", "close", "w2"])
    );
    let (rec, status) = store.task(TASK).unwrap();
    assert_eq!(rec, rec_before);
    assert_eq!(status.state, State::Done);
    assert_eq!(status.herdr, None);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "close");
    assert_eq!(ev.source, "supervisor");
    assert_eq!(
        ev.data,
        json!({
            "host": "host-a", "workspace_id": "w2", "pane_id": "w2:p1",
            "forced": false, "already_gone": false,
        })
    );
}

#[test]
fn close_of_an_adopted_task_leaves_its_workspace_to_its_owner() {
    let (_repo, state, store, script) = setup_close(State::Done, &[]);
    let dir = store.task_dir(TASK).unwrap();
    let mut rec: Value = read_json(&dir.join("task.json"));
    rec["origin"] = json!("adopted");
    std::fs::write(dir.join("task.json"), rec.to_string()).unwrap();
    store
        .set_status(TASK, |status| {
            if let Some(target) = &mut status.herdr {
                target.session = "default".to_owned();
            }
        })
        .unwrap();

    let out = stdout(
        flow(&state)
            .env("FLOW_FAKE_RUNNER", &script)
            .args(["task", "close", TASK]),
    );

    assert_eq!(out, format!("closed {TASK} (pane left to its owner)\n"));
    assert!(runner_calls(&script).is_empty());
    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.herdr, None);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "close");
    assert_eq!(
        ev.data,
        json!({
            "host": "host-a", "workspace_id": "w2", "pane_id": "w2:p1",
            "forced": false, "workspace_left": true,
        })
    );
}

#[test]
fn close_of_a_workspace_already_gone_still_retires_the_pane() {
    let missing = json!({
        "status": 1,
        "stdout": "",
        "stderr": r#"{"error":{"code":"workspace_not_found","message":"workspace w2 not found"},"id":"cli:workspace:close"}"#,
    });
    let (_repo, state, store, script) = setup_close(State::Done, &[missing]);

    let out = stdout(
        flow(&state)
            .env("FLOW_FAKE_RUNNER", &script)
            .args(["task", "close", TASK]),
    );

    assert_eq!(out, format!("closed {TASK} (workspace already gone)\n"));
    assert_eq!(runner_calls(&script).len(), 1);
    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.herdr, None);
    let ev = last_event(&store, TASK);
    assert_eq!(ev.kind, "close");
    assert_eq!(ev.data["already_gone"], true);
}

#[test]
fn close_refuses_an_open_task_unless_forced() {
    for open in [
        State::Working,
        State::Pending,
        State::Blocked,
        State::NeedsDecision,
    ] {
        let ok = json!({"status": 0, "stdout": WORKSPACE_CLOSED, "stderr": ""});
        let (_repo, state, store, script) = setup_close(open, &[ok]);

        flow(&state)
            .env("FLOW_FAKE_RUNNER", &script)
            .args(["task", "close", TASK])
            .assert()
            .code(2)
            .stderr(predicates::str::contains(format!(
                "task {TASK} is {open}, still open"
            )))
            .stderr(predicates::str::contains("--force"));
        assert!(runner_calls(&script).is_empty(), "{open}");
        assert!(store.task(TASK).unwrap().1.herdr.is_some(), "{open}");

        flow(&state)
            .env("FLOW_FAKE_RUNNER", &script)
            .args(["task", "close", TASK, "--force"])
            .assert()
            .success();
        assert_eq!(runner_calls(&script).len(), 1, "{open}");
        let (_, status) = store.task(TASK).unwrap();
        assert_eq!(status.state, open);
        assert_eq!(status.herdr, None, "{open}");
        assert_eq!(last_event(&store, TASK).data["forced"], true, "{open}");
    }
}

#[test]
fn close_of_an_orphaned_task_needs_no_force() {
    let ok = json!({"status": 0, "stdout": WORKSPACE_CLOSED, "stderr": ""});
    let (_repo, state, store, script) = setup_close(State::Orphaned, &[ok]);

    flow(&state)
        .env("FLOW_FAKE_RUNNER", &script)
        .args(["task", "close", TASK])
        .assert()
        .success();
    assert_eq!(runner_calls(&script).len(), 1);
    assert_eq!(store.task(TASK).unwrap().1.herdr, None);
}

#[test]
fn close_refuses_a_task_without_a_workspace() {
    let (_repo, state, store) = setup();
    store
        .set_status(TASK, |status| status.state = State::Done)
        .unwrap();

    flow(&state)
        .args(["task", "close", TASK])
        .assert()
        .code(2)
        .stderr(format!("task {TASK} has no herdr workspace to close\n"));
}
