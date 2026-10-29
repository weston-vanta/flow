use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/workspaces/acme"
default_branch = "main"
"#;

const MACHINE_LIST: &str = include_str!("fixtures/herdr/machine_list.json");

/// The plan `MACHINE_LIST` and `Fixture::new`'s hosts give.
const PLAN: &str = "\
Herdr:
 - add host host-b (host-b.example, session default) to herdr's saved machines.
 - remove gone host host-d from herdr's saved machines.
 - remove host host-f (host-f.example, session flow) from herdr's saved machines.
 - add host host-f (host-f.example, session default) to herdr's saved machines.
";

const SKIPPED: &str = "not adding host host-c: it did not answer the last refresh, and herdr \
     machine add has to reach it\n";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    /// A state dir with `CONFIG` and cached records of these hosts:
    /// - `host-a`, the local one, whose label a profile `MACHINE_LIST` holds names elsewhere;
    /// - `host-b`, reachable, with no profile;
    /// - `host-c`, unreachable, with no profile;
    /// - `host-d`, gone, with the profile flow would have made;
    /// - `host-e`, reachable, with the profile flow would make;
    /// - `host-f`, reachable, with a profile in the `flow` session;
    /// - `host-g`, gone, with a profile of its label for another target.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        let fx = Fixture { dir, state };
        for (n, (name, reachable, gone)) in [
            ("host-a", true, false),
            ("host-b", true, false),
            ("host-c", false, false),
            ("host-d", true, true),
            ("host-e", true, false),
            ("host-f", true, false),
            ("host-g", true, true),
        ]
        .into_iter()
        .enumerate()
        {
            fx.write_host(name, &format!("env-000{}", n + 1), reachable, gone);
        }
        fx
    }

    fn write_host(&self, name: &str, env_id: &str, reachable: bool, gone: bool) {
        let hosts = self.state.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        let record = json!({
            "name": name, "env_id": env_id, "ssh_alias": format!("{name}.example"),
            "gone": gone, "seen": "2026-10-26T09:00:00Z", "reachable": reachable,
            "load1": null, "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    /// `flow` on `host-a`, answering runner calls from `script`, with no terminal. `calls`
    /// then lists this run's calls only.
    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let _ = fs::remove_file(self.calls_log());
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env_remove("FLOW_FAKE_TTY");
        cmd
    }

    fn calls_log(&self) -> PathBuf {
        let mut log = self.script_path().into_os_string();
        log.push(".calls.jsonl");
        PathBuf::from(log)
    }

    /// `(host, argv)` of every runner call.
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        fs::read_to_string(self.calls_log())
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

    fn argvs(&self) -> Vec<Vec<String>> {
        self.calls().into_iter().map(|(_, argv)| argv).collect()
    }
}

fn ok(stdout: &str) -> Value {
    json!({"status": 0, "stdout": stdout, "stderr": ""})
}

fn listed() -> Value {
    json!({"argv": ["machine", "list"], "status": 0, "stdout": MACHINE_LIST, "stderr": ""})
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|word| (*word).to_owned()).collect()
}

/// The calls that apply `PLAN`, in order.
fn applying() -> Vec<Vec<String>> {
    vec![
        words(&["herdr", "machine", "list", "--json"]),
        words(&[
            "herdr",
            "machine",
            "add",
            "host-b.example",
            "--label",
            "host-b",
            "--remote-session",
            "default",
        ]),
        words(&[
            "herdr",
            "machine",
            "remove",
            "22222222222222222222222222222222",
        ]),
        words(&[
            "herdr",
            "machine",
            "remove",
            "11111111111111111111111111111111",
        ]),
        words(&[
            "herdr",
            "machine",
            "add",
            "host-f.example",
            "--label",
            "host-f",
            "--remote-session",
            "default",
        ]),
    ]
}

#[test]
fn sync_without_a_terminal_prints_the_plan_and_asks_for_yes() {
    let fx = Fixture::new();

    let out = fx
        .flow(&[listed()])
        .args(["inventory", "--sync"])
        .assert()
        .code(1)
        .get_output()
        .clone();

    assert_eq!(String::from_utf8(out.stdout).unwrap(), PLAN);
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!("{SKIPPED}rerun with --yes to apply\n")
    );
    assert_eq!(
        fx.calls(),
        [(
            "host-a".to_owned(),
            words(&["herdr", "machine", "list", "--json"])
        )]
    );
}

