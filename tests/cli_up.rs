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
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
/// A pane `agent_list.json` lists, and the one the locks written here record.
const LIVE_PANE: &str = "w1:p1";
/// The pane every `flow up` here is typed in, in workspace `w9`.
const HERE_PANE: &str = "w9:p9";
/// The first prompt every supervisor start passes Claude Code.
const START_PROMPT: &str = "Run the session start procedure in CLAUDE.md.";
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

    /// `flow` on host-a over this state dir, answering runner calls from `script` in order, run
    /// from a pane of herdr's `default` session.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let config_home = self.dir.path().join("config");
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env("HERDR_ENV", "1")
            .env("HERDR_PANE_ID", HERE_PANE)
            .env("HERDR_WORKSPACE_ID", "w9")
            .env("HERDR_TAB_ID", "w9:t1")
            .env("XDG_CONFIG_HOME", &config_home)
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("claude"))
            .env("HERDR_SOCKET_PATH", config_home.join("herdr/herdr.sock"))
            .env_remove(supervisor::SUPERVISOR_ENV);
        cmd
    }

    /// `flow`, run from a pane of herdr session `session`.
    fn flow_in(&self, session: &str, script: &[Value]) -> Command {
        let socket = self
            .dir
            .path()
            .join(format!("config/herdr/sessions/{session}/herdr.sock"));
        let mut cmd = self.flow(script);
        cmd.env("HERDR_SOCKET_PATH", socket);
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

    /// A supervisor's lock in workspace `w1` of herdr's `default` session.
    fn write_lock(&self, env_id: &str, host: &str, pane: &str, session: Option<&str>) {
        let dir = self.state.join("supervisor");
        fs::create_dir_all(&dir).unwrap();
        let lock = json!({
            "host_env_id": env_id,
            "herdr": {"host": host, "workspace_id": "w1", "pane_id": pane, "agent": "claude",
                "session": "default"},
            "claude_session": session,
            "since": "2026-10-26T12:00:00Z",
        });
        fs::write(dir.join("lock.json"), lock.to_string()).unwrap();
    }

    /// Changes the lock's herdr session to `session`.
    fn lock_in_session(&self, session: &str) {
        let path = self.state.join("supervisor/lock.json");
        let mut lock: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        lock["herdr"]["session"] = json!(session);
        fs::write(&path, lock.to_string()).unwrap();
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

    fn lock_text(&self) -> String {
        fs::read_to_string(self.state.join("supervisor/lock.json")).unwrap()
    }

    /// Writes the transcript Claude Code keeps for supervisor session `session`.
    fn write_transcript(&self, session: &str) {
        let dir = self.dir.path().join("claude/projects/-workspaces-flow");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{session}.jsonl")), "{}\n").unwrap();
    }

    /// The `exec:` line `flow up` prints in place of starting the supervisor under the fake
    /// runner, with `extra` before the prompt.
    fn exec_line(&self, extra: &str) -> String {
        let state = fs::canonicalize(&self.state).unwrap();
        format!(
            "exec: env FLOW_SUPERVISOR=1 FLOW_STATE={} claude --name flow-supervisor {extra}{START_PROMPT}\n",
            state.display()
        )
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
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

/// herdr's workspace list with the lock's workspace `w1` labelled `label`.
fn lock_workspace(label: &str) -> Value {
    let list = json!({"id": "cli:workspace:list", "result": {"type": "workspace_list",
        "workspaces": [{"workspace_id": "w1", "label": label}]}});
    ok(&list.to_string())
}

/// The stdout and stderr of `cmd`, which must succeed.
fn output_of(cmd: &mut Command) -> (String, String) {
    let out = cmd.assert().success().get_output().clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The stderr of `cmd`, which must exit 1 printing nothing on stdout.
fn refusal_of(cmd: &mut Command) -> String {
    let out = cmd.assert().code(1).get_output().clone();
    assert!(out.stdout.is_empty());
    String::from_utf8(out.stderr).unwrap()
}

/// The lock `flow up` takes for `HERE_PANE` in tab `w9:t1` of herdr session `session` on
/// `host-a`.
fn here(session: &str) -> HerdrTarget {
    HerdrTarget {
        host: "host-a".to_owned(),
        workspace_id: "w9".to_owned(),
        tab_id: "w9:t1".to_owned(),
        pane_id: HERE_PANE.to_owned(),
        agent: "claude".to_owned(),
        session: session.to_owned(),
    }
}

#[test]
fn up_refuses_outside_a_herdr_pane() {
    for unset in ["HERDR_ENV", "HERDR_PANE_ID", "HERDR_WORKSPACE_ID"] {
        let fx = Fixture::new();

        let stderr = refusal_of(fx.flow(&[]).env_remove(unset).arg("up"));

        assert_eq!(
            stderr, "flow up runs from a shell pane inside herdr\n",
            "{unset}"
        );
        assert!(fx.calls().is_empty());
        assert!(fx.lock().is_none());
    }
}

#[test]
fn up_refuses_a_socket_that_names_no_herdr_session() {
    let fx = Fixture::new();

    let stderr = refusal_of(
        fx.flow(&[])
            .env("HERDR_SOCKET_PATH", "/run/elsewhere/herdr.sock")
            .arg("up"),
    );

    assert!(
        stderr.contains(
            "HERDR_SOCKET_PATH /run/elsewhere/herdr.sock is neither herdr's main \
             socket nor a named session's"
        ),
        "{stderr}"
    );
    assert!(fx.calls().is_empty());
}

#[test]
fn up_runs_the_supervisor_in_its_own_pane_when_none_was_started() {
    let fx = Fixture::new();

    let (stdout, stderr) = output_of(fx.flow(&[]).arg("up"));

    assert_eq!(
        stdout,
        fx.exec_line(""),
        "the marker and state path keep the supervisor's hooks live; the prompt runs a first turn"
    );
    assert_eq!(
        stderr,
        format!("starting a fresh supervisor in pane {HERE_PANE} on host-a\n")
    );
    assert!(fx.calls().is_empty(), "no herdr call: {:?}", fx.calls());
    let lock = fx.lock().expect("lock is written");
    assert_eq!(lock.host_env_id, "env-0001");
    assert_eq!(lock.herdr, here("default"));
    assert_eq!(lock.claude_session, None);
    assert!(fx.state.join("supervisor/watch-state.json").exists());
}

#[test]
fn up_records_the_herdr_session_it_runs_in() {
    let fx = Fixture::new();

    fx.flow_in("work", &[]).arg("up").assert().success();

    assert_eq!(fx.lock().unwrap().herdr, here("work"));
}

#[test]
fn up_launch_line_names_the_state_dir_behind_a_symlink() {
    let fx = Fixture::new();
    let link = fx.dir.path().join("state-link");
    std::os::unix::fs::symlink(&fx.state, &link).unwrap();

    let (stdout, _) = output_of(fx.flow(&[]).env("FLOW_STATE", &link).arg("up"));

    assert_eq!(stdout, fx.exec_line(""));
    assert!(!stdout.contains("state-link"), "{stdout}");
}

#[test]
fn up_resumes_the_recorded_session_while_its_transcript_exists() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    fx.write_transcript("s9");

    let (stdout, stderr) = output_of(fx.flow(&[ok(NO_AGENTS)]).arg("up"));

    assert_eq!(stdout, fx.exec_line("--resume s9 "));
    assert_eq!(
        stderr,
        format!("resuming supervisor session s9 in pane {HERE_PANE} on host-a\n")
    );
    let lock = fx.lock().unwrap();
    assert_eq!(lock.herdr, here("default"));
    assert_eq!(lock.claude_session.as_deref(), Some("s9"));
}

#[test]
fn up_starts_a_fresh_session_when_the_recorded_one_has_no_transcript() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));

    let (stdout, stderr) = output_of(fx.flow(&[ok(NO_AGENTS)]).arg("up"));

    assert_eq!(stdout, fx.exec_line(""));
    let transcript = fx
        .dir
        .path()
        .join("claude/projects/-workspaces-flow/s9.jsonl");
    assert_eq!(
        stderr,
        format!(
            "no transcript of supervisor session s9 at {}; starting a fresh supervisor \
             session\nstarting a fresh supervisor in pane {HERE_PANE} on host-a\n",
            transcript.display()
        )
    );
    assert_eq!(fx.lock().unwrap().claude_session, None);
}

