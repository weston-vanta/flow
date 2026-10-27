use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::{Harness, State, TaskRecord};
use flow::supervisor::SUPERVISOR_ENV;
use flow::watch::classify::Observation;
use flow::watch::lock::Owner;
use flow::watch::queue;
use jiff::Timestamp;

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }
"#;

const HOST: &str = "env-0001";
const GRACE_MS: &str = "200";
const GUARD_BUDGET: Duration = Duration::from_secs(5);
const STOP_INPUT: &str = r#"{"hook_event_name":"Stop","stop_hook_active":false}"#;
const NO_WATCHER: &str = "flow: work under way (2 tasks) and no watcher is live; run `flow watch` in the background before ending the turn\n";

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
        Fixture { dir, state }
    }

    fn state_dir(&self) -> StateDir {
        StateDir::resolve(self.dir.path()).unwrap()
    }

    fn task(&self, id: &str, state: State) {
        let store = Store::open(&self.state_dir());
        store
            .create_task(
                TaskRecord {
                    id: id.to_owned(),
                    parent: None,
                    title: "Build feature".to_owned(),
                    host: "host-a".to_owned(),
                    cwd: PathBuf::from("/work/repo.wt/feature"),
                    repo: Some("example-repo".to_owned()),
                    harness: Harness::Claude,
                    created: Timestamp::now(),
                    owner: "supervisor".to_owned(),
                },
                "brief",
            )
            .unwrap();
        store.set_status(id, |status| status.state = state).unwrap();
    }

    /// `flow guard` as the supervisor's Stop hook runs it.
    fn guard(&self, input: &str) -> assert_cmd::assert::Assert {
        let mut cmd = self.guard_command();
        cmd.env(SUPERVISOR_ENV, "1");
        cmd.write_stdin(input).assert()
    }

    /// `flow guard` with no supervisor marker in its environment.
    fn guard_command(&self) -> Command {
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.arg("guard")
            .env("FLOW_STATE", &self.state)
            .env("ONA_ID", HOST)
            .env("FLOW_GUARD_GRACE_MS", GRACE_MS)
            .env("HOME", self.dir.path())
            .env_remove(SUPERVISOR_ENV)
            .timeout(GUARD_BUDGET);
        cmd
    }
}

#[test]
fn guard_allows_when_no_open_tasks() {
    let fixture = Fixture::new();
    fixture.guard(STOP_INPUT).code(0).stdout("").stderr("");
    fixture.task("aaaaaaab", State::Done);
    fixture.task("aaaaaaac", State::Orphaned);
    fixture.guard(STOP_INPUT).code(0).stdout("").stderr("");
}

#[test]
fn guard_blocks_without_watcher() {
    let fixture = Fixture::new();
    fixture.task("aaaaaaab", State::Working);
    fixture.task("aaaaaaac", State::NeedsDecision);
    fixture.task("aaaaaaad", State::Done);
    fixture
        .guard(STOP_INPUT)
        .code(2)
        .stdout(NO_WATCHER)
        .stderr(NO_WATCHER);
    fixture.guard("").code(2).stderr(NO_WATCHER);
}

#[test]
fn guard_allows_when_watch_lock_live() {
    let fixture = Fixture::new();
    fixture.task("aaaaaaab", State::Working);
    fixture.task("aaaaaaac", State::Blocked);
    let lock = fixture.state.join("supervisor").join("watch.lock");
    fs::create_dir_all(&lock).unwrap();
    let owner = Owner {
        pid: std::process::id(),
        host: HOST.to_owned(),
        started: Timestamp::now(),
    };
    fs::write(lock.join("owner.json"), serde_json::to_vec(&owner).unwrap()).unwrap();
    fixture.guard(STOP_INPUT).code(0).stdout("").stderr("");
    assert!(lock.join("owner.json").exists());
}

#[test]
fn guard_blocks_on_pending_wakes() {
    let fixture = Fixture::new();
    let state = fixture.state_dir();
    queue::push(&state, "heartbeat", &Observation::Heartbeat).unwrap();
    queue::push(&state, "heartbeat", &Observation::Heartbeat).unwrap();
    let message = "flow: 2 unhandled wakes; run flow wake drain\n";
    fixture
        .guard(STOP_INPUT)
        .code(2)
        .stdout(message)
        .stderr(message);
    queue::ack_through(&state, 2).unwrap();
    fixture.guard(STOP_INPUT).code(0);
}

#[test]
fn guard_respects_stop_hook_active() {
    let fixture = Fixture::new();
    fixture.task("aaaaaaab", State::Working);
    fixture
        .guard(r#"{"hook_event_name":"Stop","stop_hook_active":true}"#)
        .code(0)
        .stdout("")
        .stderr("");
}

#[test]
fn guard_ignores_non_supervisor_sessions() {
    let fixture = Fixture::new();
    fixture.task("aaaaaaab", State::Working);
    fixture.task("aaaaaaac", State::Blocked);
    fixture.guard(STOP_INPUT).code(2).stdout(NO_WATCHER);

    fixture
        .guard_command()
        .write_stdin(STOP_INPUT)
        .assert()
        .code(0)
        .stdout("")
        .stderr("");
}

#[test]
fn guard_outside_supervisor_needs_no_state_dir() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("flow")
        .expect("flow binary is built")
        .arg("guard")
        .env("FLOW_STATE", dir.path().join("missing"))
        .env_remove(SUPERVISOR_ENV)
        .write_stdin(STOP_INPUT)
        .timeout(GUARD_BUDGET)
        .assert()
        .code(0)
        .stdout("")
        .stderr("");
}

#[test]
fn guard_hands_the_turn_to_a_wake_queued_during_its_grace() {
    let fixture = Fixture::new();
    fixture.task("aaaaaaab", State::Working);
    fixture.task("aaaaaaac", State::Blocked);
    let state = fixture.state_dir();
    // The async watcher takes the lock, queues a wake and exits within the guard's grace.
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        queue::push(&state, "blocked: aaaaaaac", &Observation::Heartbeat).unwrap();
    });
    let message = "flow: 1 unhandled wakes; run flow wake drain\n";

    let mut cmd = fixture.guard_command();
    cmd.env(SUPERVISOR_ENV, "1")
        .env("FLOW_GUARD_GRACE_MS", "3000")
        .write_stdin(STOP_INPUT);
    let started = std::time::Instant::now();
    cmd.assert().code(2).stdout(message).stderr(message);

    watcher.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "the guard stopped waiting once the wake was queued: {:?}",
        started.elapsed()
    );
}
