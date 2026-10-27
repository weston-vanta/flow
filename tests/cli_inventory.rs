use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::{Value, json};

const CONFIG: &str = r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/workspaces/acme"
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

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on this state dir as `cde-a`, answering runner calls from `script`.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path());
        cmd
    }

    /// `(cde, argv)` of every runner call.
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let call: Value = serde_json::from_str(line).unwrap();
                (
                    call["cde"].as_str().unwrap().to_owned(),
                    serde_json::from_value(call["argv"].clone()).unwrap(),
                )
            })
            .collect()
    }

    fn cache_path(&self, env_id: &str) -> PathBuf {
        self.state.join("cdes").join(format!("{env_id}.json"))
    }

    fn cached(&self, env_id: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.cache_path(env_id)).unwrap()).unwrap()
    }

    fn write_cache(&self, env_id: &str, inventory: &Value) {
        fs::create_dir_all(self.state.join("cdes")).unwrap();
        fs::write(self.cache_path(env_id), inventory.to_string()).unwrap();
    }
}

fn on(cde: &str, stdout: &str) -> Value {
    json!({"cde": cde, "status": 0, "stdout": stdout})
}

/// `herdr --session default agent list` on `cde` with no default server running.
fn default_session_stopped(cde: &str) -> Value {
    json!({
        "cde": cde, "status": 1, "stdout": "",
        "stderr": include_str!("fixtures/herdr/agent_list_server_not_running.json"),
    })
}

