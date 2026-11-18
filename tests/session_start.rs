//! `flow session start`: launches an IC Session in a new herdr tab. A fake `herdr` logs each
//! call's argv and answers as the real one does; a fake `gitpod` names this CDE.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestStore;
use jiff::Timestamp;
use serde_json::{Value, json};

const MANAGER: &str = "11111111-1111-4111-8111-111111111111";

/// What the fake `herdr` prints for each `<group> <command>`, unless a test overrides it.
const ANSWERS: &[(&str, &str)] = &[
    (
        "workspace list",
        r#"echo '{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[{"workspace_id":"w1","label":"flow"},{"workspace_id":"w2","label":"Plan the factory"}]}}'"#,
    ),
    (
        "workspace create",
        r#"echo '{"id":"cli:workspace:create","result":{"workspace":{"workspace_id":"w3"},"tab":{"tab_id":"w3:t1"},"root_pane":{"pane_id":"w3:p1"}}}'"#,
    ),
    (
        "tab create",
        r#"echo '{"id":"cli:tab:create","result":{"tab":{"tab_id":"w2:t2"},"root_pane":{"pane_id":"w2:p4"}}}'"#,
    ),
    (
        "tab rename",
        r#"echo '{"id":"cli:tab:rename","result":{"tab":{"tab_id":"w3:t1"}}}'"#,
    ),
    (
        "agent start",
        r#"echo '{"id":"cli:agent:start","result":{"agent_status":"idle"}}'"#,
    ),
    (
        "agent prompt",
        r#"echo '{"id":"cli:agent:prompt","result":{}}'"#,
    ),
];

/// A herdr error as the real CLI reports it: JSON on stderr, exit 1.
fn herdr_error(code: &str) -> String {
    format!(
        r#"echo '{{"id":"cli","error":{{"code":"{code}","message":"herdr says {code}"}}}}' >&2; exit 1"#
    )
}

struct Launch {
    ts: TestStore,
    repo: PathBuf,
    brief: PathBuf,
    log: PathBuf,
    /// The calling Session's id.
    manager: &'static str,
}

impl Launch {
    /// A repo with a wt base, Task `widgets` with a `brief.md`, the Manager Session titled "Plan the factory" in the
    /// Store, a fake `gitpod` naming this CDE "Flow", and a fake `herdr` answering `ANSWERS`
    /// except where `overrides` says otherwise.
    fn new(overrides: &[(&str, &str)]) -> Self {
        let ts = TestStore::new();
        let root = ts.scratch("workspaces");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        common::git_repo(&repo, "main");
        fs::create_dir_all(root.join("repo.wt/.base/.git")).unwrap();
        let brief = ts.scratch("tasks/widgets").join("brief.md");
        fs::write(&brief, "Build widgets.\n").unwrap();
        ts.store()
            .put_facet(
                "session",
                MANAGER,
                "context",
                &json!({"title": "Plan the factory", "branch": null}),
                Timestamp::now(),
            )
            .unwrap();
        ts.fake_bin("gitpod", "[ \"$3\" = env-flow ] && echo Flow || exit 1");
        let log = ts.scratch("herdr").join("herdr.args");
        let mut cases = String::new();
        for (cmd, answer) in ANSWERS {
            let answer = overrides
                .iter()
                .find(|(c, _)| c == cmd)
                .map_or(*answer, |(_, a)| *a);
            cases.push_str(&format!("  \"{cmd}\") {answer} ;;\n"));
        }
        ts.fake_bin(
            "herdr",
            &format!(
                "echo \"$*\" >> {}\ncase \"$1 $2\" in\n{cases}  *) echo unexpected >&2; exit 9 ;;\nesac",
                log.display()
            ),
        );
        Self {
            ts,
            repo,
            brief,
            log,
            manager: MANAGER,
        }
    }

