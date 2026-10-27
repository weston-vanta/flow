//! Typed access to the herdr CLI on a CDE. Every call goes through a `Runner`, so the same code
//! drives herdr on this machine or over ssh, and every call names the herdr session explicitly
//! so flow never touches the user's default session.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::runner::{Output, Runner};
use crate::state::config::Cde;

/// The herdr session every flow worker lives in.
pub const SESSION: &str = "flow";
/// The lowest herdr private protocol whose CLI output this module parses.
pub const MIN_PROTOCOL: u32 = 22;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);
const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// The exit code ssh uses for its own failures, as opposed to the remote command's.
const SSH_FAILURE_STATUS: i32 = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    /// The name herdr uses for this status on its command line and in its output.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }
}

/// An agent herdr detected in one of its panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    /// The agent kind, such as `claude`; empty when herdr has not identified it.
    pub agent: String,
    pub status: AgentStatus,
    /// Empty when herdr does not know the pane's cwd.
    pub cwd: PathBuf,
    /// The terminal title without herdr's status glyph.
    pub title: String,
}

// Response shapes, pinned against `herdr api schema --json` at protocol 22. Every CLI command
// prints one `{"id": ..., "result": {...}}` object on success, and exits non-zero with an
// `{"error": {"code", "message"}}` object on stderr on failure. Only the fields read here are
// declared; serde ignores the rest.

#[derive(Deserialize)]
struct Response<T> {
    result: T,
}

/// `agent list` → `agent_list`; `agents` is required and holds `AgentInfo` objects.
#[derive(Deserialize)]
struct AgentList {
    agents: Vec<AgentInfo>,
}

/// `agent get`, `agent wait` → `agent_info`; `agent start` → `agent_started`; `agent prompt` →
/// `agent_prompted`. All carry a required `agent: AgentInfo`.
#[derive(Deserialize)]
struct AgentResult {
    agent: AgentInfo,
}

/// `AgentInfo`: `pane_id`, `workspace_id` and `agent_status` are required; `agent`, `cwd`,
/// `terminal_title` and `terminal_title_stripped` are nullable.
#[derive(Deserialize)]
struct AgentInfo {
    pane_id: String,
    workspace_id: String,
    agent_status: AgentStatus,
    agent: Option<String>,
    cwd: Option<PathBuf>,
    terminal_title: Option<String>,
    terminal_title_stripped: Option<String>,
}

impl From<AgentInfo> for Agent {
    fn from(info: AgentInfo) -> Agent {
        Agent {
            pane_id: info.pane_id,
            workspace_id: info.workspace_id,
            agent: info.agent.unwrap_or_default(),
            status: info.agent_status,
            cwd: info.cwd.unwrap_or_default(),
            title: info
                .terminal_title_stripped
                .or(info.terminal_title)
                .unwrap_or_default(),
        }
    }
}

/// `workspace create` → `workspace_created`, whose required fields include
/// `workspace: WorkspaceInfo` (required `workspace_id`) and `root_pane: PaneInfo` (required
/// `pane_id`). The root pane is the workspace's only pane at creation. `workspace get` returns
/// a `WorkspaceInfo`, which carries no pane ids, so the create response is the one source.
#[derive(Deserialize)]
struct WorkspaceCreated {
    workspace: WorkspaceIds,
    root_pane: PaneIds,
}

#[derive(Deserialize)]
struct WorkspaceIds {
    workspace_id: String,
}

#[derive(Deserialize)]
struct PaneIds {
    pane_id: String,
}

/// `workspace close` → `ok`.
#[derive(Deserialize)]
struct Ack {}

/// What `herdr status server` reports, from its `key: value` text lines.
enum ServerStatus {
    NotRunning,
    Running { protocol: u32 },
}

fn parse_server_status(text: &str) -> anyhow::Result<ServerStatus> {
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .map(str::trim)
    };
    match field("status") {
        Some("running") => {
            let protocol = field("private_protocol").ok_or_else(|| {
                anyhow!("herdr reports no private_protocol; flow needs protocol {MIN_PROTOCOL}")
            })?;
            let protocol = protocol
                .parse()
                .with_context(|| format!("parsing herdr private_protocol {protocol:?}"))?;
            Ok(ServerStatus::Running { protocol })
        }
        Some("not running") => Ok(ServerStatus::NotRunning),
        _ => bail!("unrecognized herdr server status:\n{text}"),
    }
}

