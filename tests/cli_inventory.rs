use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};

/// Lists herdr's `default` session beside flow's own, `flow` once `write_lock` records it.
const CONFIG: &str = r#"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[hosts]
source = "static"

[[hosts.static]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"

[[hosts.static]]
name = "host-b"
env_id = "env-0002"
ssh_alias = "host-b.example"
"#;

const PORCELAIN: &str = "\
worktree /workspaces/acme
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /workspaces/acme-worktrees/feature-x
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature-x

worktree /workspaces/acme-worktrees/bisect
HEAD 3333333333333333333333333333333333333333
detached

";

const WT_LIST: &str = r#"{"repo":"/workspaces/acme","worktrees":[{"name":"feature-x","path":"/workspaces/acme-worktrees/feature-x","mounted":true,"branch":"feature-x","dirty_files":3,"busy":true,"upper_bytes":40960}]}"#;

const LOADAVG: &str = "0.26 0.38 0.68 2/1501 1003799\n";

const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";

const AGENT_LIST: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"working","cwd":"/workspaces/acme-worktrees/feature-x","pane_id":"w4:p1","terminal_title":"✳ Build feature x","terminal_title_stripped":"Build feature x","workspace_id":"w4"}],"type":"agent_list"}}"#;

const PR_LIST: &str = r#"[{"headRefName":"feature-x","number":12,"state":"OPEN","url":"https://github.com/example/acme/pull/12"},{"headRefName":"elsewhere","number":13,"state":"OPEN","url":"https://github.com/example/acme/pull/13"}]"#;
/// Records a supervisor that `flow up` ran in herdr session `session`, which makes it
/// flow's own session.
fn write_lock(state: &Path, session: &str) {
    let dir = state.join("supervisor");
    fs::create_dir_all(&dir).unwrap();
    let lock = json!({
        "host_env_id": "env-0001",
        "herdr": {"host": "host-a", "workspace_id": "w9", "pane_id": "w9:p1", "agent": "claude",
            "session": session},
        "claude_session": null,
        "since": "2026-10-26T12:00:00Z",
    });
    fs::write(dir.join("lock.json"), lock.to_string()).unwrap();
}

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
        write_lock(&state, "flow");
        Fixture { dir, state }
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on this state dir as `host-a`, answering runner calls from `script`.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env("FLOW_FAKE_ONA", self.ona_path());
        cmd
    }

    /// The Ona listing `flow` reads; missing until `write_ona`, so listing fails by default.
    fn ona_path(&self) -> PathBuf {
        self.dir.path().join("ona.json")
    }

    fn write_ona(&self, listing: &Value) {
        fs::write(self.ona_path(), listing.to_string()).unwrap();
    }

    fn use_ona_fixture(&self) {
        fs::copy(ona_fixture(), self.ona_path()).unwrap();
    }

    /// `(host, argv)` of every runner call.
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
}

fn on(host: &str, stdout: &str) -> Value {
    json!({"host": host, "status": 0, "stdout": stdout})
}

/// `herdr --session default agent list` on `host` with no default server running.
fn default_session_stopped(host: &str) -> Value {
    json!({
        "host": host, "status": 1, "stdout": "",
        "stderr": include_str!("fixtures/herdr/agent_list_server_not_running.json"),
    })
}

/// Every probe of `host-a` succeeding, in call order.
fn host_a_probes() -> Vec<Value> {
    probes("host-a")
}

/// Every probe of `host` succeeding, in call order.
fn probes(host: &str) -> Vec<Value> {
    vec![
        on(host, PORCELAIN),
        on(host, WT_LIST),
        on(host, LOADAVG),
        on(host, SERVER_RUNNING),
        on(host, AGENT_LIST),
        default_session_stopped(host),
    ]
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|w| (*w).to_owned()).collect()
}

/// The time `days` days before now, as cached inventories record it.
fn days_ago(days: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * days)).to_string()
}

