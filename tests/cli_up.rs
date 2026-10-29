use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events;
use flow::state::task::{
    ClaudeSession, Harness, HerdrTarget, Hold, HoldOn, Origin, State, TaskRecord,
};
use flow::supervisor::{self, Lock};
use jiff::Timestamp;
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

"#;

/// Writes cached records of `host-a` (env `env-0001`) and `host-b` (env `env-0002`), the hosts
/// discovery would have found.
fn write_hosts(state: &Path) {
    let hosts = state.join("hosts");
    fs::create_dir_all(&hosts).unwrap();
    for (name, env_id) in [("host-a", "env-0001"), ("host-b", "env-0002")] {
        let record = serde_json::json!({
            "name": name, "env_id": env_id, "ssh_alias": format!("{name}.example"),
            "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
    }
}

const AGENT_GET: &str = include_str!("fixtures/herdr/agent_get.json");
const AGENT_WAIT_TIMEOUT: &str = include_str!("fixtures/herdr/agent_wait_timeout.json");
const WORKSPACE_CLOSED: &str = r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#;
const WORKSPACE_CREATE: &str = include_str!("fixtures/herdr/workspace_create.json");
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const PANE_RUN_OK: &str = r#"{"id":"cli:pane:run","result":{"type":"ok"}}"#;
const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";
/// A pane `agent_list.json` lists.
const LIVE_PANE: &str = "w1:p1";
/// The first prompt every supervisor start passes Claude Code, quoted as `pane run` types it.
const START_PROMPT: &str = "'Run the session start procedure in CLAUDE.md.'";
/// The pane a scripted `workspace create` returns.
const NEW_PANE: &str = "w3:p1";
const TASK: &str = "aaaaaaab";
const SESSION_START: &str = r#"{"hook_event_name":"SessionStart","session_id":"s1","transcript_path":"/t/s1.jsonl","cwd":"/workspaces/flow","source":"startup"}"#;

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        write_hosts(&state);
        Fixture { dir, state }
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on host-a over this state dir, answering runner calls from `script` in order,
    /// with stdout counted as a terminal so attaching prints the command it would run.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env("FLOW_FAKE_TTY", "1")
            .env_remove(supervisor::SUPERVISOR_ENV);
        cmd
    }

    /// `(host, argv)` of every runner call, in order.
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                (
                    call["host"].as_str().unwrap().to_owned(),
                    serde_json::from_value(call["argv"].clone()).unwrap(),
                )
            })
            .collect()
    }

    fn state_dir(&self) -> StateDir {
        StateDir::resolve(self.dir.path()).unwrap()
    }

    fn write_lock(&self, env_id: &str, host: &str, pane: &str, session: Option<&str>) {
        let dir = self.state.join("supervisor");
        fs::create_dir_all(&dir).unwrap();
        let lock = json!({
            "host_env_id": env_id,
            "herdr": {"host": host, "workspace_id": "w1", "pane_id": pane, "agent": "claude",
                "session": "flow"},
            "claude_session": session,
            "since": "2026-10-26T12:00:00Z",
        });
        fs::write(dir.join("lock.json"), lock.to_string()).unwrap();
    }

    /// A fresh `watch.lock` held by a watcher on the host `env_id`.
    fn write_watch_lock(&self, env_id: &str) {
        let dir = self.state.join("supervisor/watch.lock");
        fs::create_dir_all(&dir).unwrap();
        let owner = json!({"pid": 4242, "host": env_id, "started": "2026-10-26T12:00:00Z"});
        fs::write(dir.join("owner.json"), owner.to_string()).unwrap();
    }

    fn lock(&self) -> Option<Lock> {
        supervisor::load(&self.state_dir()).unwrap()
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
}

fn failed(stderr: &str) -> Value {
    json!({"status": 1, "stdout": "", "stderr": stderr})
}

/// The liveness check's answer for a live supervisor: its agent in `LIVE_PANE`, herdr
/// reporting the lock's Claude Code session `s9` for it.
fn supervisor_live() -> Vec<Value> {
    vec![ok(&lock_pane_agent("/workspaces/flow", Some("s9")))]
}

/// Answers every `agent read` as herdr does for a pane with no agent in it.
fn agent_read_fails() -> Value {
    json!({
        "argv": ["agent", "read"], "repeat": true, "status": 1, "stdout": "",
        "stderr": r#"{"error":{"code":"agent_not_found","message":"agent target w3:p1 not found"},"id":"cli:agent:read"}"#,
    })
}

