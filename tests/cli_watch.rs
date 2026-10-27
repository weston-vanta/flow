use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command as StdCommand, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::{Harness, HerdrTarget, State, TaskRecord};
use flow::watch::classify::Observation;
use flow::watch::queue;
use jiff::Timestamp;
use serde_json::{Value, json};

const CONFIG: &str = r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/work/repo"
default_branch = "main"

[[cdes]]
name = "cde-a"
env_id = "env-0001"
ssh_alias = "cde-a.example"

[[cdes]]
name = "cde-b"
env_id = "env-0002"
ssh_alias = "cde-b.example"
"#;

const TASK: &str = "aaaaaaab";
const OTHER_TASK: &str = "aaaaaaac";
/// How long any `flow watch` in these tests may run before it is killed and the test fails.
const WATCH_BUDGET: Duration = Duration::from_secs(5);
const POLL_MS: &str = "200";

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
        let fixture = Fixture { dir, state };
        fixture.script(&[]);
        fixture
    }

    fn state_dir(&self) -> StateDir {
        StateDir::resolve(self.dir.path()).unwrap()
    }

    /// A working task on `cde`, its worker in `pane_id` when given.
    fn task(&self, id: &str, cde: &str, pane_id: Option<&str>) {
        let store = Store::open(&self.state_dir());
        store
            .create_task(
                TaskRecord {
                    id: id.to_owned(),
                    parent: None,
                    title: "Build feature".to_owned(),
                    cde: cde.to_owned(),
                    worktree: PathBuf::from("/work/repo.wt/feature"),
                    harness: Harness::Claude,
                    created: Timestamp::now(),
                    owner: "supervisor".to_owned(),
                },
                "brief",
            )
            .unwrap();
        store
            .set_status(id, |status| {
                status.state = State::Working;
                status.herdr = pane_id.map(|pane_id| HerdrTarget {
                    cde: cde.to_owned(),
                    workspace_id: "w1".to_owned(),
                    pane_id: pane_id.to_owned(),
                    agent: "claude".to_owned(),
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
            ("HOME", self.dir.path().to_owned()),
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
    fx.task(TASK, "cde-a", None);
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
    fx.task(TASK, "cde-a", None);
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
fn unreachable_cde_reported_once() {
    let fx = Fixture::new();
    fx.task(TASK, "cde-b", Some("w1:p1"));
    fx.task(OTHER_TASK, "cde-b", Some("w2:p1"));
    fx.script(&[json!({
        "cde": "cde-b",
        "status": 255,
        "stderr": "ssh: connect to host cde-b.example port 22: Connection refused",
        "repeat": true,
    })]);

    let output = finish(fx.spawn_watch(&[]));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(stdout(&output), "cde-unreachable: cde-b\n");
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
            .any(|line| line.contains("absorb") && line.contains(r#""cde":"cde-b""#)),
        "{log}"
    );
}