    /// Runs `flow session start` as the Manager with `args` before the standard ones.
    fn start(&self, args: &[&str]) -> (i32, Value) {
        let out = self
            .ts
            .flow()
            .args(["session", "start"])
            .args(args)
            .env("CLAUDE_CODE_SESSION_ID", self.manager)
            .env("ONA_ID", "env-flow")
            .output()
            .unwrap();
        let json = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&out.stdout)));
        (out.status.code().unwrap(), json)
    }

    /// Starts an IC on Task `widgets` in this test's repo, plus `extra` arguments.
    fn start_widgets(&self, extra: &[&str]) -> (i32, Value) {
        let mut args = vec!["--task", "widgets"];
        args.extend_from_slice(extra);
        args.push(self.repo.to_str().unwrap());
        self.start(&args)
    }

    /// The argv of each `herdr` call so far, joined by spaces.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The `--session-id` passed to `agent start`.
fn session_id(call: &str) -> &str {
    call.split(' ')
        .skip_while(|w| *w != "--session-id")
        .nth(1)
        .expect("agent start has --session-id")
}

#[test]
fn starts_an_ic_in_a_new_tab_of_the_managers_workspace() {
    let l = Launch::new(&[]);
    let (code, out) = l.start_widgets(&[]);
    assert_eq!(code, 0, "{out}");
    let calls = l.calls();
    assert_eq!(calls.len(), 4, "{calls:#?}");
    assert_eq!(calls[0], "workspace list");
    assert_eq!(
        calls[1],
        format!(
            "tab create --workspace w2 --cwd {} --label widgets --no-focus",
            path(&l.repo)
        )
    );
    let id = session_id(&calls[2]).to_owned();
    assert_eq!(
        calls[2],
        format!(
            "agent start widgets --kind claude --pane w2:p4 --timeout 60000 -- --session-id {id} --name widgets"
        )
    );
    assert_eq!(calls[3], "agent prompt w2:p4 /flow:ic widgets");
    assert_eq!(
        out,
        json!({"id": id, "name": "widgets", "cde": "Flow", "pane": "w2:p4"})
    );
    // A UUID, version 4.
    assert_eq!(id.len(), 36);
    assert_eq!(id.as_bytes()[14], b'4');
    assert_eq!(id.matches('-').count(), 4);
}

#[test]
fn creates_the_managers_workspace_when_it_is_missing() {
    let l = Launch::new(&[(
        "workspace list",
        r#"echo '{"result":{"type":"workspace_list","workspaces":[{"workspace_id":"w1","label":"flow"}]}}'"#,
    )]);
    let (code, out) = l.start_widgets(&[]);
    assert_eq!(code, 0, "{out}");
    let calls = l.calls();
    assert_eq!(
        calls[1],
        format!(
            "workspace create --cwd {} --label Plan the factory --no-focus",
            path(&l.repo)
        )
    );
    // The new workspace's first tab is labelled with the slug, as a created tab is.
    assert_eq!(calls[2], "tab rename w3:t1 widgets");
    assert!(calls[3].starts_with("agent start widgets --kind claude --pane w3:p1 "));
    assert_eq!(out["pane"], "w3:p1");
}

#[test]
fn takes_the_id_and_workspace_as_arguments() {
    let l = Launch::new(&[]);
    let id = "22222222-2222-4222-8222-222222222222";
    let (code, out) = l.start_widgets(&["--id", id, "--workspace", "flow"]);
    assert_eq!(code, 0, "{out}");
    let calls = l.calls();
    assert!(
        calls[1].starts_with("tab create --workspace w1 "),
        "{calls:#?}"
    );
    assert_eq!(session_id(&calls[2]), id);
    assert_eq!(out["id"], id);
}

#[test]
fn names_the_workspace_after_the_managers_id_when_it_has_no_title() {
    let mut l = Launch::new(&[]);
    l.manager = "33333333-3333-4333-8333-333333333333";
    let (code, out) = l.start_widgets(&[]);
    assert_eq!(code, 0, "{out}");
    assert!(
        l.calls()[1].contains(&format!("--label {} ", l.manager)),
        "{:#?}",
        l.calls()
    );
}

/// Asserts a launch failed with exit 1, an error mentioning `needle`, a help, and exactly
/// `calls` herdr calls made.
fn assert_fails(l: &Launch, extra: &[&str], needle: &str, calls: usize) {
    let (code, out) = l.start_widgets(extra);
    assert_eq!(code, 1, "{out}");
    let error = out["error"].as_str().unwrap();
    assert!(error.contains(needle), "{error}");
    assert!(!out["help"].as_str().unwrap().is_empty());
    assert_eq!(out.as_object().unwrap().len(), 2);
    assert_eq!(l.calls().len(), calls, "{:#?}", l.calls());
}

