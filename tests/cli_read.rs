use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::Event;
use flow::state::task::{Harness, Hold, HoldOn, Origin, State, TaskRecord};
use jiff::Timestamp;
use serde_json::json;

fn flow(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
    cmd.env("FLOW_STATE", state);
    cmd
}

/// A temp repo root with an empty `state/`, and a `Store` over it. The CLI gets the state dir
/// through `FLOW_STATE` on each command, never through this process's environment.
fn setup() -> (tempfile::TempDir, PathBuf, Store) {
    let repo = tempfile::tempdir().unwrap();
    let state = repo.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let store = Store::open(&StateDir::resolve(repo.path()).unwrap());
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

#[test]
fn status_lists_open_tasks_compactly() {
    let (_repo, state, store) = setup();
    store
        .create_task(
            record("aaaaaaab", "Add retry to the example client"),
            "brief",
        )
        .unwrap();
    store
        .create_task(record("aaaaaaac", "Finished cleanup"), "brief")
        .unwrap();
    store
        .set_status("aaaaaaac", |s| s.state = State::Done)
        .unwrap();

    let out = stdout(flow(&state).arg("status"));

    let lines: Vec<_> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    let task_line = lines[0];
    assert!(
        task_line.starts_with("aaaaaaab  pending  host-a  "),
        "{out}"
    );
    assert!(
        task_line.ends_with("  Add retry to the example client"),
        "{out}"
    );
    assert!(!out.contains("aaaaaaac"), "{out}");
    assert_eq!(lines[1], "observed: 0 sessions");
}

#[test]
fn status_lists_holds() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Add retry"), "brief")
        .unwrap();
    store
        .set_status("aaaaaaab", |s| {
            s.state = State::NeedsDecision;
            s.hold = Some(Hold {
                on: HoldOn::Supervisor,
                what: "Which backoff schedule?".to_owned(),
                since: Timestamp::now(),
            });
        })
        .unwrap();

    let out = stdout(flow(&state).arg("status"));

    let lines: Vec<_> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert!(
        lines[0].starts_with("aaaaaaab  needs-decision  host-a  "),
        "{out}"
    );
    assert_eq!(lines[1], "holds:");
    assert!(
        lines[2].starts_with("aaaaaaab  supervisor  Which backoff schedule?  "),
        "{out}"
    );
    assert_eq!(lines[3], "observed: 0 sessions");
}

#[test]
fn task_show_elides_brief() {
    let (_repo, state, store) = setup();
    let brief: String = (1..=40)
        .map(|i| format!("{i:02} {}\n", "x".repeat(46)))
        .collect();
    assert_eq!(brief.len(), 2000);
    store
        .create_task(record("aaaaaaab", "Add retry"), &brief)
        .unwrap();

    let out = stdout(flow(&state).args(["task", "show", "aaaaaaab"]));

    assert!(out.contains("title       Add retry\n"), "{out}");
    assert!(out.contains("state       pending\n"), "{out}");
    let expected_head: String = brief.lines().take(10).map(|l| format!("{l}\n")).collect();
    let brief_path = std::fs::canonicalize(&state)
        .unwrap()
        .join("tasks/aaaaaaab/brief.md");
    let expected_tail = format!("… (30 more lines in {})\n", brief_path.display());
    assert!(
        out.ends_with(&format!("\n{expected_head}{expected_tail}")),
        "{out}"
    );
    assert!(!out.contains("11 xxx"), "{out}");
}