fn wait_reports(status: &str) -> Value {
    ok(&AGENT_GET.replace(
        r#""agent_status":"working""#,
        &format!(r#""agent_status":"{status}""#),
    ))
}

/// Runner answers for a fresh supervisor start that comes up idle, herdr reporting Claude Code
/// session `s1` for it.
fn fresh_start() -> Vec<Value> {
    fresh_start_as("s1")
}

/// `fresh_start`, herdr reporting Claude Code session `session` for the new supervisor.
fn fresh_start_as(session: &str) -> Vec<Value> {
    vec![
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        wait_reports("idle"),
        ok(&new_pane_agent(session)),
    ]
}

/// An agent list with the new supervisor in `NEW_PANE`, herdr reporting Claude Code session
/// `session` for it.
fn new_pane_agent(session: &str) -> String {
    lock_pane_agent("/workspaces/flow", Some(session))
        .replace(LIVE_PANE, NEW_PANE)
        .replace(r#""workspace_id":"w1""#, r#""workspace_id":"w3""#)
}

fn stdout_of(cmd: &mut Command) -> String {
    output_of(cmd).0
}

/// The stdout and stderr of `cmd`, which must succeed.
fn output_of(cmd: &mut Command) -> (String, String) {
    let out = cmd.assert().success().get_output().clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The command lines typed into panes by `pane run` calls, in order.
fn pane_run_lines(calls: &[(String, Vec<String>)]) -> Vec<String> {
    calls
        .iter()
        .filter(|(_, argv)| argv.get(3..5) == Some(&["pane".to_owned(), "run".to_owned()]))
        .map(|(_, argv)| argv.last().unwrap().clone())
        .collect()
}

/// The command line typed into the pane by the only `pane run` call.
fn pane_run_line(calls: &[(String, Vec<String>)]) -> String {
    let lines = pane_run_lines(calls);
    assert_eq!(lines.len(), 1, "{calls:?}");
    lines[0].clone()
}

fn has_subcommand(calls: &[(String, Vec<String>)], words: &[&str]) -> bool {
    calls.iter().any(|(_, argv)| {
        argv.windows(words.len())
            .any(|window| window.iter().zip(words).all(|(a, b)| a == b))
    })
}

#[test]
fn up_creates_supervisor_when_no_lock() {
    let fx = Fixture::new();

    let (stdout, stderr) = output_of(fx.flow(&fresh_start()).arg("up"));

    let calls = fx.calls();
    assert!(calls.iter().all(|(host, _)| host == "host-a"), "{calls:?}");
    assert!(
        has_subcommand(
            &calls,
            &["workspace", "create", "--cwd", "/workspaces/flow"]
        ),
        "{calls:?}"
    );
    assert!(
        has_subcommand(&calls, &["--label", "flow-supervisor"]),
        "{calls:?}"
    );
    let line = pane_run_line(&calls);
    let state = fs::canonicalize(&fx.state).unwrap();
    assert_eq!(
        line,
        format!(
            "env 'FLOW_STATE={}' 'FLOW_SUPERVISOR=1' claude --name flow-supervisor {START_PROMPT}",
            state.display()
        ),
        "the state path and marker keep the supervisor's hooks live; the prompt runs a first turn"
    );
    assert!(!line.contains("--resume"), "{line}");
    assert!(!line.contains("--settings"), "{line}");

    let lock = fx.lock().expect("lock is written");
    assert_eq!(lock.host_env_id, "env-0001");
    assert_eq!(
        lock.herdr,
        HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: "w3".to_owned(),
            pane_id: NEW_PANE.to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        }
    );
    assert_eq!(
        lock.claude_session.as_deref(),
        Some("s1"),
        "the session herdr reports for the new supervisor is recorded"
    );
    assert_eq!(
        stdout,
        format!("attach: herdr --session default agent attach {NEW_PANE}\n")
    );
    assert_eq!(
        stderr,
        format!("started a fresh supervisor on host-a (pane {NEW_PANE})\n")
    );
}

#[test]
fn up_launch_line_names_the_state_dir_behind_a_symlink() {
    let fx = Fixture::new();
    let link = fx.dir.path().join("state-link");
    std::os::unix::fs::symlink(&fx.state, &link).unwrap();

    fx.flow(&fresh_start())
        .env("FLOW_STATE", &link)
        .arg("up")
        .assert()
        .success();

    let line = pane_run_line(&fx.calls());
    let target = fs::canonicalize(&fx.state).unwrap();
    assert!(
        line.starts_with(&format!("env 'FLOW_STATE={}' ", target.display())),
        "{line}"
    );
    assert!(!line.contains("state-link"), "{line}");
}

#[test]
fn up_resumes_known_session() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let mut script = vec![ok(NO_AGENTS)];
    script.extend(fresh_start_as("s9"));

    let (stdout, stderr) = output_of(fx.flow(&script).arg("up"));

    let line = pane_run_line(&fx.calls());
    assert!(
        line.ends_with(&format!(
            "claude --name flow-supervisor --resume s9 {START_PROMPT}"
        )),
        "{line}"
    );
    assert!(
        stderr.ends_with(&format!(
            "resumed supervisor session s9 on host-a (pane {NEW_PANE})\n"
        )),
        "{stderr}"
    );
    let lock = fx.lock().unwrap();
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
    assert_eq!(lock.herdr.pane_id, NEW_PANE);
    assert!(
        stdout.ends_with(&format!("agent attach {NEW_PANE}\n")),
        "{stdout}"
    );
}

#[test]
fn up_starts_fresh_here_when_the_lock_host_is_gone() {
    let fx = Fixture::new();
    let gone_path = fx.state.join("hosts/env-0002.json");
    let mut gone: Value = serde_json::from_str(&fs::read_to_string(&gone_path).unwrap()).unwrap();
    gone["gone"] = json!(true);
    fs::write(&gone_path, gone.to_string()).unwrap();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));

    let out = fx
        .flow(&fresh_start_as("s9"))
        .arg("up")
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!(
            "supervisor host host-b is gone; starting a new supervisor here\n\
             resumed supervisor session s9 on host-a (pane {NEW_PANE})\n"
        )
    );
    let calls = fx.calls();
    assert!(calls.iter().all(|(host, _)| host == "host-a"), "{calls:?}");
    let line = pane_run_line(&calls);
    assert!(
        line.contains("claude --name flow-supervisor --resume s9"),
        "{line}"
    );
    let lock = fx.lock().unwrap();
    assert_eq!(lock.host_env_id, "env-0001");
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
}

