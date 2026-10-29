use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::task::{ClaudeSession, Harness, HerdrTarget, Origin, State, TaskRecord};
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

const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";

const FLOW_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"working","cwd":"/work/repo.wt/feature","pane_id":"w1:p1","terminal_title":"✳ aaaaaaab","terminal_title_stripped":"aaaaaaab","workspace_id":"w1"}],"type":"agent_list"}}"#;

const DEFAULT_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"idle","cwd":"/work/repo","pane_id":"w1:p1","terminal_title":"✳ Tidy docs","terminal_title_stripped":"Tidy docs","workspace_id":"w1"}],"type":"agent_list"}}"#;

const HAND_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";
const TASK_SESSION_ID: &str = "00000000-0000-4000-8000-000000000002";
const TASK_ID: &str = "aaaaaaab";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    /// A state dir with `CONFIG`, a Claude home holding the registry and transcript fixtures for
    /// a hand session in `/work/repo`, and one task whose worker runs in flow's pane `w1:p1` on
    /// `host-a`.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        write_hosts(&state);

        let claude_fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude");
        let home = dir.path().join("claude");
        fs::create_dir_all(home.join("sessions")).unwrap();
        fs::copy(
            claude_fixtures.join("sessions/1.json"),
            home.join("sessions/1.json"),
        )
        .unwrap();
        let project = home.join("projects/-work-repo");
        fs::create_dir_all(&project).unwrap();
        fs::copy(
            claude_fixtures.join("transcript.jsonl"),
            project.join(format!("{HAND_SESSION_ID}.jsonl")),
        )
        .unwrap();

        let store = Store::open(&StateDir::resolve(dir.path()).unwrap());
        store
            .create_task(
                TaskRecord {
                    id: TASK_ID.to_owned(),
                    parent: None,
                    title: "Build feature".to_owned(),
                    host: "host-a".to_owned(),
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
            .set_status(TASK_ID, |status| {
                status.state = State::Working;
                status.herdr = Some(HerdrTarget {
                    host: "host-a".to_owned(),
                    workspace_id: "w1".to_owned(),
                    pane_id: "w1:p1".to_owned(),
                    agent: "claude".to_owned(),
                    session: SESSION.to_owned(),
                });
                status.claude_session = Some(ClaudeSession {
                    session_id: TASK_SESSION_ID.to_owned(),
                    transcript_path: home.join(format!("projects/x/{TASK_SESSION_ID}.jsonl")),
                });
            })
            .unwrap();
        Fixture { dir, state }
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env("HOME", self.dir.path())
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("claude"));
        cmd
    }

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

    fn cache_path(&self, env_id: &str) -> PathBuf {
        self.state.join("hosts").join(format!("{env_id}.json"))
    }

    fn cached(&self, env_id: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.cache_path(env_id)).unwrap()).unwrap()
    }

    fn write_cache(&self, env_id: &str, inventory: &Value) {
        fs::create_dir_all(self.state.join("hosts")).unwrap();
        fs::write(self.cache_path(env_id), inventory.to_string()).unwrap();
    }

    fn follow_list(&self) -> Value {
        let path = self.state.join("supervisor/follow.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn single_host(&self) {
        fs::remove_file(self.cache_path("env-0002")).unwrap();
    }
}

fn on(host: &str, stdout: &str) -> Value {
    json!({"host": host, "status": 0, "stdout": stdout})
}

fn default_not_running(host: &str) -> Value {
    json!({
        "host": host, "status": 1, "stdout": "",
        "stderr": include_str!("fixtures/herdr/agent_list_server_not_running.json"),
    })
}

fn host_a_agents() -> Vec<Value> {
    vec![
        on("host-a", SERVER_RUNNING),
        on("host-a", FLOW_AGENTS),
        on("host-a", DEFAULT_AGENTS),
    ]
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|w| (*w).to_owned()).collect()
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

