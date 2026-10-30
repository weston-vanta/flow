use std::fs;
use std::path::PathBuf;

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

const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const SERVER_NOT_RUNNING: &str = include_str!("fixtures/herdr/agent_list_server_not_running.json");
const UNREACHABLE: &str = "ssh: connect to host host-b.example port 22: Connection timed out";
const PANE_CLOSED: &str = r#"{"id":"cli:pane:close","result":{"type":"ok"}}"#;
const NOWHERE_ON_A: &str =
    "the supervisor runs in no pane on host-a; closed nothing, so any pane it left stays open\n";
const NOWHERE_ON_B: &str =
    "the supervisor runs in no pane on host-b; closed nothing, so any pane it left stays open\n";
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

    /// The supervisor's lock: in workspace `w1`, pane `w1:p1`, of herdr session `session` on
    /// the host `env_id`, with Claude Code session `s9`.
    fn write_lock(&self, env_id: &str, host: &str, session: &str) {
        let dir = self.state.join("supervisor");
        fs::create_dir_all(&dir).unwrap();
        let herdr = json!({"host": host, "workspace_id": "w1", "pane_id": "w1:p1",
            "agent": "claude", "session": session});
        let lock = json!({
            "host_env_id": env_id, "herdr": herdr, "claude_session": "s9",
            "since": "2026-10-26T12:00:00Z",
        });
        fs::write(dir.join("lock.json"), lock.to_string()).unwrap();
    }

    /// A `watch.lock` held by process `pid` on the host `env_id`.
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
    fx.write_lock("env-0001", "host-a", "default");
    stdout_of(fx.flow(&[ok(NO_AGENTS)]).arg("down"));
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
fn down_closes_the_supervisors_pane_and_marks_the_lock_stopped() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", "default");

    let stdout = stdout_of(
        fx.flow(&[ok(&supervisor_agent()), ok(PANE_CLOSED)])
            .arg("down"),
    );

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's pane w1:p1 on host-a (herdr session default)\n{KEPT}{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "default", "agent", "list"]),
            words(&["herdr", "--session", "default", "pane", "close", "w1:p1"]),
        ]
    );
    fx.assert_stopped();
    let (_, status) = fx.store().task("aaaaaaab").unwrap();
    assert_eq!(status.state, State::Working);
}

#[test]
fn down_notes_a_supervisor_already_gone() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", "default");

    let stdout = stdout_of(fx.flow(&[ok(NO_AGENTS)]).arg("down"));

    assert_eq!(stdout, format!("{NOWHERE_ON_B}{KEPT}{NOTE}"));
    assert_eq!(fx.argvs().len(), 1);
    fx.assert_stopped();

    fx.write_lock("env-0002", "host-b", "default");
    let not_running = json!({"status": 1, "stdout": "", "stderr": SERVER_NOT_RUNNING});
    let stdout = stdout_of(fx.flow(&[not_running]).arg("down"));
    assert_eq!(fx.argvs().len(), 1);
    assert!(stdout.starts_with(NOWHERE_ON_B), "{stdout}");

    fx.write_lock("env-0002", "host-b", "default");
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
fn down_removes_the_watch_lock_whoever_holds_it() {
    for env_id in ["env-0001", "env-0002"] {
        let fx = Fixture::new();
        fx.write_lock("env-0001", "host-a", "default");
        fx.write_watch_lock(env_id, std::process::id());

        let stdout = stdout_of(fx.flow(&[ok(NO_AGENTS)]).arg("down"));

        assert_eq!(
            stdout,
            format!(
                "{NOWHERE_ON_A}{KEPT}\
                 removed state/supervisor/watch.lock; the watcher ends at its next cycle\n{NOTE}"
            )
        );
        assert!(!fx.watch_lock().exists());
    }
}

#[test]
fn down_closes_an_agent_with_no_session_in_the_supervisors_pane_only_under_its_label() {
    let workspaces = |label: &str| {
        let list = json!({"id": "cli:workspace:list", "result": {"type": "workspace_list",
            "workspaces": [{"workspace_id": "w1", "label": label}]}});
        ok(&list.to_string())
    };
    let unreported = ok(&in_lock_pane("/workspaces/flow", None));

    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", "default");
    let script = [
        unreported.clone(),
        workspaces("flow-supervisor"),
        ok(PANE_CLOSED),
    ];
    let stdout = stdout_of(fx.flow(&script).arg("down"));
    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's pane w1:p1 on host-a (herdr session default)\n{KEPT}{NOTE}"
        )
    );
    fx.assert_stopped();

    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", "default");
    let stdout = stdout_of(fx.flow(&[unreported, workspaces("Notes")]).arg("down"));
    assert_eq!(stdout, format!("{NOWHERE_ON_A}{KEPT}{NOTE}"));
    assert_eq!(fx.argvs().len(), 2);
    fx.assert_stopped();
}

