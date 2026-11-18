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

// The Jira poller in `flowd`, against a fake `acli`.

const CYCLE: &str = "flowd: jira poll";

/// A fake `acli` that logs each call's arguments, one per line and followed by `--`, to
/// `acli.log` in the scratch dir, then runs `script`. Returns the log's path.
fn fake_acli(ts: &TestStore, script: &str) -> PathBuf {
    let log = ts.scratch("acli").join("acli.log");
    ts.fake_bin(
        "acli",
        &format!("printf '%s\\n' \"$@\" -- >> '{}'\n{script}", log.display()),
    );
    log
}

fn search_ok() -> String {
    format!(
        "cat '{}/tests/fixtures/jira/search.json'",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// The argument lists of every call logged to `log`.
fn calls(log: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .split_terminator("--\n")
        .map(|call| call.lines().map(str::to_owned).collect())
        .collect()
}

/// Session `s1` owns `feat/x` (`ABC-12`), `feat/y` (`DEF-7`, its PR merged), `feat/z`
/// (`ABC-12` again), `feat/w` (`GONE-1`, which Jira does not return) and `feat/v` (none).
fn branches_with_keys(ts: &TestStore) {
    let dir = repo(ts, "repo", "main");
    for (branch, key) in [
        ("feat/x", Some("ABC-12")),
        ("feat/y", Some("DEF-7")),
        ("feat/z", Some("ABC-12")),
        ("feat/w", Some("GONE-1")),
        ("feat/v", None),
    ] {
        own(ts, &dir, branch);
        if let Some(key) = key {
            jira(ts, &dir, &["set", key, "--branch", branch]);
        }
    }
    put_pr(ts, 8, "feat/y", "MERGED");
}

/// `jira` without its `at`, which it returns parsed.
fn without_at(mut jira: Value) -> (Value, Timestamp) {
    let at = jira.as_object_mut().unwrap().remove("at").unwrap();
    (jira, at.as_str().unwrap().parse().unwrap())
}

#[test]
fn a_poll_searches_every_listed_branchs_key_once_and_writes_each() {
    let ts = TestStore::new();
    branches_with_keys(&ts);
    let log = fake_acli(&ts, &search_ok());
    let before = Timestamp::now();
    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("searched 3 keys, found 2"), "{line}");
    assert!(line.contains("0 errors"), "{line}");

    assert_eq!(
        calls(&log),
        [[
            "jira",
            "workitem",
            "search",
            "--jql",
            "key in (ABC-12, DEF-7, GONE-1)",
            "--fields",
            "key,summary,status,issuetype",
            "--paginate",
            "--json",
        ]]
    );
    let (abc, at) = without_at(branch_jira(&ts, "feat/x"));
    assert!(at >= before, "{at} is before the poll began");
    assert_eq!(
        abc,
        json!({
            "key": "ABC-12", "url": "https://vanta.atlassian.net/browse/ABC-12",
            "found": true, "summary": "Add widgets", "status": "In Review",
            "category": "indeterminate", "type": "Story",
        })
    );
    assert_eq!(without_at(branch_jira(&ts, "feat/z")).0, abc);
    // The merged PR's issue is fetched too.
    assert_eq!(
        without_at(pr_get(&ts, "acme/widgets#8")["jira"].clone()).0,
        json!({
            "key": "DEF-7", "url": "https://vanta.atlassian.net/browse/DEF-7",
            "found": true, "summary": "Fix gadgets", "status": "Done",
            "category": "done", "type": "Bug",
        })
    );
    let mut gone = unfetched("GONE-1");
    gone.as_object_mut().unwrap().remove("at");
    gone["found"] = json!(false);
    assert_eq!(without_at(branch_jira(&ts, "feat/w")).0, gone);
    assert_eq!(branch_jira(&ts, "feat/v"), Value::Null);
}

#[test]
fn a_poll_with_no_key_calls_nothing() {
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    let log = fake_acli(&ts, &search_ok());
    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("searched 0 keys"), "{line}");
    assert!(calls(&log).is_empty());
}

#[test]
fn polls_repeat_every_interval() {
    let ts = TestStore::new();
    branches_with_keys(&ts);
    let log = fake_acli(&ts, &search_ok());
    let mut flowd = ts.flowd_with_env(&[("FLOWD_JIRA_INTERVAL", "0")]);
    flowd.wait_for_line(CYCLE);
    flowd.wait_for_line(CYCLE);
    assert!(calls(&log).len() >= 2);
}

/// Runs one poll whose search runs `script`, over a fetched `ABC-12`, and asserts it wrote
/// nothing and reported one error.
fn failed_search_keeps_the_last_facts(script: &str, env: &[(&str, &str)]) {
    let ts = TestStore::new();
    branches_with_keys(&ts);
    let at: Timestamp = "2026-11-18T09:00:00Z".parse().unwrap();
    let issue = json!({
        "found": true, "summary": "Old summary", "status": "To Do", "category": "new",
        "type": "Story",
    });
    put_issue(&ts, "ABC-12", issue, at);
    let before = branch_jira(&ts, "feat/x");
    fake_acli(&ts, script);
    let mut flowd = ts.flowd_with_env(env);
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("1 errors"), "{line}");
    assert_eq!(branch_jira(&ts, "feat/x"), before);
    assert_eq!(branch_jira(&ts, "feat/w"), unfetched("GONE-1"));
}

#[test]
fn a_failing_search_writes_nothing() {
    failed_search_keeps_the_last_facts("echo '✗ Error: unauthorized' >&2; exit 1", &[]);
}

#[test]
fn an_unparseable_search_writes_nothing() {
    failed_search_keeps_the_last_facts("echo 'not json'", &[]);
}

#[test]
fn a_timed_out_search_writes_nothing() {
    failed_search_keeps_the_last_facts("exec sleep 5", &[("FLOWD_JIRA_TIMEOUT", "1")]);
}

#[test]
fn a_search_that_finds_no_key_writes_each_not_found() {
    // acli fails a search none of whose keys it can read, rather than returning `[]`.
    let ts = TestStore::new();
    let dir = repo(&ts, "repo", "feat/x");
    own(&ts, &dir, "feat/x");
    jira(&ts, &dir, &["set", "GONE-1"]);
    fake_acli(
        &ts,
        "echo '✗ Error: failed to parse JQL query: issue does not exist or you do not have permission to see it.' >&2; exit 1",
    );
    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("searched 1 keys, found 0"), "{line}");
    assert!(line.contains("0 errors"), "{line}");
    assert_eq!(branch_jira(&ts, "feat/x")["found"], json!(false));
}
