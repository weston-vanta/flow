use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use flow::herdr::SESSION;
use flow::paths::StateDir;
use flow::state::Store;
use flow::state::events::{self, Event};
use flow::state::task::{
    ClaudeSession, Harness, HerdrTarget, HoldOn, Origin, State, Status, TaskRecord,
};
use jiff::Timestamp;
use serde_json::{Value, json};

const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
"#;

const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
     private_protocol: 22\nprivate_protocol_compatible: yes\n\
     socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";
const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
const PROMPTED: &str = r#"{"id":"cli:agent:prompt","result":{"agent":{"agent":"claude","agent_status":"working","cwd":"/work/other","pane_id":"w5:p2","workspace_id":"w5"},"type":"agent_prompted"}}"#;
const PROMPT_BLOCKED: &str = r#"{"error":{"code":"agent_blocked","message":"agent w5:p2 is blocked"},"id":"cli:agent:prompt"}"#;

const HAND_SESSION_ID: &str = "00000000-0000-4000-8000-000000000003";
const TITLE: &str = "Fix the links";
const CRITERIA: &str = "- Every link in the guides resolves";
const DISPATCHED: &str = "aaaaaaab";

/// What `flow task adopt` adds when run outside the supervisor's session.
const HAND_NOTE: &str = "note: no supervisor turn is watching; the supervisor hears of this \
     task when it next ends a turn\n";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
}