/// A cached inventory of `host-b` holding one hand session in pane `w5:p1`.
fn host_b_cache() -> Value {
    json!({
        "name": "host-b", "env_id": "env-0002", "seen": "2026-10-26T09:00:00Z",
        "reachable": true, "load1": 0.5,
        "worktrees": [{"repo": "example-repo", "path": "/work/repo", "branch": "main", "busy": null, "dirty_files": null}],
        "agents": [{
            "pane_id": "w5:p1", "workspace_id": "w5", "agent": "claude", "status": "blocked",
            "cwd": "/work/other", "title": "Upgrade toolchain", "session": "default",
        }],
        "prs": [], "tool_error": null, "herdr_error": null, "pr_error": null,
    })
}

#[test]
fn sessions_lists_observed_with_summary() {
    let fx = Fixture::new();
    fx.single_host();

    let stdout = stdout_of(fx.flow(&host_a_agents()).arg("sessions"));

    assert_eq!(
        stdout,
        "HOST    STATUS   TASK      TITLE\n\
         host-a  working  aaaaaaab  Build feature\n\
         host-a  idle     observed  Tidy docs\n"
    );
    let argvs: Vec<Vec<String>> = fx.calls().into_iter().map(|(_, argv)| argv).collect();
    assert_eq!(
        argvs,
        [
            words(&["herdr", "--session", "flow", "status", "server"]),
            words(&["herdr", "--session", "flow", "agent", "list"]),
            words(&["herdr", "--session", "default", "agent", "list"]),
        ]
    );

    let cached = fx.cached("env-0001");
    assert_eq!(cached["reachable"], true);
    assert_eq!(cached["worktrees"], json!([]));
    assert_eq!(cached["prs"], json!([]));
    let sessions: Vec<&Value> = cached["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| &agent["session"])
        .collect();
    assert_eq!(sessions, [&json!("flow"), &json!("default")]);

    let stdout = stdout_of(fx.flow(&host_a_agents()).args(["sessions", "--summaries"]));
    assert_eq!(
        stdout,
        "HOST    STATUS   TASK      TITLE\n\
         host-a  working  aaaaaaab  Build feature\n\
         host-a  idle     observed  Tidy docs\n\
         \x20                          Moved the guides into one folder and fixed the links \
         between them; waiting for a review of the new …\n"
    );

    let json_out = stdout_of(fx.flow(&host_a_agents()).args(["sessions", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    assert_eq!(parsed[0]["task"], TASK_ID);
    assert_eq!(parsed[1]["task"], Value::Null);
    assert_eq!(parsed[0]["matched_by"], Value::Null);
    assert_eq!(parsed[1]["matched_by"], "cwd");
    assert_eq!(parsed[1]["session"]["session_id"], HAND_SESSION_ID);
    assert_eq!(parsed[1]["session"]["name"], "tidy-docs");
    assert!(
        parsed[1]["summary"]
            .as_str()
            .unwrap()
            .ends_with("review of the new index page."),
        "{json_out}"
    );
}

const SECOND_SESSION_ID: &str = "00000000-0000-4000-8000-000000000003";

/// Two hand sessions in `/work/repo`, each reporting its own Claude Code session id.
const DEFAULT_AGENTS_WITH_IDS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"00000000-0000-4000-8000-000000000001"},"agent_status":"idle","cwd":"/work/repo","pane_id":"w1:p1","terminal_title_stripped":"Tidy docs","workspace_id":"w1"},{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"00000000-0000-4000-8000-000000000003"},"agent_status":"working","cwd":"/work/repo","pane_id":"w1:p2","terminal_title_stripped":"Fix links","workspace_id":"w1"}],"type":"agent_list"}}"#;

