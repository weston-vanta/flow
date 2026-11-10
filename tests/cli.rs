mod common;

use common::TestStore;
use predicates::prelude::*;

#[test]
fn flow_with_no_args_prints_help() {
    TestStore::new()
        .flow()
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: flow"));
}

#[test]
fn unknown_command_is_a_usage_error_in_json() {
    let out = TestStore::new().flow().arg("bogus").assert().code(2);
    let body: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(body["error"].as_str().unwrap().contains("bogus"), "{body}");
    assert!(body["help"].is_string(), "{body}");
}

#[test]
fn guide_prints_the_agent_note() {
    TestStore::new()
        .flow()
        .arg("guide")
        .assert()
        .success()
        .stdout(include_str!("../docs/agent-note.md"));
}

#[test]
fn help_lists_guide() {
    TestStore::new()
        .flow()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("guide"));
}