#[test]
fn task_brief_is_not_a_command() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Add retry"), "brief")
        .unwrap();

    flow(&state)
        .args(["task", "brief", "aaaaaaab"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unrecognized subcommand"));
}

#[test]
fn status_hides_done_and_orphaned_unless_all() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Open work"), "brief")
        .unwrap();
    store
        .create_task(record("aaaaaaac", "Finished work"), "brief")
        .unwrap();
    store
        .set_status("aaaaaaac", |s| s.state = State::Done)
        .unwrap();
    store
        .create_task(record("aaaaaaad", "Abandoned work"), "brief")
        .unwrap();
    store
        .set_status("aaaaaaad", |s| s.state = State::Orphaned)
        .unwrap();

    let open = stdout(flow(&state).arg("status"));
    let all = stdout(flow(&state).args(["status", "--all"]));

    let open_lines: Vec<_> = open.lines().collect();
    assert_eq!(open_lines.len(), 2, "{open}");
    assert!(open_lines[0].starts_with("aaaaaaab  pending  "), "{open}");
    let all_lines: Vec<_> = all.lines().collect();
    assert_eq!(all_lines.len(), 4, "{all}");
    assert!(all_lines[0].starts_with("aaaaaaab  pending   "), "{all}");
    assert!(all_lines[1].starts_with("aaaaaaac  done      "), "{all}");
    assert!(all_lines[2].starts_with("aaaaaaad  orphaned  "), "{all}");
    assert_eq!(all_lines[3], "observed: 0 sessions");
}

#[test]
fn status_json_tasks_match_the_text_view() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Open work"), "brief")
        .unwrap();
    store
        .create_task(record("aaaaaaac", "Finished work"), "brief")
        .unwrap();
    store
        .set_status("aaaaaaac", |s| s.state = State::Done)
        .unwrap();

    let ids = |args: &[&str]| -> Vec<String> {
        let value: serde_json::Value =
            serde_json::from_str(&stdout(flow(&state).args(args))).unwrap();
        value["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|view| view["task"]["id"].as_str().unwrap().to_owned())
            .collect()
    };

    assert_eq!(ids(&["status", "--json"]), ["aaaaaaab"]);
    assert_eq!(
        ids(&["status", "--all", "--json"]),
        ["aaaaaaab", "aaaaaaac"]
    );
}

#[test]
fn status_json_lists_no_requests_as_empty() {
    let (_repo, state, _store) = setup();

    let out = stdout(flow(&state).args(["status", "--json"]));

    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["requests"], json!([]));
}

#[test]
fn status_without_tasks_says_so() {
    let (_repo, state, _store) = setup();

    let open = stdout(flow(&state).arg("status"));
    let all = stdout(flow(&state).args(["status", "--all"]));

    assert_eq!(open, "no open tasks\nobserved: 0 sessions\n");
    assert_eq!(all, "no tasks\nobserved: 0 sessions\n");
}

#[test]
fn task_list_is_not_a_command() {
    let (_repo, state, _store) = setup();

    flow(&state)
        .args(["task", "list"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unrecognized subcommand"));
}

#[test]
fn task_events_last_limits() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Add retry"), "brief")
        .unwrap();
    for i in 0..30 {
        let ev = Event {
            ts: Timestamp::now(),
            source: "worker".to_owned(),
            kind: format!("step.{i}"),
            data: json!({"n": i, "note": "y".repeat(100)}),
        };
        store.append_event("aaaaaaab", &ev).unwrap();
    }

    let out = stdout(flow(&state).args(["task", "events", "aaaaaaab", "--last", "5"]));

    let lines: Vec<_> = out.lines().collect();
    assert_eq!(lines.len(), 5, "{out}");
    // 79 characters of the event's JSON, then the ellipsis.
    let first_data = format!(r#"{{"n":25,"note":"{}"#, "y".repeat(63));
    assert!(
        lines[0].contains(&format!("  worker  step.25  {first_data}…")),
        "{out}"
    );
    assert!(lines[4].contains("  worker  step.29  "), "{out}");
}

#[test]
fn json_flag_prints_raw_json() {
    let (_repo, state, store) = setup();
    store
        .create_task(record("aaaaaaab", "Add retry"), "brief text\n")
        .unwrap();

    let out = stdout(flow(&state).args(["task", "show", "aaaaaaab", "--json"]));

    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["task"]["id"], "aaaaaaab");
    assert_eq!(value["status"]["state"], "pending");
    assert_eq!(value["brief"], "brief text\n");
}