#[test]
fn up_reports_the_supervisor_herdr_shows_running_here_and_starts_nothing() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    fx.lock_in_session("flow");
    let before = fx.lock_text();
    let script = [
        ok(&lock_pane_agent("/workspaces/flow", Some("s9"))),
        lock_workspace("Notes"),
    ];

    let (stdout, stderr) = output_of(fx.flow(&script).arg("up"));

    assert_eq!(
        stdout,
        format!("the supervisor is running in pane {LIVE_PANE} (workspace Notes)\n")
    );
    assert_eq!(stderr, "");
    assert_eq!(fx.lock_text(), before);
    let calls = fx.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for (host, argv) in &calls {
        assert_eq!(host, "host-a");
        assert_eq!(argv[..3], ["herdr", "--session", "flow"], "{argv:?}");
    }
    assert!(!fx.state.join("supervisor/watch-state.json").exists());
}

#[test]
fn up_starts_here_when_herdr_shows_no_agent_reporting_the_locks_session() {
    let listings = [
        lock_pane_agent("/workspaces/flow", Some("s-hand")),
        lock_pane_agent("/workspaces/flow", None),
        NO_AGENTS.to_owned(),
    ];
    for listing in listings {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
        fx.write_transcript("s9");

        let (stdout, _) = output_of(fx.flow(&[ok(&listing)]).arg("up"));

        assert_eq!(stdout, fx.exec_line("--resume s9 "), "{listing}");
        assert_eq!(fx.calls().len(), 1, "{listing}");
        assert_eq!(fx.lock().unwrap().herdr, here("default"));
    }
}

