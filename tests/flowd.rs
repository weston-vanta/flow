//! `flowd`'s read API: the same JSON as the CLI, read fresh from the Store, on loopback only.

mod common;

use std::fs;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::process::Command;

use common::TestStore;
use jiff::Timestamp;
use serde_json::json;

const SESSION: &str = "8f2c1a7e-3b4d-4e5f-9a6b-1c2d3e4f5a6b";

/// Runs `flow hook` with the recorded `session-start` input for Session `id`.
fn session_start(ts: &TestStore, id: &str) {
    let path = format!(
        "{}/tests/fixtures/hooks/session-start.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut input: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    input["session_id"] = json!(id);
    input["cwd"] = json!(ts.scratch("cwd").to_str().unwrap());
    ts.flow()
        .arg("hook")
        .write_stdin(input.to_string())
        .assert()
        .success();
}

/// Stores an open PR on `acme/widgets` from branch `feat/x` and makes Session `owner` its
/// owner through `flow wt create`.
fn owned_pr(ts: &TestStore, owner: &str) {
    let github = json!({
        "repo": "acme/widgets", "number": 7, "url": "https://github.com/acme/widgets/pull/7",
        "title": "Add widgets", "state": "OPEN", "isDraft": false, "headRefName": "feat/x",
        "headRefOid": "0123456789abcdef0123456789abcdef01234567", "baseRefName": "main",
        "mergeable": "MERGEABLE", "reviewDecision": "REVIEW_REQUIRED", "reviewRequests": [],
        "checks": [], "stack": null, "updatedAt": "2026-11-10T12:00:00Z",
        "additions": 10, "deletions": 2, "unresolvedReviewThreads": 0,
    });
    ts.store()
        .put_facet("pr", "acme/widgets#7", "github", &github, Timestamp::now())
        .unwrap();
    let repo = ts.scratch("repo");
    common::git_repo(&repo, "main");
    let status = Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:acme/widgets.git"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    ts.fake_bin("wt", "exit 0");
    ts.flow()
        .args(["wt", "create", "feat/x"])
        .current_dir(&repo)
        .env("CLAUDE_CODE_SESSION_ID", owner)
        .assert()
        .success();
}

fn cli_stdout(ts: &TestStore, args: &[&str]) -> String {
    let out = ts.flow().args(args).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn api_returns_the_cli_json_byte_for_byte() {
    let ts = TestStore::new();
    session_start(&ts, SESSION);
    owned_pr(&ts, SESSION);
    let flowd = ts.flowd();

    let sessions = cli_stdout(&ts, &["session", "list"]);
    assert!(sessions.contains(SESSION), "{sessions}");
    assert_eq!(flowd.get("/api/sessions"), (200, sessions));

    let prs = cli_stdout(&ts, &["pr", "list"]);
    assert!(prs.contains("acme/widgets#7"), "{prs}");
    assert_eq!(flowd.get("/api/prs"), (200, prs));
}

#[test]
fn a_hook_write_after_start_appears_in_the_next_response() {
    let ts = TestStore::new();
    let flowd = ts.flowd();
    assert_eq!(flowd.get("/api/sessions"), (200, "[]\n".to_owned()));

    session_start(&ts, SESSION);
    let (status, body) = flowd.get("/api/sessions");
    assert_eq!(status, 200);
    assert_eq!(body, cli_stdout(&ts, &["session", "list"]));
    assert!(body.contains(SESSION), "{body}");
}

#[test]
fn serves_the_embedded_page_and_404s_elsewhere() {
    let ts = TestStore::new();
    let flowd = ts.flowd();
    let (status, body) = flowd.get("/");
    assert_eq!(status, 200);
    assert!(body.contains("<title>flow</title>"), "{body}");
    for asset in ["/favicon.svg", "/app.js", "/app.css"] {
        let (status, body) = flowd.get(asset);
        assert_eq!(status, 200, "{asset}");
        assert!(!body.is_empty(), "{asset}");
    }
    assert_eq!(flowd.get("/nope").0, 404);
}

#[test]
fn refuses_connections_to_non_loopback_addresses() {
    let ts = TestStore::new();
    let flowd = ts.flowd();
    assert!(flowd.addr.ip().is_loopback(), "{}", flowd.addr);

    // This machine's outward address: connecting a UDP socket sends nothing.
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("10.255.255.255:9").unwrap();
    let ip: IpAddr = socket.local_addr().unwrap().ip();
    assert!(!ip.is_loopback() && !ip.is_unspecified(), "{ip}");
    let outward = SocketAddr::new(ip, flowd.addr.port());
    assert!(
        common::get(outward, "/api/sessions").is_err(),
        "flowd answered on {outward}"
    );
}
