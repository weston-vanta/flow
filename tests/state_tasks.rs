use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use flow::paths::StateDir;
use flow::state::Store;
use flow::state::config::{Config, HostSource};
use flow::state::events::{self, Event};
use flow::state::task::{Harness, State, TaskRecord};
use jiff::Timestamp;
use serde_json::json;

fn state_dir(repo: &Path) -> StateDir {
    fs::create_dir(repo.join("state")).unwrap();
    StateDir::resolve(repo).unwrap()
}

fn sample_record(id: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_owned(),
        parent: None,
        title: "Add retry to the example client".to_owned(),
        host: "host-a".to_owned(),
        cwd: PathBuf::from("/workspaces/example-repo.wt/retry"),
        repo: Some("example-repo".to_owned()),
        harness: Harness::Claude,
        created: "2026-10-26T12:00:00Z".parse::<Timestamp>().unwrap(),
        owner: "supervisor".to_owned(),
    }
}

#[test]
fn create_then_read_roundtrip() {
    let repo = tempfile::tempdir().unwrap();
    let state = state_dir(repo.path());
    let store = Store::open(&state);
    let rec = sample_record("aaaaaaab");

    store.create_task(rec.clone(), "Do the thing.\n").unwrap();

    let (read, status) = store.task("aaaaaaab").unwrap();
    assert_eq!(read, rec);
    assert_eq!(status.state, State::Pending);
    let task_dir = state.root().join("tasks/aaaaaaab");
    assert_eq!(
        fs::read_to_string(task_dir.join("brief.md")).unwrap(),
        "Do the thing.\n"
    );
    let kinds: Vec<_> = events::read_all(&task_dir)
        .unwrap()
        .into_iter()
        .map(|ev| ev.kind)
        .collect();
    assert_eq!(kinds, ["task.created"]);
    assert_eq!(store.list_tasks().unwrap(), [(rec, status)]);
}

#[test]
fn set_status_appends_transition_event() {
    let repo = tempfile::tempdir().unwrap();
    let state = state_dir(repo.path());
    let store = Store::open(&state);
    store
        .create_task(sample_record("aaaaaaab"), "brief")
        .unwrap();

    store
        .set_status("aaaaaaab", |status| status.state = State::Working)
        .unwrap();

    let (_, status) = store.task("aaaaaaab").unwrap();
    assert_eq!(status.state, State::Working);
    let events = events::read_all(&state.root().join("tasks/aaaaaaab")).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, "status");
    assert_eq!(events[1].data, json!({"from": "pending", "to": "working"}));
}

#[test]
fn set_status_and_append_event_advance_last_event() {
    let repo = tempfile::tempdir().unwrap();
    let state = state_dir(repo.path());
    let store = Store::open(&state);
    store
        .create_task(sample_record("aaaaaaab"), "brief")
        .unwrap();
    let (_, created) = store.task("aaaaaaab").unwrap();

    thread::sleep(Duration::from_millis(5));
    store.set_status("aaaaaaab", |_| {}).unwrap();
    let (_, unchanged) = store.task("aaaaaaab").unwrap();
    assert!(unchanged.last_event > created.last_event);
    assert_eq!(unchanged.state, State::Pending);

    thread::sleep(Duration::from_millis(5));
    let ev = Event {
        ts: Timestamp::now(),
        source: "hook".to_owned(),
        kind: "session.start".to_owned(),
        data: json!({}),
    };
    store.append_event("aaaaaaab", &ev).unwrap();
    let (_, after_event) = store.task("aaaaaaab").unwrap();
    assert_eq!(after_event.last_event, ev.ts);
    let logged = events::read_all(&state.root().join("tasks/aaaaaaab")).unwrap();
    assert_eq!(logged.last(), Some(&ev));
}

#[test]
fn config_loads_sample() {
    let repo = tempfile::tempdir().unwrap();
    let state = state_dir(repo.path());
    fs::write(
        state.root().join("config.toml"),
        r#"
[[repos]]
name = "example-repo"
path = "/workspaces/example-repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[hosts]
source = "static"

[[hosts.static]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"
local_marker_env = ["EXAMPLE_ENV_ID", "env-0001"]

[[hosts.static]]
name = "host-b"
env_id = "env-0002"
ssh_alias = "host-b.example"
"#,
    )
    .unwrap();

    let config = Config::load(&state).unwrap();

    let hosts = &config.hosts.static_hosts;
    assert_eq!(config.hosts.source, HostSource::Static);
    let names: Vec<_> = hosts.iter().map(|host| host.name.as_str()).collect();
    assert_eq!(names, ["host-a", "host-b"]);
    assert_eq!(hosts[0].env_id, "env-0001");
    assert_eq!(hosts[0].ssh_alias, "host-a.example");
    assert_eq!(
        hosts[0].local_marker_env,
        Some(("EXAMPLE_ENV_ID".to_owned(), "env-0001".to_owned()))
    );
    assert_eq!(hosts[1].local_marker_env, None);
    let repo = config.repo(None).unwrap();
    assert_eq!(repo.name, "example-repo");
    assert_eq!(repo.path, Path::new("/workspaces/example-repo"));
    assert_eq!(repo.default_branch, "main");
    assert_eq!(
        repo.worktree_tool.as_ref().map(|tool| tool.create.clone()),
        Some(["wt", "create", "{branch}"].map(str::to_owned).to_vec())
    );
    assert_eq!(config.herdr_sessions, ["flow", "default"]);
}
