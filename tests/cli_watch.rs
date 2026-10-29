use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::{Harness, HerdrTarget, Origin, State, TaskRecord};
use flow::supervisor::SUPERVISOR_ENV;
use flow::watch::classify::Observation;
use flow::watch::queue;
use jiff::Timestamp;
use serde_json::{Value, json};

/// flow in its own herdr session, `flow`, listing herdr's `default` session too.
const CONFIG: &str = r#"
herdr_session = "flow"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

"#;
/// flow's own herdr session in `CONFIG`.
const SESSION: &str = "flow";

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

const TASK: &str = "aaaaaaab";
const OTHER_TASK: &str = "aaaaaaac";
/// How long any `flow watch` in these tests may run before it is killed and the test fails.
const WATCH_BUDGET: Duration = Duration::from_secs(5);
const POLL_MS: &str = "200";
/// The first delay before a failed watch retries, so a retry or two fits in these tests.
const RETRY_MS: &str = "100";

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
        let fixture = Fixture { dir, state };
        fixture.script(&[]);
        fixture
    }

    fn state_dir(&self) -> StateDir {
        StateDir::resolve(self.dir.path()).unwrap()
    }

    /// A working task on `host`, its worker in `pane_id` when given.
    fn task(&self, id: &str, host: &str, pane_id: Option<&str>) {
        let store = Store::open(&self.state_dir());
        store
            .create_task(
                TaskRecord {
                    id: id.to_owned(),
                    parent: None,
                    title: "Build feature".to_owned(),
                    host: host.to_owned(),
                    cwd: PathBuf::from("/work/repo.wt/feature"),
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
            .set_status(id, |status| {
                status.state = State::Working;
                status.herdr = pane_id.map(|pane_id| HerdrTarget {
                    host: host.to_owned(),
                    workspace_id: "w1".to_owned(),
                    pane_id: pane_id.to_owned(),
                    agent: "claude".to_owned(),
                    session: SESSION.to_owned(),
                });
            })
            .unwrap();
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    fn script(&self, script: &[Value]) {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
    }

    fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("FLOW_STATE", self.state.clone()),
            ("ONA_ID", PathBuf::from("env-0001")),
            ("FLOW_FAKE_RUNNER", self.script_path()),
            ("FLOW_WATCH_POLL_MS", PathBuf::from(POLL_MS)),
            ("FLOW_WATCH_RETRY_MS", PathBuf::from(RETRY_MS)),
            ("HOME", self.dir.path().to_owned()),
            (SUPERVISOR_ENV, PathBuf::from("1")),
        ]
    }

    fn flow(&self) -> Command {
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.envs(self.envs()).timeout(WATCH_BUDGET);
        cmd
    }

    /// `flow watch <args>` running in the background.
    fn spawn_watch(&self, args: &[&str]) -> Child {
        StdCommand::new(assert_cmd::cargo::cargo_bin("flow"))
            .arg("watch")
            .args(args)
            .envs(self.envs())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn supervisor(&self, name: &str) -> PathBuf {
        self.state.join("supervisor").join(name)
    }

    fn queue_lines(&self) -> Vec<Value> {
        fs::read_to_string(self.supervisor("wake-queue.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Waits until the watcher's saved state records `task` as read while working, failing the
    /// test after `WATCH_BUDGET`.
    fn await_seen_working(&self, task: &str) {
        let path = self.supervisor("watch-state.json");
        let deadline = Instant::now() + WATCH_BUDGET;
        loop {
            let seen = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|state| state["seen"][task]["state"] == "working");
            if seen {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the watcher never recorded {task} as working"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The child's output once it exits; kills it and fails the test after `WATCH_BUDGET`.
fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + WATCH_BUDGET;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "flow watch did not exit within {WATCH_BUDGET:?}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn watch_exits_2_on_done_status() {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    let watch = fx.spawn_watch(&[]);
    fx.await_seen_working(TASK);

    let report = fx.dir.path().join("report.md");
    fs::write(&report, "# Done\n").unwrap();
    fx.flow()
        .args(["task", "done", TASK, "--report"])
        .arg(&report)
        .assert()
        .success();

    let output = finish(watch);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(stdout(&output), format!("done: {TASK}\n"));
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("done: {TASK}")));
    let queue = fx.queue_lines();
    assert_eq!(queue.len(), 1, "{queue:?}");
    assert_eq!(queue[0]["seq"], 1);
    assert_eq!(queue[0]["reason"], format!("done: {TASK}"));
    assert_eq!(queue[0]["obs"]["type"], "status-changed");
    assert!(!fx.supervisor("watch.lock").exists());
}

#[test]
fn second_watch_exits_0_when_locked() {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    let lock = fx.supervisor("watch.lock");
    fs::create_dir_all(&lock).unwrap();
    let owner = json!({
        "pid": std::process::id(),
        "host": "env-0001",
        "started": Timestamp::now(),
    });
    fs::write(lock.join("owner.json"), owner.to_string()).unwrap();

    fx.flow()
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .stdout("");
    assert!(lock.join("owner.json").exists());
    assert!(fx.queue_lines().is_empty());
}

#[test]
fn queued_wake_survives_restart() {
    let fx = Fixture::new();
    let obs = Observation::StatusChanged {
        task: TASK.to_owned(),
        from: State::Working,
        to: State::Done,
    };
    assert_eq!(
        queue::push(&fx.state_dir(), &format!("done: {TASK}"), &obs).unwrap(),
        1
    );

    let drained = fx.flow().args(["wake", "drain"]).assert().success();
    let out = String::from_utf8(drained.get_output().stdout.clone()).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("1  "), "{out}");
    assert!(lines[0].ends_with(&format!("done: {TASK}")), "{out}");
    assert_eq!(lines[1], "WAKE_ACK_REQUIRED: flow wake ack --through 1");

    fx.flow()
        .args(["wake", "ack", "--through", "1"])
        .assert()
        .success()
        .stdout("");
    fx.flow()
        .args(["wake", "drain"])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn unreachable_host_reported_once() {
    let fx = Fixture::new();
    fx.task(TASK, "host-b", Some("w1:p1"));
    fx.task(OTHER_TASK, "host-b", Some("w2:p1"));
    fx.script(&[json!({
        "host": "host-b",
        "status": 255,
        "stderr": "ssh: connect to host host-b.example port 22: Connection refused",
        "repeat": true,
    })]);

    let output = finish(fx.spawn_watch(&[]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(stdout(&output), "host-unreachable: host-b\n");
    assert_eq!(fx.queue_lines().len(), 1);

    fx.flow()
        .args(["wake", "ack", "--through", "1"])
        .assert()
        .success();
    let output = finish(fx.spawn_watch(&["--once"]));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output), "");
    assert_eq!(fx.queue_lines().len(), 1);
    let log = fs::read_to_string(fx.supervisor("watch.log")).unwrap();
    assert!(
        log.lines()
            .any(|line| line.contains("absorb") && line.contains(r#""host":"host-b""#)),
        "{log}"
    );
}

#[test]
fn watch_ignores_non_supervisor_sessions() {
    let fx = Fixture::new();
    fx.task(TASK, "host-b", Some("w1:p1"));
    fx.script(&[json!({
        "host": "host-b",
        "status": 255,
        "stderr": "ssh: connect to host host-b.example port 22: Connection refused",
        "repeat": true,
    })]);

    fx.flow()
        .env_remove(SUPERVISOR_ENV)
        .args(["watch", "--once"])
        .assert()
        .code(0)
        .stdout("")
        .stderr("");

    assert!(fx.queue_lines().is_empty());
    assert!(!fx.supervisor("watch.lock").exists());
    assert!(!fx.supervisor("watch.log").exists());
}

#[test]
fn watch_outside_supervisor_needs_no_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("flow")
        .expect("flow binary is built")
        .args(["watch", "--once"])
        .env("FLOW_STATE", dir.path().join("missing"))
        .env_remove(SUPERVISOR_ENV)
        .timeout(WATCH_BUDGET)
        .assert()
        .code(0)
        .stdout("")
        .stderr("");
}

#[test]
fn watch_wakes_for_a_task_done_before_first_sight() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.state.join("supervisor")).unwrap();
    fs::write(fx.supervisor("watch-state.json"), "{}").unwrap();
    fx.task(TASK, "host-a", None);
    let report = fx.dir.path().join("report.md");
    fs::write(&report, "# Done\n").unwrap();
    fx.flow()
        .args(["task", "done", TASK, "--report"])
        .arg(&report)
        .assert()
        .success();

    let output = finish(fx.spawn_watch(&["--once"]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(stdout(&output), format!("done: {TASK}\n"));
}

#[test]
fn watch_exits_0_with_nothing_to_watch() {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    fx.flow()
        .args(["task", "orphan", TASK, "--reason", "superseded"])
        .assert()
        .success();

    let output = finish(fx.spawn_watch(&[]));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(stdout(&output), "");
    assert!(fx.queue_lines().is_empty());
    assert!(!fx.supervisor("watch.lock").exists());
    let log = fs::read_to_string(fx.supervisor("watch.log")).unwrap();
    assert!(log.contains("nothing to watch"), "{log}");
}

#[test]
fn watch_failure_after_taking_the_lock_wakes() {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    fs::create_dir_all(fx.supervisor("watch-state.json")).unwrap();

    let output = finish(fx.spawn_watch(&[]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let out = stdout(&output);
    assert!(out.starts_with("watch-failed: "), "{out}");
    assert!(out.contains("watch-state.json"), "{out}");
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("watch-failed: "));
    let queue = fx.queue_lines();
    assert_eq!(queue.len(), 1, "{queue:?}");
    assert_eq!(queue[0]["reason"], out.trim_end());
    assert_eq!(queue[0]["obs"]["type"], "watch-failed");
    assert!(!fx.supervisor("watch.lock").exists());
}

#[test]
fn watch_keeps_running_when_a_task_cannot_be_read() {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    let status = fx.state.join("tasks").join(TASK).join("status.json");
    fs::write(&status, "{not json").unwrap();

    let mut watch = fx.spawn_watch(&[]);
    thread::sleep(Duration::from_millis(1500));
    let exited = watch.try_wait().unwrap();
    watch.kill().unwrap();
    let output = watch.wait_with_output().unwrap();
    assert!(exited.is_none(), "flow watch exited: {output:?}");
    assert!(fx.queue_lines().is_empty());
    let log = fs::read_to_string(fx.supervisor("watch.log")).unwrap();
    assert!(log.contains("reading tasks"), "{log}");
    assert!(!log.contains("nothing to watch"), "{log}");
}

/// A task whose watcher state cannot be read, and a first `flow watch` that woke for it.
fn failed_watch() -> (Fixture, String) {
    let fx = Fixture::new();
    fx.task(TASK, "host-a", None);
    fs::create_dir_all(fx.supervisor("watch-state.json")).unwrap();
    let output = finish(fx.spawn_watch(&[]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let reason = stdout(&output).trim_end().to_owned();
    assert!(reason.starts_with("watch-failed: "), "{reason}");
    fx.flow()
        .args(["wake", "ack", "--through", "1"])
        .assert()
        .success();
    (fx, reason)
}

/// Waits long enough for a watch that started failing to retry several times.
fn retries() {
    thread::sleep(Duration::from_millis(1500));
}

#[test]
fn repeated_watch_failure_wakes_once_and_holds_the_lock() {
    let (fx, _) = failed_watch();
    let mut watch = fx.spawn_watch(&[]);
    retries();
    let exited = watch.try_wait().unwrap();
    let locked = fx.supervisor("watch.lock").join("owner.json").exists();
    watch.kill().unwrap();
    let output = watch.wait_with_output().unwrap();
    assert!(exited.is_none(), "flow watch exited: {output:?}");
    assert!(locked);
    assert_eq!(fx.queue_lines().len(), 1, "{:?}", fx.queue_lines());
}

#[test]
fn watch_resumes_once_its_failure_clears() {
    let (fx, _) = failed_watch();
    let watch = fx.spawn_watch(&[]);
    retries();
    fs::remove_dir(fx.supervisor("watch-state.json")).unwrap();
    fx.await_seen_working(TASK);
    let report = fx.dir.path().join("report.md");
    fs::write(&report, "# Done\n").unwrap();
    fx.flow()
        .args(["task", "done", TASK, "--report"])
        .arg(&report)
        .assert()
        .success();

    let output = finish(watch);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(stdout(&output), format!("done: {TASK}\n"));
    assert_eq!(fx.queue_lines().len(), 2, "{:?}", fx.queue_lines());
}

#[test]
fn changed_watch_failure_wakes_again() {
    let (fx, first) = failed_watch();
    let watch = fx.spawn_watch(&[]);
    retries();
    let path = fx.supervisor("watch-state.json");
    fs::remove_dir(&path).unwrap();
    std::os::unix::fs::symlink(&path, &path).unwrap();

    let output = finish(watch);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let second = stdout(&output).trim_end().to_owned();
    assert!(second.starts_with("watch-failed: "), "{second}");
    assert_ne!(second, first);
    let queue = fx.queue_lines();
    assert_eq!(queue.len(), 2, "{queue:?}");
    assert_eq!(queue[1]["reason"], second);
}
