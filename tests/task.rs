//! `flow task create`, `join`, `list` and `get`, and `task` on Session reads.

mod common;

use common::{TestStore, error_body, json_out};
use serde_json::{Value, json};

#[test]
fn create_makes_the_task_directory_and_prints_slug_and_dir() {
    let ts = TestStore::new();
    let out = ts
        .flow()
        .args(["task", "create", "widgets-2"])
        .assert()
        .success();
    let dir = ts.home().join("tasks").join("widgets-2");
    assert_eq!(
        json_out(&out),
        json!({ "slug": "widgets-2", "dir": dir.to_str().unwrap() })
    );
    assert!(dir.is_dir());
    assert_eq!(list(&ts), json!([]), "a created Task has no Sessions on it");
}

#[test]
fn create_rejects_a_malformed_slug_as_usage() {
    let ts = TestStore::new();
    for slug in ["Widgets", "1widgets", "-w", "w/x", "", &"w".repeat(33)] {
        let out = ts.flow().args(["task", "create", slug]).assert().code(2);
        error_body(&out);
    }
    assert!(!ts.home().join("tasks").join("Widgets").exists());
    ts.flow()
        .args(["task", "create", &"w".repeat(32)])
        .assert()
        .success();
}

#[test]
fn create_fails_when_the_task_directory_exists() {
    let ts = TestStore::new();
    ts.flow()
        .args(["task", "create", "widgets"])
        .assert()
        .success();
    let out = ts
        .flow()
        .args(["task", "create", "widgets"])
        .assert()
        .code(1);
    error_body(&out);
}

fn create(ts: &TestStore, slug: &str) {
    ts.flow().args(["task", "create", slug]).assert().success();
}

fn join(ts: &TestStore, session: &str, slug: &str) -> assert_cmd::assert::Assert {
    ts.flow()
        .args(["task", "join", slug])
        .env("CLAUDE_CODE_SESSION_ID", session)
        .assert()
}

fn get(ts: &TestStore, slug: &str) -> Value {
    json_out(&ts.flow().args(["task", "get", slug]).assert().success())
}

fn list(ts: &TestStore) -> Value {
    json_out(&ts.flow().args(["task", "list"]).assert().success())
}

#[test]
fn a_session_is_on_no_task_until_it_joins_one() {
    let ts = TestStore::new();
    join(&ts, "s1", "widgets").code(1);
    create(&ts, "widgets");
    create(&ts, "gadgets");
    ts.store()
        .put_facet(
            "session",
            "s1",
            "status",
            &json!({"state": "idle"}),
            jiff::Timestamp::now(),
        )
        .unwrap();
    assert_eq!(ts.session("s1")["task"], Value::Null);
    assert_eq!(ts.sessions()[0]["task"], Value::Null);

    join(&ts, "s1", "widgets").success();
    assert_eq!(ts.session("s1")["task"], "widgets");
    assert_eq!(ts.sessions()[0]["task"], "widgets");

    join(&ts, "s1", "gadgets").success();
    assert_eq!(ts.session("s1")["task"], "gadgets");
}

#[test]
fn join_without_a_task_directory_points_at_create() {
    let ts = TestStore::new();
    let body = error_body(&join(&ts, "s1", "widgets").code(1));
    assert!(
        body["help"].as_str().unwrap().contains("flow task create"),
        "{body}"
    );
    let out = ts.flow().args(["session", "get", "s1"]).assert().code(1);
    error_body(&out);
}

#[test]
fn join_outside_a_session_fails() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    let out = ts.flow().args(["task", "join", "widgets"]).assert().code(1);
    error_body(&out);
    assert_eq!(list(&ts), json!([]));
}

#[test]
fn get_prints_one_task_with_its_sessions() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    let dir = ts.home().join("tasks").join("widgets");
    assert_eq!(
        get(&ts, "widgets"),
        json!({ "slug": "widgets", "dir": dir.to_str().unwrap(), "sessions": [], "resources": [] })
    );
    join(&ts, "s2", "widgets").success();
    join(&ts, "s1", "widgets").success();
    assert_eq!(get(&ts, "widgets")["sessions"], json!(["s1", "s2"]));
}

#[test]
fn get_of_a_slug_with_no_task_directory_is_an_error() {
    let ts = TestStore::new();
    let out = ts.flow().args(["task", "get", "widgets"]).assert().code(1);
    error_body(&out);
}

#[test]
fn list_prints_tasks_with_listed_sessions_in_slug_order() {
    let ts = TestStore::new();
    for slug in ["zebra", "alpha", "idle-task"] {
        create(&ts, slug);
    }
    join(&ts, "s1", "zebra").success();
    join(&ts, "s2", "alpha").success();
    join(&ts, "s3", "zebra").success();
    let tasks = list(&ts);
    let slugs: Vec<&str> = tasks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["alpha", "zebra"]);
    assert_eq!(tasks[1]["sessions"], json!(["s1", "s3"]));
    assert_eq!(tasks[1]["resources"], json!([]));
    assert_eq!(
        tasks[1]["dir"],
        ts.home().join("tasks").join("zebra").to_str().unwrap()
    );
}

#[test]
fn a_task_drops_out_of_the_list_when_its_sessions_do() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    join(&ts, "s1", "widgets").success();
    assert_eq!(list(&ts).as_array().unwrap().len(), 1);
    // Done a day and more ago, owning no Branch: no longer listed.
    let long_ago = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(48);
    ts.store()
        .put_facet(
            "session",
            "s1",
            "status",
            &json!({"state": "done"}),
            long_ago,
        )
        .unwrap();
    assert_eq!(list(&ts), json!([]));
    assert_eq!(get(&ts, "widgets")["sessions"], json!([]));
}
