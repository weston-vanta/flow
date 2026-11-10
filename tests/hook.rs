//! `flow hook`: recorded Claude Code hook JSON in, Sessions out through `flow session get`.

mod common;

use std::fs;
use std::path::Path;

use common::{TestStore, git_repo};
use flow::store::Put;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

const ID: &str = "8f2c1a7e-3b4d-4e5f-9a6b-1c2d3e4f5a6b";

/// A recorded hook input from `tests/fixtures/hooks`, with its session id and cwd replaced.
fn event(fixture: &str, session_id: &str, cwd: &Path) -> String {
    let path = format!(
        "{}/tests/fixtures/hooks/{fixture}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut input: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    input["session_id"] = json!(session_id);
    input["cwd"] = json!(cwd.to_str().unwrap());
    input.to_string()
}

/// Runs `flow hook` with `stdin` and `env`, asserting it exits 0 and prints nothing.
fn hook(ts: &TestStore, stdin: impl Into<Vec<u8>>, env: &[(&str, &str)]) {
    let mut cmd = ts.flow();
    cmd.arg("hook").write_stdin(stdin);
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.assert().success().stdout("").stderr("");
}

fn state(ts: &TestStore, id: &str) -> Value {
    ts.session(id)["status"]["state"].clone()
}

fn at(facet: &Value) -> Timestamp {
    facet["at"].as_str().unwrap().parse().unwrap()
}

#[test]
fn every_event_in_the_table_writes_its_facets() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    git_repo(&repo, "feat/hooks");
    ts.fake_bin(
        "gitpod",
        r#"[ "$1 $2 $3" = "environment get env-123" ] && echo Flow-test || exit 1"#,
    );
    let env = [
        ("ONA_ID", "env-123"),
        ("HERDR_PANE_ID", "w1:p2"),
        ("HERDR_SOCKET_PATH", "/run/herdr/herdr.sock"),
    ];

    hook(&ts, event("session-start", ID, &repo), &env);
    let session = ts.session(ID);
    let started_at = session["origin"]["at"].as_str().unwrap().to_owned();
    // RFC 3339 UTC with microseconds.
    assert_eq!(
        started_at.len(),
        "2026-11-10T12:00:00.000000Z".len(),
        "{started_at}"
    );
    assert!(started_at.ends_with('Z'), "{started_at}");
    assert_eq!(
        session,
        json!({
            "id": ID,
            "origin": {
                "at": started_at,
                "cde": "Flow-test",
                "cdeId": "env-123",
                "pane": "w1:p2",
                "herdrSocket": "/run/herdr/herdr.sock",
                "cwd": repo.to_str().unwrap(),
                "transcriptPath": "/home/vscode/.claude/projects/-workspaces-obsidian/8f2c1a7e-3b4d-4e5f-9a6b-1c2d3e4f5a6b.jsonl",
            },
            "status": { "at": started_at, "state": "idle" },
            "context": { "at": started_at, "title": null, "branch": "feat/hooks" },
        })
    );

    let steps = [
        ("user-prompt-submit", "running"),
        ("permission-request", "waiting"),
        ("post-tool-use", "running"),
        ("notification-elicitation-dialog", "waiting"),
        ("notification-elicitation-complete", "running"),
        ("notification-elicitation-dialog", "waiting"),
        // A reminder changes nothing.
        ("notification-idle-prompt", "waiting"),
        ("post-tool-use", "running"),
        ("stop", "idle"),
        ("user-prompt-submit", "running"),
        ("stop-failure", "idle"),
        ("session-end", "done"),
    ];
    let mut last_at = at(&session["status"]);
    for (fixture, expected) in steps {
        hook(&ts, event(fixture, ID, &repo), &env);
        let status = &ts.session(ID)["status"];
        assert_eq!(status["state"], expected, "after {fixture}");
        assert!(
            at(status) >= last_at,
            "after {fixture}: status.at went back"
        );
        last_at = at(status);
    }
    // Only SessionStart writes origin.
    assert_eq!(ts.session(ID)["origin"]["at"], started_at);
}

#[test]
fn stop_rereads_the_branch() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    git_repo(&repo, "feat/one");
    hook(&ts, event("session-start", ID, &repo), &[]);
    let status = std::process::Command::new("git")
        .args(["symbolic-ref", "HEAD", "refs/heads/feat/two"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());

    hook(&ts, event("user-prompt-submit", ID, &repo), &[]);
    assert_eq!(ts.session(ID)["context"]["branch"], "feat/one");
    hook(&ts, event("stop", ID, &repo), &[]);
    let context = &ts.session(ID)["context"];
    assert_eq!(context["branch"], "feat/two");
    assert_eq!(context["title"], Value::Null);
}

#[test]
fn branch_is_null_outside_git() {
    let ts = TestStore::new();
    let dir = ts.scratch("not-a-repo");
    hook(&ts, event("session-start", ID, &dir), &[]);
    assert_eq!(ts.session(ID)["context"]["branch"], Value::Null);
}

#[test]
fn facets_never_written_are_null() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    // A Session that started before flow's hook was registered: no SessionStart seen.
    hook(&ts, event("user-prompt-submit", ID, &repo), &[]);
    let session = ts.session(ID);
    assert_eq!(session["origin"], Value::Null);
    assert_eq!(session["context"], Value::Null);
    assert_eq!(session["status"]["state"], "running");
    assert_eq!(
        session.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["context", "id", "origin", "status"]
    );
}