#[test]
fn up_starts_here_when_no_herdr_server_runs_in_the_locks_session() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    fx.lock_in_session("flow");
    let not_running = json!({"status": 1, "stdout": "",
        "stderr": include_str!("fixtures/herdr/agent_list_server_not_running.json")});

    let (stdout, _) = output_of(fx.flow(&[not_running]).arg("up"));

    assert_eq!(stdout, fx.exec_line(""));
    assert_eq!(fx.lock().unwrap().herdr, here("default"));
}

#[test]
fn up_warns_of_a_supervisor_recorded_on_another_host_and_starts_here_without_asking_it() {
    for (env_id, host) in [("env-0002", "host-b"), ("env-0009", "host-z")] {
        let fx = Fixture::new();
        fx.write_lock(env_id, host, LIVE_PANE, Some("s9"));
        fx.write_transcript("s9");

        let (stdout, stderr) = output_of(fx.flow(&[]).arg("up"));

        let since =
            flow::render::relative("2026-10-26T12:00:00Z".parse().unwrap(), Timestamp::now());
        assert_eq!(
            stderr,
            format!(
                "the previous supervisor was recorded on {host} since {since} ago; if it is \
                 still running there, exit it\n\
                 resuming supervisor session s9 in pane {HERE_PANE} on host-a\n"
            )
        );
        assert_eq!(stdout, fx.exec_line("--resume s9 "));
        assert!(fx.calls().is_empty(), "{:?}", fx.calls());
        let lock = fx.lock().unwrap();
        assert_eq!(lock.host_env_id, "env-0001");
        assert_eq!(lock.herdr, here("default"));
    }
}

#[test]
fn up_starts_here_without_a_warning_when_the_lock_host_is_gone() {
    let fx = Fixture::new();
    let gone_path = fx.state.join("hosts/env-0002.json");
    let mut gone: Value = serde_json::from_str(&fs::read_to_string(&gone_path).unwrap()).unwrap();
    gone["gone"] = json!(true);
    fs::write(&gone_path, gone.to_string()).unwrap();
    fx.write_lock("env-0002", "host-b", LIVE_PANE, None);

    let (stdout, stderr) = output_of(fx.flow(&[]).arg("up"));

    assert_eq!(
        stderr,
        format!("starting a fresh supervisor in pane {HERE_PANE} on host-a\n")
    );
    assert_eq!(stdout, fx.exec_line(""));
    assert!(fx.calls().is_empty());
}

