mod common;

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::task::{ClaudeSession, Harness, HerdrTarget, Origin, State, Status, TaskRecord};
use jiff::Timestamp;
use serde_json::{Value, json};

const AGENT_GET: &str = include_str!("fixtures/herdr/agent_get.json");
const WORKSPACE_CREATE: &str = include_str!("fixtures/herdr/workspace_create.json");
const TAB_RENAME: &str = include_str!("fixtures/herdr/tab_rename.json");
const NO_WORKSPACES: &str =
    r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[]}}"#;
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const PANE_RUN_OK: &str = r#"{"id":"cli:pane:run","result":{"type":"ok"}}"#;
const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";

const ID: &str = "aaaaaaab";
const WORKTREE: &str = "/workspaces/acme-worktrees/search-index";
const SESSION: &str = "0b5e7f3c-1111-2222-3333-444455556666";
const TRANSCRIPT: &str = "/home/dev/.claude/projects/-workspaces-acme-worktrees-search-index/\
     0b5e7f3c-1111-2222-3333-444455556666.jsonl";
/// The pane and tab a scripted `workspace create` returns.
const PANE: &str = "w3:p1";
const TAB: &str = "w3:t1";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    /// State with live host `host-a` and task `ID`, dispatched there, in `state` with a
    /// recorded Claude Code session and no herdr target.
    fn new(state: State) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("config.toml"), "").unwrap();
        let hosts = root.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        let record = json!({
            "name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example",
            "gone": false, "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join("env-0001.json"), record.to_string()).unwrap();
        let fx = Fixture { dir, state: root };
        fx.store()
            .create_task(task_record(Origin::Dispatched), "brief\n")
            .unwrap();
        fx.store()
            .set_status(ID, |status| {
                status.state = state;
                status.claude_session = Some(ClaudeSession {
                    session_id: SESSION.to_owned(),
                    transcript_path: PathBuf::from(TRANSCRIPT),
                });
            })
            .unwrap();
        fx
    }

    fn store(&self) -> Store {
        Store::open(&StateDir::at(&self.state).unwrap())
    }

    fn status(&self) -> Status {
        self.store().task(ID).unwrap().1
    }

    fn events(&self) -> Vec<Event> {
        events::read_all(&self.store().task_dir(ID).unwrap()).unwrap()
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = common::flow();
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env_remove("FLOW_SUPERVISOR");
        cmd
    }

    /// The argv of every runner call, in order.
    fn calls(&self) -> Vec<Vec<String>> {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                serde_json::from_value(call["argv"].clone()).unwrap()
            })
            .collect()
    }

    /// Runs `flow task resume ID <extra>` against `script`, expecting exit code 2, and returns
    /// its stderr.
    fn refused(&self, script: &[Value], extra: &[&str]) -> String {
        let out = self
            .flow(script)
            .args(["task", "resume", ID])
            .args(extra)
            .assert()
            .code(2)
            .get_output()
            .clone();
        String::from_utf8(out.stderr).unwrap()
    }
}

fn task_record(origin: Origin) -> TaskRecord {
    TaskRecord {
        id: ID.to_owned(),
        parent: None,
        title: "Rebuild the search index".to_owned(),
        host: "host-a".to_owned(),
        cwd: PathBuf::from(WORKTREE),
        repo: None,
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: "supervisor".to_owned(),
        origin,
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
}

/// `test <flag> ...` on the host exiting `status`.
fn test_exits(flag: &str, status: i32) -> Value {
    json!({"argv": ["test", flag], "status": status, "stdout": "", "stderr": ""})
}

/// Runner answers for a resume whose checks pass and whose worker starts working.
fn starts_working() -> Vec<Value> {
    vec![
        test_exits("-s", 0),
        test_exits("-d", 0),
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(NO_WORKSPACES),
        ok(WORKSPACE_CREATE),
        ok(TAB_RENAME),
        ok(PANE_RUN_OK),
        ok(AGENT_GET),
    ]
}

fn herdr_calls<'a>(calls: &'a [Vec<String>], subcommand: &[&str]) -> Vec<&'a Vec<String>> {
    calls
        .iter()
        .filter(|argv| {
            argv.first().is_some_and(|program| program == "herdr")
                && argv.len() >= 3 + subcommand.len()
                && argv[3..3 + subcommand.len()] == *subcommand
        })
        .collect()
}