#[test]
fn up_clears_the_watch_lock_before_starting_a_supervisor() {
    let fx = Fixture::new();
    let gone_path = fx.state.join("hosts/env-0002.json");
    let mut gone: Value = serde_json::from_str(&fs::read_to_string(&gone_path).unwrap()).unwrap();
    gone["gone"] = json!(true);
    fs::write(&gone_path, gone.to_string()).unwrap();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, None);
    fx.write_watch_lock("env-0002");

    let out = fx
        .flow(&fresh_start())
        .arg("up")
        .assert()
        .success()
        .get_output()
        .clone();

    assert!(!fx.state.join("supervisor/watch.lock").exists());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("removed state/supervisor/watch.lock"),
        "{stderr}"
    );
    assert!(
        stderr.ends_with(&format!(
            "started a fresh supervisor on host-a (pane {NEW_PANE})\n"
        )),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 3, "{stderr}");
}

#[test]
fn up_attaches_to_the_supervisor_where_herdr_moved_it() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let moved = lock_pane_agent("/workspaces/flow", Some("s9"))
        .replace(LIVE_PANE, "w4:p2")
        .replace(r#""workspace_id":"w1""#, r#""workspace_id":"w4""#);

    let stdout = stdout_of(fx.flow(&[ok(&moved)]).arg("up"));

    assert_eq!(fx.calls().len(), 1);
    assert_eq!(stdout, "attach: herdr --session flow agent attach w4:p2\n");
    let lock = fx.lock().unwrap();
    assert_eq!(lock.herdr.pane_id, "w4:p2");
    assert_eq!(lock.herdr.workspace_id, "w4");
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
}
#[test]
fn up_attaches_to_a_live_supervisor_in_the_session_its_lock_records() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let lock_path = fx.state.join("supervisor/lock.json");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["herdr"]["session"] = json!("work");
    fs::write(&lock_path, lock.to_string()).unwrap();

    let stdout = stdout_of(fx.flow(&supervisor_live()).arg("up"));

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].1[..5],
        ["herdr", "--session", "work", "agent", "list"]
    );
    assert_eq!(
        stdout,
        format!("attach: herdr --session work agent attach {LIVE_PANE}\n")
    );
}

