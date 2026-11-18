//! The listing rule and `flowd`'s retention sweep, over state backdated through the library
//! Store.

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use common::TestStore;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

const DAY: i64 = 24;

/// `hours` ago.
fn ago(hours: i64) -> Timestamp {
    Timestamp::now() - SignedDuration::from_hours(hours)
}

/// `minutes` ago.
fn ago_minutes(minutes: i64) -> Timestamp {
    Timestamp::now() - SignedDuration::from_mins(minutes)
}

fn put_status(ts: &TestStore, id: &str, state: &str, at: Timestamp) {
    ts.store()
        .put_facet("session", id, "status", &json!({ "state": state }), at)
        .unwrap();
}

/// Stores PR `acme/widgets#number` from branch `head` in `state`, fetched at `at`.
fn put_pr(ts: &TestStore, number: u64, head: &str, state: &str, at: Timestamp) {
    let github = json!({
        "repo": "acme/widgets", "number": number,
        "url": format!("https://github.com/acme/widgets/pull/{number}"),
        "title": "Add widgets", "state": state, "isDraft": false, "headRefName": head,
        "headRefOid": "0123456789abcdef0123456789abcdef01234567", "baseRefName": "main",
        "mergeable": "MERGEABLE", "reviewDecision": null, "reviewRequests": [],
        "checks": [], "stack": null, "updatedAt": "2026-11-10T12:00:00Z",
    });
    ts.store()
        .put_facet(
            "pr",
            &format!("acme/widgets#{number}"),
            "github",
            &github,
            at,
        )
        .unwrap();
}

/// Records Session `session` as the reporter of branch `head` of `acme/widgets` at `at`.
fn put_branch(ts: &TestStore, head: &str, session: &str, at: Timestamp) {
    ts.store()
        .put_facet(
            "branch",
            &format!("acme/widgets:{head}"),
            "reporter",
            &json!({ "session": session }),
            at,
        )
        .unwrap();
}

