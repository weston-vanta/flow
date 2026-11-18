//! `flow branch jira set/clear` and the `jira` field on `flow branch list`, `flow pr list` and
//! `flow pr get`. Branches are owned through `flow wt create` with a fake `wt`; PRs and the
//! Jira poller's facts are written through the library Store, standing in for their Writers.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestStore;
use jiff::Timestamp;
use serde_json::{Value, json};

/// A repository at `name` whose origin remote is `acme/widgets` on GitHub, with `branch`
/// checked out.
fn repo(ts: &TestStore, name: &str, branch: &str) -> PathBuf {
    let dir = ts.scratch(name);
    common::git_repo(&dir, branch);
    let status = Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:acme/widgets.git"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

/// Records Session `s1` as running and as the owner of `branch` in `dir`'s repository.
fn own(ts: &TestStore, dir: &Path, branch: &str) {
    ts.fake_bin("wt", "exit 0");
    ts.store()
        .put_facet(
            "session",
            "s1",
            "status",
            &json!({ "state": "running" }),
            Timestamp::now(),
        )
        .unwrap();
    ts.flow()
        .args(["wt", "create", branch])
        .current_dir(dir)
        .env("CLAUDE_CODE_SESSION_ID", "s1")
        .assert()
        .success();
}

/// Runs `flow branch jira <args>` in `dir` and returns its exit code and JSON.
fn jira(ts: &TestStore, dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = ts
        .flow()
        .args(["branch", "jira"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let body = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&out.stdout)));
    (out.status.code().unwrap(), body)
}

/// The `jira` of the listed Branch `branch`.
fn branch_jira(ts: &TestStore, branch: &str) -> Value {
    let out = ts.flow().args(["branch", "list"]).assert().success();
    let branches: Vec<Value> = serde_json::from_slice(&out.get_output().stdout).unwrap();
    branches
        .into_iter()
        .find(|b| b["branch"] == branch)
        .unwrap_or_else(|| panic!("{branch} not listed"))["jira"]
        .clone()
}

/// The `jira` a Branch carries before any fetch of `key`.
fn unfetched(key: &str) -> Value {
    json!({
        "key": key, "url": format!("https://vanta.atlassian.net/browse/{key}"),
        "found": null, "at": null, "summary": null, "status": null, "category": null,
        "type": null,
    })
}

#[test]
fn set_links_the_branch_checked_out_here() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    assert_eq!(branch_jira(&ts, "feat/x"), Value::Null);

    let (code, body) = jira(&ts, &dir, &["set", "ABC-12"]);
    assert_eq!(code, 0, "{body}");
    assert_eq!(branch_jira(&ts, "feat/x"), unfetched("ABC-12"));
}

#[test]
fn set_with_branch_links_a_branch_of_this_repository_and_replaces_its_key() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "main");
    own(&ts, &dir, "feat/x");

    let (code, body) = jira(&ts, &dir, &["set", "ABC-12", "--branch", "feat/x"]);
    assert_eq!(code, 0, "{body}");
    assert_eq!(
        body,
        json!({ "branch": "acme/widgets:feat/x", "jira": { "key": "ABC-12" } })
    );
    assert_eq!(branch_jira(&ts, "feat/x"), unfetched("ABC-12"));

    jira(&ts, &dir, &["set", "XY_2-7", "--branch", "feat/x"]);
    assert_eq!(branch_jira(&ts, "feat/x"), unfetched("XY_2-7"));
}

#[test]
fn clear_unlinks_the_branch() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    jira(&ts, &dir, &["set", "ABC-12"]);

    let (code, body) = jira(&ts, &dir, &["clear"]);
    assert_eq!(code, 0, "{body}");
    assert_eq!(
        body,
        json!({ "branch": "acme/widgets:feat/x", "jira": null })
    );
    assert_eq!(branch_jira(&ts, "feat/x"), Value::Null);

    jira(&ts, &dir, &["set", "ABC-12", "--branch", "feat/x"]);
    let (code, _) = jira(&ts, &dir, &["clear", "--branch", "feat/x"]);
    assert_eq!(code, 0);
    assert_eq!(branch_jira(&ts, "feat/x"), Value::Null);
}

#[test]
fn a_malformed_key_is_a_usage_error_and_records_nothing() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    for key in ["abc-12", "ABC-0", "ABC12", "ABC-1x", "1BC-2"] {
        let (code, body) = jira(&ts, &dir, &["set", key]);
        assert_eq!(code, 2, "{key}: {body}");
        assert!(body["error"].as_str().unwrap().contains(key), "{body}");
        assert!(body["help"].is_string(), "{body}");
    }
    assert_eq!(branch_jira(&ts, "feat/x"), Value::Null);
}