/// Every probe of `cde-a` succeeding, in call order.
fn cde_a_probes() -> Vec<Value> {
    vec![
        on("cde-a", PORCELAIN),
        on("cde-a", WT_LIST),
        on("cde-a", LOADAVG),
        on("cde-a", SERVER_RUNNING),
        on("cde-a", AGENT_LIST),
        default_session_stopped("cde-a"),
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

fn single_cde_config(fx: &Fixture) {
    let config = CONFIG
        .split("[[cdes]]")
        .take(2)
        .collect::<Vec<_>>()
        .join("[[cdes]]");
    fs::write(fx.state.join("config.toml"), config).unwrap();
}

#[test]
fn refresh_parses_worktrees_load_and_joins_prs() {
    let fx = Fixture::new();
    single_cde_config(&fx);
    let mut script = cde_a_probes();
    script.push(on("cde-a", PR_LIST));

    let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

    let mut lines = stdout.lines();
    let header = lines.next().unwrap();
    assert!(
        header.starts_with("cde-a  reachable  load 0.26  3 worktrees  1 agents  (seen "),
        "{stdout}"
    );
    assert_eq!(
        lines.collect::<Vec<_>>(),
        [
            "  main        /workspaces/acme",
            "  feature-x   /workspaces/acme-worktrees/feature-x  PR #12 OPEN  agent: Build feature x working",
            "  (detached)  /workspaces/acme-worktrees/bisect",
        ],
        "{stdout}"
    );

    let cached = fx.cached("env-0001");
    assert_eq!(cached["name"], "cde-a");
    assert_eq!(cached["env_id"], "env-0001");
    assert_eq!(cached["reachable"], true);
    assert_eq!(cached["load1"], 0.26);
    assert_eq!(
        cached["worktrees"],
        json!([
            {"path": "/workspaces/acme", "branch": "main", "busy": null, "dirty_files": null},
            {"path": "/workspaces/acme-worktrees/feature-x", "branch": "feature-x", "busy": true, "dirty_files": 3},
            {"path": "/workspaces/acme-worktrees/bisect", "branch": null, "busy": null, "dirty_files": null},
        ])
    );
    assert_eq!(
        cached["prs"],
        json!([{"branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"}])
    );
    assert_eq!(cached["agents"][0]["pane_id"], "w4:p1");
    assert_eq!(cached["tool_error"], Value::Null);
    assert_eq!(cached["herdr_error"], Value::Null);

    let in_monorepo = |tool: &[&str]| {
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
            &in_monorepo(&["wt", "--json", "list"]),
            &words(&["cat", "/proc/loadavg"]),
            &words(&["herdr", "--session", "flow", "status", "server"]),
            &words(&["herdr", "--session", "flow", "agent", "list"]),
            &words(&["herdr", "--session", "default", "agent", "list"]),
            &in_monorepo(&[
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
fn unreachable_cde_is_recorded_not_fatal() {
    let fx = Fixture::new();
    let earlier = "2026-10-26T09:00:00Z";
    fx.write_cache(
        "env-0002",
        &json!({
            "name": "cde-b", "env_id": "env-0002", "seen": earlier, "reachable": true,
            "load1": 1.5,
            "worktrees": [{"path": "/workspaces/acme-worktrees/feature-y", "branch": "feature-y", "busy": false, "dirty_files": 0}],
            "agents": [],
            "prs": [{"branch": "feature-y", "number": 14, "url": "https://github.com/example/acme/pull/14", "state": "OPEN"}],
            "tool_error": null, "herdr_error": null,
        }),
    );
    let mut script = vec![json!({
        "cde": "cde-b", "status": 255, "stdout": "", "repeat": true,
        "stderr": "ssh: Could not resolve hostname cde-b.example: Name or service not known\n",
    })];
    script.extend(cde_a_probes());
    script.push(on("cde-a", PR_LIST));

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
        stderr.contains("cde-b") && stderr.contains("Could not resolve hostname"),
        "{stderr}"
    );
    let blocks: Vec<&str> = stdout.split("\n\n").collect();
    assert_eq!(blocks.len(), 2, "{stdout}");
    assert!(blocks[0].starts_with("cde-a  reachable  "), "{stdout}");
    assert!(
        blocks[1].starts_with("cde-b  unreachable  load -  1 worktrees  0 agents  (seen "),
        "{stdout}"
    );
    assert!(
        blocks[1].contains("  feature-y  /workspaces/acme-worktrees/feature-y  PR #14 OPEN"),
        "{stdout}"
    );

    let cached = fx.cached("env-0002");
    assert_eq!(cached["reachable"], false);
    assert_eq!(cached["seen"], earlier);
    assert_eq!(cached["load1"], Value::Null);
    assert_eq!(cached["worktrees"][0]["branch"], "feature-y");
    assert_eq!(cached["prs"][0]["number"], 14);
    let cde_b_calls = fx
        .calls()
        .into_iter()
        .filter(|(cde, _)| cde == "cde-b")
        .count();
    assert_eq!(
        cde_b_calls, 1,
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
            "name": "cde-a", "env_id": "env-0001", "seen": seen.to_string(), "reachable": true,
            "load1": 0.5,
            "worktrees": [
                {"path": "/workspaces/acme-worktrees/feature-x", "branch": "feature-x", "busy": true, "dirty_files": 2},
            ],
            "agents": [{
                "pane_id": "w4:p1", "workspace_id": "w4", "agent": "claude", "status": "idle",
                "cwd": "/workspaces/acme-worktrees/feature-x", "title": "",
            }],
            "prs": [{"branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"}],
            "tool_error": "wt: command not found",
            "herdr_error": null,
        }),
    );

    let stdout = stdout_of(fx.flow(&[]).arg("inventory"));

    assert_eq!(
        stdout,
        "cde-a  reachable  load 0.50  1 worktrees  1 agents  (seen 5m)\n\
         \x20 worktree tool failed: wt: command not found\n\
         \x20 feature-x  /workspaces/acme-worktrees/feature-x  PR #12 OPEN  agent: w4:p1 idle\n"
    );
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());

    let json_out = stdout_of(fx.flow(&[]).args(["inventory", "--json"]));
    let parsed: Value = serde_json::from_str(&json_out).unwrap();
    assert_eq!(parsed[0]["name"], "cde-a");
    assert_eq!(parsed[0]["worktrees"][0]["dirty_files"], 2);
    assert!(fx.calls().is_empty());
}

#[test]
fn gh_failure_is_recorded_in_cache_and_output() {
    let gh_failed = json!({
        "cde": "cde-a", "status": 4, "stdout": "",
        "stderr": "To get started with GitHub CLI, please run:  gh auth login\n",
    });
    let mut script = cde_a_probes();
    script.push(gh_failed);

    let with_cache = Fixture::new();
    single_cde_config(&with_cache);
    with_cache.write_cache(
        "env-0001",
        &json!({
            "name": "cde-a", "env_id": "env-0001", "seen": "2026-10-26T09:00:00Z",
            "reachable": true, "load1": 0.5, "worktrees": [], "agents": [],
            "prs": [
                {"branch": "feature-x", "number": 12, "url": "https://github.com/example/acme/pull/12", "state": "OPEN"},
                {"branch": "removed", "number": 9, "url": "https://github.com/example/acme/pull/9", "state": "OPEN"},
            ],
        }),
    );
    let without_cache = Fixture::new();
    single_cde_config(&without_cache);

    for (fx, expected_prs) in [(&with_cache, json!([12])), (&without_cache, json!([]))] {
        let stdout = stdout_of(fx.flow(&script).args(["inventory", "--refresh"]));

        let gh_line = stdout.lines().nth(1).unwrap();
        assert!(gh_line.starts_with("  gh failed: "), "{stdout}");
        assert!(gh_line.contains("gh auth login"), "{stdout}");
        assert_eq!(
            stdout.contains("PR #12 OPEN"),
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

    let mut recovered = cde_a_probes();
    recovered.push(on("cde-a", PR_LIST));
    stdout_of(with_cache.flow(&recovered).args(["inventory", "--refresh"]));
    assert_eq!(with_cache.cached("env-0001")["pr_error"], Value::Null);
}
