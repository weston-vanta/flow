use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

fn flow() -> Command {
    Command::cargo_bin("flow").expect("flow binary is built")
}

#[test]
fn missing_state_dir_exits_3() {
    flow()
        .env("FLOW_STATE", "/nonexistent/path")
        .arg("status")
        .assert()
        .code(3)
        .stdout("")
        .stderr(
            "state directory not found: /nonexistent/path \
             (expected a symlink to /efs-home/flow/state)\n",
        );
}

#[test]
fn dangling_state_symlink_exits_3_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("state");
    std::os::unix::fs::symlink(dir.path().join("unmounted"), &link).unwrap();

    flow()
        .env("FLOW_STATE", &link)
        .arg("status")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!(
            "state directory not found: {}",
            link.display()
        )));

    let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "only the dangling symlink should exist");
}

#[test]
fn status_on_empty_state_has_no_open_tasks() {
    let dir = tempfile::tempdir().unwrap();

    flow()
        .env("FLOW_STATE", dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout("no open tasks\nadopted: 0 sessions\n");
}