#[test]
fn up_attaches_when_lock_live() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    fx.write_watch_lock("env-0001");
    let before = fs::read_to_string(fx.state.join("supervisor/lock.json")).unwrap();

    let (stdout, stderr) = output_of(fx.flow(&supervisor_live()).arg("up"));

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        !has_subcommand(&calls, &["workspace", "create"]),
        "{calls:?}"
    );
    assert_eq!(
        stdout,
        format!("attach: herdr --session flow agent attach {LIVE_PANE}\n")
    );
    assert_eq!(
        stderr,
        format!("attaching to the supervisor on host-a (pane {LIVE_PANE})\n")
    );
    assert_eq!(
        fs::read_to_string(fx.state.join("supervisor/lock.json")).unwrap(),
        before
    );
    assert!(
        fx.state.join("supervisor/watch.lock/owner.json").exists(),
        "a live supervisor's watcher keeps its lock"
    );
    assert!(!fx.state.join("supervisor/watch-state.json").exists());
}

#[test]
fn up_starts_fresh_beside_another_agent_and_touches_nothing_of_it() {
    for agents in [
        lock_pane_agent("/workspaces/flow", Some("s-hand")),
        lock_pane_agent("/workspaces/flow", None).replace(LIVE_PANE, "w1:p2"),
        NO_AGENTS.to_owned(),
    ] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
        let mut script = vec![ok(&agents)];
        script.extend(fresh_start_as("s9"));

        let stdout = stdout_of(fx.flow(&script).arg("up"));

        let calls = fx.calls();
        assert!(
            has_subcommand(&calls, &["workspace", "create"]),
            "{agents}: {calls:?}"
        );
        assert!(!has_subcommand(&calls, &["close"]), "{agents}: {calls:?}");
        assert_eq!(fx.lock().unwrap().herdr.pane_id, NEW_PANE);
        assert!(
            stdout.ends_with(&format!("agent attach {NEW_PANE}\n")),
            "{stdout}"
        );
    }
}
/// An agent list with one agent, in `LIVE_PANE` of workspace `w1`, running in `cwd`, herdr
/// reporting Claude Code session `session` for it.
fn lock_pane_agent(cwd: &str, session: Option<&str>) -> String {
    let mut agent = json!({
        "agent": "claude", "agent_status": "idle", "cwd": cwd, "pane_id": LIVE_PANE,
        "terminal_title_stripped": "Read the docs", "workspace_id": "w1",
    });
    if let Some(session) = session {
        agent["agent_session"] =
            json!({"agent": "claude", "kind": "id", "source": "herdr:claude", "value": session});
    }
    json!({"id": "cli:agent:list", "result": {"type": "agent_list", "agents": [agent]}}).to_string()
}