fn cli(ts: &TestStore, args: &[&str]) -> String {
    let out = ts.flow().args(args).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

/// The string `name` of each item of a JSON array.
fn field(json: &str, name: &str) -> Vec<String> {
    let items: Vec<Value> = serde_json::from_str(json).unwrap();
    items
        .iter()
        .map(|item| item[name].as_str().unwrap().to_owned())
        .collect()
}

/// Whether `flow <kind> get <id>` finds it.
fn exists(ts: &TestStore, kind: &str, id: &str) -> bool {
    ts.flow()
        .args([kind, "get", id])
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn the_cli_and_flowd_list_the_same_sessions_and_prs() {
    let ts = TestStore::new();
    // Never done: listed however old (29 days, so flowd's start-up sweep, racing the requests
    // below, leaves it alone).
    put_status(&ts, "running", "running", ago(29 * DAY));
    // Done long ago, but owns an open PR.
    put_status(&ts, "open-pr", "done", ago(2 * DAY));
    put_branch(&ts, "feat/open", "open-pr", ago(2 * DAY));
    put_pr(&ts, 1, "feat/open", "OPEN", ago(1));
    // Done within 24 h, with a worktree but no PR.
    put_status(&ts, "recent", "done", ago(1));
    put_branch(&ts, "feat/recent", "recent", ago(2));
    // Done over 5 min ago, and never created a worktree.
    put_status(&ts, "scratch", "done", ago_minutes(6));
    // Done past 24 h; its PR closed past 24 h too.
    put_status(&ts, "stale", "done", ago(2 * DAY));
    put_branch(&ts, "feat/stale", "stale", ago(3 * DAY));
    put_pr(&ts, 2, "feat/stale", "CLOSED", ago(25));
    // Done past 24 h, but its PR merged within 24 h.
    put_status(&ts, "merged", "done", ago(2 * DAY));
    put_branch(&ts, "feat/merged", "merged", ago(3 * DAY));
    put_pr(&ts, 3, "feat/merged", "MERGED", ago(23));

    let sessions = cli(&ts, &["session", "list"]);
    assert_eq!(
        field(&sessions, "id"),
        ["merged", "open-pr", "recent", "running"]
    );
    let prs = cli(&ts, &["pr", "list"]);
    assert_eq!(field(&prs, "key"), ["acme/widgets#1", "acme/widgets#3"]);

    // Hidden is not gone: the gets still return it.
    assert!(exists(&ts, "session", "stale"));
    assert!(exists(&ts, "pr", "acme/widgets#2"));

    let flowd = ts.flowd();
    assert_eq!(flowd.get("/api/sessions"), (200, sessions));
    assert_eq!(flowd.get("/api/prs"), (200, prs));
}

/// Runs `flow hook` with the recorded `session-start` input for Session `id`.
fn session_start(ts: &TestStore, id: &str) {
    let path = format!(
        "{}/tests/fixtures/hooks/session-start.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut input: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    input["session_id"] = json!(id);
    input["cwd"] = json!(ts.scratch("cwd").to_str().unwrap());
    ts.flow()
        .arg("hook")
        .write_stdin(input.to_string())
        .assert()
        .success();
}

fn owner(ts: &TestStore, pr: &str) -> Value {
    let pr: Value = serde_json::from_str(&cli(ts, &["pr", "get", pr])).unwrap();
    pr["owner"].clone()
}

#[test]
fn a_done_session_that_created_no_worktree_is_listed_for_five_minutes() {
    let ts = TestStore::new();
    put_status(&ts, "fresh", "done", ago_minutes(4));
    put_status(&ts, "ended", "done", ago_minutes(6));
    // Its Branch was taken over by a later Session's worktree, so it reports none.
    put_status(&ts, "superseded", "done", ago_minutes(6));
    put_branch(&ts, "feat/taken", "superseded", ago_minutes(30));
    put_branch(&ts, "feat/taken", "fresh", ago_minutes(10));
    // Reports a Branch with no PR: the 24 h window.
    put_status(&ts, "worktree", "done", ago_minutes(6));
    put_branch(&ts, "feat/wt", "worktree", ago_minutes(30));

    let sessions = cli(&ts, &["session", "list"]);
    assert_eq!(field(&sessions, "id"), ["fresh", "worktree"]);
    assert!(exists(&ts, "session", "ended"));
}

#[test]
fn flowd_sweeps_old_records_at_start_but_keeps_those_tied_to_open_prs() {
    let ts = TestStore::new();
    let old = ago(31 * DAY);
    // Removed: an old done Session, an old stuck Session, and an old closed PR with its Branch.
    put_status(&ts, "old", "done", old);
    put_status(&ts, "stuck", "running", old);
    put_branch(&ts, "feat/old", "old", old);
    put_pr(&ts, 1, "feat/old", "CLOSED", old);
    // Kept: an old Session that owns an open PR, and that PR's old Branch.
    put_status(&ts, "owner", "done", old);
    put_branch(&ts, "feat/open", "owner", old);
    put_pr(&ts, 2, "feat/open", "OPEN", ago(0));
    // Kept: a Session whose newest facet is under 30 days old.
    put_status(&ts, "young", "done", ago(29 * DAY));
    // Kept: a recently closed PR. Its old Branch goes, so it loses its owner.
    put_branch(&ts, "feat/closed", "young", old);
    put_pr(&ts, 3, "feat/closed", "CLOSED", ago(1));
    assert_eq!(owner(&ts, "acme/widgets#3"), "young");

    let _flowd = ts.flowd();
    // The sweep removes Branches last, so the lost owner marks the end of its pass.
    for _ in 0..200 {
        if owner(&ts, "acme/widgets#3").is_null() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(owner(&ts, "acme/widgets#3"), Value::Null);

    assert!(!exists(&ts, "session", "old"));
    assert!(!exists(&ts, "session", "stuck"));
    assert!(!exists(&ts, "pr", "acme/widgets#1"));
    assert!(exists(&ts, "session", "owner"));
    assert!(exists(&ts, "session", "young"));
    assert_eq!(owner(&ts, "acme/widgets#2"), "owner");

    // A removed Session that resumes is recreated by its next SessionStart.
    session_start(&ts, "old");
    let session: Value = serde_json::from_str(&cli(&ts, &["session", "get", "old"])).unwrap();
    assert_eq!(session["status"]["state"], "idle");
}

/// Records Task `slug`'s Resources as written at `at`.
fn put_resources(ts: &TestStore, slug: &str, at: Timestamp) {
    let resources = json!([{ "url": format!("https://docs.example/{slug}"), "title": null }]);
    ts.store()
        .put_facet("task", slug, "resources", &resources, at)
        .unwrap();
}

fn resources(ts: &TestStore, slug: &str) -> Value {
    let task: Value = serde_json::from_str(&cli(ts, &["task", "get", slug])).unwrap();
    task["resources"].clone()
}

#[test]
fn flowd_sweeps_old_task_records_no_session_is_on_but_keeps_the_task_directory() {
    let ts = TestStore::new();
    let old = ago(31 * DAY);
    for slug in ["kept", "orphan", "young", "zombie"] {
        cli(&ts, &["task", "create", slug]);
    }
    // Kept: an old record a Session in the Store is on, though not listed.
    put_resources(&ts, "kept", old);
    put_status(&ts, "s1", "done", ago(29 * DAY));
    ts.store()
        .put_facet(
            "session",
            "s1",
            "task",
            &json!({ "slug": "kept" }),
            ago(29 * DAY),
        )
        .unwrap();
    // Removed: an old record whose only Session is itself swept.
    put_resources(&ts, "orphan", old);
    put_status(&ts, "s2", "done", old);
    ts.store()
        .put_facet("session", "s2", "task", &json!({ "slug": "orphan" }), old)
        .unwrap();
    // Kept: a record under 30 days old that no Session is on.
    put_resources(&ts, "young", ago(29 * DAY));
    // Removed: an old record no Session was ever on. Swept last, so it marks the pass's end.
    put_resources(&ts, "zombie", old);

    let _flowd = ts.flowd();
    for _ in 0..200 {
        if resources(&ts, "zombie") == json!([]) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(resources(&ts, "zombie"), json!([]));
    assert_eq!(resources(&ts, "orphan"), json!([]));
    assert_eq!(resources(&ts, "kept").as_array().unwrap().len(), 1);
    assert_eq!(resources(&ts, "young").as_array().unwrap().len(), 1);
    // The Task directory stays, so its slug stays taken.
    assert!(ts.home().join("tasks").join("zombie").is_dir());
    ts.flow()
        .args(["task", "create", "zombie"])
        .assert()
        .code(1);
}

/// The `jira` of each listed Branch, by branch name.
fn branch_jiras(ts: &TestStore) -> std::collections::HashMap<String, Value> {
    let branches: Vec<Value> = serde_json::from_str(&cli(ts, &["branch", "list"])).unwrap();
    branches
        .into_iter()
        .map(|b| (b["branch"].as_str().unwrap().to_owned(), b["jira"].clone()))
        .collect()
}

#[test]
fn flowd_sweeps_jira_issues_fetched_over_30_days_ago() {
    let ts = TestStore::new();
    put_status(&ts, "s1", "running", ago(0));
    for (head, key, fetched) in [
        ("feat/old", "OLD-1", 31 * DAY),
        ("feat/new", "NEW-1", 29 * DAY),
    ] {
        put_branch(&ts, head, "s1", ago(0));
        ts.store()
            .put_facet(
                "branch",
                &format!("acme/widgets:{head}"),
                "jira",
                &json!({ "key": key }),
                ago(0),
            )
            .unwrap();
        ts.store()
            .put_facet(
                "jira",
                key,
                "issue",
                &json!({ "found": true, "summary": "Widgets" }),
                ago(fetched),
            )
            .unwrap();
    }
    assert_eq!(branch_jiras(&ts)["feat/old"]["found"], true);

    // The default fake `acli` fails, so the Jira poller rewrites nothing.
    let _flowd = ts.flowd();
    for _ in 0..200 {
        if branch_jiras(&ts)["feat/old"]["found"].is_null() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let jiras = branch_jiras(&ts);
    assert_eq!(jiras["feat/old"]["found"], Value::Null);
    assert_eq!(jiras["feat/new"]["found"], true);
}
