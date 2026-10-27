use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::task::{Harness, TaskRecord};
use jiff::Timestamp;
use serde_json::json;

const TASK: &str = "aaaaaaab";
const SESSION_START: &str = r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t/s1.jsonl","cwd":"/w","source":"startup"}"#;

fn flow(state: &Path) -> Command {
    let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
    cmd.env("FLOW_STATE", state);
    cmd
}

fn setup() -> (tempfile::TempDir, PathBuf, Store) {
    let repo = tempfile::tempdir().unwrap();
    let state = repo.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let store = Store::open(&StateDir::resolve(repo.path()).unwrap());
    store
        .create_task(
            TaskRecord {
                id: TASK.to_owned(),
                parent: None,
                title: "Add retry to the example client".to_owned(),
                host: "host-a".to_owned(),
                worktree: PathBuf::from("/workspaces/example-repo.wt/retry"),
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
            },
            "brief",
        )
        .unwrap();
    (repo, state, store)
}

fn events(store: &Store) -> Vec<Event> {
    events::read_all(&store.task_dir(TASK).unwrap()).unwrap()
}

fn hook(state: &Path, stdin: &str) -> assert_cmd::assert::Assert {
    flow(state)
        .args(["hook", "claude", "--task", TASK])
        .write_stdin(stdin)
        .assert()
}

#[test]
fn hook_session_start_records_session() {
    let (_repo, state, store) = setup();

    hook(&state, SESSION_START).success().stdout("");

    let (_, status) = store.task(TASK).unwrap();
    let session = status.claude_session.expect("session is recorded");
    assert_eq!(session.session_id, "s1");
    assert_eq!(session.transcript_path, PathBuf::from("/t/s1.jsonl"));
    let hook_events: Vec<_> = events(&store)
        .into_iter()
        .filter(|ev| ev.kind.starts_with("hook."))
        .collect();
    assert_eq!(hook_events.len(), 1, "{hook_events:?}");
    assert_eq!(hook_events[0].kind, "hook.sessionstart");
    assert_eq!(hook_events[0].source, "hook");
    assert_eq!(
        hook_events[0].data,
        json!({"session_id": "s1", "transcript_path": "/t/s1.jsonl", "cwd": "/w"})
    );
}

#[test]
fn hook_stop_appends_event_and_keeps_session() {
    let (_repo, state, store) = setup();
    hook(&state, SESSION_START).success();

    let stop = r#"{"hook_event_name":"Stop","session_id":"s1","transcript_path":"/t/s1.jsonl","cwd":"/w","stop_hook_active":false}"#;
    hook(&state, stop).success().stdout("");

    let ev = events(&store).pop().unwrap();
    assert_eq!(ev.kind, "hook.stop");
    assert_eq!(ev.source, "hook");
    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.claude_session.unwrap().session_id, "s1");
}

#[test]
fn hook_rejects_malformed_stdin() {
    let (_repo, state, store) = setup();
    let before = events(&store);

    for stdin in ["", "not json", r#"{"hook_event_name":"Stop"}"#] {
        let out = hook(&state, stdin)
            .failure()
            .stdout("")
            .get_output()
            .clone();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(
            stderr.lines().count(),
            1,
            "stdin {stdin:?} gave stderr {stderr:?}"
        );
    }

    assert_eq!(events(&store), before);
    let (_, status) = store.task(TASK).unwrap();
    assert_eq!(status.claude_session, None);
}

#[test]
fn hook_rejects_unknown_task() {
    let (_repo, state, _store) = setup();

    flow(&state)
        .args(["hook", "claude", "--task", "zzzzzzzz"])
        .write_stdin(SESSION_START)
        .assert()
        .failure()
        .stderr("no such task: zzzzzzzz\n");
}