#[test]
fn up_refuses_beside_an_agent_it_cannot_tell_from_the_supervisor() {
    for session in [Some("s9"), None] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", LIVE_PANE, session);
        let before = fs::read_to_string(fx.state.join("supervisor/lock.json")).unwrap();
        let mut script = vec![ok(&lock_pane_agent("/workspaces/flow", None))];
        script.extend(fresh_start());

        let out = fx
            .flow(&script)
            .arg("up")
            .assert()
            .code(1)
            .get_output()
            .clone();

        assert_eq!(
            String::from_utf8(out.stderr).unwrap(),
            "pane w1:p1 on host-a holds an agent herdr reports no Claude Code session for, so \
             flow cannot tell it from the supervisor. flow up started nothing, since a second \
             supervisor would resume the same session. If it is your own session, `flow down \
             --forget` marks the supervisor stopped and leaves the pane alone; if it is the \
             supervisor, exit it and run `flow down`; then `flow up`.\n",
            "{session:?}"
        );
        assert!(out.stdout.is_empty());
        assert_eq!(fx.calls().len(), 1);
        assert_eq!(
            fs::read_to_string(fx.state.join("supervisor/lock.json")).unwrap(),
            before
        );
    }
}
#[test]
fn up_starts_fresh_beside_an_open_tasks_worker_in_the_lock_pane() {
    for (recorded, reported) in [(true, Some("s-worker")), (false, None)] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
        create_task_with_pane(&fx)
            .set_status(TASK, |status| {
                if recorded {
                    status.claude_session = Some(ClaudeSession {
                        session_id: "s-worker".to_owned(),
                        transcript_path: PathBuf::from("/home/dev/.claude/projects/x/y.jsonl"),
                    });
                } else if let Some(target) = &mut status.herdr {
                    target.host = "host-a".to_owned();
                    target.workspace_id = "w1".to_owned();
                    target.pane_id = LIVE_PANE.to_owned();
                }
            })
            .unwrap();
        let mut script = vec![ok(&lock_pane_agent("/workspaces/flow", reported))];
        script.extend(fresh_start_as("s9"));

        let stdout = stdout_of(fx.flow(&script).arg("up"));

        let calls = fx.calls();
        assert!(
            has_subcommand(&calls, &["workspace", "create"]),
            "{reported:?}: {calls:?}"
        );
        assert!(!has_subcommand(&calls, &["close"]), "{calls:?}");
        assert!(
            stdout.ends_with(&format!("agent attach {NEW_PANE}\n")),
            "{stdout}"
        );
        let watch_state = fx.state.join("supervisor/watch-state.json");
        let seeded: Value =
            serde_json::from_str(&fs::read_to_string(watch_state).unwrap()).unwrap();
        assert_eq!(seeded["seen"][TASK]["state"], "pending", "{seeded}");
    }
}
#[test]
fn up_starts_fresh_session_when_resume_fails() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let mut script = vec![
        ok(NO_AGENTS),
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(AGENT_WAIT_TIMEOUT),
        ok("$ claude --resume s9\nNo conversation found with session ID: s9\n$ \n"),
        ok(WORKSPACE_CLOSED),
    ];
    script.extend(fresh_start().into_iter().skip(1));
    // herdr's `agent read` fails once Claude Code has exited, so the pane must be read as a
    // terminal.
    script.insert(0, agent_read_fails());

    let out = fx
        .flow(&script)
        .arg("up")
        .assert()
        .success()
        .get_output()
        .clone();

    let calls = fx.calls();
    let lines = pane_run_lines(&calls);
    assert_eq!(lines.len(), 2, "{calls:?}");
    assert!(
        lines[0].ends_with(&format!("--resume s9 {START_PROMPT}")),
        "{}",
        lines[0]
    );
    assert!(!lines[1].contains("--resume"), "{}", lines[1]);
    assert!(lines[1].ends_with(START_PROMPT), "{}", lines[1]);
    assert!(
        has_subcommand(&calls, &["workspace", "close", "w3"]),
        "{calls:?}"
    );
    assert!(
        has_subcommand(&calls, &["pane", "read", NEW_PANE]),
        "{calls:?}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("resume of session s9 failed; starting a fresh supervisor session"),
        "{stderr}"
    );
    assert!(
        stderr.ends_with(&format!(
            "started a fresh supervisor on host-a (pane {NEW_PANE})\n"
        )),
        "{stderr}"
    );
    let lock = fx.lock().unwrap();
    assert_eq!(lock.claude_session.as_deref(), Some("s1"));
    assert_eq!(lock.herdr.pane_id, NEW_PANE);
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .ends_with(&format!("agent attach {NEW_PANE}\n"))
    );
}

#[test]
fn up_starts_fresh_within_one_poll_when_claude_cannot_find_the_session() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let not_found = r#"{"error":{"code":"agent_not_found","message":"agent target w3:p1 not found"},"id":"cli:agent:wait"}"#;
    let mut script = vec![
        ok(NO_AGENTS),
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        failed(not_found),
        ok("$ claude --resume s9\nNo conversation found with session ID: s9\n$ \n"),
        ok(WORKSPACE_CLOSED),
    ];
    script.extend(fresh_start().into_iter().skip(1));

    let started = std::time::Instant::now();
    let out = fx
        .flow(&script)
        .env("FLOW_HANDSHAKE_POLL_MS", "60000")
        .arg("up")
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .get_output()
        .clone();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "no poll interval passed: {:?}",
        started.elapsed()
    );
    let calls = fx.calls();
    let lines = pane_run_lines(&calls);
    assert_eq!(lines.len(), 2, "{calls:?}");
    assert!(!lines[1].contains("--resume"), "{}", lines[1]);
    assert!(
        has_subcommand(&calls, &["workspace", "close", "w3"]),
        "{calls:?}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("resume of session s9 failed; starting a fresh supervisor session"),
        "{stderr}"
    );
    assert_eq!(fx.lock().unwrap().claude_session.as_deref(), Some("s1"));
}