impl Fixture {
    /// A state dir with `CONFIG`, cached records of `host-a` and `host-b`, and an empty Claude
    /// home.
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("config.toml"), CONFIG).unwrap();
        fs::create_dir_all(dir.path().join("claude")).unwrap();
        let fx = Fixture { dir, state };
        fx.write_host("host-a", "env-0001", false);
        fx.write_host("host-b", "env-0002", false);
        fx
    }

    fn write_host(&self, name: &str, env_id: &str, gone: bool) {
        let hosts = self.state.join("hosts");
        fs::create_dir_all(&hosts).unwrap();
        let record = json!({
            "name": name, "env_id": env_id, "ssh_alias": format!("{name}.example"),
            "gone": gone, "seen": "2026-10-26T09:00:00Z", "reachable": true, "load1": null,
            "worktrees": [], "agents": [], "prs": [],
        });
        fs::write(hosts.join(format!("{env_id}.json")), record.to_string()).unwrap();
    }

    fn script_path(&self) -> PathBuf {
        self.dir.path().join("runner.json")
    }

    fn claude_home(&self) -> PathBuf {
        self.dir.path().join("claude")
    }

    fn flow(&self, script: &[Value]) -> Command {
        fs::write(self.script_path(), Value::from(script.to_vec()).to_string()).unwrap();
        let mut cmd = Command::cargo_bin("flow").expect("flow binary is built");
        cmd.env("FLOW_STATE", &self.state)
            .env("ONA_ID", "env-0001")
            .env("FLOW_FAKE_RUNNER", self.script_path())
            .env("HOME", self.dir.path())
            .env("CLAUDE_CONFIG_DIR", self.claude_home())
            .env_remove("FLOW_SUPERVISOR");
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

    fn store(&self) -> Store {
        Store::open(&StateDir::resolve(self.dir.path()).unwrap())
    }

    /// The tasks other than `DISPATCHED`.
    fn adopted(&self) -> Vec<(TaskRecord, Status)> {
        self.store()
            .list_tasks()
            .unwrap()
            .into_iter()
            .filter(|(record, _)| record.id != DISPATCHED)
            .collect()
    }

    fn only_adopted(&self) -> (TaskRecord, Status) {
        let mut adopted = self.adopted();
        assert_eq!(adopted.len(), 1, "{adopted:?}");
        adopted.remove(0)
    }

    fn events(&self, id: &str) -> Vec<Event> {
        events::read_all(&self.store().task_dir(id).unwrap()).unwrap()
    }

    fn write_follow(&self, follow: &Value) {
        fs::create_dir_all(self.state.join("supervisor")).unwrap();
        fs::write(
            self.state.join("supervisor/follow.json"),
            follow.to_string(),
        )
        .unwrap();
    }

    fn follow_list(&self) -> Value {
        let path = self.state.join("supervisor/follow.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// An open task dispatched to `host-b` whose worker runs in `session`'s pane `pane_id`.
    fn task_in(&self, session: &str, pane_id: &str) {
        let store = self.store();
        store
            .create_task(
                TaskRecord {
                    id: DISPATCHED.to_owned(),
                    parent: None,
                    title: "Build feature".to_owned(),
                    host: "host-b".to_owned(),
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
            .set_status(DISPATCHED, |status| {
                status.state = State::Working;
                status.herdr = Some(HerdrTarget {
                    host: "host-b".to_owned(),
                    workspace_id: "w5".to_owned(),
                    pane_id: pane_id.to_owned(),
                    agent: "claude".to_owned(),
                    session: session.to_owned(),
                });
            })
            .unwrap();
    }
}

fn on(host: &str, stdout: &str) -> Value {
    json!({"host": host, "status": 0, "stdout": stdout})
}

/// A hand session in pane `pane_id` of herdr's `default` session on `host-b`.
fn hand(pane_id: &str, title: &str) -> Value {
    json!({
        "agent": "claude", "agent_status": "idle", "cwd": "/work/other",
        "pane_id": pane_id, "terminal_title_stripped": title, "workspace_id": "w5",
    })
}

/// `hand("w5:p2", "Fix links")`, reporting its Claude Code session id.
fn fix_links() -> Value {
    let mut agent = hand("w5:p2", "Fix links");
    agent["agent_session"] = json!({"agent": "claude", "kind": "id", "source": "herdr:claude", "value": HAND_SESSION_ID});
    agent
}

/// The answers to one listing of `host-b` whose `default` session holds `agents`.
fn host_b_lists(agents: &[Value]) -> Vec<Value> {
    let list = json!({"id": "cli:agent:list", "result": {"agents": agents, "type": "agent_list"}});
    vec![
        on("host-b", SERVER_RUNNING),
        on("host-b", NO_AGENTS),
        on("host-b", &list.to_string()),
    ]
}

/// A listing of `host-b` holding `agents`, then a prompt answered with `prompt`.
fn adopting(agents: &[Value], prompt: Value) -> Vec<Value> {
    let mut script = host_b_lists(agents);
    script.push(prompt);
    script
}

fn adopt_args(session: &str, extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "task",
        "adopt",
        "host-b",
        session,
        "--title",
        TITLE,
        "--criteria",
        CRITERIA,
    ]
    .iter()
    .map(|word| (*word).to_owned())
    .collect();
    args.extend(extra.iter().map(|word| (*word).to_owned()));
    args
}

fn adoption_message(id: &str, brief: &Path) -> String {
    format!(
        "You are now task {id}, run by a supervisor. Read {} and follow its Reporting section \
         exactly; keep going with the work already under way.",
        brief.display()
    )
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

#[test]
fn adopt_takes_over_a_hand_session_as_a_task() {
    let fx = Fixture::new();
    fx.write_follow(&json!({"follow": [
        {"host": "host-b", "pane_id": "w5:p1", "session": "default", "title": "Upgrade toolchain"},
        {"host": "host-b", "pane_id": "w5:p2", "session": "default", "title": "Fix links"},
    ]}));
    let agents = [hand("w5:p1", "Upgrade toolchain"), fix_links()];

    let stdout = stdout_of(
        fx.flow(&adopting(&agents, on("host-b", PROMPTED)))
            .args(adopt_args(
                "Fix links",
                &["--brief", "Finish fixing the links."],
            )),
    );

    let (record, status) = fx.only_adopted();
    let id = record.id.clone();
    assert_eq!(
        stdout,
        format!("adopted {id} on host-b (w5:p2)\n{HAND_NOTE}")
    );
    assert_eq!(
        record,
        TaskRecord {
            id: id.clone(),
            parent: None,
            title: TITLE.to_owned(),
            host: "host-b".to_owned(),
            cwd: PathBuf::from("/work/other"),
            repo: None,
            harness: Harness::Claude,
            created: record.created,
            owner: "supervisor".to_owned(),
            origin: Origin::Adopted,
        }
    );
    assert_eq!(status.state, State::Working);
    assert_eq!(status.hold, None);
    assert_eq!(
        status.herdr,
        Some(HerdrTarget {
            host: "host-b".to_owned(),
            workspace_id: "w5".to_owned(),
            pane_id: "w5:p2".to_owned(),
            agent: "claude".to_owned(),
            session: "default".to_owned(),
        })
    );
    assert_eq!(
        status.claude_session,
        Some(ClaudeSession {
            session_id: HAND_SESSION_ID.to_owned(),
            transcript_path: fx
                .claude_home()
                .join(format!("projects/-work-other/{HAND_SESSION_ID}.jsonl")),
        })
    );

    let brief = fx.store().brief(&id).unwrap();
    assert!(
        brief.starts_with(&format!("# Task {id}: {TITLE}\n")),
        "{brief}"
    );
    assert!(brief.contains("`/work/other`"), "{brief}");
    assert!(brief.contains("\nFinish fixing the links.\n"), "{brief}");
    assert!(brief.contains(&format!("\n{CRITERIA}\n")), "{brief}");
    assert!(
        !brief.contains("Work only inside this directory"),
        "{brief}"
    );
    assert!(
        brief.contains(&format!("task status {id} working")),
        "{brief}"
    );
    assert!(
        !fx.store()
            .task_dir(&id)
            .unwrap()
            .join("claude-settings.json")
            .exists(),
        "an adopted task gets no settings file"
    );

    let brief_path = fs::canonicalize(fx.store().brief_path(&id).unwrap()).unwrap();
    let message = adoption_message(&id, &brief_path);
    let calls = fx.calls();
    assert_eq!(calls.len(), 4, "{calls:?}");
    assert!(calls.iter().all(|(host, _)| host == "host-b"), "{calls:?}");
    let prompt = &calls[3].1;
    assert_eq!(
        prompt[..6],
        ["herdr", "--session", "default", "agent", "prompt", "w5:p2"]
    );
    assert_eq!(prompt[6], message);
    assert_eq!(
        prompt[7..],
        ["--wait", "--until", "working", "--until", "blocked"]
    );

    let events = fx.events(&id);
    let adopted = events.iter().find(|ev| ev.kind == "adopted").unwrap();
    assert_eq!(
        adopted.data,
        json!({"host": "host-b", "session": "default", "pane_id": "w5:p2", "title": "Fix links"})
    );
    let steer = events.iter().find(|ev| ev.kind == "steer").unwrap();
    assert_eq!(steer.source, "supervisor");
    assert_eq!(steer.data, json!({ "text": message }));

    assert_eq!(
        fx.follow_list(),
        json!({"follow": [
            {"host": "host-b", "pane_id": "w5:p1", "session": "default", "title": "Upgrade toolchain"},
        ]})
    );
}

#[test]
fn adopt_reads_a_brief_file_and_defaults_the_body() {
    let fx = Fixture::new();
    fx.task_in(SESSION, "w5:p2");

    stdout_of(
        fx.flow(&adopting(
            &[hand("w5:p2", "Fix links")],
            on("host-b", PROMPTED),
        ))
        .args(adopt_args("w5:p2", &["--parent", DISPATCHED])),
    );

    let (record, status) = fx.only_adopted();
    assert_eq!(record.parent.as_deref(), Some(DISPATCHED));
    assert_eq!(status.claude_session, None);
    let brief = fx.store().brief(&record.id).unwrap();
    assert!(
        brief.contains(
            "\nAdopted from a session started by hand on host-b. The conversation so far is the \
             context; continue the work already under way.\n"
        ),
        "{brief}"
    );

    let file = fx.dir.path().join("ask.md");
    fs::write(&file, "Finish the links from the file.\n").unwrap();
    let file = file.to_str().unwrap();
    stdout_of(
        fx.flow(&adopting(&[hand("w5:p3", "Other")], on("host-b", PROMPTED)))
            .args(adopt_args("Other", &["--brief-file", file])),
    );
    let newest = fx
        .adopted()
        .into_iter()
        .find(|(record, _)| record.parent.is_none())
        .unwrap();
    let brief = fx.store().brief(&newest.0.id).unwrap();
    assert!(
        brief.contains("\nFinish the links from the file.\n"),
        "{brief}"
    );
}

#[test]
fn adopt_refuses_what_it_cannot_take_over() {
    let fx = Fixture::new();
    let mut codex = hand("w5:p2", "Fix links");
    codex["agent"] = json!("codex");
    let mut nowhere = hand("w5:p2", "Fix links");
    nowhere["cwd"] = Value::Null;
    let cases = [
        (
            vec![codex],
            "Fix links",
            "flow adopts Claude Code sessions only; this one is codex\n".to_owned(),
        ),
        (
            vec![nowhere],
            "Fix links",
            "herdr reports no working directory for pane w5:p2 on host-b\n".to_owned(),
        ),
        (
            vec![hand("w5:p2", "Fix links")],
            "Nothing here",
            "no hand session titled or in pane Nothing here on host-b; run flow sessions\n"
                .to_owned(),
        ),
        (
            vec![hand("w5:p1", "Fix links"), hand("w5:p2", "Fix links")],
            "Fix links",
            "several hand sessions on host-b are titled \"Fix links\"; pass the pane id:\n\
             w5:p1  Fix links\n\
             w5:p2  Fix links\n"
                .to_owned(),
        ),
    ];
    for (n, (agents, session, refusal)) in cases.into_iter().enumerate() {
        fx.flow(&host_b_lists(&agents))
            .args(adopt_args(session, &[]))
            .assert()
            .code(2)
            .stderr(refusal);
        assert!(fx.adopted().is_empty());
        assert_eq!(
            fx.calls().len(),
            3 * (n + 1),
            "one listing per refusal, no prompt"
        );
    }
}

#[test]
fn adopt_refuses_a_pane_an_open_task_runs_in() {
    let fx = Fixture::new();
    fx.task_in("default", "w5:p2");

    fx.flow(&host_b_lists(&[hand("w5:p2", "Fix links")]))
        .args(adopt_args("Fix links", &[]))
        .assert()
        .code(2)
        .stderr(format!(
            "pane w5:p2 on host-b is already task {DISPATCHED}\n"
        ));
    assert!(fx.adopted().is_empty());
}

#[test]
fn adopt_refuses_a_gone_or_unknown_host() {
    let fx = Fixture::new();
    fx.write_host("host-b", "env-0002", true);

    fx.flow(&[])
        .args(adopt_args("Fix links", &[]))
        .assert()
        .code(2)
        .stderr("host host-b is gone\n");
    let mut args = adopt_args("Fix links", &[]);
    args[2] = "host-z".to_owned();
    fx.flow(&[])
        .args(&args)
        .assert()
        .code(2)
        .stderr(predicates::str::contains("host-z"));
    assert!(fx.calls().is_empty());
    assert!(fx.adopted().is_empty());
}

#[test]
fn adopt_whose_message_is_refused_holds_the_task_and_exits_1() {
    let fx = Fixture::new();
    let refused = json!({"host": "host-b", "status": 1, "stdout": "", "stderr": PROMPT_BLOCKED});

    let out = fx
        .flow(&adopting(&[hand("w5:p2", "Fix links")], refused))
        .args(adopt_args("Fix links", &[]))
        .assert()
        .code(1)
        .get_output()
        .clone();

    let (record, status) = fx.only_adopted();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout,
        format!("adopted {} on host-b (w5:p2)\n{HAND_NOTE}", record.id)
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("agent_blocked"), "{stderr}");
    assert_eq!(status.state, State::Blocked);
    let hold = status.hold.unwrap();
    assert_eq!(hold.on, HoldOn::Supervisor);
    assert!(
        hold.what.starts_with("adoption message not delivered: "),
        "{}",
        hold.what
    );
    assert!(hold.what.contains("agent_blocked"), "{}", hold.what);
    assert!(status.herdr.is_some());
    let events = fx.events(&record.id);
    assert!(events.iter().any(|ev| ev.kind == "adopted"));
    assert!(!events.iter().any(|ev| ev.kind == "steer"));
}

#[test]
fn sessions_show_an_adopted_pane_as_its_task() {
    let fx = Fixture::new();
    fx.write_host("host-a", "env-0001", true);
    let agents = [fix_links()];
    stdout_of(
        fx.flow(&adopting(&agents, on("host-b", PROMPTED)))
            .args(adopt_args("Fix links", &[])),
    );
    let (record, _) = fx.only_adopted();

    let stdout = stdout_of(fx.flow(&host_b_lists(&agents)).arg("sessions"));

    assert_eq!(
        stdout,
        format!(
            "HOST    STATUS  TASK      TITLE\n\
             host-b  idle    {}  {TITLE}\n",
            record.id
        )
    );
    let status = stdout_of(fx.flow(&[]).arg("status"));
    assert!(status.ends_with("observed: 0 sessions\n"), "{status}");
}