#[test]
fn stops_when_claude_is_blocked_at_a_dialog() {
    let l = Launch::new(&[("agent start", &herdr_error("agent_not_ready"))]);
    assert_fails(&l, &[], "blocked at a dialog", 3);
    assert!(l.calls()[2].starts_with("agent start "));
}

#[test]
fn stops_when_the_pane_is_busy() {
    let l = Launch::new(&[("agent start", &herdr_error("agent_pane_busy"))]);
    assert_fails(&l, &[], "busy", 3);
}

#[test]
fn stops_when_no_herdr_server_runs() {
    let l = Launch::new(&[("workspace list", &herdr_error("server_not_running"))]);
    assert_fails(&l, &[], "no herdr server", 1);
}

#[test]
fn stops_when_the_prompt_is_refused() {
    let l = Launch::new(&[("agent prompt", &herdr_error("agent_blocked"))]);
    assert_fails(&l, &[], "agent_blocked", 4);
}

#[test]
fn stops_when_the_id_is_already_in_use() {
    let l = Launch::new(&[]);
    assert_fails(&l, &["--id", MANAGER], "already in use", 0);
}

#[test]
fn stops_when_the_repo_dir_is_missing() {
    let l = Launch::new(&[]);
    fs::remove_dir_all(&l.repo).unwrap();
    assert_fails(&l, &[], "repo dir", 0);
}

#[test]
fn stops_when_the_repo_has_no_wt_base() {
    let l = Launch::new(&[]);
    fs::remove_dir_all(l.repo.with_file_name("repo.wt")).unwrap();
    assert_fails(&l, &[], "no wt base", 0);
}

#[test]
fn stops_when_the_brief_is_missing() {
    let l = Launch::new(&[]);
    fs::remove_file(&l.brief).unwrap();
    assert_fails(&l, &[], "brief.md", 0);
}

#[test]
fn stops_when_the_task_does_not_exist() {
    let l = Launch::new(&[]);
    let (code, out) = l.start(&["--task", "gadgets", path(&l.repo)]);
    assert_eq!(code, 1, "{out}");
    assert!(out["error"].as_str().unwrap().contains("brief.md"), "{out}");
    assert!(l.calls().is_empty());
}

#[test]
fn name_and_brief_are_usage_errors_pointing_at_task() {
    let l = Launch::new(&[]);
    for args in [
        vec!["--name", "widgets", path(&l.repo)],
        vec!["--brief", path(&l.brief), path(&l.repo)],
        vec![
            "--name",
            "widgets",
            "--brief",
            path(&l.brief),
            path(&l.repo),
        ],
    ] {
        let (code, out) = l.start(&args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out["help"].as_str().unwrap().contains("--task"), "{out}");
    }
    assert!(l.calls().is_empty());
}

#[test]
fn bad_arguments_are_usage_errors() {
    let l = Launch::new(&[]);
    for args in [
        vec!["--task", "Widgets!", path(&l.repo)],
        vec!["--task", "widgets", "--id", "not-a-uuid", path(&l.repo)],
    ] {
        let (code, out) = l.start(&args);
        assert_eq!(code, 2, "{args:?}: {out}");
        assert!(out["help"].is_string());
    }
    let (code, _) = l.start(&[path(&l.repo)]);
    assert_eq!(code, 2);
    assert!(l.calls().is_empty());
}

#[test]
fn needs_a_workspace_outside_a_session() {
    let l = Launch::new(&[]);
    let out =
        l.ts.flow()
            .args(["session", "start", "--task", "widgets", path(&l.repo)])
            .output()
            .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out["error"].as_str().unwrap().contains("--workspace"));
}

/// What the fake `gitpod environment list -o json` prints: a running CDE "Remote" and a
/// stopped one, "Cold".
const ENVIRONMENTS: &str = r#"[{"id":"env-remote","metadata":{"name":"Remote"},"status":{"phase":"ENVIRONMENT_PHASE_RUNNING"}},{"id":"env-cold","metadata":{"name":"Cold"},"status":{"phase":"ENVIRONMENT_PHASE_STOPPED"}}]"#;