#[test]
fn up_keeps_the_session_when_a_resume_fails_for_another_reason() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let script = vec![
        ok(NO_AGENTS),
        ok(SERVER_RUNNING),
        ok(WORKSPACE_CREATE),
        ok(PANE_RUN_OK),
        agent_read_fails(),
        failed(AGENT_WAIT_TIMEOUT),
        ok("$ claude --resume s9\n Loading conversation…\n"),
    ];

    let out = fx
        .flow(&script)
        .arg("up")
        .assert()
        .code(1)
        .get_output()
        .clone();

    let calls = fx.calls();
    assert_eq!(pane_run_lines(&calls).len(), 1, "{calls:?}");
    assert!(
        !has_subcommand(&calls, &["workspace", "close"]),
        "{calls:?}"
    );
    assert!(
        has_subcommand(&calls, &["pane", "read", NEW_PANE]),
        "{calls:?}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("did not start within"), "{stderr}");
    assert!(stderr.contains("left open for inspection"), "{stderr}");
    assert!(!stderr.contains("starting a fresh"), "{stderr}");
    let lock = fx.lock().unwrap();
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
    assert_eq!(lock.herdr.pane_id, NEW_PANE);
}

#[test]
fn up_attaches_over_ssh_to_a_live_remote_supervisor() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));

    let stdout = stdout_of(fx.flow(&supervisor_live()).arg("up"));

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].0, "host-b");
    assert!(stdout.starts_with("attach: ssh -t "), "{stdout}");
    assert!(stdout.contains(" host-b.example zsh -lc "), "{stdout}");
    assert!(stdout.contains("agent"), "{stdout}");
}

#[test]
fn up_refuses_when_lock_host_unreachable() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));
    let unreachable = json!({"status": 255, "stdout": "", "stderr": "ssh: connect timed out"});

    let out = fx
        .flow(&[unreachable])
        .arg("up")
        .assert()
        .failure()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("supervisor host host-b unreachable"),
        "{stderr}"
    );
    assert!(stderr.contains("ssh: connect timed out"), "{stderr}");
    assert!(
        stderr.contains(
            "running or attaching to the supervisor from this machine needs the operator's Ona \
             login (`gitpod login`) and `gitpod environment ssh-config` here"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("lock.json"),
        "removing the lock would lose the session to resume: {stderr}"
    );
    assert!(!has_subcommand(&fx.calls(), &["workspace", "create"]));
    assert_eq!(fx.lock().unwrap().herdr.host, "host-b");
}

/// What `flow up` prints in place of attaching when stdout is not a terminal.
const NO_TERMINAL: &str = "stdout is not a terminal, so flow did not attach; to attach, run: ";

#[test]
fn up_without_a_terminal_prints_the_command_to_run() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));

    let (stdout, stderr) = output_of(
        fx.flow(&supervisor_live())
            .env_remove("FLOW_FAKE_TTY")
            .arg("up"),
    );

    assert_eq!(
        stdout,
        format!("{NO_TERMINAL}herdr --session flow agent attach {LIVE_PANE}\n")
    );
    assert_eq!(
        stderr,
        format!("attaching to the supervisor on host-a (pane {LIVE_PANE})\n")
    );
    assert_eq!(fx.calls().len(), 1, "only the liveness check runs");
}

#[test]
fn up_without_a_terminal_still_starts_the_supervisor() {
    let fx = Fixture::new();

    let (stdout, stderr) = output_of(
        fx.flow(&fresh_start())
            .env_remove("FLOW_FAKE_TTY")
            .arg("up"),
    );

    assert_eq!(
        stdout,
        format!("{NO_TERMINAL}herdr --session default agent attach {NEW_PANE}\n")
    );
    assert_eq!(
        stderr,
        format!("started a fresh supervisor on host-a (pane {NEW_PANE})\n")
    );
    assert_eq!(fx.lock().unwrap().herdr.pane_id, NEW_PANE);
}

#[test]
fn up_without_a_terminal_quotes_the_remote_command_as_one_word() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));

    let stdout = stdout_of(
        fx.flow(&supervisor_live())
            .env_remove("FLOW_FAKE_TTY")
            .arg("up"),
    );

    assert!(
        stdout.starts_with(&format!("{NO_TERMINAL}ssh -t ")),
        "{stdout}"
    );
    assert!(stdout.contains(" host-b.example 'zsh -lc "), "{stdout}");
}

