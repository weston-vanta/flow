//! `flow wt create`, `flow pr list` and `flow pr get`. PRs are set up through the library
//! Store, standing in for the PR Writer.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use common::TestStore;
use jiff::Timestamp;
use serde_json::{Value, json};

/// A repository whose origin remote is `remote`, for `flow wt create` to run in.
fn repo(ts: &TestStore, remote: &str) -> PathBuf {
    let dir = ts.scratch("repo");
    common::git_repo(&dir, "main");
    let status = Command::new("git")
        .args(["remote", "add", "origin", remote])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

/// A fake `wt` that appends its arguments to `wt.args` and exits with `code`.
fn fake_wt(ts: &TestStore, code: i32) -> PathBuf {
    let log = ts.scratch("wt").join("wt.args");
    ts.fake_bin(
        "wt",
        &format!(
            "echo \"$@\" >> {}\necho wt-out\necho wt-err >&2\nexit {code}",
            log.display()
        ),
    );
    log
}

/// Runs `flow wt create <args>` in `dir` as Session `session`.
fn wt_create(
    ts: &TestStore,
    dir: &PathBuf,
    session: Option<&str>,
    args: &[&str],
) -> assert_cmd::assert::Assert {
    let mut cmd = ts.flow();
    cmd.args(["wt", "create"]).args(args).current_dir(dir);
    if let Some(session) = session {
        cmd.env("CLAUDE_CODE_SESSION_ID", session);
    }
    cmd.assert()
}

/// Stores PR `repo#number` with head branch `head`, as the PR Writer would.
fn put_pr(ts: &TestStore, repo: &str, number: u64, head: &str) {
    let github = json!({
        "repo": repo,
        "number": number,
        "url": format!("https://github.com/{repo}/pull/{number}"),
        "title": "Add widgets",
        "state": "OPEN",
        "isDraft": false,
        "headRefName": head,
        "headRefOid": "0123456789abcdef0123456789abcdef01234567",
        "baseRefName": "main",
        "mergeable": "MERGEABLE",
        "reviewDecision": "REVIEW_REQUIRED",
        "reviewRequests": [{ "login": "alice" }, { "team": "acme/reviewers" }],
        "checks": [{ "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS" }],
        "stack": { "position": 1 },
        "updatedAt": "2026-11-10T12:00:00Z",
        "additions": 10,
        "deletions": 2,
        "unresolvedReviewThreads": 1,
    });
    ts.store()
        .put_facet(
            "pr",
            &format!("{repo}#{number}"),
            "github",
            &github,
            Timestamp::now(),
        )
        .unwrap();
}

fn prs(ts: &TestStore) -> Vec<Value> {
    let out = ts.flow().args(["pr", "list"]).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn pr(ts: &TestStore, key: &str) -> Value {
    let out = ts.flow().args(["pr", "get", key]).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

fn owner(ts: &TestStore, key: &str) -> Value {
    pr(ts, key)["owner"].clone()
}

#[test]
fn wt_create_with_a_session_owns_the_branch_and_passes_wt_through() {
    let ts = TestStore::new();
    let dir = repo(&ts, "git@github.com:acme/widgets.git");
    let log = fake_wt(&ts, 0);
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    wt_create(&ts, &dir, Some("s1"), &["feat/x", "--from", "main"])
        .success()
        .stdout("wt-out\n")
        .stderr("wt-err\n");
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "create feat/x --from main\n"
    );
    assert_eq!(owner(&ts, "acme/widgets#7"), "s1");
}

#[test]
fn wt_failing_writes_nothing_and_passes_its_exit_status() {
    let ts = TestStore::new();
    let dir = repo(&ts, "git@github.com:acme/widgets.git");
    fake_wt(&ts, 3);
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    wt_create(&ts, &dir, Some("s1"), &["feat/x"])
        .code(3)
        .stdout("wt-out\n");
    assert_eq!(owner(&ts, "acme/widgets#7"), Value::Null);
}

#[test]
fn wt_create_outside_a_session_only_runs_wt() {
    let ts = TestStore::new();
    let dir = repo(&ts, "git@github.com:acme/widgets.git");
    let log = fake_wt(&ts, 0);
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    wt_create(&ts, &dir, None, &["feat/x"]).success();
    assert_eq!(fs::read_to_string(log).unwrap(), "create feat/x\n");
    assert_eq!(owner(&ts, "acme/widgets#7"), Value::Null);
}

#[test]
fn the_later_reporter_owns_the_pr() {
    let ts = TestStore::new();
    let dir = repo(&ts, "git@github.com:acme/widgets.git");
    fake_wt(&ts, 0);
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    wt_create(&ts, &dir, Some("first"), &["feat/x"]).success();
    assert_eq!(prs(&ts)[0]["owner"], "first");
    wt_create(&ts, &dir, Some("second"), &["feat/x"]).success();
    let listed = prs(&ts);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["owner"], "second");
}

#[test]
fn ssh_and_https_remotes_both_name_the_repo() {
    for remote in [
        "git@github.com:acme/widgets.git",
        "ssh://git@github.com/acme/widgets.git",
        "https://github.com/acme/widgets.git",
        "https://github.com/acme/widgets",
    ] {
        let ts = TestStore::new();
        let dir = repo(&ts, remote);
        fake_wt(&ts, 0);
        put_pr(&ts, "acme/widgets", 7, "feat/x");
        wt_create(&ts, &dir, Some("s1"), &["feat/x"]).success();
        assert_eq!(owner(&ts, "acme/widgets#7"), "s1", "{remote}");
    }
}

#[test]
fn an_unparseable_remote_keeps_wts_status_and_reports_on_stderr() {
    let ts = TestStore::new();
    let dir = repo(&ts, "/srv/git/widgets.git");
    fake_wt(&ts, 0);
    let out = wt_create(&ts, &dir, Some("s1"), &["feat/x"])
        .success()
        .stdout("wt-out\n");
    let stderr = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    let body: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert!(body["error"].as_str().unwrap().contains("origin"), "{body}");
    assert!(body["help"].is_string(), "{body}");
}

#[test]
fn wt_create_passes_help_to_wt() {
    let ts = TestStore::new();
    let dir = repo(&ts, "git@github.com:acme/widgets.git");
    let log = fake_wt(&ts, 0);
    wt_create(&ts, &dir, None, &["feat/x", "--help"]).success();
    assert_eq!(fs::read_to_string(log).unwrap(), "create feat/x --help\n");
}

#[test]
fn unowned_prs_are_left_out_of_the_list_but_can_be_got() {
    let ts = TestStore::new();
    let dir = repo(&ts, "https://github.com/acme/widgets.git");
    fake_wt(&ts, 0);
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    put_pr(&ts, "acme/widgets", 8, "feat/y");
    put_pr(&ts, "acme/other", 7, "feat/x");
    wt_create(&ts, &dir, Some("s1"), &["feat/x"]).success();

    let listed = prs(&ts);
    let keys: Vec<&Value> = listed.iter().map(|p| &p["key"]).collect();
    assert_eq!(keys, [&json!("acme/widgets#7")]);

    let unowned = pr(&ts, "acme/widgets#8");
    assert_eq!(unowned["key"], "acme/widgets#8");
    assert_eq!(unowned["owner"], Value::Null);
    assert_eq!(unowned["github"]["headRefName"], "feat/y");
}

#[test]
fn a_pr_carries_its_github_facet_in_full() {
    let ts = TestStore::new();
    put_pr(&ts, "acme/widgets", 7, "feat/x");
    let got = pr(&ts, "acme/widgets#7");
    let github = &got["github"];
    assert!(github["at"].is_string(), "{got}");
    assert_eq!(github["state"], "OPEN");
    assert_eq!(github["isDraft"], false);
    assert_eq!(
        github["reviewRequests"],
        json!([{ "login": "alice" }, { "team": "acme/reviewers" }])
    );
    assert_eq!(github["stack"], json!({ "position": 1 }));
    assert_eq!(github["additions"], 10);
    assert_eq!(github["deletions"], 2);
    assert_eq!(github["unresolvedReviewThreads"], 1);
}

#[test]
fn get_of_a_missing_pr_is_an_error() {
    let ts = TestStore::new();
    let out = ts
        .flow()
        .args(["pr", "get", "acme/widgets#9"])
        .assert()
        .code(1);
    let body: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("acme/widgets#9"),
        "{body}"
    );
    assert!(body["help"].is_string(), "{body}");
}

#[test]
fn help_lists_the_reads_and_wt_create() {
    let ts = TestStore::new();
    let out = ts.flow().arg("--help").assert().success();
    let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for command in ["session", "pr", "wt"] {
        assert!(help.contains(command), "{help}");
    }
    for (command, subs) in [("session", ["list", "get"]), ("pr", ["list", "get"])] {
        let out = ts.flow().args([command, "--help"]).assert().success();
        let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        for sub in subs {
            assert!(help.contains(sub), "{help}");
        }
    }
    let out = ts.flow().args(["wt", "--help"]).assert().success();
    let help = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(help.contains("create"), "{help}");
}