pub struct Herdr {
    runner: Arc<dyn Runner>,
    session: String,
}

impl Herdr {
    /// herdr in flow's own session.
    pub fn new(runner: Arc<dyn Runner>) -> Herdr {
        Herdr::with_session(runner, SESSION)
    }

    /// herdr in another session, for throwaway sessions such as live smoke checks.
    pub fn with_session(runner: Arc<dyn Runner>, session: &str) -> Herdr {
        Herdr {
            runner,
            session: session.to_owned(),
        }
    }

    /// Makes sure this session's herdr server runs on `cde` and speaks at least
    /// `MIN_PROTOCOL`, starting a detached server when none runs.
    pub fn ensure_server(&self, cde: &Cde) -> anyhow::Result<()> {
        if let ServerStatus::Running { protocol } = self.server_status(cde)? {
            return check_protocol(cde, protocol);
        }
        self.start_detached_server(cde)?;
        let deadline = Instant::now() + SERVER_START_TIMEOUT;
        loop {
            if let ServerStatus::Running { protocol } = self.server_status(cde)? {
                return check_protocol(cde, protocol);
            }
            if Instant::now() >= deadline {
                bail!(
                    "herdr server for session {} on {} did not start within {} s",
                    self.session,
                    cde.name,
                    SERVER_START_TIMEOUT.as_secs()
                );
            }
            thread::sleep(SERVER_POLL_INTERVAL);
        }
    }

    fn server_status(&self, cde: &Cde) -> anyhow::Result<ServerStatus> {
        parse_server_status(&self.call(cde, &self.argv(&["status", "server"]))?.stdout)
    }

    /// Starts the server in its own session with every stream on /dev/null, so it outlives
    /// this process and, over ssh, does not hold the connection open.
    fn start_detached_server(&self, cde: &Cde) -> anyhow::Result<()> {
        let mut argv = words(&[
            "setsid",
            "-f",
            "sh",
            "-c",
            r#"exec "$@" </dev/null >/dev/null 2>&1"#,
            "sh",
        ]);
        argv.extend(self.argv(&["server"]));
        let status = self
            .runner
            .spawn(cde, &argv)?
            .wait()
            .with_context(|| format!("waiting for `{}` on {}", argv.join(" "), cde.name))?;
        match status.code() {
            Some(0) => Ok(()),
            Some(SSH_FAILURE_STATUS) => bail!(
                "cde unreachable: {} while starting the herdr server",
                cde.name
            ),
            _ => bail!("`{}` on {} failed: {status}", argv.join(" "), cde.name),
        }
    }

    pub fn agents(&self, cde: &Cde) -> anyhow::Result<Vec<Agent>> {
        let list: AgentList = self.call_json(cde, &self.argv(&["agent", "list"]))?;
        Ok(list.agents.into_iter().map(Agent::from).collect())
    }

    /// Creates a workspace rooted at `cwd` without moving the user's focus, and returns its
    /// `(workspace_id, pane_id)` where the pane is the workspace's first and only pane.
    pub fn create_workspace(
        &self,
        cde: &Cde,
        cwd: &Path,
        label: &str,
    ) -> anyhow::Result<(String, String)> {
        let cwd = cwd
            .to_str()
            .ok_or_else(|| anyhow!("workspace cwd {} is not UTF-8", cwd.display()))?;
        let created: WorkspaceCreated = self.call_json(
            cde,
            &self.argv(&[
                "workspace",
                "create",
                "--cwd",
                cwd,
                "--label",
                label,
                "--no-focus",
            ]),
        )?;
        Ok((created.workspace.workspace_id, created.root_pane.pane_id))
    }

    pub fn close_workspace(&self, cde: &Cde, workspace_id: &str) -> anyhow::Result<()> {
        self.call_json::<Ack>(cde, &self.argv(&["workspace", "close", workspace_id]))?;
        Ok(())
    }

    /// Starts agent `kind` named `name` in `pane_id`, passing it `args`. herdr returns once the
    /// agent is ready for input, and fails if it exits or blocks during startup.
    pub fn start_agent(
        &self,
        cde: &Cde,
        pane_id: &str,
        name: &str,
        kind: &str,
        args: &[String],
    ) -> anyhow::Result<()> {
        let mut argv = self.argv(&["agent", "start", name, "--kind", kind, "--pane", pane_id]);
        argv.push("--".to_owned());
        argv.extend_from_slice(args);
        self.call_json::<AgentResult>(cde, &argv)?;
        Ok(())
    }