#[test]
fn resume_reopens_a_done_task_and_relaunches_its_session_with_the_task_settings() {
    let fx = Fixture::new(State::Done);

    let out = fx
        .flow(&starts_working())
        .args([
            "task",
            "resume",
            ID,
            "--prompt",
            "Address the review comments on the PR.",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.starts_with(&format!("resumed {ID} on host-a ({PANE})\n")),
        "{stdout}"
    );
    let calls = fx.calls();
    assert_eq!(calls[0], ["test", "-s", TRANSCRIPT]);
    assert_eq!(calls[1], ["test", "-d", WORKTREE]);
    assert_eq!(
        herdr_calls(&calls, &["tab", "rename"])[0][5..],
        [TAB, &format!("Rebuild the search index · {ID}")]
    );

    let status = fx.status();
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert_eq!(
        status.herdr,
        Some(HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: "w3".to_owned(),
            tab_id: TAB.to_owned(),
            pane_id: PANE.to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        })
    );

    let events = fx.events();
    let resume = events.iter().find(|ev| ev.kind == "resume").unwrap();
    assert_eq!(resume.data["from"], "done");
    assert_eq!(resume.data["session_id"], SESSION);
    assert_eq!(
        resume.data["prompt"],
        "Address the review comments on the PR."
    );
    let reopened = events.iter().find(|ev| ev.kind == "status").unwrap();
    assert!(
        events
            .iter()
            .any(|ev| ev.kind == "status" && ev.data == json!({"from": "done", "to": "working"})),
        "{reopened:?}"
    );
    let started = events
        .iter()
        .find(|ev| ev.kind == "worker.started")
        .unwrap();
    let argv: Vec<String> = serde_json::from_value(started.data["argv"].clone()).unwrap();
    let state = fs::canonicalize(&fx.state).unwrap();
    let task_dir = state.join("tasks").join(ID);
    assert_eq!(
        argv[..2],
        ["env", &format!("FLOW_STATE={}", state.display())]
    );
    assert_eq!(
        argv[2..9],
        [
            "claude",
            "--settings",
            task_dir.join("claude-settings.json").to_str().unwrap(),
            "--name",
            ID,
            "--resume",
            SESSION,
        ]
    );
    let message = &argv[9];
    assert_eq!(argv.len(), 10, "{argv:?}");
    assert!(
        message.contains(&format!(
            "Reporting section of your brief ({})",
            task_dir.join("brief.md").display()
        )),
        "{message}"
    );
    assert!(
        message.contains(&format!("task done {ID} --report <path>")),
        "{message}"
    );
    assert!(
        message.ends_with("\n\nAddress the review comments on the PR."),
        "{message}"
    );
    assert!(task_dir.join("claude-settings.json").is_file());
}

#[test]
fn resume_reopens_an_orphaned_task() {
    let fx = Fixture::new(State::Orphaned);

    fx.flow(&starts_working())
        .args(["task", "resume", ID])
        .assert()
        .success();

    assert_eq!(fx.status().state, State::Working);
    let events = fx.events();
    let resume = events.iter().find(|ev| ev.kind == "resume").unwrap();
    assert_eq!(resume.data["from"], "orphaned");
    assert_eq!(resume.data["prompt"], Value::Null);
}

#[test]
fn resume_refuses_an_open_task() {
    let fx = Fixture::new(State::Working);

    let stderr = fx.refused(&[], &[]);

    assert!(stderr.contains("is working, still open"), "{stderr}");
    assert!(fx.calls().is_empty());
}

