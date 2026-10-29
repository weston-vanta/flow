use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::{Harness, Origin, State, TaskRecord};
use jiff::Timestamp;
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
"#;

/// Workspaces `w1` and `w2`, both workers'.
const WORKSPACE_LIST: &str = include_str!("fixtures/herdr/workspace_list.json");
const WORKSPACE_CLOSED: &str = r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#;
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const AGENT_LIST: &str = include_str!("fixtures/herdr/agent_list.json");
const SERVER_NOT_RUNNING: &str = include_str!("fixtures/herdr/agent_list_server_not_running.json");
const UNREACHABLE: &str = "ssh: connect to host host-b.example port 22: Connection timed out";
const EMPTY_WORKSPACES: &str =
    r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[]}}"#;
const NOTE: &str = "1 open tasks keep running; flow up resumes the supervisor\n";
const KEPT: &str =
    "marked the supervisor stopped in state/supervisor/lock.json, keeping session s9 to resume\n";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    /// A state dir with `CONFIG`, cached records of `host-a` (env `env-0001`, this machine) and
    /// `host-b` (env `env-0002`), and two tasks, one of them open.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        let fx = Fixture { dir, state };
        fx.write_host("host-a", "env-0001", false);
        fx.write_host("host-b", "env-0002", false);
        let store = fx.store();
        for (id, state) in [("aaaaaaab", State::Working), ("aaaaaaac", State::Done)] {
            store
                .create_task(
                    TaskRecord {
                        id: id.to_owned(),
                        parent: None,
                        title: "Build feature".to_owned(),
                        host: "host-a".to_owned(),
                        cwd: PathBuf::from("/workspaces/acme"),
                        repo: None,
                        harness: Harness::Claude,
                        created: Timestamp::now(),
                        owner: "supervisor".to_owned(),
                        origin: Origin::Dispatched,
                    },
                    "brief",
                )
                .unwrap();
            store.set_status(id, |status| status.state = state).unwrap();
        }
        fx
    }

    fn store(&self) -> Store {
        Store::open(&StateDir::resolve(self.dir.path()).unwrap())
    }

    fn write_host(&self, name: &str, env_id: &str, gone: bool) {
        let hosts = self.state.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        let record = json!({
            "name": name, "env_id": env_id, "ssh_alias": format!("{name}.example"),
            "gone": gone, "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
    }

    /// The supervisor's lock: in workspace `w1`, pane `w1:p1`, of `session` on the host
    /// `env_id`, or with no session recorded, as locks written before sessions were.
    fn write_lock(&self, env_id: &str, host: &str, session: Option<&str>) {
        let dir = self.state.join("supervisor");
        fs::create_dir_all(&dir).unwrap();
        let mut herdr =
            json!({"host": host, "workspace_id": "w1", "pane_id": "w1:p1", "agent": "claude"});
        if let Some(session) = session {
            herdr["session"] = json!(session);
        }
        let lock = json!({
            "host_env_id": env_id, "herdr": herdr, "claude_session": "s9",
            "since": "2026-10-26T12:00:00Z",
        });
        fs::write(dir.join("lock.json"), lock.to_string()).unwrap();
    }

    /// A fresh `watch.lock` held by process `pid` on the host `env_id`.
    fn write_watch_lock(&self, env_id: &str, pid: u32) {
        let dir = self.watch_lock();
        fs::create_dir_all(&dir).unwrap();
        let owner = json!({"pid": pid, "host": env_id, "started": "2026-10-26T12:00:00Z"});
        fs::write(dir.join("owner.json"), owner.to_string()).unwrap();
    }

    /// Asserts the lock is kept, marked stopped, with its session to resume.
    fn assert_stopped(&self) {
        let lock: Value =
            serde_json::from_str(&fs::read_to_string(self.lock_path()).unwrap()).unwrap();
        assert!(lock["stopped"].is_string(), "{lock}");
        assert_eq!(lock["claude_session"], "s9", "{lock}");
    }

    fn lock_path(&self) -> PathBuf {
        self.state.join("supervisor/lock.json")
    }

    fn watch_lock(&self) -> PathBuf {
        self.state.join("supervisor/watch.lock")
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on `host-a` with no terminal, answering runner calls from `script`. `argvs`
    /// then lists this run's calls only.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let _ = fs::remove_file(self.calls_log());
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env_remove("FLOW_FAKE_TTY")
            .env_remove("FLOW_SUPERVISOR");
        cmd
    }

    fn calls_log(&self) -> PathBuf {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        PathBuf::from(log)
    }

    fn argvs(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.calls_log())
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                serde_json::from_value(call["argv"].clone()).unwrap()
            })
            .collect()
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
}