#[test]
fn sessions_match_hand_sessions_by_the_id_herdr_reports() {
    let fx = Fixture::new();
    fx.single_host();
    // Newer than the fixture's session in the same cwd, so a cwd match would pick it for both.
    let home = fx.dir.path().join("claude");
    let registry = json!({
        "sessionId": SECOND_SESSION_ID, "cwd": "/work/repo", "status": "busy",
        "updatedAt": 1_793_000_900_000_i64, "pidDomain": "machine-0001",
    });
    fs::write(home.join("sessions/2.json"), registry.to_string()).unwrap();
    let summary = json!({"type": "system", "subtype": "away_summary", "content": "Fixing links."});
    fs::write(
        home.join(format!("projects/-work-repo/{SECOND_SESSION_ID}.jsonl")),
        format!("{summary}\n"),
    )
    .unwrap();
    let script = [
        on("host-a", SERVER_RUNNING),
        on("host-a", FLOW_AGENTS),
        on("host-a", DEFAULT_AGENTS_WITH_IDS),
    ];

    let json_out = stdout_of(fx.flow(&script).args(["sessions", "--json"]));

    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    let matched: Vec<(&Value, &Value)> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                &entry["session"]["session_id"],
                &entry["agent"]["agent_session_id"],
            )
        })
        .collect();
    assert_eq!(
        matched,
        [
            (&Value::Null, &Value::Null),
            (&json!(HAND_SESSION_ID), &json!(HAND_SESSION_ID)),
            (&json!(SECOND_SESSION_ID), &json!(SECOND_SESSION_ID)),
        ],
        "{json_out}"
    );
    assert!(
        parsed[1]["summary"]
            .as_str()
            .unwrap()
            .ends_with("review of the new index page."),
        "{json_out}"
    );
    assert_eq!(parsed[2]["summary"], "Fixing links.");
    let matched_by: Vec<&Value> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["matched_by"])
        .collect();
    assert_eq!(matched_by, [&Value::Null, &json!("id"), &json!("id")]);
}

#[test]
fn sessions_without_agents_say_so() {
    let fx = Fixture::new();
    fx.single_host();
    let script = [
        on("host-a", SERVER_RUNNING),
        on(
            "host-a",
            r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#,
        ),
        default_not_running("host-a"),
    ];

    assert_eq!(stdout_of(fx.flow(&script).arg("sessions")), "no agents\n");
}

#[test]
fn sessions_update_only_agents_in_an_existing_cache() {
    let fx = Fixture::new();
    fx.single_host();
    let earlier = json!({
        "name": "host-a", "env_id": "env-0001", "seen": "2026-10-26T09:00:00Z",
        "reachable": true, "load1": 1.25,
        "worktrees": [{"repo": "example-repo", "path": "/work/repo", "branch": "main", "busy": null, "dirty_files": null}],
        "agents": [],
        "prs": [{"repo": "example-repo", "branch": "main", "number": 3, "url": "https://example.com/pull/3", "state": "OPEN"}],
        "tool_error": "wt: command not found", "herdr_error": null, "pr_error": null,
    });
    fx.write_cache("env-0001", &earlier);
    let script = [
        on("host-a", SERVER_RUNNING),
        on("host-a", FLOW_AGENTS),
        default_not_running("host-a"),
    ];

    let stdout = stdout_of(fx.flow(&script).arg("sessions"));

    assert_eq!(
        stdout,
        "HOST    STATUS   TASK      TITLE\nhost-a  working  aaaaaaab  Build feature\n"
    );
    let cached = fx.cached("env-0001");
    assert_ne!(cached["seen"], earlier["seen"]);
    assert_eq!(cached["agents"][0]["pane_id"], "w1:p1");
    assert_eq!(cached["agents"].as_array().unwrap().len(), 1);
    for unchanged in ["load1", "worktrees", "prs", "tool_error"] {
        assert_eq!(cached[unchanged], earlier[unchanged], "{unchanged}");
    }
}