#[test]
fn up_reads_a_lock_an_earlier_version_marked_stopped() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", LIVE_PANE, Some("s9"));
    let path = fx.state.join("supervisor/lock.json");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    lock["stopped"] = json!("2026-10-27T08:00:00Z");
    fs::write(&path, lock.to_string()).unwrap();
    fx.write_transcript("s9");

    let (stdout, _) = output_of(fx.flow(&[ok(NO_AGENTS)]).arg("up"));

    assert_eq!(stdout, fx.exec_line("--resume s9 "));
    assert!(!fx.lock_text().contains("stopped"), "{}", fx.lock_text());
}

#[test]
fn up_clears_the_watch_lock_before_starting_a_supervisor() {
    let fx = Fixture::new();
    fx.write_watch_lock("env-0002");

    let (_, stderr) = output_of(fx.flow(&[]).arg("up"));

    assert!(!fx.state.join("supervisor/watch.lock").exists());
    assert_eq!(
        stderr,
        format!(
            "removed state/supervisor/watch.lock left by the previous supervisor\n\
             starting a fresh supervisor in pane {HERE_PANE} on host-a\n"
        )
    );
}

#[test]
fn attach_and_down_are_not_commands() {
    for command in ["attach", "down"] {
        let fx = Fixture::new();

        fx.flow(&[])
            .arg(command)
            .assert()
            .code(2)
            .stderr(predicates::str::contains("unrecognized subcommand"));
    }
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
                tab_id: "w2:t1".to_owned(),
                pane_id: "w2:p1".to_owned(),
                agent: "claude".to_owned(),
                session: "flow".to_owned(),
            });
        })
        .unwrap();
    store
}

/// herdr's agent list with one agent in the task's pane `w2:p1`, reporting `session` when given.
fn task_pane_agent(session: Option<&str>) -> Value {
    ok(
        &lock_pane_agent("/workspaces/example-repo.wt/retry", session)
            .replace(LIVE_PANE, "w2:p1")
            .replace(
                r#""workspace_id":"w1""#,
                r#""workspace_id":"w2","tab_id":"w2:t1""#,
            ),
    )
}

/// herdr's tab list with the task's tab `w2:t1` under the label dispatch gives it.
fn task_tab() -> Value {
    let tab = json!({"tab_id": "w2:t1", "workspace_id": "w2",
        "label": format!("Add retry to the example client · {TASK}")});
    let list = json!({"id": "cli:tab:list", "result": {"type": "tab_list", "tabs": [tab]}});
    ok(&list.to_string())
}

#[test]
fn steer_prompts_pane_and_logs_event() {
    let fx = Fixture::new();
    let store = create_task_with_pane(&fx);
    let prompted = AGENT_GET.replace("agent_info", "agent_prompted");

    fx.flow(&[task_pane_agent(None), task_tab(), ok(&prompted)])
        .args(["steer", TASK, "Use the existing retry helper."])
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0].1, ["herdr", "--session", "flow", "agent", "list"]);
    assert_eq!(calls[1].1, ["herdr", "--session", "flow", "tab", "list"]);
    let (host, argv) = &calls[2];
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

    fx.flow(&[task_pane_agent(None), task_tab(), ok(&prompted)])
        .args(["steer", TASK, "Use the existing retry helper."])
        .assert()
        .success();

    let calls = fx.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(
        calls[2].1[..6],
        ["herdr", "--session", "default", "agent", "prompt", "w2:p1"]
    );
}

#[test]
fn steer_refuses_a_pane_another_agent_runs_in() {
    let fx = Fixture::new();
    let store = create_task_with_pane(&fx);
    store
        .set_status(TASK, |status| {
            status.claude_session = Some(ClaudeSession {
                session_id: "s-worker".to_owned(),
                transcript_path: PathBuf::from("/home/dev/.claude/projects/x/y.jsonl"),
            });
        })
        .unwrap();
    let events_before = events::read_all(&store.task_dir(TASK).unwrap()).unwrap();

    fx.flow(&[task_pane_agent(Some("s-other"))])
        .args(["steer", TASK, "hello"])
        .assert()
        .code(2)
        .stderr(format!(
            "task {TASK}'s pane w2:p1 on host-b holds another agent than the task's worker, \
             since herdr reuses pane ids; flow sent nothing\n"
        ));

    assert_eq!(fx.calls().len(), 1);
    assert_eq!(
        events::read_all(&store.task_dir(TASK).unwrap()).unwrap(),
        events_before
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

        fx.flow(&[task_pane_agent(None), task_tab(), ok(&prompted)])
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