fn failed(status: i32, stderr: &str) -> Value {
    json!({"status": status, "stdout": "", "stderr": stderr})
}

/// `WORKSPACE_LIST` with `w1` labelled as the supervisor's.
fn supervisor_listed() -> String {
    relabelled("flow-supervisor")
}

/// `WORKSPACE_LIST` with `w1` labelled `label`.
fn relabelled(label: &str) -> String {
    let listed = WORKSPACE_LIST.replacen(
        r#""label":"login-form""#,
        &format!(r#""label":"{label}""#),
        1,
    );
    assert_ne!(listed, WORKSPACE_LIST);
    listed
}

/// An agent list with one agent in the lock's pane `w1:p1`, running in `cwd`, herdr reporting
/// Claude Code session `session` for it.
fn in_lock_pane(cwd: &str, session: Option<&str>) -> String {
    let mut agent = json!({
        "agent": "claude", "agent_status": "idle", "cwd": cwd, "pane_id": "w1:p1",
        "terminal_title_stripped": "flow-supervisor", "workspace_id": "w1",
    });
    if let Some(session) = session {
        agent["agent_session"] =
            json!({"agent": "claude", "kind": "id", "source": "herdr:claude", "value": session});
    }
    json!({"id": "cli:agent:list", "result": {"type": "agent_list", "agents": [agent]}}).to_string()
}

/// The supervisor's agent list: it runs in its pane with the lock's session.
fn supervisor_agent() -> String {
    in_lock_pane("/workspaces/flow", Some("s9"))
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|word| (*word).to_owned()).collect()
}

fn stdout_of(cmd: &mut Command) -> String {
    let out = cmd.assert().success().get_output().clone();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Waits for `child` to exit, failing the test after a few seconds.
fn exited(child: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn down_without_a_lock_has_nothing_to_stop() {
    let fx = Fixture::new();

    let stdout = stdout_of(fx.flow(&[]).arg("down"));

    assert_eq!(stdout, "no supervisor recorded; nothing to stop\n");
    assert!(fx.argvs().is_empty());
}

#[test]
fn down_of_a_stopped_supervisor_has_nothing_to_stop() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));
    stdout_of(fx.flow(&[ok(NO_AGENTS), ok(WORKSPACE_LIST)]).arg("down"));
    let before = fs::read_to_string(fx.lock_path()).unwrap();

    let stdout = stdout_of(fx.flow(&[]).arg("down"));

    assert_eq!(
        stdout,
        "the supervisor is already stopped; nothing to stop\n"
    );
    assert!(fx.argvs().is_empty());
    assert_eq!(fs::read_to_string(fx.lock_path()).unwrap(), before);
}

#[test]
fn down_closes_the_supervisors_workspace_and_marks_the_lock_stopped() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[
            ok(&supervisor_agent()),
            ok(&supervisor_listed()),
            ok(WORKSPACE_CLOSED),
        ])
        .arg("down"),
    );

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's workspace w1 on host-a (herdr session default)\n\
             {KEPT}{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "default", "agent", "list"]),
            words(&["herdr", "--session", "default", "workspace", "list"]),
            words(&["herdr", "--session", "default", "workspace", "close", "w1"]),
        ]
    );
    fx.assert_stopped();
    let (_, status) = fx.store().task("aaaaaaab").unwrap();
    assert_eq!(status.state, State::Working);
}