#[test]
fn sessions_reaching_herdr_clear_a_probe_failure() {
    let fx = Fixture::new();
    fx.single_host();
    let mut earlier = host_b_cache();
    earlier["name"] = json!("host-a");
    earlier["env_id"] = json!("env-0001");
    earlier["reachable"] = json!(false);
    earlier["probe_error"] = json!("ssh: connect to host host-a.example: timed out");
    fx.write_cache("env-0001", &earlier);
    let script = [
        on("host-a", SERVER_RUNNING),
        on("host-a", FLOW_AGENTS),
        default_not_running("host-a"),
    ];

    stdout_of(fx.flow(&script).arg("sessions"));

    let cached = fx.cached("env-0001");
    assert_eq!(cached["reachable"], true);
    assert_eq!(cached["probe_error"], Value::Null);
    let inventory = stdout_of(fx.flow(&[]).arg("inventory"));
    assert!(
        inventory.starts_with("host-a  reachable · load 0.50 · seen "),
        "{inventory}"
    );
    assert!(!inventory.contains("probe failed"), "{inventory}");
}

#[test]
fn sessions_record_a_flow_session_failure() {
    let fx = Fixture::new();
    fx.single_host();
    let mut earlier = host_b_cache();
    earlier["name"] = json!("host-a");
    earlier["env_id"] = json!("env-0001");
    earlier["herdr_error"] = json!("an older herdr failure");
    fx.write_cache("env-0001", &earlier);
    let script = [
        json!({"host": "host-a", "status": 1, "stdout": "", "stderr": "internal error\n"}),
        on("host-a", DEFAULT_AGENTS),
    ];

    let out = fx
        .flow(&script)
        .arg("sessions")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.contains("herdr --session flow status server") && stderr.contains("internal error"),
        "{stderr}"
    );
    assert_eq!(
        stdout,
        "HOST    STATUS  TASK      TITLE\nhost-a  idle    observed  Tidy docs\n"
    );
    let cached = fx.cached("env-0001");
    assert_eq!(cached["herdr_error"], stderr.trim_end());
    assert_eq!(cached["agents"].as_array().unwrap().len(), 1);
    assert_eq!(cached["agents"][0]["session"], "default");

    stdout_of(fx.flow(&host_a_agents()).arg("sessions"));
    assert_eq!(fx.cached("env-0001")["herdr_error"], Value::Null);
}

#[test]
fn sessions_fall_back_to_cache_for_unreachable_host() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let mut script = vec![json!({
        "host": "host-b", "status": 255, "stdout": "", "repeat": true,
        "stderr": "ssh: Could not resolve hostname host-b.example: Name or service not known\n",
    })];
    script.extend(host_a_agents());

    let out = fx
        .flow(&script)
        .arg("sessions")
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("host unreachable: host-b"), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{stdout}");
    assert_eq!(lines[3], "host-b  blocked  observed  Upgrade toolchain");
    assert_eq!(fx.cached("env-0002"), host_b_cache());
}

