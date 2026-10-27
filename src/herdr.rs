//! Typed access to the herdr CLI on a host. Every call goes through a `Runner`, so the same code
//! drives herdr on this machine or over ssh, and every call names the herdr session explicitly
//! so flow touches another session, such as the user's default one, only where it asks to.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::runner::{Output, Runner, shell_quote};
use crate::state::config::Host;

/// The herdr session every flow worker lives in.
pub const SESSION: &str = "flow";
/// The lowest herdr private protocol whose CLI output this module parses.
pub const MIN_PROTOCOL: u32 = 22;

/// herdr's logical key names for `pane send-keys`. `--help` documents only `esc`; these two
/// were confirmed against herdr 0.9.5, which rejects unknown names with `invalid_key`.
pub const KEY_ENTER: &str = "enter";
pub const KEY_DOWN: &str = "down";

/// The `error.code` herdr reports when a wait's `--timeout` expires.
const TIMEOUT_CODE: &str = "timeout";
/// The `error.code` herdr reports when a target pane does not exist.
const AGENT_NOT_FOUND_CODE: &str = "agent_not_found";
/// The `error.code` herdr reports when a target workspace does not exist.
const WORKSPACE_NOT_FOUND_CODE: &str = "workspace_not_found";
/// The `error.code` herdr reports when no server runs for the session.
const SERVER_NOT_RUNNING_CODE: &str = "server_not_running";
/// Not a herdr code: `call_or_code` returns it for a non-zero exit with nothing on stdout or
/// stderr, when the caller lists it.
const NO_ANSWER: &str = "no_answer";
/// How every error from a call that ssh could not deliver starts.
const UNREACHABLE_PREFIX: &str = "host unreachable:";

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
    /// herdr's `unknown`, and any status a newer herdr reports that this build does not know.
    #[serde(other)]
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
    /// The herdr session the agent's pane lives in. Pane ids are unique only within a session.
    /// Inventories cached before sessions were recorded held only flow's own agents.
    #[serde(default = "flow_session")]
    pub session: String,
}