    /// Blocks until the agent in `pane_id` reaches one of `until` (herdr's default set of idle,
    /// done and blocked when empty), failing after `timeout_ms` when given.
    pub fn wait(
        &self,
        cde: &Cde,
        pane_id: &str,
        until: &[AgentStatus],
        timeout_ms: Option<u64>,
    ) -> anyhow::Result<AgentStatus> {
        let mut argv = self.argv(&["agent", "wait", pane_id]);
        for status in until {
            argv.extend(words(&["--until", status.as_str()]));
        }
        if let Some(timeout_ms) = timeout_ms {
            argv.extend(["--timeout".to_owned(), timeout_ms.to_string()]);
        }
        let result: AgentResult = self.call_json(cde, &argv)?;
        Ok(result.agent.agent_status)
    }

    /// Submits `text` to the agent in `pane_id` and returns the first idle, done or blocked
    /// status herdr observes after it.
    pub fn prompt(&self, cde: &Cde, pane_id: &str, text: &str) -> anyhow::Result<AgentStatus> {
        let result: AgentResult = self.call_json(
            cde,
            &self.argv(&["agent", "prompt", pane_id, text, "--wait"]),
        )?;
        Ok(result.agent.agent_status)
    }

    /// The last `lines` lines of the agent's terminal as plain text.
    pub fn read(&self, cde: &Cde, pane_id: &str, lines: u32) -> anyhow::Result<String> {
        let lines = lines.to_string();
        let argv = self.argv(&[
            "agent", "read", pane_id, "--lines", &lines, "--format", "text",
        ]);
        Ok(self.call(cde, &argv)?.stdout)
    }

    /// The argv that attaches a terminal to the agent in `pane_id`, to run on the agent's CDE.
    pub fn attach_argv(&self, pane_id: &str) -> Vec<String> {
        self.argv(&["agent", "attach", pane_id])
    }

    /// `herdr --session <session> <args...>`.
    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut argv = words(&["herdr", "--session", &self.session]);
        argv.extend(words(args));
        argv
    }

    /// Runs `argv` on `cde`, failing on a non-zero exit.
    fn call(&self, cde: &Cde, argv: &[String]) -> anyhow::Result<Output> {
        let output = self.runner.run(cde, argv, None)?;
        let command = argv.join(" ");
        let stderr = output.stderr.trim();
        if output.status == SSH_FAILURE_STATUS && output.stdout.is_empty() {
            bail!("cde unreachable: {} (`{command}`): {stderr}", cde.name);
        }
        if output.status != 0 {
            bail!(
                "`{command}` on {} exited {}: {stderr}",
                cde.name,
                output.status
            );
        }
        Ok(output)
    }

    fn call_json<T: DeserializeOwned>(&self, cde: &Cde, argv: &[String]) -> anyhow::Result<T> {
        let output = self.call(cde, argv)?;
        let response: Response<T> = serde_json::from_str(&output.stdout).with_context(|| {
            format!(
                "parsing output of `{}` on {}\nstdout: {}\nstderr: {}",
                argv.join(" "),
                cde.name,
                output.stdout.trim(),
                output.stderr.trim()
            )
        })?;
        Ok(response.result)
    }
}