#[test]
fn up_without_a_terminal_creates_the_control_dir_its_ssh_command_names() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));
    let runtime = fx.dir.path().join("runtime");
    fs::create_dir(&runtime).unwrap();

    let stdout = stdout_of(
        fx.flow(&supervisor_live())
            .env("XDG_RUNTIME_DIR", &runtime)
            .env_remove("FLOW_FAKE_TTY")
            .arg("up"),
    );

    let control_dir = runtime.join("flow");
    assert!(
        stdout.contains(&format!("ControlPath={}/ssh-%C", control_dir.display())),
        "{stdout}"
    );
    assert!(control_dir.is_dir(), "{} is missing", control_dir.display());
}

#[test]
fn attach_is_not_a_command() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, None);

    fx.flow(&[])
        .arg("attach")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unrecognized subcommand"));
}

fn create_task_with_pane(fx: &Fixture) -> Store {
    let store = Store::open(&fx.state_dir());
    store
        .create_task(
            TaskRecord {
                id: TASK.to_owned(),
                parent: None,
                title: "Add retry to the example client".to_owned(),
                host: "host-b".to_owned(),
                cwd: PathBuf::from("/workspaces/example-repo.wt/retry"),
                repo: Some("example-repo".to_owned()),
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
                origin: Origin::Dispatched,
            },
            "brief",
        )
        .unwrap();
    store
        .set_status(TASK, |status| {
            status.herdr = Some(HerdrTarget {
                host: "host-b".to_owned(),
                workspace_id: "w2".to_owned(),
                pane_id: "w2:p1".to_owned(),
                agent: "claude".to_owned(),
                session: "flow".to_owned(),
            });
        })
        .unwrap();
    store
}

#[test]
fn steer_prompts_pane_and_logs_event() {
    let fx = Fixture::new();
    let store = create_task_with_pane(&fx);
    let prompted = AGENT_GET.replace("agent_info", "agent_prompted");

    fx.flow(&[ok(&prompted)])
        .args(["steer", TASK, "Use the existing retry helper."])
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (host, argv) = &calls[0];
    assert_eq!(host, "host-b");
    assert_eq!(
        argv[..6],
        ["herdr", "--session", "flow", "agent", "prompt", "w2:p1"]
    );
    assert_eq!(argv[6], "Use the existing retry helper.");
    assert_eq!(
        argv[7..],
        ["--wait", "--until", "working", "--until", "blocked"]
    );
    let ev = events::read_all(&store.task_dir(TASK).unwrap())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(ev.kind, "steer");
    assert_eq!(ev.source, "supervisor");
    assert_eq!(ev.data, json!({"text": "Use the existing retry helper."}));
}

#[test]
fn steer_prompts_an_adopted_task_in_its_own_herdr_session() {
    let fx = Fixture::new();
    let store = create_task_with_pane(&fx);
    store
        .set_status(TASK, |status| {
            if let Some(target) = &mut status.herdr {
                target.session = "default".to_owned();
            }
        })
        .unwrap();
    let prompted = AGENT_GET.replace("agent_info", "agent_prompted");

    fx.flow(&[ok(&prompted)])
        .args(["steer", TASK, "Use the existing retry helper."])
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].1[..6],
        ["herdr", "--session", "default", "agent", "prompt", "w2:p1"]
    );
}

#[test]
fn steer_hands_an_escalated_task_back_to_its_worker() {
    let prompted = AGENT_GET.replace("agent_info", "agent_prompted");
    let cases = [
        (
            State::NeedsDecision,
            Some(HoldOn::Supervisor),
            State::Working,
            None,
        ),
        (
            State::Blocked,
            Some(HoldOn::Supervisor),
            State::Working,
            None,
        ),
        (
            State::NeedsDecision,
            Some(HoldOn::User),
            State::Working,
            Some(HoldOn::User),
        ),
        (State::Working, None, State::Working, None),
        (State::Pending, None, State::Pending, None),
    ];
    for (before, hold, after, hold_after) in cases {
        let fx = Fixture::new();
        let store = create_task_with_pane(&fx);
        store
            .set_status(TASK, |status| {
                status.state = before;
                status.hold = hold.map(|on| Hold {
                    on,
                    what: "Retry forever?".to_owned(),
                    since: Timestamp::now(),
                });
            })
            .unwrap();

        fx.flow(&[ok(&prompted)])
            .args(["steer", TASK, "Give up after five tries."])
            .assert()
            .success();

        let (_, status) = store.task(TASK).unwrap();
        assert_eq!(status.state, after, "steering a {before} task");
        assert_eq!(
            status.hold.map(|hold| hold.on),
            hold_after,
            "steering a {before} task"
        );
    }
}