fn flow_session() -> String {
    SESSION.to_owned()
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

impl AgentInfo {
    fn into_agent(self, session: &str) -> Agent {
        Agent {
            pane_id: self.pane_id,
            workspace_id: self.workspace_id,
            agent: self.agent.unwrap_or_default(),
            status: self.agent_status,
            cwd: self.cwd.unwrap_or_default(),
            title: self
                .terminal_title_stripped
                .or(self.terminal_title)
                .unwrap_or_default(),
            session: session.to_owned(),
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

/// `workspace list` → `workspace_list`.
#[derive(Deserialize)]
struct WorkspaceList {
    workspaces: Vec<Workspace>,
}

/// A herdr workspace: its id and the label it was created with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Deserialize)]
struct WorkspaceIds {
    workspace_id: String,
}

#[derive(Deserialize)]
struct PaneIds {
    pane_id: String,
}

/// `pane wait-output` → `output_matched`, whose required fields include `pane_id`.
#[derive(Deserialize)]
struct OutputMatched {
    #[expect(dead_code, reason = "declared so a response without it fails to parse")]
    pane_id: String,
}

/// A failed command's stderr: `{"error": {"code", "message"}, "id"}`, `code` required.
#[derive(Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    code: String,
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

    /// herdr in another session, such as the one the user starts sessions in by hand or a
    /// throwaway one for live smoke checks.
    pub fn with_session(runner: Arc<dyn Runner>, session: &str) -> Herdr {
        Herdr {
            runner,
            session: session.to_owned(),
        }
    }

    /// Makes sure this session's herdr server runs on `host` and speaks at least
    /// `MIN_PROTOCOL`, starting a detached server when none runs.
    pub fn ensure_server(&self, host: &Host) -> anyhow::Result<()> {
        if let ServerStatus::Running { protocol } = self.server_status(host)? {
            return check_protocol(host, protocol);
        }
        self.start_detached_server(host)?;
        let deadline = Instant::now() + SERVER_START_TIMEOUT;
        loop {
            if let ServerStatus::Running { protocol } = self.server_status(host)? {
                return check_protocol(host, protocol);
            }
            if Instant::now() >= deadline {
                bail!(
                    "herdr server for session {} on {} did not start within {} s",
                    self.session,
                    host.name,
                    SERVER_START_TIMEOUT.as_secs()
                );
            }
            thread::sleep(SERVER_POLL_INTERVAL);
        }
    }

    fn server_status(&self, host: &Host) -> anyhow::Result<ServerStatus> {
        parse_server_status(&self.call(host, &self.argv(&["status", "server"]))?.stdout)
    }

    /// Starts the server in its own session with every stream on /dev/null, so it outlives
    /// this process and, over ssh, does not hold the connection open.
    fn start_detached_server(&self, host: &Host) -> anyhow::Result<()> {
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
            .spawn(host, &argv)?
            .wait()
            .with_context(|| format!("waiting for `{}` on {}", argv.join(" "), host.name))?;
        match status.code() {
            Some(0) => Ok(()),
            Some(SSH_FAILURE_STATUS) => bail!(
                "{UNREACHABLE_PREFIX} {} while starting the herdr server",
                host.name
            ),
            _ => bail!("`{}` on {} failed: {status}", argv.join(" "), host.name),
        }
    }

    pub fn agents(&self, host: &Host) -> anyhow::Result<Vec<Agent>> {
        let list: AgentList = self.call_json(host, &self.argv(&["agent", "list"]))?;
        Ok(self.agents_from(list))
    }

    /// This session's agents on `host`, or `None` when no server runs for the session. Unlike
    /// `agents` after `ensure_server`, this never starts a server.
    pub fn agents_if_running(&self, host: &Host) -> anyhow::Result<Option<Vec<Agent>>> {
        let argv = self.argv(&["agent", "list"]);
        let Some(output) = self.call_unless(host, &argv, SERVER_NOT_RUNNING_CODE)? else {
            return Ok(None);
        };
        let list: AgentList = parse_result(host, &argv, &output)?;
        Ok(Some(self.agents_from(list)))
    }

    fn agents_from(&self, list: AgentList) -> Vec<Agent> {
        list.agents
            .into_iter()
            .map(|info| info.into_agent(&self.session))
            .collect()
    }

    /// Creates a workspace rooted at `cwd` without moving the user's focus, and returns its
    /// `(workspace_id, pane_id)` where the pane is the workspace's first and only pane.
    pub fn create_workspace(
        &self,
        host: &Host,
        cwd: &Path,
        label: &str,
    ) -> anyhow::Result<(String, String)> {
        let cwd = cwd
            .to_str()
            .ok_or_else(|| anyhow!("workspace cwd {} is not UTF-8", cwd.display()))?;
        let created: WorkspaceCreated = self.call_json(
            host,
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

    /// The workspaces in flow's session on `host`.
    pub fn workspaces(&self, host: &Host) -> anyhow::Result<Vec<Workspace>> {
        let list: WorkspaceList = self.call_json(host, &self.argv(&["workspace", "list"]))?;
        Ok(list.workspaces)
    }

    /// Closes `workspace_id`; `false` when herdr no longer knows it, so there was nothing to
    /// close.
    pub fn close_workspace(&self, host: &Host, workspace_id: &str) -> anyhow::Result<bool> {
        let argv = self.argv(&["workspace", "close", workspace_id]);
        let Some(output) = self.call_unless(host, &argv, WORKSPACE_NOT_FOUND_CODE)? else {
            return Ok(false);
        };
        parse_result::<Ack>(host, &argv, &output)?;
        Ok(true)
    }

    /// Starts agent `kind` named `name` in `pane_id`, passing it `args`. herdr returns once the
    /// agent is ready for input, and fails if it exits or blocks during startup.
    pub fn start_agent(
        &self,
        host: &Host,
        pane_id: &str,
        name: &str,
        kind: &str,
        args: &[String],
    ) -> anyhow::Result<()> {
        let mut argv = self.argv(&["agent", "start", name, "--kind", kind, "--pane", pane_id]);
        argv.push("--".to_owned());
        argv.extend_from_slice(args);
        self.call_json::<AgentResult>(host, &argv)?;
        Ok(())
    }

    /// Blocks until the agent in `pane_id` reaches one of `until` (herdr's default set of idle,
    /// done and blocked when empty) and returns that status, or `None` once `timeout_ms`
    /// passes.
    pub fn wait(
        &self,
        host: &Host,
        pane_id: &str,
        until: &[AgentStatus],
        timeout_ms: Option<u64>,
    ) -> anyhow::Result<Option<AgentStatus>> {
        let argv = self.wait_argv(pane_id, until, timeout_ms);
        let Some(output) = self.call_or_timeout(host, &argv)? else {
            return Ok(None);
        };
        let result: AgentResult = parse_result(host, &argv, &output)?;
        Ok(Some(result.agent.agent_status))
    }

    /// `wait`, telling apart a pane where herdr sees no agent (yet) and a wait that exited
    /// non-zero without a word, both of which `wait` reports as errors.
    pub fn wait_for_agent(
        &self,
        host: &Host,
        pane_id: &str,
        until: &[AgentStatus],
        timeout_ms: Option<u64>,
    ) -> anyhow::Result<Waited> {
        let argv = self.wait_argv(pane_id, until, timeout_ms);
        let codes = [TIMEOUT_CODE, AGENT_NOT_FOUND_CODE, NO_ANSWER];
        let output = match self.call_or_code(host, &argv, &codes)? {
            Ok(output) => output,
            Err(TIMEOUT_CODE) => return Ok(Waited::TimedOut),
            Err(NO_ANSWER) => return Ok(Waited::NoAnswer),
            Err(_) => return Ok(Waited::NoAgent),
        };
        let result: AgentResult = parse_result(host, &argv, &output)?;
        Ok(Waited::Reached(result.agent.agent_status))
    }

    /// Starts the `wait` call without blocking on it. herdr's stderr is folded into the child's
    /// stdout, which `wait_end` reads once the child exits.
    pub fn spawn_wait(
        &self,
        host: &Host,
        pane_id: &str,
        until: &[AgentStatus],
        timeout_ms: Option<u64>,
    ) -> anyhow::Result<Child> {
        let mut argv = words(&["sh", "-c", r#"exec "$@" 2>&1"#, "sh"]);
        argv.extend(self.wait_argv(pane_id, until, timeout_ms));
        self.runner.spawn(host, &argv)
    }

    fn wait_argv(
        &self,
        pane_id: &str,
        until: &[AgentStatus],
        timeout_ms: Option<u64>,
    ) -> Vec<String> {
        let mut argv = self.argv(&["agent", "wait", pane_id]);
        for status in until {
            argv.extend(words(&["--until", status.as_str()]));
        }
        if let Some(timeout_ms) = timeout_ms {
            argv.extend(["--timeout".to_owned(), timeout_ms.to_string()]);
        }
        argv
    }

    /// Types `argv` as one command line into the shell in `pane_id`, followed by Enter. herdr
    /// joins its arguments with spaces without quoting, so each word is quoted here. Success
    /// means herdr queued the line, not that the command started.
    pub fn pane_run(&self, host: &Host, pane_id: &str, argv: &[String]) -> anyhow::Result<()> {
        if argv.is_empty() {
            bail!("no command to run in pane {pane_id} on {}", host.name);
        }
        let line = argv
            .iter()
            .map(|word| shell_word(word))
            .collect::<Vec<_>>()
            .join(" ");
        self.call(host, &self.argv(&["pane", "run", pane_id, &line]))?;
        Ok(())
    }

    /// Waits up to `timeout_ms` for the pane's recent output, existing output included, to
    /// match the Rust regex `pattern`; `false` when the timeout passes first.
    pub fn pane_wait_output(
        &self,
        host: &Host,
        pane_id: &str,
        pattern: &str,
        timeout_ms: u64,
    ) -> anyhow::Result<bool> {
        let timeout_ms = timeout_ms.to_string();
        let argv = self.argv(&[
            "pane",
            "wait-output",
            "--regex",
            pattern,
            "--timeout",
            &timeout_ms,
            pane_id,
        ]);
        let Some(output) = self.call_or_timeout(host, &argv)? else {
            return Ok(false);
        };
        parse_result::<OutputMatched>(host, &argv, &output)?;
        Ok(true)
    }

    /// Sends `keys`, herdr logical key names such as `KEY_ENTER`, to the pane in order.
    pub fn pane_send_keys(&self, host: &Host, pane_id: &str, keys: &[&str]) -> anyhow::Result<()> {
        if keys.is_empty() {
            bail!("no keys to send to pane {pane_id} on {}", host.name);
        }
        let mut argv = self.argv(&["pane", "send-keys", pane_id]);
        argv.extend(words(keys));
        self.call(host, &argv)?;
        Ok(())
    }

    /// Submits `text` to the agent in `pane_id` and returns the first status in `until` herdr
    /// observes after it (idle, done or blocked when `until` is empty). herdr rejects the
    /// prompt when the agent is blocked.
    pub fn prompt(
        &self,
        host: &Host,
        pane_id: &str,
        text: &str,
        until: &[AgentStatus],
    ) -> anyhow::Result<AgentStatus> {
        let mut argv = self.argv(&["agent", "prompt", pane_id, text, "--wait"]);
        for status in until {
            argv.extend(words(&["--until", status.as_str()]));
        }
        let result: AgentResult = self.call_json(host, &argv)?;
        Ok(result.agent.agent_status)
    }

    /// The last `lines` lines of the terminal in `pane_id`, whether or not herdr sees an agent
    /// there.
    pub fn pane_read(&self, host: &Host, pane_id: &str, lines: u32) -> anyhow::Result<String> {
        let lines = lines.to_string();
        let argv = self.argv(&[
            "pane", "read", pane_id, "--lines", &lines, "--format", "text",
        ]);
        Ok(self.call(host, &argv)?.stdout)
    }

    /// The last `lines` lines of the agent's screen in `pane_id`; fails with `agent_not_found`
    /// when herdr sees no agent there.
    pub fn read(&self, host: &Host, pane_id: &str, lines: u32) -> anyhow::Result<String> {
        let lines = lines.to_string();
        let argv = self.argv(&[
            "agent", "read", pane_id, "--lines", &lines, "--format", "text",
        ]);
        Ok(self.call(host, &argv)?.stdout)
    }

    /// The argv that attaches a terminal to the agent in `pane_id`, to run on the agent's host.
    pub fn attach_argv(&self, pane_id: &str) -> Vec<String> {
        attach_argv(&self.session, pane_id)
    }

    /// `herdr --session <session> <args...>`.
    fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut argv = words(&["herdr", "--session", &self.session]);
        argv.extend(words(args));
        argv
    }

    /// Runs `argv` on `host`, failing on a non-zero exit.
    fn call(&self, host: &Host, argv: &[String]) -> anyhow::Result<Output> {
        self.call_or_timeout(host, argv)?
            .ok_or_else(|| anyhow!("`{}` on {} timed out", argv.join(" "), host.name))
    }

    /// Runs `argv` on `host`: `None` when herdr fails with its `timeout` error code, an error
    /// for any other non-zero exit.
    fn call_or_timeout(&self, host: &Host, argv: &[String]) -> anyhow::Result<Option<Output>> {
        self.call_unless(host, argv, TIMEOUT_CODE)
    }

    /// Runs `argv` on `host`: `None` when herdr fails with error code `code`, an error for any
    /// other non-zero exit.
    fn call_unless(
        &self,
        host: &Host,
        argv: &[String],
        code: &'static str,
    ) -> anyhow::Result<Option<Output>> {
        Ok(self.call_or_code(host, argv, &[code])?.ok())
    }

    /// Runs `argv` on `host`: `Err(code)` when herdr fails with one of `codes`, or when it exits
    /// non-zero with no output and `codes` holds `NO_ANSWER`; an error for any other non-zero
    /// exit.
    fn call_or_code(
        &self,
        host: &Host,
        argv: &[String],
        codes: &[&'static str],
    ) -> anyhow::Result<Result<Output, &'static str>> {
        let output = self.runner.run(host, argv, None)?;
        let command = argv.join(" ");
        let stderr = output.stderr.trim();
        if output.status == SSH_FAILURE_STATUS && output.stdout.is_empty() {
            bail!("{UNREACHABLE_PREFIX} {} (`{command}`): {stderr}", host.name);
        }
        if output.status == 0 {
            return Ok(Ok(output));
        }
        if let Some(code) = error_code(stderr)
            && let Some(code) = codes.iter().find(|known| **known == code)
        {
            return Ok(Err(*code));
        }
        let silent = stderr.is_empty() && output.stdout.trim().is_empty();
        if silent && codes.contains(&NO_ANSWER) {
            return Ok(Err(NO_ANSWER));
        }
        let reason = if silent {
            " (no output)".to_owned()
        } else {
            format!(": {stderr}")
        };
        bail!(
            "`{command}` on {} exited {}{reason}",
            host.name,
            output.status
        );
    }

    fn call_json<T: DeserializeOwned>(&self, host: &Host, argv: &[String]) -> anyhow::Result<T> {
        parse_result(host, argv, &self.call(host, argv)?)
    }
}

/// The argv that attaches a terminal to the agent in `pane_id` of herdr session `session`.
pub fn attach_argv(session: &str, pane_id: &str) -> Vec<String> {
    words(&["herdr", "--session", session, "agent", "attach", pane_id])
}

/// How a `wait_for_agent` call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// The agent reached this status.
    Reached(AgentStatus),
    /// The wait's `--timeout` passed first.
    TimedOut,
    /// herdr sees no agent in the pane: it has not detected one yet, or the pane is gone.
    NoAgent,
    /// The wait exited non-zero with nothing on stdout or stderr, saying nothing about the
    /// agent.
    NoAnswer,
}

/// How a `spawn_wait` child ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitEnd {
    /// The agent reached this status.
    Reached(AgentStatus),
    /// The wait's `--timeout` passed first.
    TimedOut,
    /// herdr does not know the pane, or no server runs for the session.
    Gone,
    /// ssh could not reach the host.
    Unreachable,
    /// Anything else, described.
    Failed(String),
}

/// Reads a `spawn_wait` child's exit code (`None` when a signal killed it) and its output,
/// stdout and stderr merged. The result is the first line that parses as one, so a stray
/// stderr line cannot hide it.
pub fn wait_end(code: Option<i32>, stdout: &str) -> WaitEnd {
    let stdout = stdout.trim();
    if code == Some(SSH_FAILURE_STATUS) && stdout.is_empty() {
        return WaitEnd::Unreachable;
    }
    let reached = stdout.lines().find_map(|line| {
        serde_json::from_str::<Response<AgentResult>>(line)
            .ok()
            .map(|response| response.result.agent.agent_status)
    });
    if code == Some(0)
        && let Some(status) = reached
    {
        return WaitEnd::Reached(status);
    }
    match error_code(stdout).as_deref() {
        Some(TIMEOUT_CODE) => WaitEnd::TimedOut,
        Some(AGENT_NOT_FOUND_CODE | SERVER_NOT_RUNNING_CODE) => WaitEnd::Gone,
        _ => WaitEnd::Failed(match code {
            Some(code) => format!("exited {code}: {stdout}"),
            None => format!("killed by a signal: {stdout}"),
        }),
    }
}

/// Whether `err`, or an error it wraps, says ssh could not reach the host, as opposed to herdr
/// failing there.
pub fn is_unreachable(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().starts_with(UNREACHABLE_PREFIX))
}

fn parse_result<T: DeserializeOwned>(
    host: &Host,
    argv: &[String],
    output: &Output,
) -> anyhow::Result<T> {
    let response: Response<T> = serde_json::from_str(&output.stdout).with_context(|| {
        format!(
            "parsing output of `{}` on {}\nstdout: {}\nstderr: {}",
            argv.join(" "),
            host.name,
            output.stdout.trim(),
            output.stderr.trim()
        )
    })?;
    Ok(response.result)
}

/// The `error.code` of the herdr error object in `stderr`, if there is one.
fn error_code(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        serde_json::from_str::<ErrorResponse>(line)
            .ok()
            .map(|response| response.error.code)
    })
}

/// `word` as typed into an interactive shell: bare when it has no characters any shell treats
/// specially, single-quoted otherwise.
pub(crate) fn shell_word(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:,+@".contains(c));
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(shell_quote(word))
    }
}