#[test]
fn status_counts_observed_sessions_from_cache() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let mut host_a = host_b_cache();
    host_a["name"] = json!("host-a");
    host_a["env_id"] = json!("env-0001");
    host_a["agents"] = json!([
        {"pane_id": "w1:p1", "workspace_id": "w1", "agent": "claude", "status": "working",
         "cwd": "/work/repo.wt/feature", "title": "Build feature", "session": "flow"},
        {"pane_id": "w1:p1", "workspace_id": "w1", "agent": "claude", "status": "idle",
         "cwd": "/work/repo", "title": "Tidy docs", "session": "default"},
    ]);
    fx.write_cache("env-0001", &host_a);

    let stdout = stdout_of(fx.flow(&[]).arg("status"));

    assert_eq!(
        stdout.lines().last(),
        Some("observed: 2 sessions"),
        "{stdout}"
    );
    let json_out = stdout_of(fx.flow(&[]).args(["status", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    assert_eq!(parsed["observed"], 2);
    assert!(fx.calls().is_empty());
}

const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;

/// herdr's `default` session on `host-b` as `agents` (pane id, title) pairs list it.
fn host_b_agents(agents: &[(&str, &str)]) -> Vec<Value> {
    let agents: Vec<Value> = agents
        .iter()
        .map(|(pane_id, title)| {
            json!({
                "agent": "claude", "agent_status": "idle", "cwd": "/work/other",
                "pane_id": pane_id, "terminal_title_stripped": title, "workspace_id": "w5",
            })
        })
        .collect();
    let list = json!({"id": "cli:agent:list", "result": {"agents": agents, "type": "agent_list"}});
    vec![
        on("host-b", SERVER_RUNNING),
        on("host-b", NO_AGENTS),
        on("host-b", &list.to_string()),
    ]
}

fn followed(pane_id: &str, title: &str) -> Value {
    json!({"host": "host-b", "pane_id": pane_id, "session": "default", "title": title})
}

#[test]
fn follow_resolves_a_title_after_listing_the_host_and_is_idempotent() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let agents = [("w5:p1", "Upgrade toolchain"), ("w5:p2", "Fix links")];

    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["follow", "host-b", "Fix links"]),
    );
    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["follow", "host-b", "Fix links"]),
    );

    assert_eq!(
        fx.follow_list(),
        json!({"follow": [followed("w5:p2", "Fix links")]})
    );
    let calls = fx.calls();
    assert_eq!(calls.len(), 6, "{calls:?}");
    assert!(calls.iter().all(|(host, _)| host == "host-b"), "{calls:?}");
    assert_eq!(fx.cached("env-0002")["agents"].as_array().unwrap().len(), 2);
}

#[test]
fn follow_resolves_a_pane_id() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let agents = [("w5:p1", "Upgrade toolchain"), ("w5:p2", "Fix links")];

    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["follow", "host-b", "w5:p1"]),
    );

    assert_eq!(
        fx.follow_list(),
        json!({"follow": [followed("w5:p1", "Upgrade toolchain")]})
    );
}

#[test]
fn follow_refuses_a_title_several_sessions_share() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let agents = [("w5:p1", "Fix links"), ("w5:p2", "Fix links")];

    fx.flow(&host_b_agents(&agents))
        .args(["follow", "host-b", "Fix links"])
        .assert()
        .code(2)
        .stderr(
            "several hand sessions on host-b are titled \"Fix links\"; pass the pane id:\n\
             w5:p1  Fix links\n\
             w5:p2  Fix links\n",
        );
    assert!(!fx.state.join("supervisor/follow.json").exists());
}

#[test]
fn follow_refuses_what_names_no_observed_session() {
    let fx = Fixture::new();
    fx.single_host();

    let worker = "pane w1:p1 on host-a is already task aaaaaaab\n";
    for (target, refusal) in [
        (
            "Nothing here",
            "no hand session titled or in pane Nothing here on host-a; run flow sessions\n",
        ),
        ("aaaaaaab", worker),
        ("w1:p1", worker),
    ] {
        let script = [
            on("host-a", SERVER_RUNNING),
            on("host-a", FLOW_AGENTS),
            default_not_running("host-a"),
        ];
        fx.flow(&script)
            .args(["follow", "host-a", target])
            .assert()
            .code(2)
            .stderr(refusal);
    }
    assert!(!fx.state.join("supervisor/follow.json").exists());
}