#[test]
fn down_notes_a_supervisor_workspace_already_gone() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", Some("default"));

    let stdout = stdout_of(fx.flow(&[ok(NO_AGENTS), ok(WORKSPACE_LIST)]).arg("down"));

    assert_eq!(
        stdout,
        format!(
            "the supervisor's pane w1:p1 on host-b is already gone\n\
             {KEPT}{NOTE}"
        )
    );
    assert_eq!(fx.argvs().len(), 2);
    fx.assert_stopped();

    fx.write_lock("env-0002", "host-b", Some("default"));
    let script = [json!({"status": 1, "stdout": "", "stderr": SERVER_NOT_RUNNING})];
    let stdout = stdout_of(fx.flow(&script).arg("down"));
    assert_eq!(fx.argvs().len(), 1);
    assert!(
        stdout.starts_with("the supervisor's pane w1:p1 on host-b is already gone\n"),
        "{stdout}"
    );

    fx.write_lock("env-0002", "host-b", Some("default"));
    fx.write_host("host-b", "env-0002", true);
    let stdout = stdout_of(fx.flow(&[]).arg("down"));
    assert!(
        stdout
            .starts_with("the supervisor's pane w1:p1 on host-b is already gone: host-b is gone\n"),
        "{stdout}"
    );
    fx.assert_stopped();
}

#[test]
fn down_leaves_a_local_process_that_is_not_a_flow_watch_running() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));
    let mut bystander = StdCommand::new("sleep").arg("30").spawn().unwrap();
    fx.write_watch_lock("env-0001", bystander.id());

    let stdout = stdout_of(fx.flow(&[ok(NO_AGENTS), ok(WORKSPACE_LIST)]).arg("down"));

    assert_eq!(
        stdout,
        format!(
            "the supervisor's pane w1:p1 on host-a is already gone\n\
             {KEPT}\
             left pid {} running: it runs `sleep 30`, not flow watch\n\
             removed state/supervisor/watch.lock\n{NOTE}",
            bystander.id()
        )
    );
    assert!(!fx.watch_lock().exists());
    assert!(bystander.try_wait().unwrap().is_none());
    bystander.kill().unwrap();
    assert!(exited(&mut bystander));
}

#[test]
fn down_clears_another_hosts_watch_lock_without_signalling_anything() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));
    let mut bystander = StdCommand::new("sleep").arg("30").spawn().unwrap();
    fx.write_watch_lock("env-0002", bystander.id());

    let stdout = stdout_of(fx.flow(&[ok(NO_AGENTS), ok(WORKSPACE_LIST)]).arg("down"));

    assert!(
        stdout.contains(
            "removed state/supervisor/watch.lock; the watcher on env-0002 ends at its next cycle\n"
        ),
        "{stdout}"
    );
    assert!(!fx.watch_lock().exists());
    assert!(bystander.try_wait().unwrap().is_none());
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

#[test]
fn down_offers_to_stop_an_emptied_session_that_is_not_flows_own() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", None);
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
        ok(EMPTY_WORKSPACES),
        ok("stopping\n"),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's workspace w1 on host-a (herdr session flow)\n\
             {KEPT}\
             Herdr:\n \
             - stop the server of herdr session flow on host-a, which has no agents and no \
             workspaces left.\n\
             stopping\n\
             stopped herdr session flow's server on host-a\n{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "flow", "agent", "list"]),
            words(&["herdr", "--session", "flow", "workspace", "list"]),
            words(&["herdr", "--session", "flow", "workspace", "close", "w1"]),
            words(&["herdr", "--session", "flow", "agent", "list"]),
            words(&["herdr", "--session", "flow", "workspace", "list"]),
            words(&["herdr", "--session", "flow", "server", "stop"]),
        ]
    );
}

#[test]
fn down_offers_no_stop_while_the_session_keeps_workspaces_without_agents() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("flow"));
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
        ok(WORKSPACE_LIST),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert_eq!(fx.argvs().len(), 5);
}

#[test]
fn down_never_offers_to_stop_herdrs_default_session() {
    let fx = Fixture::new();
    fs::write(
        fx.state.join("config.toml"),
        format!("herdr_session = \"flow\"\n{CONFIG}"),
    )
    .unwrap();
    fx.write_lock("env-0001", "host-a", Some("default"));
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
        ok(EMPTY_WORKSPACES),
        ok("stopping\n"),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert_eq!(fx.argvs().len(), 3, "{:?}", fx.argvs());
}