#[test]
fn set_outside_a_github_repository_is_an_error() {
    let ts = TestStore::new();
    let dir = ts.scratch("plain");
    common::git_repo(&dir, "feat/x");
    let (code, body) = jira(&ts, &dir, &["set", "ABC-12"]);
    assert_eq!(code, 1, "{body}");
    assert!(body["error"].as_str().unwrap().contains("origin"), "{body}");
    assert!(body["help"].is_string(), "{body}");
}

/// Stores PR `acme/widgets#number` with head `head` in `state`, as the PR Writer would.
fn put_pr(ts: &TestStore, number: u64, head: &str, state: &str) {
    let github = json!({
        "repo": "acme/widgets", "number": number,
        "url": format!("https://github.com/acme/widgets/pull/{number}"),
        "title": "Add widgets", "state": state, "isDraft": false, "headRefName": head,
        "headRefOid": "0123456789abcdef0123456789abcdef01234567", "baseRefName": "main",
        "mergeable": "MERGEABLE", "updatedAt": "2026-11-10T12:00:00Z",
    });
    ts.store()
        .put_facet(
            "pr",
            &format!("acme/widgets#{number}"),
            "github",
            &github,
            Timestamp::now(),
        )
        .unwrap();
}

/// Writes the Jira poller's `issue` facts for `key`, fetched at `at`.
fn put_issue(ts: &TestStore, key: &str, issue: Value, at: Timestamp) {
    ts.store()
        .put_facet("jira", key, "issue", &issue, at)
        .unwrap();
}

fn pr_list(ts: &TestStore) -> Vec<Value> {
    let out = ts.flow().args(["pr", "list"]).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn pr_get(ts: &TestStore, key: &str) -> Value {
    let out = ts.flow().args(["pr", "get", key]).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

#[test]
fn prs_carry_their_head_branchs_jira_issue_with_the_fetched_facts() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "main");
    own(&ts, &dir, "feat/x");
    own(&ts, &dir, "feat/y");
    put_pr(&ts, 7, "feat/x", "OPEN");
    put_pr(&ts, 8, "feat/y", "OPEN");
    jira(&ts, &dir, &["set", "ABC-12", "--branch", "feat/x"]);

    // Before any fetch.
    let listed = pr_list(&ts);
    assert_eq!(listed[0]["jira"], unfetched("ABC-12"));
    assert_eq!(listed[1]["jira"], Value::Null);
    assert_eq!(pr_get(&ts, "acme/widgets#8")["jira"], Value::Null);

    let at: Timestamp = "2026-11-18T09:00:00Z".parse().unwrap();
    put_issue(
        &ts,
        "ABC-12",
        json!({
            "found": true, "summary": "Add widgets", "status": "In Review",
            "category": "indeterminate", "type": "Story",
        }),
        at,
    );
    let fetched = json!({
        "key": "ABC-12", "url": "https://vanta.atlassian.net/browse/ABC-12",
        "found": true, "at": "2026-11-18T09:00:00.000000Z", "summary": "Add widgets",
        "status": "In Review", "category": "indeterminate", "type": "Story",
    });
    assert_eq!(pr_list(&ts)[0]["jira"], fetched);
    assert_eq!(pr_get(&ts, "acme/widgets#7")["jira"], fetched);
    assert_eq!(branch_jira(&ts, "feat/x"), fetched);
    let out = ts.flow().args(["pr", "list", "--all"]).assert().success();
    let all: Vec<Value> = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(all[0]["jira"], fetched);
}

#[test]
fn a_key_jira_did_not_find_carries_only_found_and_at() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    jira(&ts, &dir, &["set", "OLD-3"]);
    let at: Timestamp = "2026-11-18T09:00:00Z".parse().unwrap();
    put_issue(&ts, "OLD-3", json!({ "found": false }), at);

    let mut expected = unfetched("OLD-3");
    expected["found"] = json!(false);
    expected["at"] = json!("2026-11-18T09:00:00.000000Z");
    assert_eq!(branch_jira(&ts, "feat/x"), expected);
}

#[test]
fn the_link_stays_after_the_pr_merges() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    put_pr(&ts, 7, "feat/x", "OPEN");
    jira(&ts, &dir, &["set", "ABC-12"]);
    put_pr(&ts, 7, "feat/x", "MERGED");

    let listed = pr_list(&ts);
    assert_eq!(listed[0]["github"]["state"], "MERGED");
    assert_eq!(listed[0]["jira"], unfetched("ABC-12"));
    assert_eq!(pr_get(&ts, "acme/widgets#7")["jira"], unfetched("ABC-12"));
}