#[test]
fn follow_uses_the_cache_when_the_host_cannot_be_listed() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let script = [json!({
        "host": "host-b", "status": 255, "stdout": "", "repeat": true,
        "stderr": "ssh: Could not resolve hostname host-b.example: Name or service not known\n",
    })];

    let out = fx
        .flow(&script)
        .args(["follow", "host-b", "Upgrade toolchain"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with(
            "could not list agents on host-b, using the cached ones: host unreachable: host-b"
        ),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert_eq!(
        fx.follow_list(),
        json!({"follow": [followed("w5:p1", "Upgrade toolchain")]})
    );
    assert_eq!(fx.cached("env-0002"), host_b_cache());
}

#[test]
fn follow_says_which_sessions_failed_when_the_rest_answered() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let mut script = host_b_agents(&[("w5:p2", "Fix links")]);
    script[1] = json!({"host": "host-b", "status": 1, "stdout": "", "stderr": "internal error\n"});

    let out = fx
        .flow(&script)
        .args(["follow", "host-b", "Fix links"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("could not list every herdr session on host-b: "),
        "{stderr}"
    );
    assert!(!stderr.contains("cached"), "{stderr}");
    assert_eq!(
        fx.follow_list(),
        json!({"follow": [followed("w5:p2", "Fix links")]})
    );
}

#[test]
fn unfollow_removes_a_followed_session_by_title() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let agents = [("w5:p1", "Upgrade toolchain")];
    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["follow", "host-b", "Upgrade toolchain"]),
    );

    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["unfollow", "host-b", "Upgrade toolchain"]),
    );

    assert_eq!(fx.follow_list(), json!({"follow": []}));
    fx.flow(&host_b_agents(&agents))
        .args(["unfollow", "host-b", "Upgrade toolchain"])
        .assert()
        .code(2)
        .stderr("not following Upgrade toolchain on host-b\n");
}

#[test]
fn unfollow_removes_a_stale_pane_id() {
    let fx = Fixture::new();
    fx.write_cache("env-0002", &host_b_cache());
    let follow = json!({"follow": [
        {"host": "host-b", "pane_id": "w9:p9", "session": "default", "title": "Gone session"},
    ]});
    fs::create_dir_all(fx.state.join("supervisor")).unwrap();
    fs::write(fx.state.join("supervisor/follow.json"), follow.to_string()).unwrap();
    let agents = [("w5:p1", "Upgrade toolchain")];

    stdout_of(
        fx.flow(&host_b_agents(&agents))
            .args(["unfollow", "host-b", "w9:p9"]),
    );

    assert_eq!(fx.follow_list(), json!({"follow": []}));
    fx.flow(&host_b_agents(&agents))
        .args(["unfollow", "host-b", "w8:p8"])
        .assert()
        .code(2)
        .stderr("no hand session titled or in pane w8:p8 on host-b; run flow sessions\n");
}

#[test]
fn sessions_mark_a_followed_hand_session() {
    let fx = Fixture::new();
    fx.single_host();
    stdout_of(
        fx.flow(&host_a_agents())
            .args(["follow", "host-a", "Tidy docs"]),
    );

    let stdout = stdout_of(fx.flow(&host_a_agents()).arg("sessions"));

    assert_eq!(
        stdout,
        "HOST    STATUS   TASK      TITLE\n\
         host-a  working  aaaaaaab  Build feature\n\
         host-a  idle     followed  Tidy docs\n"
    );
    let json_out = stdout_of(fx.flow(&host_a_agents()).args(["sessions", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    let marks: Vec<&Value> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["followed"])
        .collect();
    assert_eq!(marks, [&json!(false), &json!(true)], "{json_out}");
}

/// `default`'s agents on `host-a` when flow runs there too: the supervisor in `w1:p1`, task
/// `aaaaaaab`'s worker in `w2:p1`, a hand session in `w3:p1`, and one adopted as task
/// `aaaaaaac` in `w4:p1`.
const SHARED_SESSION_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
{"agent":"claude","agent_status":"idle","cwd":"/workspaces/flow","pane_id":"w1:p1","terminal_title_stripped":"flow-supervisor","workspace_id":"w1"},
{"agent":"claude","agent_status":"working","cwd":"/work/repo.wt/feature","pane_id":"w2:p1","terminal_title_stripped":"aaaaaaab","workspace_id":"w2"},
{"agent":"claude","agent_status":"idle","cwd":"/work/other","pane_id":"w3:p1","terminal_title_stripped":"Tidy docs","workspace_id":"w3"},
{"agent":"claude","agent_status":"idle","cwd":"/work/app","pane_id":"w4:p1","terminal_title_stripped":"Fix links","workspace_id":"w4"}
],"type":"agent_list"}}"#;

