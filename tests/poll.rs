//! The PR Writer in `flowd`, against a fake `gh` serving fixture GraphQL responses.

mod common;

use std::fs;

use common::TestStore;
use jiff::Timestamp;
use serde_json::{Value, json};

const CYCLE: &str = "flowd: pr poll wrote";

/// A fake `gh` that logs each query to `calls.log` in the scratch dir and answers from the
/// fixtures by what the query asks for. `gadgets` is the shell to run for `acme/gadgets`'s
/// detail query.
fn fake_gh(ts: &TestStore, search: &str, gadgets: &str) -> std::path::PathBuf {
    let fixtures = format!("{}/tests/fixtures/github", env!("CARGO_MANIFEST_DIR"));
    let log = ts.scratch("gh").join("calls.log");
    ts.fake_bin(
        "gh",
        &format!(
            r#"printf '%s\n' "$*" >> '{log}'
case "$*" in
  *'search('*) {search} ;;
  *'name: "widgets"'*'after: '*) cat '{fixtures}/widgets-contexts-2.json' ;;
  *'name: "widgets"'*) cat '{fixtures}/widgets.json' ;;
  *'name: "gadgets"'*) {gadgets} ;;
  *) echo "unexpected query: $*" >&2; exit 1 ;;
esac"#,
            log = log.display(),
        ),
    );
    log
}

fn search_ok() -> String {
    format!(
        "cat '{}/tests/fixtures/github/search.json'",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn gadgets_ok() -> String {
    format!(
        "cat '{}/tests/fixtures/github/gadgets.json'",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn pr_get(ts: &TestStore, key: &str) -> Value {
    let out = ts.flow().args(["pr", "get", key]).assert().success();
    serde_json::from_slice(&out.get_output().stdout).unwrap()
}

/// `value` without the `at` stamp of its `github` facet.
fn github_fields(pr: &Value) -> Value {
    let mut github = pr["github"].clone();
    github.as_object_mut().unwrap().remove("at");
    github
}

/// Stores `github` for `key` through the library Store, observed at `at`.
fn seed(ts: &TestStore, key: &str, github: &Value, at: Timestamp) {
    ts.store()
        .put_facet("pr", key, "github", github, at)
        .unwrap();
}

fn calls(log: &std::path::Path) -> String {
    fs::read_to_string(log).unwrap_or_default()
}

#[test]
fn a_poll_writes_every_field_of_each_pr() {
    let ts = TestStore::new();
    fake_gh(&ts, &search_ok(), &gadgets_ok());
    let before = Timestamp::now();
    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("wrote 3 PRs"), "{line}");
    assert!(line.contains("for 5 points, 0 errors"), "{line}");

    let pr = pr_get(&ts, "acme/widgets#7");
    assert_eq!(pr["key"], "acme/widgets#7");
    assert_eq!(pr["owner"], Value::Null);
    let at: Timestamp = pr["github"]["at"].as_str().unwrap().parse().unwrap();
    assert!(at >= before, "{at} is before the poll began");
    assert_eq!(
        github_fields(&pr),
        json!({
            "repo": "acme/widgets", "number": 7, "url": "https://github.com/acme/widgets/pull/7",
            "title": "[ABC-123] Add widgets", "state": "OPEN", "isDraft": false,
            "headRefName": "feat/x", "headRefOid": "0123456789abcdef0123456789abcdef01234567",
            "baseRefName": "main", "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED",
            "reviewRequests": [{"login": "alice"}, {"team": "platform"}],
            // Required only: the cancelled `lint` and the failed `ci/optional` are not
            // required. `build`'s failed run is replaced by its later rerun, from page 2.
            "checks": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "ci/legacy", "status": "PENDING", "conclusion": null},
                {"name": "security", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "test", "status": "IN_PROGRESS", "conclusion": null},
            ],
            "stack": {"position": 2},
            "updatedAt": "2026-11-10T11:00:00Z", "additions": 120, "deletions": 4,
            "unresolvedReviewThreads": 2,
        })
    );

    let merged = pr_get(&ts, "acme/widgets#5");
    assert_eq!(merged["github"]["state"], "MERGED");
    assert_eq!(merged["github"]["checks"], json!([]));
    assert_eq!(merged["github"]["stack"], Value::Null);

    let draft = pr_get(&ts, "acme/gadgets#3");
    assert_eq!(draft["github"]["isDraft"], true);
    assert_eq!(draft["github"]["mergeable"], "CONFLICTING");
    assert_eq!(draft["github"]["reviewDecision"], Value::Null);
}

/// Polls with `acme/gadgets`'s batch running `gadgets`, over a stored `acme/gadgets#3`, and
/// asserts that PR keeps its old facet while `acme/widgets`'s batch still writes.
fn failed_batch_keeps_the_last_facet(gadgets: &str, env: &[(&str, &str)]) {
    let ts = TestStore::new();
    fake_gh(&ts, &search_ok(), gadgets);
    let old_at: Timestamp = "2026-11-09T08:00:00Z".parse().unwrap();
    let old = json!({
        "repo": "acme/gadgets", "number": 3, "url": "https://github.com/acme/gadgets/pull/3",
        "title": "Old title", "state": "OPEN", "isDraft": false, "headRefName": "feat/g",
        "headRefOid": "1111111111111111111111111111111111111111", "baseRefName": "main",
        "mergeable": "MERGEABLE", "reviewDecision": null, "reviewRequests": [], "checks": [],
        "stack": null, "updatedAt": "2026-11-09T07:00:00Z", "additions": 1, "deletions": 0,
        "unresolvedReviewThreads": 0,
    });
    seed(&ts, "acme/gadgets#3", &old, old_at);

    let mut flowd = ts.flowd_with_env(env);
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("wrote 2 PRs"), "{line}");
    assert!(line.contains("1 errors"), "{line}");

    let kept = pr_get(&ts, "acme/gadgets#3");
    assert_eq!(kept["github"]["at"], "2026-11-09T08:00:00.000000Z");
    assert_eq!(github_fields(&kept), old);
    assert_eq!(
        pr_get(&ts, "acme/widgets#7")["github"]["title"],
        "[ABC-123] Add widgets"
    );
}