#[test]
fn down_keeps_the_lock_when_the_supervisors_host_cannot_be_reached() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", "default");
    let before = fs::read_to_string(fx.lock_path()).unwrap();

    let out = fx
        .flow(&[failed(255, UNREACHABLE)])
        .arg("down")
        .assert()
        .code(1)
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with(
            "closing the supervisor's pane on host-b; nothing stopped, and the lock is kept: \
             host unreachable: host-b"
        ),
        "{stderr}"
    );
    assert!(
        stderr.ends_with(
            "\nthe supervisor may still run on host-b, so flow keeps its lock until it can stop \
             it there. Either restore ssh access to host-b from this machine (`gitpod login`, \
             then `gitpod environment ssh-config`) and run `flow down` again, or stop host-b's \
             environment in Ona and run `flow inventory --refresh`, which records it stopped; \
             then `flow down` marks the supervisor stopped without ssh and `flow up` starts one \
             here.\n"
        ),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(fx.lock_path()).unwrap(), before);
}

#[test]
fn down_marks_the_supervisor_stopped_without_ssh_when_ona_lists_its_host_stopped() {
    let fx = Fixture::new();
    let path = fx.state.join("hosts/env-0002.json");
    let mut record: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    record["ona_phase"] = json!("ENVIRONMENT_PHASE_STOPPED");
    fs::write(&path, record.to_string()).unwrap();
    fx.write_lock("env-0002", "host-b", "default");

    let stdout = stdout_of(fx.flow(&[]).arg("down"));

    assert_eq!(
        stdout,
        format!(
            "the supervisor's pane w1:p1 on host-b is already gone: host-b is not running\n\
             {KEPT}{NOTE}"
        )
    );
    assert!(fx.argvs().is_empty());
    fx.assert_stopped();
}

#[test]
fn down_needs_no_runner_call_for_an_unknown_host() {
    let fx = Fixture::new();
    fx.write_lock("env-0009", "host-z", "default");

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
fn down_leaves_another_session_in_the_supervisors_pane_alone() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", "default");

    let hand = ok(&in_lock_pane("/workspaces/flow", Some("s-hand")));

    let stdout = stdout_of(fx.flow(&[hand]).arg("down"));

    assert_eq!(stdout, format!("{NOWHERE_ON_A}{KEPT}{NOTE}"));
    assert_eq!(fx.argvs().len(), 1);
    fx.assert_stopped();
}

#[test]
fn down_finds_the_supervisor_in_another_listed_herdr_session() {
    let fx = Fixture::new();
    fs::write(
        fx.state.join("config.toml"),
        format!("herdr_sessions = [\"other\"]\n{CONFIG}"),
    )
    .unwrap();
    fx.write_lock("env-0001", "host-a", "default");
    let elsewhere = supervisor_agent().replace("w1:p1", "w2:p3");

    let stdout = stdout_of(
        fx.flow(&[ok(NO_AGENTS), ok(&elsewhere), ok(PANE_CLOSED)])
            .arg("down"),
    );

    assert!(
        stdout.starts_with("closed the supervisor's pane w2:p3 on host-a (herdr session other)\n"),
        "{stdout}"
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "default", "agent", "list"]),
            words(&["herdr", "--session", "other", "agent", "list"]),
            words(&["herdr", "--session", "other", "pane", "close", "w2:p3"]),
        ]
    );
}

#[test]
fn down_closes_the_supervisors_pane_wherever_herdr_moved_it() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", "default");
    let moved = supervisor_agent().replace("w1:p1", "w4:p2");

    let stdout = stdout_of(fx.flow(&[ok(&moved), ok(PANE_CLOSED)]).arg("down"));

    assert!(
        stdout
            .starts_with("closed the supervisor's pane w4:p2 on host-a (herdr session default)\n"),
        "{stdout}"
    );
    assert_eq!(
        fx.argvs()[1],
        words(&["herdr", "--session", "default", "pane", "close", "w4:p2"])
    );
    fx.assert_stopped();
}