/// `default`'s workspaces on `host-a` when flow runs there: `w1` is the supervisor's.
const SHARED_SESSION_WORKSPACES: &str = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[
{"workspace_id":"w1","label":"flow-supervisor"},{"workspace_id":"w2","label":"Build feature · aaaaaaab"},
{"workspace_id":"w3","label":"docs"},{"workspace_id":"w4","label":"links"}]}}"#;

impl Fixture {
    /// flow in herdr's `default` session alone: the supervisor, task `TASK`'s worker and task
    /// `aaaaaaac`, adopted, all in it on `host-a`, as `SHARED_SESSION_AGENTS` lists them.
    fn sharing_default(&self) {
        fs::write(
            self.state.join("config.toml"),
            CONFIG.replace(
                "herdr_session = \"flow\"\nherdr_sessions = [\"flow\", \"default\"]\n",
                "",
            ),
        )
        .unwrap();
        let store = Store::open(&StateDir::resolve(self.dir.path()).unwrap());
        let target = |pane_id: &str| HerdrTarget {
            host: "host-a".to_owned(),
            workspace_id: pane_id.split(':').next().unwrap().to_owned(),
            pane_id: pane_id.to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        };
        store
            .set_status(TASK_ID, |status| status.herdr = Some(target("w2:p1")))
            .unwrap();
        let (mut adopted, _) = store.task(TASK_ID).unwrap();
        adopted.id = "aaaaaaac".to_owned();
        adopted.title = "Fix the links".to_owned();
        adopted.origin = Origin::Adopted;
        store.create_task(adopted, "brief").unwrap();
        store
            .set_status("aaaaaaac", |status| {
                status.state = State::Working;
                status.herdr = Some(target("w4:p1"));
            })
            .unwrap();
        flow::supervisor::take(
            &StateDir::resolve(self.dir.path()).unwrap(),
            "env-0001",
            target("w1:p1"),
        )
        .unwrap();
    }
}

#[test]
fn workers_the_supervisor_and_hand_sessions_share_herdrs_default_session() {
    let fx = Fixture::new();
    fx.single_host();
    fx.sharing_default();
    let script = [
        on("host-a", SERVER_RUNNING),
        on("host-a", SHARED_SESSION_AGENTS),
        on("host-a", SHARED_SESSION_WORKSPACES),
    ];

    let stdout = stdout_of(fx.flow(&script).arg("sessions"));

    assert_eq!(
        stdout,
        "HOST    STATUS   TASK        TITLE\n\
         host-a  working  aaaaaaab    Build feature\n\
         host-a  idle     aaaaaaac    Fix the links\n\
         host-a  idle     observed    Tidy docs\n\
         host-a  idle     supervisor  flow-supervisor\n"
    );
    let argvs: Vec<Vec<String>> = fx.calls().into_iter().map(|(_, argv)| argv).collect();
    assert_eq!(
        argvs,
        [
            words(&["herdr", "--session", "default", "status", "server"]),
            words(&["herdr", "--session", "default", "agent", "list"]),
            words(&["herdr", "--session", "default", "workspace", "list"]),
        ]
    );
    let json_out = stdout_of(fx.flow(&script).args(["sessions", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    let supervisor: Vec<bool> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["supervisor"].as_bool().unwrap())
        .collect();
    assert_eq!(supervisor, [true, false, false, false]);

    let status = stdout_of(fx.flow(&[]).arg("status"));
    assert_eq!(
        status.lines().last(),
        Some("observed: 1 sessions"),
        "{status}"
    );

    for (target, refusal) in [
        (
            "flow-supervisor",
            "pane w1:p1 on host-a is the supervisor's\n",
        ),
        ("w4:p1", "pane w4:p1 on host-a is already task aaaaaaac\n"),
    ] {
        fx.flow(&script)
            .args(["follow", "host-a", target])
            .assert()
            .code(2)
            .stderr(refusal);
    }
    stdout_of(fx.flow(&script).args(["follow", "host-a", "Tidy docs"]));
    assert_eq!(
        fx.follow_list(),
        json!({"follow": [{"host": "host-a", "pane_id": "w3:p1", "session": "default", "title": "Tidy docs"}]})
    );
}

/// `SHARED_SESSION_AGENTS` after herdr gave the panes of the supervisor, task `aaaaaaab` and the
/// done task `aaaaaaac` to sessions the user started: the first two report Claude Code sessions
/// of their own, the third none.
const REUSED_PANE_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"s-hand-1"},"agent_status":"idle","cwd":"/workspaces/flow","pane_id":"w1:p1","terminal_title_stripped":"Read the docs","workspace_id":"w1"},
{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"s-hand-2"},"agent_status":"working","cwd":"/work/repo.wt/feature","pane_id":"w2:p1","terminal_title_stripped":"Try a fix","workspace_id":"w2"},
{"agent":"claude","agent_status":"idle","cwd":"/work/app","pane_id":"w4:p1","terminal_title_stripped":"Fix links","workspace_id":"w4"}
],"type":"agent_list"}}"#;

