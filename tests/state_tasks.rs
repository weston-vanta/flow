use std::fs;
use std::path::{Path, PathBuf};

use flow::paths::StateDir;
use flow::state::Store;
use flow::state::config::Config;
use flow::state::events;
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
        cde: "cde-a".to_owned(),
        worktree: PathBuf::from("/workspaces/example-repo.wt/retry"),
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
fn config_loads_sample() {
    let repo = tempfile::tempdir().unwrap();
    let state = state_dir(repo.path());
    fs::write(
        state.root().join("config.toml"),
        r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/workspaces/example-repo"
default_branch = "main"

[[cdes]]
name = "cde-a"
env_id = "env-0001"
ssh_alias = "cde-a.example"
local_marker_env = ["EXAMPLE_ENV_ID", "env-0001"]

[[cdes]]
name = "cde-b"
env_id = "env-0002"
ssh_alias = "cde-b.example"
"#,
    )
    .unwrap();

    let config = Config::load(&state).unwrap();

    let names: Vec<_> = config.cdes.iter().map(|cde| cde.name.as_str()).collect();
    assert_eq!(names, ["cde-a", "cde-b"]);
    assert_eq!(config.cdes[0].env_id, "env-0001");
    assert_eq!(config.cdes[0].ssh_alias, "cde-a.example");
    assert_eq!(
        config.cdes[0].local_marker_env,
        Some(("EXAMPLE_ENV_ID".to_owned(), "env-0001".to_owned()))
    );
    assert_eq!(config.cdes[1].local_marker_env, None);
    assert_eq!(config.monorepo.path, Path::new("/workspaces/example-repo"));
    assert_eq!(config.monorepo.default_branch, "main");
    assert_eq!(config.worktree_tool.create, ["wt", "create", "{branch}"]);
}