#[test]
fn an_older_post_tool_use_after_stop_leaves_idle() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    hook(&ts, event("user-prompt-submit", ID, &repo), &[]);
    hook(&ts, event("stop", ID, &repo), &[]);
    let stop_at = at(&ts.session(ID)["status"]);

    // An async PostToolUse that started just before Stop lands after it.
    let put = ts
        .store()
        .put_facet(
            "session",
            ID,
            "status",
            &json!({"state": "running"}),
            stop_at - SignedDuration::from_millis(1),
        )
        .unwrap();
    assert_eq!(put, Put::Stale);
    let status = &ts.session(ID)["status"];
    assert_eq!(status["state"], "idle");
    assert_eq!(at(status), stop_at);
}

#[test]
fn a_post_tool_use_hook_older_than_the_stored_stop_is_dropped() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    // The Stop is stored as observed later than the PostToolUse hook process that follows.
    let stop_at = Timestamp::now() + SignedDuration::from_secs(3600);
    ts.store()
        .put_facet("session", ID, "status", &json!({"state": "idle"}), stop_at)
        .unwrap();

    hook(&ts, event("post-tool-use", ID, &repo), &[]);
    let status = &ts.session(ID)["status"];
    assert_eq!(status["state"], "idle");
    assert_eq!(at(status).as_microsecond(), stop_at.as_microsecond());
}

#[test]
fn a_write_with_an_equal_at_applies() {
    let ts = TestStore::new();
    let store = ts.store();
    let at = Timestamp::now();
    store
        .put_facet("session", ID, "status", &json!({"state": "running"}), at)
        .unwrap();
    let put = store
        .put_facet("session", ID, "status", &json!({"state": "idle"}), at)
        .unwrap();
    assert_eq!(put, Put::Applied);
    assert_eq!(state(&ts, ID), "idle");
}

#[test]
fn clear_ends_one_session_and_starts_another_in_the_same_pane() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    let env = [("HERDR_PANE_ID", "w1:p2")];
    let cleared = "11111111-1111-4111-8111-111111111111";

    hook(&ts, event("session-start", ID, &repo), &env);
    hook(&ts, event("user-prompt-submit", ID, &repo), &env);
    hook(&ts, event("stop", ID, &repo), &env);
    let mut end: Value = serde_json::from_str(&event("session-end", ID, &repo)).unwrap();
    end["reason"] = json!("clear");
    hook(&ts, end.to_string(), &env);
    let mut start: Value = serde_json::from_str(&event("session-start", cleared, &repo)).unwrap();
    start["source"] = json!("clear");
    hook(&ts, start.to_string(), &env);

    let sessions = ts.sessions();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    let by_id = |id: &str| sessions.iter().find(|s| s["id"] == id).unwrap().clone();
    assert_eq!(by_id(ID)["status"]["state"], "done");
    assert_eq!(by_id(cleared)["status"]["state"], "idle");
    assert_eq!(by_id(ID)["origin"]["pane"], "w1:p2");
    assert_eq!(by_id(cleared)["origin"]["pane"], "w1:p2");
}

#[test]
fn session_start_for_an_existing_id_rewrites_origin() {
    let ts = TestStore::new();
    let first = ts.scratch("first");
    let second = ts.scratch("second");
    git_repo(&second, "feat/resumed");

    hook(
        &ts,
        event("session-start", ID, &first),
        &[("HERDR_PANE_ID", "w1:p1"), ("ONA_ID", "env-1")],
    );
    hook(&ts, event("user-prompt-submit", ID, &first), &[]);
    hook(&ts, event("session-end", ID, &first), &[]);
    let mut resume: Value = serde_json::from_str(&event("session-start", ID, &second)).unwrap();
    resume["source"] = json!("resume");
    hook(&ts, resume.to_string(), &[("HERDR_PANE_ID", "w3:p1")]);

    let session = ts.session(ID);
    assert_eq!(session["origin"]["pane"], "w3:p1");
    assert_eq!(session["origin"]["cwd"], second.to_str().unwrap());
    assert_eq!(session["origin"]["cdeId"], Value::Null);
    assert_eq!(session["origin"]["cde"], Value::Null);
    assert_eq!(session["status"]["state"], "idle");
    assert_eq!(session["context"]["branch"], "feat/resumed");
    assert_eq!(ts.sessions().len(), 1);
}

#[test]
fn cde_is_null_when_gitpod_fails() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    ts.fake_bin("gitpod", "echo 'not logged in' >&2; exit 1");
    hook(
        &ts,
        event("session-start", ID, &repo),
        &[("ONA_ID", "env-123")],
    );
    let origin = &ts.session(ID)["origin"];
    assert_eq!(origin["cde"], Value::Null);
    assert_eq!(origin["cdeId"], "env-123");
}

#[test]
fn malformed_stdin_exits_0_and_logs() {
    for stdin in ["", "not json", "{\"hook_event_name\": \"Stop\"}"] {
        let ts = TestStore::new();
        hook(&ts, stdin, &[]);
        assert_eq!(
            ts.hook_log().len(),
            1,
            "stdin {stdin:?}: {:?}",
            ts.hook_log()
        );
        assert_eq!(ts.sessions(), Vec::<Value>::new());
    }
}

#[test]
fn an_unknown_event_exits_0_and_logs() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    let mut input: Value = serde_json::from_str(&event("stop", ID, &repo)).unwrap();
    input["hook_event_name"] = json!("PreCompact");
    hook(&ts, input.to_string(), &[]);
    let log = ts.hook_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("PreCompact"), "{log:?}");
    assert_eq!(ts.sessions(), Vec::<Value>::new());
}

#[test]
fn an_unwritable_store_exits_0_and_logs() {
    let ts = TestStore::new();
    let repo = ts.scratch("repo");
    // A file where the Store's directory should be: nothing can be written under it.
    fs::write(ts.store_path(), "").unwrap();
    hook(&ts, event("session-start", ID, &repo), &[]);
    let log = ts.hook_log();
    assert_eq!(log.len(), 1, "{log:?}");
}