#[test]
fn hand_sessions_in_reused_panes_are_observed_and_can_be_followed() {
    let fx = Fixture::new();
    fx.single_host();
    fx.sharing_default();
    let state = StateDir::resolve(fx.dir.path()).unwrap();
    flow::supervisor::set_session(&state, Some("s-sup")).unwrap();
    Store::open(&state)
        .set_status("aaaaaaac", |status| status.state = State::Done)
        .unwrap();
    let script = [
        on("host-a", SERVER_RUNNING),
        on("host-a", REUSED_PANE_AGENTS),
        on("host-a", SHARED_SESSION_WORKSPACES),
    ];

    let json_out = stdout_of(fx.flow(&script).args(["sessions", "--json"]));

    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    let rows: Vec<(String, Value, bool)> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["agent"]["pane_id"].as_str().unwrap().to_owned(),
                entry["task"].clone(),
                entry["supervisor"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("w1:p1".to_owned(), Value::Null, false),
            ("w2:p1".to_owned(), Value::Null, false),
            ("w4:p1".to_owned(), Value::Null, false),
        ]
    );
    for target in ["Read the docs", "Try a fix", "Fix links"] {
        stdout_of(fx.flow(&script).args(["follow", "host-a", target]));
    }
    let list = fx.follow_list();
    let followed: Vec<&str> = list["follow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["pane_id"].as_str().unwrap())
        .collect();
    assert_eq!(followed, ["w1:p1", "w2:p1", "w4:p1"]);
}

#[test]
fn a_session_in_the_flow_checkout_is_the_supervisor_only_in_its_labelled_workspace() {
    let fx = Fixture::new();
    fx.single_host();
    fx.sharing_default();
    let relabelled = SHARED_SESSION_WORKSPACES.replace("flow-supervisor", "scratch");
    let rows = |workspaces: &str| {
        let script = [
            on("host-a", SERVER_RUNNING),
            on("host-a", SHARED_SESSION_AGENTS),
            on("host-a", workspaces),
        ];
        let json_out = stdout_of(fx.flow(&script).args(["sessions", "--json"]));
        let parsed: Value = serde_json::from_str(&json_out).unwrap();
        parsed
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["agent"]["pane_id"] == "w1:p1")
            .map(|entry| entry["supervisor"].as_bool().unwrap())
            .collect::<Vec<_>>()
    };

    assert_eq!(rows(SHARED_SESSION_WORKSPACES), [true]);
    assert_eq!(rows(&relabelled), [false]);
}