#[test]
fn sync_with_yes_applies_the_plan_in_order_passing_herdr_output_through() {
    let fx = Fixture::new();
    let script = [
        listed(),
        ok("saved host-b\n"),
        ok("removed host-d\n"),
        ok("removed host-f\n"),
        ok("saved host-f\n"),
    ];

    let out = fx
        .flow(&script)
        .args(["inventory", "--sync", "--yes"])
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!(
            "{PLAN}saved host-b\nremoved host-d\nremoved host-f\nsaved host-f\n\
             herdr machines now match the inventory\n"
        )
    );
    assert_eq!(String::from_utf8(out.stderr).unwrap(), SKIPPED);
    assert_eq!(fx.argvs(), applying());
    assert!(fx.calls().iter().all(|(host, _)| host == "host-a"));
}

#[test]
fn sync_asks_at_a_terminal_and_applies_only_on_yes() {
    let fx = Fixture::new();
    let script = [listed(), ok(""), ok(""), ok(""), ok("")];

    let declined = fx
        .flow(&script)
        .env("FLOW_FAKE_TTY", "1")
        .args(["inventory", "--sync"])
        .write_stdin("n\n")
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(declined.stdout).unwrap(),
        format!("{PLAN}\nProceed? [y/N] nothing applied\n")
    );
    assert_eq!(fx.argvs().len(), 1);

    let accepted = fx
        .flow(&script)
        .env("FLOW_FAKE_TTY", "1")
        .args(["inventory", "--sync"])
        .write_stdin("y\n")
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(accepted.stdout).unwrap(),
        format!("{PLAN}\nProceed? [y/N] herdr machines now match the inventory\n")
    );
    assert_eq!(fx.argvs(), applying());
}

#[test]
fn sync_with_nothing_to_do_says_so() {
    let fx = Fixture::new();
    let matching = json!([{
        "id": "0123456789abcdef0123456789abcdef", "label": "host-b",
        "target": "host-b.example", "session": "default", "enabled": true, "selected": false,
    }]);
    for env in 3..=7 {
        fs::remove_file(fx.state.join(format!("hosts/env-000{env}.json"))).unwrap();
    }

    let out = fx
        .flow(&[ok(&matching.to_string())])
        .args(["inventory", "--sync"])
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "herdr machines already match the inventory\n"
    );
    assert!(out.stderr.is_empty());
}

#[test]
fn sync_stops_at_the_first_failure_saying_what_was_applied() {
    let fx = Fixture::new();
    let script = [
        listed(),
        ok("saved host-b\n"),
        json!({"status": 1, "stdout": "", "stderr": "no such profile\n"}),
    ];

    let out = fx
        .flow(&script)
        .args(["inventory", "--sync", "--yes"])
        .assert()
        .code(1)
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{PLAN}saved host-b\n")
    );
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!(
            "{SKIPPED}no such profile\n\
             `herdr machine remove 22222222222222222222222222222222` exited 1 removing gone host \
             host-d; applied 1 of 4 steps before it\n"
        )
    );
    assert_eq!(fx.argvs(), applying()[..3]);
}

#[test]
fn sync_json_prints_the_plan_and_applies_only_with_yes() {
    let fx = Fixture::new();

    let out = fx
        .flow(&[listed()])
        .args(["inventory", "--sync", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();

    let plan: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        plan,
        json!({
            "steps": [
                {"action": "add", "host": "host-b", "target": "host-b.example", "session": "default"},
                {"action": "remove", "host": "host-d", "profile_id": "22222222222222222222222222222222",
                 "target": "host-d.example", "session": "default", "gone": true},
                {"action": "remove", "host": "host-f", "profile_id": "11111111111111111111111111111111",
                 "target": "host-f.example", "session": "flow", "gone": false},
                {"action": "add", "host": "host-f", "target": "host-f.example", "session": "default"},
            ],
            "skipped": ["host-c"],
        })
    );
    assert_eq!(String::from_utf8(out.stderr).unwrap(), SKIPPED);
    assert_eq!(fx.argvs().len(), 1);

    let script = [listed(), ok("saved host-b\n"), ok(""), ok(""), ok("")];
    let out = fx
        .flow(&script)
        .args(["inventory", "--sync", "--json", "--yes"])
        .assert()
        .success()
        .get_output()
        .clone();
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed, plan);
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!("{SKIPPED}saved host-b\n")
    );
    assert_eq!(fx.argvs(), applying());
}

#[test]
fn sync_refuses_on_a_machine_that_is_no_known_host() {
    let fx = Fixture::new();

    fx.flow(&[])
        .env("ONA_ID", "env-0099")
        .args(["inventory", "--sync"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("is not a known host"));
    assert!(fx.calls().is_empty());
}
