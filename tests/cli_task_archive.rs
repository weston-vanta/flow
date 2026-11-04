mod common;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events;
use flow::state::requests::NewRequest;
use flow::state::task::{Harness, HerdrTarget, Origin, State, TaskRecord};
use jiff::Timestamp;
use serde_json::json;

const DONE: &str = "aaaaaaab";
const OPEN: &str = "aaaaaaac";

struct Fixture {
    _dir: tempfile::TempDir,
    state: PathBuf,
    store: Store,
}

impl Fixture {
    /// State holding a done task and a working one, both on `host-a`, with no host records.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        let store = Store::open(&StateDir::at(&state).unwrap());
        let fx = Fixture {
            _dir: dir,
            state,
            store,
        };
        fx.task(DONE, State::Done);
        fx.task(OPEN, State::Working);
        fx
    }

    fn task(&self, id: &str, state: State) {
        let record = TaskRecord {
            id: id.to_owned(),
            parent: None,
            title: "a task".to_owned(),
            host: "host-a".to_owned(),
            cwd: PathBuf::from("/work/repo"),
            repo: None,
            harness: Harness::Claude,
            created: Timestamp::now(),
            owner: "supervisor".to_owned(),
            origin: Origin::Dispatched,
        };
        self.store.create_task(record, "brief").unwrap();
        self.store
            .set_status(id, |status| status.state = state)
            .unwrap();
    }

    /// Gives task `id` a herdr pane on `host`.
    fn target(&self, id: &str, host: &str) {
        self.store
            .set_status(id, |status| {
                status.herdr = Some(HerdrTarget {
                    host: host.to_owned(),
                    workspace_id: "w1".to_owned(),
                    tab_id: "w1:t1".to_owned(),
                    pane_id: "w1:p1".to_owned(),
                    agent: "claude".to_owned(),
                    session: "flow".to_owned(),
                });
            })
            .unwrap();
    }

    /// A cached host record named `name`, gone or not.
    fn host(&self, name: &str, gone: bool) {
        let dir = self.state.join("hosts");
        fs::create_dir_all(&dir).unwrap();
        let record = json!({
            "name": name, "env_id": format!("env-{name}"), "ssh_alias": name, "gone": gone,
            "seen": Timestamp::now(), "reachable": false, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(dir.join(format!("env-{name}.json")), record.to_string()).unwrap();
    }

    fn flow(&self) -> Command {
        let mut cmd = common::flow();
        cmd.env("FLOW_STATE", &self.state);
        cmd
    }

    fn exists(&self, id: &str) -> bool {
        self.state.join("tasks").join(id).exists()
    }

    fn archived(&self, id: &str) -> PathBuf {
        self.state.join("archive/tasks").join(id)
    }
}

fn stdout(cmd: &mut Command) -> String {
    String::from_utf8(cmd.assert().success().get_output().stdout.clone()).unwrap()
}

fn refused(cmd: &mut Command) -> String {
    String::from_utf8(cmd.assert().code(2).get_output().stderr.clone()).unwrap()
}

fn listed_ids(state: &Path) -> Vec<String> {
    let mut cmd = common::flow();
    cmd.env("FLOW_STATE", state)
        .args(["status", "--all", "--json"]);
    let out: serde_json::Value = serde_json::from_str(&stdout(&mut cmd)).unwrap();
    out["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["task"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn archive_moves_a_done_task_out_of_view_and_logs_it() {
    let fx = Fixture::new();

    let out = stdout(fx.flow().args(["task", "archive", DONE]));

    assert_eq!(
        out,
        format!(
            "archived {DONE} to {}; its working directory /work/repo on host-a is left as it \
             was\n",
            fx.archived(DONE).display()
        )
    );
    assert!(!fx.exists(DONE));
    let archived = events::read_all(&fx.archived(DONE)).unwrap();
    assert_eq!(archived.last().unwrap().kind, "archive");
    assert!(fx.archived(DONE).join("brief.md").exists());
    assert_eq!(listed_ids(&fx.state), [OPEN]);
    fx.flow().args(["task", "show", DONE]).assert().failure();
}

#[test]
fn delete_removes_the_record() {
    let fx = Fixture::new();
    fx.store
        .set_status(OPEN, |status| status.state = State::Orphaned)
        .unwrap();

    let out = stdout(fx.flow().args(["task", "archive", "--delete", DONE, OPEN]));

    assert_eq!(
        out,
        format!(
            "deleted {DONE}; its working directory /work/repo on host-a is left as it was\n\
             deleted {OPEN}; its working directory /work/repo on host-a is left as it was\n"
        )
    );
    assert!(!fx.exists(DONE) && !fx.exists(OPEN));
    assert!(!fx.archived(DONE).exists());
    assert_eq!(fs::read_dir(fx.state.join("tasks")).unwrap().count(), 0);
}

#[test]
fn an_open_task_is_refused_and_nothing_changes() {
    let fx = Fixture::new();

    let err = refused(fx.flow().args(["task", "archive", DONE, OPEN]));

    assert_eq!(
        err,
        format!(
            "task {OPEN} is working, still open; `flow task orphan` it first if its work \
             will not finish; no task was changed\n"
        )
    );
    assert!(fx.exists(DONE) && fx.exists(OPEN));
}

#[test]
fn a_pane_on_a_live_host_is_refused_until_closed() {
    let fx = Fixture::new();
    fx.host("host-a", false);
    fx.target(DONE, "host-a");

    let err = refused(fx.flow().args(["task", "archive", DONE]));

    assert!(err.contains("`flow task close` it first"), "{err}");
    assert!(fx.exists(DONE));
}

#[test]
fn a_pane_on_a_gone_or_forgotten_host_does_not_hold_the_task() {
    let fx = Fixture::new();
    fx.host("er", true);
    fx.target(DONE, "er");
    fx.task("aaaaaaad", State::Orphaned);
    fx.target("aaaaaaad", "account-filter");

    stdout(fx.flow().args(["task", "archive", DONE, "aaaaaaad"]));

    assert!(fx.archived(DONE).exists() && fx.archived("aaaaaaad").exists());
}

#[test]
fn pending_requests_are_refused() {
    let fx = Fixture::new();
    let (req, _) = fx
        .store
        .add_request(
            DONE,
            &NewRequest {
                title: "sibling",
                host: None,
                brief: b"brief",
            },
        )
        .unwrap();

    let err = refused(fx.flow().args(["task", "archive", DONE]));

    assert!(err.contains(&req), "{err}");
    fx.store.consume_request(&req).unwrap();
    stdout(fx.flow().args(["task", "archive", DONE]));
}