impl Launch {
    /// A launch whose fake `gitpod` lists `ENVIRONMENTS` and whose fake `ssh` logs its argv,
    /// one per line, and then runs `answer`.
    fn remote(answer: &str) -> Self {
        let l = Self::new(&[]);
        l.ts.fake_bin(
            "gitpod",
            &format!(
                "[ \"$*\" = 'environment list -o json' ] && echo '{ENVIRONMENTS}' && exit 0\n\
                 [ \"$3\" = env-flow ] && echo Flow || exit 1"
            ),
        );
        l.ts.fake_bin(
            "ssh",
            &format!(
                "printf '%s\\n' \"$@\" > {}\n{answer}",
                l.ssh_log().display()
            ),
        );
        l
    }

    fn ssh_log(&self) -> PathBuf {
        self.log.with_file_name("ssh.args")
    }

    /// The argv of the `ssh` call, if there was one.
    fn ssh_args(&self) -> Vec<String> {
        fs::read_to_string(self.ssh_log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

const REMOTE_ID: &str = "44444444-4444-4444-8444-444444444444";

#[test]
fn starts_an_ic_on_another_cde_over_ssh() {
    let l = Launch::remote(&format!(
        r#"echo '{{"id":"{REMOTE_ID}","name":"widgets","cde":null,"pane":"w5:p2"}}'"#
    ));
    let (code, out) = l.start_widgets(&["--cde", "Remote", "--id", REMOTE_ID]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        out,
        json!({"id": REMOTE_ID, "name": "widgets", "cde": "Remote", "pane": "w5:p2"})
    );
    assert_eq!(
        l.ssh_args(),
        [
            "-o".to_owned(),
            "BatchMode=yes".to_owned(),
            "-o".to_owned(),
            "ConnectTimeout=10".to_owned(),
            "env-remote.ona.environment".to_owned(),
            format!(
                "zsh -lc ''\\''/efs-home/flow/bin/flow'\\'' '\\''session'\\'' '\\''start'\\'' \
                 '\\''--id'\\'' '\\''{REMOTE_ID}'\\'' '\\''--task'\\'' '\\''widgets'\\'' \
                 '\\''--workspace'\\'' '\\''Plan the factory'\\'' '\\''{}'\\'''",
                path(&l.repo)
            ),
        ]
    );
    assert!(
        l.calls().is_empty(),
        "no local herdr call: {:#?}",
        l.calls()
    );
}

#[test]
fn chooses_the_remote_ics_id_before_ssh() {
    let l = Launch::remote("echo '{\"error\":\"x\",\"help\":\"y\"}'; exit 1");
    l.start_widgets(&["--cde", "Remote"]);
    let command = l.ssh_args().pop().unwrap();
    let id = command
        .split("'\\''")
        .skip_while(|w| *w != "--id")
        .nth(2)
        .unwrap()
        .to_owned();
    assert_eq!(id.len(), 36, "{command}");
}

#[test]
fn passes_a_remote_failure_through_unchanged() {
    let l = Launch::remote(
        r#"echo '{"error":"no herdr server is running: down","help":"start herdr yourself"}'; exit 1"#,
    );
    let (code, out) = l.start_widgets(&["--cde", "Remote"]);
    assert_eq!(code, 1);
    assert_eq!(
        out,
        json!({"error": "no herdr server is running: down", "help": "start herdr yourself"})
    );
}

#[test]
fn reports_an_unreachable_cde() {
    let l = Launch::remote("echo 'ssh: connect to host: timed out' >&2; exit 255");
    assert_fails(&l, &["--cde", "Remote"], "timed out", 0);
}

#[test]
fn never_starts_a_stopped_cde() {
    let l = Launch::remote("exit 0");
    assert_fails(&l, &["--cde", "Cold"], "not running", 0);
    assert!(l.ssh_args().is_empty());
}

#[test]
fn stops_when_the_cde_is_not_found() {
    let l = Launch::remote("exit 0");
    assert_fails(&l, &["--cde", "Nowhere"], "Nowhere", 0);
    assert!(l.ssh_args().is_empty());
}