#[test]
fn steer_refuses_task_without_pane() {
    let fx = Fixture::new();
    let store = create_task_with_pane(&fx);
    store
        .set_status(TASK, |status| status.herdr = None)
        .unwrap();

    fx.flow(&[])
        .args(["steer", TASK, "hello"])
        .assert()
        .failure()
        .stderr(format!("task {TASK} has no herdr target\n"));
    assert!(fx.calls().is_empty());
}

fn hook_supervisor(fx: &Fixture, stdin: &str, marked: bool) -> assert_cmd::assert::Assert {
    let mut cmd = fx.flow(&[]);
    if marked {
        cmd.env(supervisor::SUPERVISOR_ENV, "1");
    }
    cmd.args(["hook", "supervisor"]).write_stdin(stdin).assert()
}

#[test]
fn hook_supervisor_records_session() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s0"));

    hook_supervisor(&fx, SESSION_START, true)
        .success()
        .stdout("");

    let lock = fx.lock().unwrap();
    assert_eq!(lock.claude_session.as_deref(), Some("s1"));
    assert_eq!(lock.herdr.pane_id, LIVE_PANE);
}

#[test]
fn hook_supervisor_ignores_other_sessions_and_events() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s0"));
    let stop = r#"{"hook_event_name":"Stop","session_id":"s2","transcript_path":"/t/s2.jsonl","cwd":"/workspaces/flow"}"#;

    hook_supervisor(&fx, SESSION_START, false)
        .success()
        .stdout("");
    hook_supervisor(&fx, stop, true).success().stdout("");

    assert_eq!(fx.lock().unwrap().claude_session.as_deref(), Some("s0"));
}

#[test]
fn hook_supervisor_without_lock_is_silent() {
    let fx = Fixture::new();

    hook_supervisor(&fx, SESSION_START, true)
        .success()
        .stdout("")
        .stderr("");

    assert!(fx.lock().is_none());
    assert!(!Path::new(&fx.state.join("supervisor/lock.json")).exists());
}

#[test]
fn hook_supervisor_ignores_bad_input_and_corrupt_lock() {
    let fx = Fixture::new();

    for stdin in ["", "not json", r#"{"hook_event_name":"SessionStart"}"#] {
        hook_supervisor(&fx, stdin, true)
            .code(0)
            .stdout("")
            .stderr("");
    }

    fs::create_dir_all(fx.state.join("supervisor")).unwrap();
    fs::write(fx.state.join("supervisor/lock.json"), "{corrupt").unwrap();
    hook_supervisor(&fx, SESSION_START, true)
        .code(0)
        .stdout("")
        .stderr("");
    assert_eq!(
        fs::read_to_string(fx.state.join("supervisor/lock.json")).unwrap(),
        "{corrupt"
    );
}

#[test]
fn hook_supervisor_outside_supervisor_needs_no_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("flow")
        .expect("flow binary is built")
        .env("FLOW_STATE", dir.path().join("missing"))
        .env_remove(supervisor::SUPERVISOR_ENV)
        .args(["hook", "supervisor"])
        .write_stdin(SESSION_START)
        .assert()
        .code(0)
        .stdout("")
        .stderr("");
}

#[test]
fn up_after_down_resumes_the_stopped_supervisor_here_without_probing_its_pane() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, Some("s9"));
    let path = fx.state.join("supervisor/lock.json");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    lock["stopped"] = json!("2026-10-27T08:00:00Z");
    fs::write(&path, lock.to_string()).unwrap();

    let (_, stderr) = output_of(fx.flow(&fresh_start_as("s9")).arg("up"));

    let calls = fx.calls();
    assert!(calls.iter().all(|(host, _)| host == "host-a"), "{calls:?}");
    let line = pane_run_line(&calls);
    assert!(
        line.contains("claude --name flow-supervisor --resume s9"),
        "{line}"
    );
    assert!(
        stderr.contains(&format!(
            "resumed supervisor session s9 on host-a (pane {NEW_PANE})"
        )),
        "{stderr}"
    );
    let lock = fx.lock().unwrap();
    assert_eq!(lock.stopped, None);
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
    assert_eq!(lock.host_env_id, "env-0001");
}