/// `stdout` with every age in seconds read as `0s`: the second can tick over between a host's
/// probe and the render.
fn settled(stdout: &str) -> String {
    let mut out = String::new();
    let mut rest = stdout;
    while let Some(at) = rest.find("seen ") {
        let (before, from) = rest.split_at(at + "seen ".len());
        out.push_str(before);
        let digits = from.len() - from.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && from[digits..].starts_with("s ago") {
            out.push('0');
            rest = &from[digits..];
        } else {
            rest = from;
        }
    }
    out.push_str(rest);
    out
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

fn single_host_config(fx: &Fixture) {
    let config = CONFIG
        .split("[[hosts.static]]")
        .take(2)
        .collect::<Vec<_>>()
        .join("[[hosts.static]]");
    fs::write(fx.state.join("config.toml"), config).unwrap();
}

#[test]
fn refresh_parses_worktrees_load_and_joins_prs() {
    let fx = Fixture::new();
    single_host_config(&fx);
    let mut stale = bare_record("host-a", "env-0001", "host-a.example");
    stale["reachable"] = json!(false);
    stale["probe_error"] = json!("ssh: connect to host host-a.example: timed out");
    fx.write_cache("env-0001", &stale);
    let mut script = host_a_probes();
    script.push(on("host-a", PR_LIST));

    let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    assert_eq!(
        settled(&stdout),
        "\
host-a  reachable · load 0.26 · seen 0s ago
  agents (1)
    working [flow]  Build feature x  /workspaces/acme-worktrees/feature-x
  repos
    example-repo  on main · 2 worktrees
      feature-x   PR #12 open
      (detached)               /workspaces/acme-worktrees/bisect
"
    );

    let cached = fx.cached("env-0001");
    assert_eq!(cached["name"], "host-a");
    assert_eq!(cached["env_id"], "env-0001");
    assert_eq!(cached["reachable"], true);
    assert_eq!(
        cached["probe_error"],
        Value::Null,
        "a probe that answers clears it"
    );
    assert_eq!(cached["load1"], 0.26);
    assert_eq!(
        cached["worktrees"],
        json!([
            {"repo": "example-repo", "path": "/workspaces/acme", "branch": "main", "busy": null, "dirty_files": null},
            {"repo": "example-repo", "path": "/workspaces/acme-worktrees/feature-x", "branch": "feature-x", "busy": true, "dirty_files": 3},
            {"repo": "example-repo", "path": "/workspaces/acme-worktrees/bisect", "branch": null, "busy": null, "dirty_files": null},
        ])
    );
    assert_eq!(
        cached["prs"],
        json!([{"repo": "example-repo", "branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"}])
    );
    assert_eq!(cached["agents"][0]["pane_id"], "w4:p1");
    assert_eq!(cached["tool_error"], Value::Null);
    assert_eq!(cached["herdr_error"], Value::Null);

    let in_repo = |tool: &[&str]| {
        let mut argv = words(&[
            "sh",
            "-c",
            r#"cd "$1" && shift && exec "$@""#,
            "sh",
            "/workspaces/acme",
        ]);
        argv.extend(words(tool));
        argv
    };
    let calls = fx.calls();
    let argvs: Vec<&Vec<String>> = calls.iter().map(|(_, argv)| argv).collect();
    assert_eq!(
        argvs,
        [
            &words(&[
                "git",
                "-C",
                "/workspaces/acme",
                "worktree",
                "list",
                "--porcelain"
            ]),
            &in_repo(&["wt", "--json", "list"]),
            &words(&["cat", "/proc/loadavg"]),
            &words(&["herdr", "--session", "flow", "status", "server"]),
            &words(&["herdr", "--session", "flow", "agent", "list"]),
            &words(&["herdr", "--session", "default", "agent", "list"]),
            &in_repo(&[
                "gh",
                "pr",
                "list",
                "--author",
                "@me",
                "--state",
                "open",
                "--json",
                "number,url,headRefName,state",
                "--limit",
                "100",
            ]),
        ]
    );
}

#[test]
fn unreachable_host_is_recorded_not_fatal() {
    let fx = Fixture::new();
    let earlier = days_ago(2);
    fx.write_cache(
        "env-0002",
        &json!({
            "name": "host-b", "env_id": "env-0002", "ssh_alias": "host-b.example", "seen": earlier,
            "reachable": true, "load1": 1.5,
            "worktrees": [{"repo": "example-repo", "path": "/workspaces/acme-worktrees/feature-y", "branch": "feature-y", "busy": false, "dirty_files": 0}],
            "agents": [],
            "prs": [{"repo": "example-repo", "branch": "feature-y", "number": 14, "url": "https://github.com/example/acme/pull/14", "state": "OPEN"}],
            "tool_error": null, "herdr_error": null,
        }),
    );
    let mut script = vec![json!({
        "host": "host-b", "status": 255, "stdout": "", "repeat": true,
        "stderr": "ssh: Could not resolve hostname host-b.example: Name or service not known\n",
    })];
    script.extend(host_a_probes());
    script.push(on("host-a", PR_LIST));

    let out = fx
        .flow(&script)
        .args(["inventory", "--refresh"])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("host-b") && stderr.contains("Could not resolve hostname"),
        "{stderr}"
    );
    let blocks: Vec<&str> = stdout.split("\n\n").collect();
    assert_eq!(blocks.len(), 2, "{stdout}");
    assert!(blocks[0].starts_with("host-a  reachable · "), "{stdout}");
    assert_eq!(
        blocks[1],
        "\
host-b  unreachable · seen 2d ago
  probe failed: ssh: Could not resolve hostname host-b.example: Name or service not known
  agents (0)
  repos
    example-repo  1 worktree
      feature-y  PR #14 open
",
        "{stdout}"
    );

    let cached = fx.cached("env-0002");
    assert_eq!(cached["reachable"], false);
    assert_eq!(cached["seen"], earlier.as_str());
    assert_eq!(
        cached["probe_error"],
        "ssh: Could not resolve hostname host-b.example: Name or service not known"
    );
    assert_eq!(cached["load1"], Value::Null);
    assert_eq!(cached["worktrees"][0]["branch"], "feature-y");
    assert_eq!(cached["prs"][0]["number"], 14);
    let host_b_calls = fx
        .calls()
        .into_iter()
        .filter(|(host, _)| host == "host-b")
        .count();
    assert_eq!(
        host_b_calls, 1,
        "probing stops at the first unreachable answer"
    );
}

