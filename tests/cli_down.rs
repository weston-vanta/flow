use std::fs;
use std::path::{Path, PathBuf};
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
const NOTE: &str = "1 open tasks keep running; flow up starts a new supervisor\n";

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
    let listed =
        WORKSPACE_LIST.replacen(r#""label":"login-form""#, r#""label":"flow-supervisor""#, 1);
    assert_ne!(listed, WORKSPACE_LIST);
    listed
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
fn down_closes_the_supervisors_workspace_and_removes_the_lock() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));

    let stdout = stdout_of(
        fx.flow(&[ok(&supervisor_listed()), ok(WORKSPACE_CLOSED)])
            .arg("down"),
    );

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's workspace w1 on host-a (herdr session default)\n\
             removed state/supervisor/lock.json\n{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "default", "workspace", "list"]),
            words(&["herdr", "--session", "default", "workspace", "close", "w1"]),
        ]
    );
    assert!(!fx.lock_path().exists());
    let (_, status) = fx.store().task("aaaaaaab").unwrap();
    assert_eq!(status.state, State::Working);
}

#[test]
fn down_notes_a_supervisor_workspace_already_gone() {
    let fx = Fixture::new();
    fx.write_lock("env-0002", "host-b", Some("default"));

    let stdout = stdout_of(fx.flow(&[ok(WORKSPACE_LIST)]).arg("down"));

    assert_eq!(
        stdout,
        format!(
            "the supervisor's pane w1:p1 on host-b is already gone\n\
             removed state/supervisor/lock.json\n{NOTE}"
        )
    );
    assert_eq!(fx.argvs().len(), 1);
    assert!(!fx.lock_path().exists());

    fx.write_lock("env-0002", "host-b", Some("default"));
    let script = [json!({
        "status": 1, "stdout": "",
        "stderr": SERVER_NOT_RUNNING.replace("cli:agent:list", "cli:workspace:list"),
    })];
    let stdout = stdout_of(fx.flow(&script).arg("down"));
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
    assert!(!fx.lock_path().exists());
}

#[test]
fn down_stops_a_live_local_watcher_and_clears_its_lock() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));
    let mut watcher = StdCommand::new("sleep").arg("30").spawn().unwrap();
    fx.write_watch_lock("env-0001", watcher.id());

    let stdout = stdout_of(fx.flow(&[ok(WORKSPACE_LIST)]).arg("down"));

    assert!(exited(&mut watcher), "the watcher still runs");
    assert_eq!(
        stdout,
        format!(
            "the supervisor's pane w1:p1 on host-a is already gone\n\
             removed state/supervisor/lock.json\n\
             stopped the watcher (pid {})\n\
             removed state/supervisor/watch.lock\n{NOTE}",
            watcher.id()
        )
    );
    assert!(!fx.watch_lock().exists());
}

#[test]
fn down_clears_another_hosts_watch_lock_without_signalling_anything() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("default"));
    let mut bystander = StdCommand::new("sleep").arg("30").spawn().unwrap();
    fx.write_watch_lock("env-0002", bystander.id());

    let stdout = stdout_of(fx.flow(&[ok(WORKSPACE_LIST)]).arg("down"));

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
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
        ok("stopping\n"),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert_eq!(
        stdout,
        format!(
            "closed the supervisor's workspace w1 on host-a (herdr session flow)\n\
             removed state/supervisor/lock.json\n\
             Herdr:\n \
             - stop the server of herdr session flow on host-a, which has no agents left.\n\
             stopping\n\
             stopped herdr session flow's server on host-a\n{NOTE}"
        )
    );
    assert_eq!(
        fx.argvs(),
        [
            words(&["herdr", "--session", "flow", "workspace", "list"]),
            words(&["herdr", "--session", "flow", "workspace", "close", "w1"]),
            words(&["herdr", "--session", "flow", "agent", "list"]),
            words(&["herdr", "--session", "flow", "server", "stop"]),
        ]
    );
}

#[test]
fn down_without_a_terminal_leaves_an_emptied_session_running_with_a_hint() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("flow"));
    let script = [
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(NO_AGENTS),
    ];

    let stdout = stdout_of(fx.flow(&script).arg("down"));

    assert!(
        stdout.ends_with(&format!(
            "left herdr session flow's server running on host-a; to stop it, run there: herdr \
             --session flow server stop\n{NOTE}"
        )),
        "{stdout}"
    );
    assert_eq!(fx.argvs().len(), 3);
}

#[test]
fn down_leaves_a_session_with_agents_and_flows_own_session_running() {
    let fx = Fixture::new();
    fx.write_lock("env-0001", "host-a", Some("flow"));
    let script = [
        ok(&supervisor_listed()),
        ok(WORKSPACE_CLOSED),
        ok(AGENT_LIST),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));

    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert_eq!(fx.argvs().len(), 3);

    fx.write_lock("env-0001", "host-a", Some("default"));
    let script = [ok(&supervisor_listed()), ok(WORKSPACE_CLOSED)];
    let stdout = stdout_of(fx.flow(&script).args(["down", "--yes"]));
    assert!(!stdout.contains("Herdr:"), "{stdout}");
    assert!(
        fx.argvs()
            .iter()
            .all(|argv| !argv.contains(&"server".to_owned()) && !argv.contains(&"agent".to_owned())),
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
    assert!(!Path::new(&fx.lock_path()).exists());
}