fn check_protocol(cde: &Cde, protocol: u32) -> anyhow::Result<()> {
    if protocol < MIN_PROTOCOL {
        bail!(
            "herdr on {} speaks protocol {protocol}; flow needs protocol {MIN_PROTOCOL} or later",
            cde.name
        );
    }
    Ok(())
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|word| (*word).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use crate::runner::{FakeRunner, Scripted};

    use super::*;

    const AGENT_LIST: &str = include_str!("../tests/fixtures/herdr/agent_list.json");
    const AGENT_GET: &str = include_str!("../tests/fixtures/herdr/agent_get.json");
    const WORKSPACE_CREATE: &str = include_str!("../tests/fixtures/herdr/workspace_create.json");

    const STATUS_NOT_RUNNING: &str =
        "status: not running\nsocket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";

    fn status_running(protocol: u32) -> String {
        format!(
            "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
             private_protocol: {protocol}\nprivate_protocol_compatible: yes\n\
             socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n"
        )
    }

    fn ok(stdout: &str) -> Scripted {
        Scripted {
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    fn cde() -> Cde {
        Cde {
            name: "cde-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "cde-a.example".to_owned(),
            local_marker_env: None,
        }
    }

    fn herdr(script: Vec<Scripted>) -> (Herdr, Arc<FakeRunner>) {
        let fake = Arc::new(FakeRunner::new(script));
        (Herdr::new(fake.clone()), fake)
    }

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    fn argvs(fake: &FakeRunner) -> Vec<Vec<String>> {
        fake.calls().into_iter().map(|call| call.argv).collect()
    }

    #[test]
    fn agents_parses_fixture() {
        let (herdr, fake) = herdr(vec![ok(AGENT_LIST)]);
        let agents = herdr.agents(&cde()).unwrap();
        assert_eq!(
            agents,
            [
                Agent {
                    pane_id: "w1:p1".to_owned(),
                    workspace_id: "w1".to_owned(),
                    agent: "claude".to_owned(),
                    status: AgentStatus::Idle,
                    cwd: PathBuf::from("/workspaces/acme-worktrees/login-form"),
                    title: "Refactor login form".to_owned(),
                },
                Agent {
                    pane_id: "w2:p1".to_owned(),
                    workspace_id: "w2".to_owned(),
                    agent: "claude".to_owned(),
                    status: AgentStatus::Working,
                    cwd: PathBuf::from("/workspaces/acme-worktrees/cache-fix"),
                    title: "Fix cache eviction".to_owned(),
                },
            ]
        );
        assert_eq!(
            argvs(&fake),
            [words(&["herdr", "--session", "flow", "agent", "list"])]
        );
    }

    #[test]
    fn wait_passes_until_flags() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        let status = herdr
            .wait(
                &cde(),
                "w2:p1",
                &[AgentStatus::Blocked, AgentStatus::Done],
                Some(5000),
            )
            .unwrap();
        assert_eq!(status, AgentStatus::Working);
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "agent",
                "wait",
                "w2:p1",
                "--until",
                "blocked",
                "--until",
                "done",
                "--timeout",
                "5000",
            ])]
        );
    }

    #[test]
    fn wait_without_timeout_omits_the_flag() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        herdr
            .wait(&cde(), "w2:p1", &[AgentStatus::Done], None)
            .unwrap();
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "agent",
                "wait",
                "w2:p1",
                "--until",
                "done",
            ])]
        );
    }

    #[test]
    fn ensure_server_fails_on_old_protocol() {
        let (herdr, fake) = herdr(vec![ok(&status_running(21))]);
        let err = herdr.ensure_server(&cde()).unwrap_err().to_string();
        assert!(err.contains("protocol 22"), "{err}");
        assert!(err.contains("21"), "{err}");
        assert_eq!(fake.calls().len(), 1, "no server start attempted");
    }

    #[test]
    fn ensure_server_accepts_running_server() {
        let (herdr, fake) = herdr(vec![ok(&status_running(22))]);
        herdr.ensure_server(&cde()).unwrap();
        assert_eq!(
            argvs(&fake),
            [words(&["herdr", "--session", "flow", "status", "server"])]
        );
    }

    #[test]
    fn ensure_server_starts_a_detached_server_when_not_running() {
        let (herdr, fake) = herdr(vec![
            ok(STATUS_NOT_RUNNING),
            ok(""),
            ok(&status_running(22)),
        ]);
        herdr.ensure_server(&cde()).unwrap();
        let calls = argvs(&fake);
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(
            calls[1],
            words(&[
                "setsid",
                "-f",
                "sh",
                "-c",
                r#"exec "$@" </dev/null >/dev/null 2>&1"#,
                "sh",
                "herdr",
                "--session",
                "flow",
                "server",
            ])
        );
    }

    #[test]
    fn create_workspace_returns_workspace_and_root_pane() {
        let (herdr, fake) = herdr(vec![ok(WORKSPACE_CREATE)]);
        let ids = herdr
            .create_workspace(
                &cde(),
                Path::new("/workspaces/acme-worktrees/search-index"),
                "search-index",
            )
            .unwrap();
        assert_eq!(ids, ("w3".to_owned(), "w3:p1".to_owned()));
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "workspace",
                "create",
                "--cwd",
                "/workspaces/acme-worktrees/search-index",
                "--label",
                "search-index",
                "--no-focus",
            ])]
        );
    }

    #[test]
    fn start_agent_passes_args_after_double_dash() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        herdr
            .start_agent(
                &cde(),
                "w2:p1",
                "cache-fix",
                "claude",
                &words(&["--name", "cache-fix", "Read the brief."]),
            )
            .unwrap();
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "agent",
                "start",
                "cache-fix",
                "--kind",
                "claude",
                "--pane",
                "w2:p1",
                "--",
                "--name",
                "cache-fix",
                "Read the brief.",
            ])]
        );
    }

    #[test]
    fn prompt_waits_and_returns_status() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        let status = herdr.prompt(&cde(), "w2:p1", "keep going").unwrap();
        assert_eq!(status, AgentStatus::Working);
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "agent",
                "prompt",
                "w2:p1",
                "keep going",
                "--wait",
            ])]
        );
    }

    #[test]
    fn read_returns_plain_text() {
        let (herdr, fake) = herdr(vec![ok("line one\nline two\n")]);
        assert_eq!(
            herdr.read(&cde(), "w2:p1", 40).unwrap(),
            "line one\nline two\n"
        );
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "agent",
                "read",
                "w2:p1",
                "--lines",
                "40",
                "--format",
                "text",
            ])]
        );
    }

    #[test]
    fn attach_argv_targets_the_pane() {
        let (herdr, _) = herdr(Vec::new());
        assert_eq!(
            herdr.attach_argv("w2:p1"),
            words(&["herdr", "--session", "flow", "agent", "attach", "w2:p1"])
        );
    }

    #[test]
    fn every_call_passes_session_flow() {
        let (herdr, fake) = herdr(vec![
            ok(STATUS_NOT_RUNNING),
            ok(""),
            ok(&status_running(22)),
            ok(AGENT_LIST),
            ok(WORKSPACE_CREATE),
            ok(AGENT_GET),
            ok(AGENT_GET),
            ok(AGENT_GET),
            ok("text"),
            ok(r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#),
        ]);
        let target = cde();
        herdr.ensure_server(&target).unwrap();
        herdr.agents(&target).unwrap();
        herdr
            .create_workspace(&target, Path::new("/workspaces/acme"), "label")
            .unwrap();
        herdr
            .start_agent(&target, "w3:p1", "name", "claude", &[])
            .unwrap();
        herdr
            .wait(&target, "w3:p1", &[AgentStatus::Done], None)
            .unwrap();
        herdr.prompt(&target, "w3:p1", "hello").unwrap();
        herdr.read(&target, "w3:p1", 10).unwrap();
        herdr.close_workspace(&target, "w3").unwrap();

        let mut calls = argvs(&fake);
        assert_eq!(calls.len(), 10, "{calls:?}");
        calls.push(herdr.attach_argv("w3:p1"));
        for argv in calls {
            assert!(
                argv.windows(2).any(|pair| pair == ["--session", "flow"]),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn unreachable_cde_is_reported_as_such() {
        let (herdr, _) = herdr(vec![Scripted {
            status: 255,
            stderr: "ssh: connect to host cde-a.example port 22: Connection refused\n".to_owned(),
            ..Scripted::default()
        }]);
        let err = herdr.agents(&cde()).unwrap_err().to_string();
        assert!(err.starts_with("cde unreachable:"), "{err}");
        assert!(
            err.contains("cde-a") && err.contains("Connection refused"),
            "{err}"
        );
    }

    #[test]
    fn failed_command_reports_command_and_stderr() {
        let (herdr, _) = herdr(vec![Scripted {
            status: 1,
            stderr: r#"{"error":{"code":"agent_not_found","message":"agent target w9:p1 not found"},"id":"cli:agent:wait"}"#.to_owned(),
            ..Scripted::default()
        }]);
        let err = herdr
            .wait(&cde(), "w9:p1", &[AgentStatus::Done], None)
            .unwrap_err()
            .to_string();
        assert!(!err.starts_with("cde unreachable:"), "{err}");
        assert!(err.contains("agent wait w9:p1"), "{err}");
        assert!(err.contains("agent_not_found"), "{err}");
    }

    #[test]
    fn unparsable_output_reports_command_and_stderr() {
        let (herdr, _) = herdr(vec![Scripted {
            stdout: "not json".to_owned(),
            stderr: "warning: config reload pending".to_owned(),
            ..Scripted::default()
        }]);
        let err = format!("{:#}", herdr.agents(&cde()).unwrap_err());
        assert!(err.contains("agent list"), "{err}");
        assert!(err.contains("config reload pending"), "{err}");
    }
}