#[test]
fn down_closes_a_relabelled_supervisor_workspace() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[
            ok(&supervisor_agent()),
            ok(&relabelled("my notes")),
            ok(WORKSPACE_CLOSED),
        ])
        .arg("down"),
    );

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's workspace w1 on host-a (herdr session default), relabelled \
             \"my notes\"\n{KEPT}{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs()[2],
        words(&["herdr", "--session", "default", "workspace", "close", "w1"])
    );
    fx.assert_stopped();
}

#[test]
fn down_closes_the_workspace_a_dead_supervisor_left_under_its_label() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[
            ok(NO_AGENTS),
            ok(&supervisor_listed()),
            ok(WORKSPACE_CLOSED),
        ])
        .arg("down"),
    );

    assert!(
        stdout.starts_with(
            "closed the supervisor's workspace w1 on host-a (herdr session default)\n"
        ),
        "{stdout}"
    );
    fx.assert_stopped();
}

#[test]
fn down_refuses_when_the_supervisors_pane_runs_another_session() {
    let unconfirmed = |why: &str| {
        format!(
            "pane w1:p1 on host-a holds an agent flow cannot tell from the supervisor: it runs in \
             /workspaces/flow in the supervisor's workspace w1, but {why}. flow down stopped \
             nothing and kept the lock; if it is not the supervisor, `flow down --force` stops \
             it\n"
        )
    };
    for (agents, listed, refusal) in [
        (
            in_lock_pane("/workspaces/flow", Some("s-hand")),
            supervisor_listed(),
            unconfirmed("herdr reports Claude Code session s-hand for it, not the supervisor's s9"),
        ),
        (
            AGENT_LIST.to_owned(),
            supervisor_listed(),
            "pane w1:p1 on host-a runs claude in /workspaces/acme-worktrees/login-form, \
             workspace w1, not the supervisor in /workspaces/flow, workspace w1 labelled \
             \"flow-supervisor\"; flow down stopped nothing and kept the lock\n"
                .to_owned(),
        ),
        (
            in_lock_pane("/workspaces/flow", None),
            relabelled("my notes"),
            unconfirmed(
                "herdr reports no Claude Code session for it, and its workspace is labelled \
                 \"my notes\", not \"flow-supervisor\"",
            ),
        ),
    ] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", Some("default"));
        let mut bystander = StdCommand::new("sleep").arg("30").spawn().unwrap();
        fx.write_watch_lock("env-0001", bystander.id());
        let before = fs::read_to_string(fx.lock_path()).unwrap();
        let script = [ok(&agents), ok(&listed), ok(WORKSPACE_CLOSED)];

        let out = fx
            .flow(&script)
            .arg("down")
            .assert()
            .code(2)
            .get_output()
            .clone();

        assert_eq!(String::from_utf8(out.stderr).unwrap(), refusal);
        assert!(out.stdout.is_empty());
        assert_eq!(fx.argvs().len(), 2);
        assert_eq!(fs::read_to_string(fx.lock_path()).unwrap(), before);
        assert!(fx.watch_lock().exists());
        assert!(bystander.try_wait().unwrap().is_none());
        bystander.kill().unwrap();
        assert!(exited(&mut bystander));
    }
}

#[test]
fn down_without_a_terminal_leaves_an_emptied_session_running_with_a_hint() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("flow"));
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
        ok(EMPTY_WORKSPACES),
    ];

    let stdout = stdout_of(fx.flow(&script).arg("down"));

    assert!(
        stdout.ends_with(&format!(
            "left herdr session flow's server running on host-a; to stop it, run there: herdr \
             --session flow server stop\n{NOTE}"
        )),
        "{stdout}"
    );
    assert_eq!(fx.argvs().len(), 5);
}

#[test]
fn down_leaves_a_session_with_agents_and_flows_own_session_running() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("flow"));
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(AGENT_LIST),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert_eq!(fx.argvs().len(), 4);

    fx.write_lock("env-0001", "host-a", Some("default"));
    let script = [
        ok(&supervisor_agent()),
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
    ];
    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));
    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert!(
        fx.argvs().len() == 3
            && fx
                .argvs()
                .iter()
                .all(|argv| !argv.contains(&"server".to_owned())),
        "{:?}",
        fx.argvs()
    );
}

