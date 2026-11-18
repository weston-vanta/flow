//! `flow task resource add` and `remove`, and `resources` on Task reads.

mod common;

use common::TestStore;
use serde_json::{Value, json};

fn json_out(out: &assert_cmd::assert::Assert) -> Value {
    serde_json::from_slice(&out.get_output().stdout).expect("flow prints JSON")
}

/// Asserts `out` is an `{error, help}` body and returns it.
fn error_body(out: &assert_cmd::assert::Assert) -> Value {
    let body = json_out(out);
    assert!(body["error"].is_string(), "{body}");
    assert!(body["help"].is_string(), "{body}");
    body
}

fn create(ts: &TestStore, slug: &str) {
    ts.flow().args(["task", "create", slug]).assert().success();
}

fn join(ts: &TestStore, session: &str, slug: &str) {
    ts.flow()
        .args(["task", "join", slug])
        .env("CLAUDE_CODE_SESSION_ID", session)
        .assert()
        .success();
}

fn resources(ts: &TestStore, slug: &str) -> Value {
    let out = ts.flow().args(["task", "get", slug]).assert().success();
    json_out(&out)["resources"].clone()
}

/// Runs `flow task resource <args>` as Session `session`, if any.
fn resource(ts: &TestStore, session: Option<&str>, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = ts.flow();
    cmd.args(["task", "resource"]).args(args);
    match session {
        Some(id) => cmd.env("CLAUDE_CODE_SESSION_ID", id),
        None => cmd.env_remove("CLAUDE_CODE_SESSION_ID"),
    };
    cmd.assert()
}

#[test]
fn add_with_task_records_resources_in_order_with_null_titles() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    resource(
        &ts,
        None,
        &[
            "add",
            "https://acme.atlassian.net/browse/ABC-1",
            "--title",
            "Source",
            "--task",
            "widgets",
        ],
    )
    .success();
    resource(
        &ts,
        None,
        &["add", "file:///efs-home/notes.md", "--task", "widgets"],
    )
    .success();
    assert_eq!(
        resources(&ts, "widgets"),
        json!([
            { "url": "https://acme.atlassian.net/browse/ABC-1", "title": "Source" },
            { "url": "file:///efs-home/notes.md", "title": null },
        ])
    );
}

#[test]
fn re_adding_a_url_replaces_its_title_and_remove_drops_it() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    join(&ts, "s1", "widgets");
    let s1 = Some("s1");
    resource(&ts, s1, &["add", "http://docs.example/a", "--title", "A"]).success();
    resource(&ts, s1, &["add", "https://docs.example/b"]).success();
    resource(&ts, s1, &["add", "http://docs.example/a", "--title", "A2"]).success();
    assert_eq!(
        resources(&ts, "widgets"),
        json!([
            { "url": "http://docs.example/a", "title": "A2" },
            { "url": "https://docs.example/b", "title": null },
        ])
    );
    resource(&ts, s1, &["remove", "http://docs.example/a"]).success();
    assert_eq!(
        resources(&ts, "widgets"),
        json!([{ "url": "https://docs.example/b", "title": null }])
    );
    // The list carries them too.
    let out = ts.flow().args(["task", "list"]).assert().success();
    assert_eq!(
        json_out(&out)[0]["resources"],
        json!([{ "url": "https://docs.example/b", "title": null }])
    );
}

#[test]
fn only_absolute_https_http_and_file_urls_are_accepted() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    for url in [
        "ftp://example.com/x",
        "docs.example/a",
        "/efs-home/notes.md",
        "https://",
        "file:notes.md",
        "mailto:a@example.com",
    ] {
        let out = resource(&ts, None, &["add", url, "--task", "widgets"]).failure();
        error_body(&out);
    }
    assert_eq!(resources(&ts, "widgets"), json!([]));
}

#[test]
fn without_task_both_fail_when_the_session_is_on_no_task() {
    let ts = TestStore::new();
    create(&ts, "widgets");
    ts.store()
        .put_facet(
            "session",
            "s1",
            "status",
            &json!({"state": "idle"}),
            jiff::Timestamp::now(),
        )
        .unwrap();
    for session in [Some("s1"), None] {
        for args in [
            &["add", "https://docs.example/a"][..],
            &["remove", "https://docs.example/a"][..],
        ] {
            let body = error_body(&resource(&ts, session, args).code(1));
            assert!(body["help"].as_str().unwrap().contains("--task"), "{body}");
        }
    }
}

#[test]
fn task_with_no_task_directory_fails() {
    let ts = TestStore::new();
    for args in [
        &["add", "https://docs.example/a", "--task", "widgets"][..],
        &["remove", "https://docs.example/a", "--task", "widgets"][..],
    ] {
        let body = error_body(&resource(&ts, None, args).code(1));
        assert!(
            body["help"].as_str().unwrap().contains("flow task create"),
            "{body}"
        );
    }
}