#[test]
fn resume_refuses_while_the_tab_is_open() {
    let fx = Fixture::new(State::Done);
    fx.store()
        .set_status(ID, |status| {
            status.herdr = Some(HerdrTarget {
                host: "host-a".to_owned(),
                workspace_id: "w1".to_owned(),
                tab_id: "w1:t2".to_owned(),
                pane_id: "w1:p2".to_owned(),
                agent: "claude".to_owned(),
                session: "default".to_owned(),
            });
        })
        .unwrap();

    let stderr = fx.refused(&[], &[]);

    assert!(stderr.contains("still open"), "{stderr}");
    assert!(
        stderr.contains(&format!("flow task close {ID}")),
        "{stderr}"
    );
    assert!(fx.calls().is_empty());
    assert_eq!(fx.status().state, State::Done);
}

#[test]
fn resume_refuses_a_task_without_a_session() {
    let fx = Fixture::new(State::Done);
    fx.store()
        .set_status(ID, |status| status.claude_session = None)
        .unwrap();

    let stderr = fx.refused(&[], &[]);

    assert!(
        stderr.contains("recorded no Claude Code session"),
        "{stderr}"
    );
    assert!(fx.calls().is_empty());
}

#[test]
fn resume_refuses_when_the_transcript_is_missing_on_the_host() {
    let fx = Fixture::new(State::Done);

    let stderr = fx.refused(&[test_exits("-s", 1)], &[]);

    assert!(
        stderr.contains(&format!("no transcript of task {ID}'s session {SESSION}")),
        "{stderr}"
    );
    assert!(stderr.contains("on host-a"), "{stderr}");
    assert_eq!(fx.calls(), [["test", "-s", TRANSCRIPT]]);
    assert_eq!(fx.status().state, State::Done);
    assert!(!fx.events().iter().any(|ev| ev.kind == "resume"));
}

#[test]
fn resume_refuses_when_the_working_directory_is_gone() {
    let fx = Fixture::new(State::Done);

    let stderr = fx.refused(&[test_exits("-s", 0), test_exits("-d", 1)], &[]);

    assert!(stderr.contains("working directory"), "{stderr}");
    assert!(stderr.contains("is gone from host-a"), "{stderr}");
    assert_eq!(fx.status().state, State::Done);
}

#[test]
fn resume_refuses_an_adopted_task() {
    let fx = Fixture::new(State::Done);
    let task_json = fx.store().task_dir(ID).unwrap().join("task.json");
    let rec = task_record(Origin::Adopted);
    fs::write(&task_json, serde_json::to_vec(&rec).unwrap()).unwrap();

    let stderr = fx.refused(&[], &[]);

    assert!(stderr.contains("adopted"), "{stderr}");
    assert!(fx.calls().is_empty());
}

#[test]
fn resume_refuses_an_archived_task() {
    let fx = Fixture::new(State::Done);
    fx.store().archive_task(ID).unwrap();

    let stderr = fx.refused(&[], &[]);

    assert!(stderr.contains("archived"), "{stderr}");
    assert!(fx.calls().is_empty());
}

#[test]
fn resume_that_fails_to_start_returns_the_task_to_done() {
    let fx = Fixture::new(State::Done);
    let script = [
        test_exits("-s", 0),
        test_exits("-d", 0),
        ok(SERVER_RUNNING),
        ok(NO_AGENTS),
        ok(NO_WORKSPACES),
        ok(WORKSPACE_CREATE),
        ok(TAB_RENAME),
        json!({"status": 1, "stdout": "", "stderr": "herdr: pane not found"}),
    ];

    fx.flow(&script)
        .args(["task", "resume", ID])
        .assert()
        .failure();

    let status = fx.status();
    assert_eq!(status.state, State::Done);
    assert_eq!(status.hold, None);
    assert_eq!(
        status.herdr.map(|target| target.tab_id).as_deref(),
        Some(TAB),
        "the tab stays recorded for `flow task close` to retire"
    );
    let failed = fx.events();
    let failed = failed.iter().find(|ev| ev.kind == "resume.failed").unwrap();
    assert!(failed.data["error"].is_string(), "{failed:?}");
}