#[test]
fn down_keeps_the_lock_when_the_supervisors_host_cannot_be_reached() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", Some("default"));
    let before = fs::read_to_string(fx.lock_path()).unwrap();

    let out = fx
        .flow(&[failed(255, UNREACHABLE)])
        .arg("down")
        .assert()
        .code(1)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("host unreachable: host-b"), "{stderr}");
    assert!(stderr.contains("nothing stopped"), "{stderr}");
    assert_eq!(fs::read_to_string(fx.lock_path()).unwrap(), before);
}

#[test]
fn down_needs_no_runner_call_for_an_unknown_host() {
    let fx = Fixture::new();
    fx.write_lock("env-0009", "host-z", Some("default"));

    let stdout = stdout_of(fx.flow(&[]).arg("down"));

    assert!(
        stdout.starts_with(
            "the supervisor's pane w1:p1 on host-z is already gone: env-0009 is not a known host\n"
        ),
        "{stdout}"
    );
    assert!(fx.argvs().is_empty());
    fx.assert_stopped();
}

#[test]
fn down_tells_a_supervisor_herdr_reports_no_session_for_by_its_workspace() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[
            ok(&in_lock_pane("/workspaces/flow", None)),
            ok(&supervisor_listed()),
            ok(WORKSPACE_CLOSED),
        ])
        .arg("down"),
    );

    assert!(
        stdout.starts_with(
            "closed the supervisor's workspace w1 on host-a (herdr session default)\n"
        ),
        "{stdout}"
    );
    fx.assert_stopped();
}

/// `supervisor_listed` with `w1` holding `panes` panes.
fn supervisor_listed_with_panes(panes: u32) -> String {
    let listed =
        supervisor_listed().replacen(r#""pane_count":1"#, &format!(r#""pane_count":{panes}"#), 1);
    assert_ne!(listed, supervisor_listed());
    listed
}

const PANE_CLOSED: &str = r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#;

#[test]
fn down_closes_only_the_supervisors_pane_when_its_workspace_holds_others() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[
            ok(&supervisor_agent()),
            ok(&supervisor_listed_with_panes(3)),
            ok(PANE_CLOSED),
        ])
        .arg("down"),
    );

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's pane w1:p1 on host-a (herdr session default); workspace w1 \
             keeps its other 2 panes\n{KEPT}{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs()[2],
        words(&["herdr", "--session", "default", "pane", "close", "w1:p1"])
    );
    fx.assert_stopped();
}

#[test]
fn down_force_closes_the_locks_workspace_whatever_runs_there() {
    for (agents, panes, closed) in [
        (
            in_lock_pane("/workspaces/flow", Some("s-hand")),
            1,
            "workspace w1",
        ),
        (AGENT_LIST.to_owned(), 2, "pane w1:p1"),
        (NO_AGENTS.to_owned(), 1, "workspace w1"),
    ] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", Some("default"));
        let listed = if panes == 1 {
            relabelled("my notes")
        } else {
            relabelled("my notes").replacen(r#""pane_count":1"#, r#""pane_count":2"#, 1)
        };
        let script = [
            json!({"argv": ["agent", "list"], "status": 0, "stdout": agents, "stderr": ""}),
            ok(&listed),
            ok(WORKSPACE_CLOSED),
        ];

        let stdout = stdout_of(fx.flow(&script).args(["down", "--force"]));

        let close = if panes == 1 {
            words(&["herdr", "--session", "default", "workspace", "close", "w1"])
        } else {
            words(&["herdr", "--session", "default", "pane", "close", "w1:p1"])
        };
        assert_eq!(
            fx.argvs(),
            [
                words(&["herdr", "--session", "default", "workspace", "list"]),
                close
            ]
        );
        assert!(
            stdout.starts_with(&format!(
                "closed the lock's {closed} on host-a (herdr session default) without checking \
                 what runs there (--force)\n"
            )),
            "{stdout}"
        );
        fx.assert_stopped();
    }
}

#[test]
fn down_force_notes_a_workspace_already_gone() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(fx.flow(&[ok(EMPTY_WORKSPACES)]).args(["down", "--force"]));

    assert!(
        stdout.starts_with("the supervisor's pane w1:p1 on host-a is already gone\n"),
        "{stdout}"
    );
    fx.assert_stopped();
}
