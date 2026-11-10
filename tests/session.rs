//! `flow session list` and `flow session get`.

mod common;

use common::TestStore;
use jiff::Timestamp;
use serde_json::{Value, json};

fn put_status(ts: &TestStore, id: &str, state: &str) {
    ts.store()
        .put_facet(
            "session",
            id,
            "status",
            &json!({ "state": state }),
            Timestamp::now(),
        )
        .unwrap();
}

fn error_body(out: &assert_cmd::assert::Assert) -> Value {
    let body: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(body["error"].is_string(), "{body}");
    assert!(body["help"].is_string(), "{body}");
    body
}

#[test]
fn list_of_an_empty_store_is_an_empty_array() {
    assert_eq!(TestStore::new().sessions(), Vec::<Value>::new());
}

#[test]
fn list_returns_every_session_ordered_by_id() {
    let ts = TestStore::new();
    let ids: Vec<String> = (0..40).map(|i| format!("session-{i:02}")).collect();
    for id in ids.iter().rev() {
        put_status(&ts, id, "idle");
    }
    let listed: Vec<Value> = ts.sessions().iter().map(|s| s["id"].clone()).collect();
    assert_eq!(listed, ids.iter().map(|id| json!(id)).collect::<Vec<_>>());
}

#[test]
fn ids_with_path_characters_round_trip() {
    let ts = TestStore::new();
    let ids = ["a/b:c", "50%/x", "owner/repo#12", "acme/widgets:feat%2Fx"];
    for id in ids {
        put_status(&ts, id, "running");
    }
    for id in ids {
        assert_eq!(ts.session(id)["id"], id);
    }
    let mut listed: Vec<String> = ts
        .sessions()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    listed.sort();
    let mut expected: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(listed, expected);
}

#[test]
fn get_defaults_to_the_calling_session() {
    let ts = TestStore::new();
    put_status(&ts, "mine", "running");
    put_status(&ts, "other", "idle");
    let out = ts
        .flow()
        .args(["session", "get"])
        .env("CLAUDE_CODE_SESSION_ID", "mine")
        .assert()
        .success();
    let session: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(session["id"], "mine");
    assert_eq!(session["status"]["state"], "running");
}

#[test]
fn get_of_a_missing_id_is_an_error() {
    let ts = TestStore::new();
    let out = ts.flow().args(["session", "get", "nope"]).assert().code(1);
    let body = error_body(&out);
    assert!(body["error"].as_str().unwrap().contains("nope"), "{body}");
}

#[test]
fn get_with_no_id_outside_a_session_is_a_usage_error() {
    let out = TestStore::new()
        .flow()
        .args(["session", "get"])
        .assert()
        .code(2);
    error_body(&out);
}

#[test]
fn hook_is_hidden_from_help() {
    let out = TestStore::new().flow().arg("--help").assert().success();
    let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(help.contains("session"), "{help}");
    assert!(!help.contains("hook"), "{help}");
}