fn check_protocol(host: &Host, protocol: u32) -> anyhow::Result<()> {
    if protocol < MIN_PROTOCOL {
        bail!(
            "herdr on {} speaks protocol {protocol}; flow needs protocol {MIN_PROTOCOL} or later",
            host.name
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
    const AGENT_LIST_SERVER_NOT_RUNNING: &str =
        include_str!("../tests/fixtures/herdr/agent_list_server_not_running.json");
    const WORKSPACE_CREATE: &str = include_str!("../tests/fixtures/herdr/workspace_create.json");
    const AGENT_WAIT_TIMEOUT: &str =
        include_str!("../tests/fixtures/herdr/agent_wait_timeout.json");
    const PANE_WAIT_OUTPUT_MATCHED: &str =
        include_str!("../tests/fixtures/herdr/pane_wait_output_matched.json");
    const PANE_WAIT_OUTPUT_TIMEOUT: &str =
        include_str!("../tests/fixtures/herdr/pane_wait_output_timeout.json");

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

    fn failed(stderr: &str) -> Scripted {
        Scripted {
            status: 1,
            stderr: stderr.to_owned(),
            ..Scripted::default()
        }
    }

    fn host() -> Host {
        Host {
            name: "host-a".to_owned(),
            env_id: "env-0001".to_owned(),
            ssh_alias: "host-a.example".to_owned(),
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
        let agents = herdr.agents(&host()).unwrap();
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
                    session: "flow".to_owned(),
                },
                Agent {
                    pane_id: "w2:p1".to_owned(),
                    workspace_id: "w2".to_owned(),
                    agent: "claude".to_owned(),
                    status: AgentStatus::Working,
                    cwd: PathBuf::from("/workspaces/acme-worktrees/cache-fix"),
                    title: "Fix cache eviction".to_owned(),
                    session: "flow".to_owned(),
                },
            ]
        );
        assert_eq!(
            argvs(&fake),
            [words(&["herdr", "--session", "flow", "agent", "list"])]
        );
    }

    #[test]
    fn agents_record_their_session() {
        let fake = Arc::new(FakeRunner::new(vec![ok(AGENT_LIST)]));
        let herdr = Herdr::with_session(fake.clone(), "default");
        let agents = herdr.agents_if_running(&host()).unwrap().unwrap();
        assert!(agents.iter().all(|agent| agent.session == "default"));
        assert_eq!(
            argvs(&fake),
            [words(&["herdr", "--session", "default", "agent", "list"])]
        );
    }

    #[test]
    fn agents_if_running_is_none_without_a_server() {
        let fake = Arc::new(FakeRunner::new(vec![
            failed(AGENT_LIST_SERVER_NOT_RUNNING),
            failed(r#"{"error":{"code":"internal","message":"boom"},"id":"cli:agent:list"}"#),
        ]));
        let herdr = Herdr::with_session(fake, "default");
        assert_eq!(herdr.agents_if_running(&host()).unwrap(), None);
        let err = herdr.agents_if_running(&host()).unwrap_err();
        assert!(!is_unreachable(&err), "{err:#}");
    }

    #[test]
    fn wait_passes_until_flags() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        let status = herdr
            .wait(
                &host(),
                "w2:p1",
                &[AgentStatus::Blocked, AgentStatus::Done],
                Some(5000),
            )
            .unwrap();
        assert_eq!(status, Some(AgentStatus::Working));
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
    fn spawn_wait_folds_stderr_into_stdout() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        let out = herdr
            .spawn_wait(&host(), "w2:p1", &[AgentStatus::Done], Some(5000))
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_eq!(
            wait_end(out.status.code(), &String::from_utf8(out.stdout).unwrap()),
            WaitEnd::Reached(AgentStatus::Working)
        );
        assert_eq!(
            argvs(&fake),
            [words(&[
                "sh",
                "-c",
                r#"exec "$@" 2>&1"#,
                "sh",
                "herdr",
                "--session",
                "flow",
                "agent",
                "wait",
                "w2:p1",
                "--until",
                "done",
                "--timeout",
                "5000",
            ])]
        );
    }

    #[test]
    fn wait_end_reads_failures() {
        let not_found = r#"{"error":{"code":"agent_not_found","message":"agent target w9:p1 not found"},"id":"cli:agent:wait"}"#;
        assert_eq!(wait_end(Some(1), not_found), WaitEnd::Gone);
        assert_eq!(
            wait_end(Some(1), AGENT_LIST_SERVER_NOT_RUNNING),
            WaitEnd::Gone
        );
        assert_eq!(wait_end(Some(1), AGENT_WAIT_TIMEOUT), WaitEnd::TimedOut);
        assert_eq!(wait_end(Some(255), ""), WaitEnd::Unreachable);
        assert!(matches!(wait_end(Some(1), "boom"), WaitEnd::Failed(_)));
        assert!(matches!(wait_end(None, ""), WaitEnd::Failed(_)));
    }

    #[test]
    fn wait_end_skips_noise_before_the_result() {
        let stdout = format!("warning: config file has an unknown key\n{AGENT_GET}");
        assert_eq!(
            wait_end(Some(0), &stdout),
            WaitEnd::Reached(AgentStatus::Working)
        );
    }

    #[test]
    fn unknown_agent_status_reads_as_unknown() {
        let status: AgentStatus = serde_json::from_str(r#""paused""#).unwrap();
        assert_eq!(status, AgentStatus::Unknown);
        let list = AGENT_LIST.replacen(r#""agent_status":"idle""#, r#""agent_status":"paused""#, 1);
        assert_ne!(list, AGENT_LIST);
        let (herdr, _) = herdr(vec![ok(&list)]);
        let agents = herdr.agents(&host()).unwrap();
        assert_eq!(agents[0].status, AgentStatus::Unknown);
        assert_eq!(
            serde_json::to_string(&AgentStatus::Unknown).unwrap(),
            r#""unknown""#
        );
    }

    #[test]
    fn wait_timeout_returns_none() {
        let (herdr, _) = herdr(vec![failed(AGENT_WAIT_TIMEOUT)]);
        let status = herdr
            .wait(&host(), "w2:p1", &[AgentStatus::Done], Some(100))
            .unwrap();
        assert_eq!(status, None);
    }

    #[test]
    fn pane_run_sends_one_quoted_command_line() {
        let (herdr, fake) = herdr(vec![ok("")]);
        herdr
            .pane_run(
                &host(),
                "w3:p1",
                &words(&[
                    "claude",
                    "--settings",
                    "/tmp/a b/s.json",
                    "Read it's brief.",
                ]),
            )
            .unwrap();
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "pane",
                "run",
                "w3:p1",
                r"claude --settings '/tmp/a b/s.json' 'Read it'\''s brief.'",
            ])]
        );
    }

    #[test]
    fn pane_run_rejects_empty_argv() {
        let (herdr, fake) = herdr(Vec::new());
        assert!(herdr.pane_run(&host(), "w3:p1", &[]).is_err());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn pane_wait_output_reports_a_match() {
        let (herdr, fake) = herdr(vec![ok(PANE_WAIT_OUTPUT_MATCHED)]);
        assert!(
            herdr
                .pane_wait_output(&host(), "w1:p1", "trust this folder", 20_000)
                .unwrap()
        );
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "pane",
                "wait-output",
                "--regex",
                "trust this folder",
                "--timeout",
                "20000",
                "w1:p1",
            ])]
        );
    }

    #[test]
    fn pane_wait_output_timeout_returns_false() {
        let (herdr, _) = herdr(vec![failed(PANE_WAIT_OUTPUT_TIMEOUT)]);
        assert!(
            !herdr
                .pane_wait_output(&host(), "w1:p1", "trust", 500)
                .unwrap()
        );
    }

    #[test]
    fn pane_wait_output_other_failure_is_an_error() {
        let (herdr, _) = herdr(vec![failed(
            r#"{"error":{"code":"pane_not_found","message":"pane w9:p1 not found"},"id":"cli:pane:wait-output"}"#,
        )]);
        let err = herdr
            .pane_wait_output(&host(), "w9:p1", "trust", 500)
            .unwrap_err()
            .to_string();
        assert!(err.contains("pane_not_found"), "{err}");
    }

    #[test]
    fn pane_send_keys_passes_each_key() {
        let (herdr, fake) = herdr(vec![ok("")]);
        herdr
            .pane_send_keys(&host(), "w1:p1", &[KEY_DOWN, KEY_ENTER])
            .unwrap();
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "pane",
                "send-keys",
                "w1:p1",
                "down",
                "enter",
            ])]
        );
    }

    #[test]
    fn wait_without_timeout_omits_the_flag() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        herdr
            .wait(&host(), "w2:p1", &[AgentStatus::Done], None)
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
        let err = herdr.ensure_server(&host()).unwrap_err().to_string();
        assert!(err.contains("protocol 22"), "{err}");
        assert!(err.contains("21"), "{err}");
        assert_eq!(fake.calls().len(), 1, "no server start attempted");
    }

    #[test]
    fn ensure_server_accepts_running_server() {
        let (herdr, fake) = herdr(vec![ok(&status_running(22))]);
        herdr.ensure_server(&host()).unwrap();
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
        herdr.ensure_server(&host()).unwrap();
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
    fn workspaces_lists_ids_and_labels() {
        let (herdr, fake) = herdr(vec![ok(include_str!(
            "../tests/fixtures/herdr/workspace_list.json"
        ))]);
        assert_eq!(
            herdr.workspaces(&host()).unwrap(),
            [
                Workspace {
                    workspace_id: "w1".to_owned(),
                    label: "login-form".to_owned()
                },
                Workspace {
                    workspace_id: "w2".to_owned(),
                    label: "cache-fix".to_owned()
                },
            ]
        );
        assert_eq!(
            argvs(&fake),
            [words(&["herdr", "--session", "flow", "workspace", "list"])]
        );
    }

    #[test]
    fn close_workspace_reports_a_workspace_already_gone() {
        let closed = r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#;
        let missing = r#"{"error":{"code":"workspace_not_found","message":"workspace w9 not found"},"id":"cli:workspace:close"}"#;
        let (herdr, _) = herdr(vec![ok(closed), failed(missing), failed("boom")]);
        assert!(herdr.close_workspace(&host(), "w2").unwrap());
        assert!(!herdr.close_workspace(&host(), "w9").unwrap());
        assert!(herdr.close_workspace(&host(), "w9").is_err());
    }

    #[test]
    fn create_workspace_returns_workspace_and_root_pane() {
        let (herdr, fake) = herdr(vec![ok(WORKSPACE_CREATE)]);
        let ids = herdr
            .create_workspace(
                &host(),
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
                &host(),
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
        let status = herdr.prompt(&host(), "w2:p1", "keep going", &[]).unwrap();
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
    fn prompt_passes_until_states() {
        let (herdr, fake) = herdr(vec![ok(AGENT_GET)]);
        herdr
            .prompt(
                &host(),
                "w2:p1",
                "keep going",
                &[AgentStatus::Working, AgentStatus::Blocked],
            )
            .unwrap();
        assert_eq!(
            argvs(&fake)[0][7..],
            words(&["--wait", "--until", "working", "--until", "blocked"])
        );
    }

    #[test]
    fn read_returns_plain_text() {
        let (herdr, fake) = herdr(vec![ok("line one\nline two\n")]);
        assert_eq!(
            herdr.read(&host(), "w2:p1", 40).unwrap(),
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
    fn pane_read_reads_the_terminal_without_an_agent() {
        let (herdr, fake) = herdr(vec![ok("$ claude --resume s9\nNo conversation found\n")]);
        assert_eq!(
            herdr.pane_read(&host(), "w2:p1", 40).unwrap(),
            "$ claude --resume s9\nNo conversation found\n"
        );
        assert_eq!(
            argvs(&fake),
            [words(&[
                "herdr",
                "--session",
                "flow",
                "pane",
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
            ok(""),
            ok(PANE_WAIT_OUTPUT_MATCHED),
            ok(""),
        ]);
        let target = host();
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
        herdr.prompt(&target, "w3:p1", "hello", &[]).unwrap();
        herdr.read(&target, "w3:p1", 10).unwrap();
        herdr.close_workspace(&target, "w3").unwrap();
        herdr
            .pane_run(&target, "w3:p1", &words(&["claude"]))
            .unwrap();
        herdr
            .pane_wait_output(&target, "w3:p1", "trust", 100)
            .unwrap();
        herdr
            .pane_send_keys(&target, "w3:p1", &[KEY_ENTER])
            .unwrap();

        let mut calls = argvs(&fake);
        assert_eq!(calls.len(), 13, "{calls:?}");
        calls.push(herdr.attach_argv("w3:p1"));
        for argv in calls {
            assert!(
                argv.windows(2).any(|pair| pair == ["--session", "flow"]),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn unreachable_host_is_reported_as_such() {
        let (herdr, _) = herdr(vec![Scripted {
            status: 255,
            stderr: "ssh: connect to host host-a.example port 22: Connection refused\n".to_owned(),
            ..Scripted::default()
        }]);
        let err = herdr.agents(&host()).unwrap_err();
        assert!(is_unreachable(&err), "{err:#}");
        let err = err.to_string();
        assert!(err.starts_with("host unreachable:"), "{err}");
        assert!(
            err.contains("host-a") && err.contains("Connection refused"),
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
            .wait(&host(), "w9:p1", &[AgentStatus::Done], None)
            .unwrap_err()
            .to_string();
        assert!(!err.starts_with("host unreachable:"), "{err}");
        assert!(err.contains("agent wait w9:p1"), "{err}");
        assert!(err.contains("agent_not_found"), "{err}");
    }

    #[test]
    fn failed_command_without_output_says_so() {
        let (herdr, _) = herdr(vec![Scripted {
            status: 1,
            ..Scripted::default()
        }]);
        let err = herdr
            .wait(&host(), "w9:p1", &[AgentStatus::Done], None)
            .unwrap_err()
            .to_string();
        assert!(err.ends_with("exited 1 (no output)"), "{err}");
    }

    #[test]
    fn unparsable_output_reports_command_and_stderr() {
        let (herdr, _) = herdr(vec![Scripted {
            stdout: "not json".to_owned(),
            stderr: "warning: config reload pending".to_owned(),
            ..Scripted::default()
        }]);
        let err = format!("{:#}", herdr.agents(&host()).unwrap_err());
        assert!(err.contains("agent list"), "{err}");
        assert!(err.contains("config reload pending"), "{err}");
    }
}