#[test]
fn a_failing_batch_keeps_its_prs_last_facet_while_others_write() {
    failed_batch_keeps_the_last_facet("echo 'HTTP 502' >&2; exit 1", &[]);
}

#[test]
fn a_timed_out_batch_keeps_its_prs_last_facet_while_others_write() {
    failed_batch_keeps_the_last_facet("exec sleep 30", &[("FLOWD_GH_TIMEOUT", "1")]);
}

#[test]
fn a_closed_pr_with_an_unchanged_updated_at_is_not_refetched() {
    let ts = TestStore::new();
    let log = fake_gh(&ts, &search_ok(), &gadgets_ok());
    let merged_at: Timestamp = "2026-11-02T00:00:00Z".parse().unwrap();
    let merged = json!({
        "repo": "acme/widgets", "number": 5, "url": "https://github.com/acme/widgets/pull/5",
        "title": "Widget base", "state": "MERGED", "isDraft": false, "headRefName": "feat/base",
        "headRefOid": "89abcdef0123456789abcdef0123456789abcdef", "baseRefName": "main",
        "mergeable": "UNKNOWN", "reviewDecision": "APPROVED", "reviewRequests": [], "checks": [],
        "stack": null, "updatedAt": "2026-11-01T09:00:00Z", "additions": 30, "deletions": 0,
        "unresolvedReviewThreads": 0,
    });
    seed(&ts, "acme/widgets#5", &merged, merged_at);
    // An open PR is refetched even when its `updatedAt` is unchanged.
    let mut open = merged.clone();
    open["number"] = json!(3);
    open["repo"] = json!("acme/gadgets");
    open["state"] = json!("OPEN");
    open["updatedAt"] = json!("2026-11-10T10:00:00Z");
    seed(&ts, "acme/gadgets#3", &open, merged_at);

    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("wrote 2 PRs"), "{line}");

    let calls = calls(&log);
    assert!(calls.contains("pullRequest(number: 7)"), "{calls}");
    assert!(calls.contains("pullRequest(number: 3)"), "{calls}");
    assert!(!calls.contains("pullRequest(number: 5)"), "{calls}");
    assert_eq!(
        pr_get(&ts, "acme/widgets#5")["github"]["at"],
        "2026-11-02T00:00:00.000000Z"
    );
    assert_ne!(
        pr_get(&ts, "acme/gadgets#3")["github"]["at"],
        "2026-11-02T00:00:00.000000Z"
    );
}

#[test]
fn a_failed_discovery_skips_detail() {
    let ts = TestStore::new();
    let log = fake_gh(&ts, "echo 'HTTP 401' >&2; exit 1", &gadgets_ok());
    let mut flowd = ts.flowd();
    let line = flowd.wait_for_line(CYCLE);
    assert!(line.contains("wrote 0 PRs"), "{line}");
    assert!(line.contains("1 errors"), "{line}");
    assert!(!calls(&log).contains("pullRequest("));
    ts.flow()
        .args(["pr", "get", "acme/widgets#7"])
        .assert()
        .failure();
}