#[test]
fn inventory_without_refresh_reads_cache() {
    let fx = Fixture::new();
    let seen = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(5);
    fx.write_cache(
        "env-0001",
        &json!({
            "name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example",
            "seen": seen.to_string(), "reachable": true, "load1": 0.5,
            "worktrees": [
                {"repo": "example-repo", "path": "/workspaces/acme-worktrees/feature-x", "branch": "feature-x", "busy": true, "dirty_files": 2},
            ],
            "agents": [{
                "pane_id": "w4:p1", "workspace_id": "w4", "agent": "claude", "status": "idle",
                "cwd": "/workspaces/acme-worktrees/feature-x", "title": "", "session": "flow",
            }],
            "prs": [{"repo": "example-repo", "branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"}],
            "tool_error": "wt: command not found",
            "herdr_error": null,
        }),
    );

    let stdout = stdout_of(fx.flow(&[]).arg("inventory"));

    assert_eq!(
        stdout,
        "\
host-a  reachable · load 0.50 · seen 5m ago
  worktree tool failed: wt: command not found
  agents (1)
    idle [flow]  w4:p1  /workspaces/acme-worktrees/feature-x
  repos
    example-repo  1 worktree
      feature-x  PR #12 open
"
    );
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());

    let json_out = stdout_of(fx.flow(&[]).args(["inventory", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    assert_eq!(parsed[0]["name"], "host-a");
    assert_eq!(parsed[0]["worktrees"][0]["dirty_files"], 2);
    assert!(fx.calls().is_empty());
}

#[test]
fn inventory_json_describes_each_host() {
    let fx = Fixture::new();
    let mut host_a = bare_record("host-a", "env-0001", "env-0001.ona.environment");
    host_a["ona_phase"] = json!("ENVIRONMENT_PHASE_RUNNING");
    fx.write_cache("env-0001", &host_a);
    let mut host_b = bare_record("host-b", "env-0002", "host-b.example");
    host_b["gone"] = json!(true);
    host_b["reachable"] = json!(false);
    fx.write_cache("env-0002", &host_b);

    let json_out = stdout_of(fx.flow(&[]).args(["inventory", "--json"]));

    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    let fields = [
        "name",
        "env_id",
        "ssh_alias",
        "local",
        "phase",
        "seen",
        "reachable",
        "gone",
    ];
    let hosts: Vec<Value> = parsed
        .as_array()
        .unwrap()
        .iter()
        .map(|host| {
            fields
                .iter()
                .map(|field| (field.to_string(), host[field].clone()))
                .collect()
        })
        .collect();
    assert_eq!(
        hosts,
        [
            json!({
                "name": "host-a", "env_id": "env-0001", "ssh_alias": "env-0001.ona.environment",
                "local": true, "phase": "ENVIRONMENT_PHASE_RUNNING", "seen": host_a["seen"],
                "reachable": true, "gone": false,
            }),
            json!({
                "name": "host-b", "env_id": "env-0002", "ssh_alias": "host-b.example",
                "local": false, "phase": null, "seen": host_b["seen"], "reachable": false,
                "gone": true,
            }),
        ]
    );
}

#[test]
fn gh_failure_is_recorded_in_cache_and_output() {
    let gh_failed = json!({
        "host": "host-a", "status": 4, "stdout": "",
        "stderr": "To get started with GitHub CLI, please run:  gh auth login\n",
    });
    let mut script = host_a_probes();
    script.push(gh_failed);

    let with_cache = Fixture::new();
    single_host_config(&with_cache);
    with_cache.write_cache(
        "env-0001",
        &json!({
            "name": "host-a", "env_id": "env-0001", "ssh_alias": "host-a.example",
            "seen": days_ago(2),
            "reachable": true, "load1": 0.5, "worktrees": [], "agents": [],
            "prs": [
                {"repo": "example-repo", "branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"},
                {"repo": "example-repo", "branch": "removed", "number": 9, "url": "https://github.com/example/acme/pull/9", "state": "OPEN"},
            ],
        }),
    );
    let without_cache = Fixture::new();
    single_host_config(&without_cache);

    for (fx, expected_prs) in [(&with_cache, json!([12])), (&without_cache, json!([]))] {
        let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

        let gh_line = stdout.lines().nth(1).unwrap();
        assert!(gh_line.starts_with("  gh failed: "), "{stdout}");
        assert!(gh_line.contains("gh auth login"), "{stdout}");
        assert_eq!(
            stdout.contains("PR #12 open"),
            expected_prs == json!([12]),
            "{stdout}"
        );
        assert!(!stdout.contains("PR #9"), "{stdout}");

        let cached = fx.cached("env-0001");
        let pr_error = cached["pr_error"].as_str().unwrap();
        assert!(pr_error.contains("gh auth login"), "{pr_error}");
        let numbers: Vec<Value> = cached["prs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pr| pr["number"].clone())
            .collect();
        assert_eq!(Value::from(numbers), expected_prs);
    }

    let mut recovered = host_a_probes();
    recovered.push(on("host-a", PR_LIST));
    stdout_of(with_cache.flow(&recovered).args(["inventory", "--refresh"]));
    assert_eq!(with_cache.cached("env-0001")["pr_error"], Value::Null);
}

#[test]
fn inventory_lists_worktrees_per_repo() {
    let fx = Fixture::new();
    fs::write(
        fx.state.join("config.toml"),
        r#"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[[repos]]
name = "other-repo"
path = "/work/other"
default_branch = "main"

[hosts]
source = "static"

[[hosts.static]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"
"#,
    )
    .unwrap();
    let repo_porcelain = "\
worktree /work/repo
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /work/repo-worktrees/feature-x
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature-x
";
    let other_porcelain = "\
worktree /work/other
HEAD 4444444444444444444444444444444444444444
branch refs/heads/main

worktree /work/other-worktrees/fix-y
HEAD 5555555555555555555555555555555555555555
branch refs/heads/fix-y
";
    let wt_list =
        r#"{"worktrees":[{"path":"/work/repo-worktrees/feature-x","dirty_files":1,"busy":false}]}"#;
    let repo_prs = r#"[{"headRefName":"feature-x","number":12,"state":"OPEN","url":"https://github.com/example/example-repo/pull/12"},{"headRefName":"fix-y","number":13,"state":"OPEN","url":"https://github.com/example/example-repo/pull/13"}]"#;
    let other_prs = r#"[{"headRefName":"fix-y","number":21,"state":"OPEN","url":"https://github.com/example/other-repo/pull/21"}]"#;
    let script = [
        on("host-a", repo_porcelain),
        on("host-a", wt_list),
        on("host-a", other_porcelain),
        on("host-a", LOADAVG),
        on("host-a", SERVER_RUNNING),
        on(
            "host-a",
            r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#,
        ),
        default_session_stopped("host-a"),
        on("host-a", repo_prs),
        on("host-a", other_prs),
    ];

    let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    assert_eq!(
        settled(&stdout),
        "\
host-a  reachable · load 0.26 · seen 0s ago
  agents (0)
  repos
    example-repo  on main · 1 worktree
      feature-x  PR #12 open
    other-repo    on main · 1 worktree
      fix-y  PR #21 open
"
    );

    let cached = fx.cached("env-0001");
    assert_eq!(
        cached["worktrees"],
        json!([
            {"repo": "example-repo", "path": "/work/repo", "branch": "main", "busy": null, "dirty_files": null},
            {"repo": "example-repo", "path": "/work/repo-worktrees/feature-x", "branch": "feature-x", "busy": false, "dirty_files": 1},
            {"repo": "other-repo", "path": "/work/other", "branch": "main", "busy": null, "dirty_files": null},
            {"repo": "other-repo", "path": "/work/other-worktrees/fix-y", "branch": "fix-y", "busy": null, "dirty_files": null},
        ])
    );
    let prs: Vec<(Value, Value)> = cached["prs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pr| (pr["repo"].clone(), pr["number"].clone()))
        .collect();
    assert_eq!(
        prs,
        [
            (json!("example-repo"), json!(12)),
            (json!("other-repo"), json!(21))
        ],
        "a PR joins only worktrees of its own repo"
    );
    assert_eq!(cached["tool_error"], Value::Null);

    let in_dir = |dir: &str, tool: &[&str]| {
        let mut argv = words(&["sh", "-c", r#"cd "$1" && shift && exec "$@""#, "sh", dir]);
        argv.extend(words(tool));
        argv
    };
    let gh = [
        "gh",
        "pr",
        "list",
        "--author",
        "@me",
        "--state",
        "open",
        "--json",
        "number,url,headRefName,state",
        "--limit",
        "100",
    ];
    let calls = fx.calls();
    let argvs: Vec<&Vec<String>> = calls.iter().map(|(_, argv)| argv).collect();
    assert_eq!(
        argvs,
        [
            &words(&["git", "-C", "/work/repo", "worktree", "list", "--porcelain"]),
            &in_dir("/work/repo", &["wt", "--json", "list"]),
            &words(&[
                "git",
                "-C",
                "/work/other",
                "worktree",
                "list",
                "--porcelain"
            ]),
            &words(&["cat", "/proc/loadavg"]),
            &words(&["herdr", "--session", "flow", "status", "server"]),
            &words(&["herdr", "--session", "flow", "agent", "list"]),
            &words(&["herdr", "--session", "default", "agent", "list"]),
            &in_dir("/work/repo", &gh),
            &in_dir("/work/other", &gh),
        ]
    );
}

const TWO_REPOS_TWO_HOSTS: &str = r#"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"

[[repos]]
name = "other-repo"
path = "/work/other"
default_branch = "main"

[hosts]
source = "static"

[[hosts.static]]
name = "host-a"
env_id = "env-0001"
ssh_alias = "host-a.example"

[[hosts.static]]
name = "host-b"
env_id = "env-0002"
ssh_alias = "host-b.example"
"#;

const REPO_PORCELAIN: &str = "\
worktree /work/repo
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main
";

const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;

fn gh_in(dir: &str) -> Vec<String> {
    words(&[
        "sh",
        "-c",
        r#"cd "$1" && shift && exec "$@""#,
        "sh",
        dir,
        "gh",
        "pr",
        "list",
        "--author",
        "@me",
        "--state",
        "open",
        "--json",
        "number,url,headRefName,state",
        "--limit",
        "100",
    ])
}

/// `host-a`'s probes with `/work/other` answering `other_git`, without any `gh` call.
fn host_a_without_other(other_git: Value) -> Vec<Value> {
    vec![
        on("host-a", REPO_PORCELAIN),
        other_git,
        on("host-a", LOADAVG),
        on("host-a", SERVER_RUNNING),
        on("host-a", NO_AGENTS),
        default_session_stopped("host-a"),
    ]
}

fn other_git_failed(stderr: &str) -> Value {
    json!({"host": "host-a", "status": 128, "stdout": "", "stderr": stderr})
}

const NO_SUCH_DIR: &str = "fatal: cannot change to '/work/other': No such file or directory\n";

#[test]
fn inventory_skips_repos_missing_on_a_host() {
    let fx = Fixture::new();
    fs::write(fx.state.join("config.toml"), TWO_REPOS_TWO_HOSTS).unwrap();
    let other_porcelain = "\
worktree /work/other
HEAD 4444444444444444444444444444444444444444
branch refs/heads/main

worktree /work/other-worktrees/fix-y
HEAD 5555555555555555555555555555555555555555
branch refs/heads/fix-y
";
    let other_prs = r#"[{"headRefName":"fix-y","number":21,"state":"OPEN","url":"https://github.com/example/other-repo/pull/21"}]"#;
    let mut script = host_a_without_other(other_git_failed(NO_SUCH_DIR));
    script.push(on("host-a", "[]"));
    script.extend([
        on("host-b", REPO_PORCELAIN),
        on("host-b", other_porcelain),
        on("host-b", LOADAVG),
        on("host-b", SERVER_RUNNING),
        on("host-b", NO_AGENTS),
        default_session_stopped("host-b"),
        on("host-b", other_prs),
    ]);

    let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    assert_eq!(
        settled(&stdout),
        "\
host-a  reachable · load 0.26 · seen 0s ago
  agents (0)
  repos
    example-repo  on main
    other-repo    missing at /work/other

host-b  reachable · load 0.26 · seen 0s ago
  agents (0)
  repos
    example-repo  on main
    other-repo    on main · 1 worktree
      fix-y  PR #21 open
"
    );
    let cached = fx.cached("env-0001");
    assert_eq!(cached["reachable"], true);
    assert_eq!(cached["missing_repos"], json!(["other-repo"]));
    assert_eq!(cached["worktrees"][0]["repo"], "example-repo");
    assert_eq!(cached["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(cached["pr_error"], Value::Null);
    assert_eq!(fx.cached("env-0002")["pr_error"], Value::Null);
    assert_eq!(fx.cached("env-0002")["missing_repos"], json!([]));
    let gh_calls: Vec<(String, Vec<String>)> = fx
        .calls()
        .into_iter()
        .filter(|(_, argv)| argv.get(5).is_some_and(|program| program == "gh"))
        .collect();
    assert_eq!(
        gh_calls,
        [
            ("host-a".to_owned(), gh_in("/work/repo")),
            ("host-b".to_owned(), gh_in("/work/other")),
        ],
        "each repo's PRs are listed on the local host, or on a host that has the repo"
    );
}

#[test]
fn inventory_reports_a_repo_no_host_has() {
    let fx = Fixture::new();
    let config = TWO_REPOS_TWO_HOSTS
        .split("[[hosts.static]]")
        .take(2)
        .collect::<Vec<_>>()
        .join("[[hosts.static]]");
    fs::write(fx.state.join("config.toml"), config).unwrap();
    let mut script = host_a_without_other(other_git_failed(
        "fatal: not a git repository (or any of the parent directories): .git\n",
    ));
    script.push(on("host-a", "[]"));

    let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    assert!(
        stdout.contains("  gh failed: other-repo: no reachable host has repo other-repo\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("    other-repo    missing at /work/other\n"),
        "{stdout}"
    );
    assert_eq!(
        fx.cached("env-0001")["pr_error"],
        "other-repo: no reachable host has repo other-repo"
    );
    let gh_calls = fx
        .calls()
        .into_iter()
        .filter(|(_, argv)| argv.get(5).is_some_and(|program| program == "gh"))
        .count();
    assert_eq!(gh_calls, 1);
}

#[test]
fn inventory_fails_a_host_whose_repo_git_cannot_read() {
    let fx = Fixture::new();
    fs::write(fx.state.join("config.toml"), TWO_REPOS_TWO_HOSTS).unwrap();
    let mut script = host_a_without_other(other_git_failed(
        "fatal: cannot change to '/work/other': Permission denied\n",
    ));
    script.push(
        json!({"host": "host-b", "status": 255, "stdout": "", "repeat": true,
        "stderr": "ssh: connect to host host-b.example: Connection refused\n"}),
    );

    let out = fx
        .flow(&script)
        .args(["inventory", "--refresh"])
        .assert()
        .failure()
        .get_output()
        .clone();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("Permission denied"), "{stderr}");
    assert!(stderr.contains("could not refresh host-a"), "{stderr}");
}

const LAB: &str = "00000000-0000-4000-8000-000000000001";
const SIDE_PROJECT: &str = "00000000-0000-4000-8000-000000000002";
const SCRATCH_BOX: &str = "00000000-0000-4000-8000-000000000003";

/// Repos and `CONFIG`'s herdr sessions only: hosts come from Ona.
const ONA_CONFIG: &str = r#"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
worktree_tool = { create = ["wt", "create", "{branch}"] }
"#;

fn ona_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ona/environment_list.json")
}

/// A cached record of a host that has never been probed.
fn bare_record(name: &str, env_id: &str, ssh_alias: &str) -> Value {
    json!({
        "name": name, "env_id": env_id, "ssh_alias": ssh_alias,
        "seen": days_ago(8), "reachable": true, "load1": null,
        "worktrees": [], "agents": [], "prs": [],
    })
}

/// Host names of every runner call, deduplicated and sorted.
fn called_hosts(fx: &Fixture) -> Vec<String> {
    let mut hosts: Vec<String> = fx.calls().into_iter().map(|(host, _)| host).collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

fn stderr_of(cmd: &mut Command) -> String {
    let out = cmd.assert().success().get_output().clone();
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn refresh_adds_new_hosts_and_marks_gone() {
    let fx = Fixture::new();
    fs::write(fx.state.join("config.toml"), ONA_CONFIG).unwrap();
    fx.use_ona_fixture();
    fx.write_cache(LAB, &bare_record("lab", LAB, ""));
    fx.write_cache("env-retired", &bare_record("retired", "env-retired", ""));
    let mut script = probes("lab");
    script.extend(probes("side-project"));
    script.push(on("lab", PR_LIST));

    let stdout = stdout_of(
        fx.flow(&script)
            .env("ONA_ID", LAB)
            .args(["inventory", "--refresh"]),
    );

    let lab = fx.cached(LAB);
    assert_eq!(lab["name"], "lab");
    assert_eq!(lab["ona_name"], "Lab");
    assert_eq!(lab["ssh_alias"], format!("{LAB}.ona.environment").as_str());
    assert_eq!(lab["ona_phase"], "ENVIRONMENT_PHASE_RUNNING");
    assert_eq!(lab["gone"], false);
    assert_eq!(lab["reachable"], true);
    assert_eq!(lab["worktrees"].as_array().unwrap().len(), 3);
    assert_eq!(lab["prs"][0]["number"], 12);

    let added = fx.cached(SIDE_PROJECT);
    assert_eq!(added["name"], "side-project");
    assert_eq!(added["ona_name"], "Side Project");
    assert_eq!(added["gone"], false);
    assert_eq!(added["reachable"], true);

    let stopped = fx.cached(SCRATCH_BOX);
    assert_eq!(stopped["name"], "scratch-box");
    assert_eq!(stopped["ona_phase"], "ENVIRONMENT_PHASE_STOPPED");
    assert_eq!(stopped["reachable"], false);
    assert_eq!(stopped["gone"], false);
    assert_eq!(stopped["worktrees"], json!([]));

    let retired = fx.cached("env-retired");
    assert_eq!(retired["gone"], true);
    assert_eq!(retired["name"], "retired");

    assert_eq!(
        called_hosts(&fx),
        ["lab", "side-project"],
        "stopped and gone hosts are not probed"
    );
    let headers: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .collect();
    assert_eq!(headers.len(), 4, "{stdout}");
    assert!(
        headers[0].starts_with("lab           reachable · "),
        "{stdout}"
    );
    assert!(
        headers[1].starts_with("retired       gone · seen "),
        "{stdout}"
    );
    assert_eq!(headers[2], "scratch-box   never probed", "{stdout}");
    assert!(
        headers[3].starts_with("side-project  reachable · "),
        "{stdout}"
    );

    let listed: Value =
        serde_json::from_str(&stdout_of(fx.flow(&[]).args(["inventory", "--json"]))).unwrap();
    let hosts: Vec<(&str, bool)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|host| {
            (
                host["name"].as_str().unwrap(),
                host["gone"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        hosts,
        [
            ("lab", false),
            ("retired", true),
            ("scratch-box", false),
            ("side-project", false)
        ],
        "{listed}"
    );
}

#[test]
fn refresh_without_ona_keeps_cached_hosts() {
    let fx = Fixture::new();
    fs::write(fx.state.join("config.toml"), ONA_CONFIG).unwrap();
    fx.write_cache(
        "env-0001",
        &bare_record("host-a", "env-0001", "host-a.example"),
    );
    fx.write_cache(
        "env-0002",
        &bare_record("host-b", "env-0002", "host-b.example"),
    );
    let script = || {
        let mut script = probes("host-a");
        script.extend(probes("host-b"));
        script.push(on("host-a", PR_LIST));
        script
    };

    let stderr = stderr_of(fx.flow(&script()).args(["inventory", "--refresh"]));

    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "{stderr}");
    assert!(lines[0].starts_with("ona: "), "{stderr}");
    assert!(lines[0].ends_with("; using 2 cached hosts"), "{stderr}");
    assert_eq!(called_hosts(&fx), ["host-a", "host-b"]);
    for (env_id, name, alias) in [
        ("env-0001", "host-a", "host-a.example"),
        ("env-0002", "host-b", "host-b.example"),
    ] {
        let cached = fx.cached(env_id);
        assert_eq!(cached["name"], name);
        assert_eq!(cached["ssh_alias"], alias);
        assert_eq!(cached["gone"], false);
        assert_eq!(cached["reachable"], true);
    }

    fx.write_ona(&json!([{
        "id": "env-0001",
        "metadata": {"name": "Renamed"},
        "status": {"phase": "ENVIRONMENT_PHASE_RUNNING"},
    }]));
    let stderr = stderr_of(fx.flow(&script()).args(["inventory", "--refresh"]));

    assert_eq!(
        stderr,
        "ona: only the current environment is visible; run gitpod login\n"
    );
    assert_eq!(fx.cached("env-0001")["name"], "host-a");
    assert_eq!(fx.cached("env-0002")["gone"], false);
}

#[test]
fn static_hosts_bypass_discovery() {
    let fx = Fixture::new();
    fx.write_cache("env-0001", &bare_record("old-name", "env-0001", ""));
    fx.write_cache("env-retired", &bare_record("retired", "env-retired", ""));
    let mut script = host_a_probes();
    script.extend(probes("host-b"));
    script.push(on("host-a", PR_LIST));

    stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    let host_a = fx.cached("env-0001");
    assert_eq!(host_a["name"], "host-a");
    assert_eq!(host_a["ssh_alias"], "host-a.example");
    assert_eq!(host_a["gone"], false);
    assert_eq!(host_a["ona_name"], Value::Null);
    assert_eq!(fx.cached("env-0002")["ssh_alias"], "host-b.example");
    assert_eq!(fx.cached("env-retired")["gone"], true);
    assert_eq!(called_hosts(&fx), ["host-a", "host-b"]);
}

#[test]
fn name_overrides_apply() {
    let fx = Fixture::new();
    let config = format!(
        r#"{ONA_CONFIG}
[hosts]
exclude = ["{SCRATCH_BOX}"]

[hosts.names]
"{LAB}" = "side-project"
"#
    );
    fs::write(fx.state.join("config.toml"), config).unwrap();
    fx.use_ona_fixture();
    let mut script = probes("side-project");
    script.extend(probes("side-project-2"));
    script.push(on("side-project", PR_LIST));

    stdout_of(
        fx.flow(&script)
            .env("ONA_ID", LAB)
            .args(["inventory", "--refresh"]),
    );

    let lab = fx.cached(LAB);
    assert_eq!(lab["name"], "side-project");
    assert_eq!(lab["ona_name"], "Lab");
    let second = fx.cached(SIDE_PROJECT);
    assert_eq!(
        second["name"], "side-project-2",
        "a name taken by a lower id gets a suffix"
    );
    assert!(
        !fx.cache_path(SCRATCH_BOX).exists(),
        "an excluded environment gets no record"
    );
    assert_eq!(called_hosts(&fx), ["side-project", "side-project-2"]);
}
