//! `flow hook`: turns one Claude Code hook event into Session facet writes.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;
use serde::Deserialize;

use crate::session::{Activity, Context, Origin, Session, State, Status};
use crate::store::{self, Store};
use crate::transcript;

/// The fields flow reads from Claude Code's hook JSON; the rest are ignored.
#[derive(Deserialize)]
struct HookInput {
    session_id: String,
    hook_event_name: String,
    cwd: Option<String>,
    transcript_path: Option<String>,
    notification_type: Option<String>,
    /// `UserPromptSubmit`'s prompt.
    prompt: Option<String>,
    /// `Stop` and `StopFailure`'s final assistant text.
    last_assistant_message: Option<String>,
}

/// What the hook takes from its environment. Empty variables count as unset.
#[derive(Debug, Clone, Default)]
pub struct HookEnv {
    /// `ONA_ID`: the CDE's id.
    pub ona_id: Option<String>,
    /// `HERDR_PANE_ID`.
    pub pane: Option<String>,
    /// `HERDR_SOCKET_PATH`.
    pub herdr_socket: Option<String>,
}

impl HookEnv {
    pub fn from_env() -> Self {
        let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            ona_id: var("ONA_ID"),
            pane: var("HERDR_PANE_ID"),
            herdr_socket: var("HERDR_SOCKET_PATH"),
        }
    }
}

/// Writes the facets one hook event implies, each stamped `at` (when the hook process
/// started). Events flow does not use are an error, so they show up in the log.
pub fn handle_hook(store: &Store, stdin: &str, env: &HookEnv, at: Timestamp) -> Result<()> {
    let input: HookInput = serde_json::from_str(stdin).context("hook input is not hook JSON")?;
    write_facets(store, &input, env, at)
        .with_context(|| format!("{} for session {}", input.hook_event_name, input.session_id))
}

fn write_facets(store: &Store, input: &HookInput, env: &HookEnv, at: Timestamp) -> Result<()> {
    let id = input.session_id.as_str();
    let status = |state| Session::put(store, id, &Status { state }, at);
    match input.hook_event_name.as_str() {
        "SessionStart" => {
            // Status first: it needs no lookup, so it lands even if the gitpod call below
            // outlives the hook's timeout.
            status(State::Idle)?;
            let cde = env.ona_id.as_deref().and_then(cde_name);
            let branch = git_branch(input.cwd.as_deref());
            let origin = Origin {
                cde,
                cde_id: env.ona_id.clone(),
                pane: env.pane.clone(),
                herdr_socket: env.herdr_socket.clone(),
                cwd: input.cwd.clone(),
                transcript_path: input.transcript_path.clone(),
            };
            Session::put(store, id, &origin, at)?;
            let title = transcript(input).title;
            Session::put(store, id, &Context { title, branch }, at)?;
        }
        "UserPromptSubmit" => {
            status(State::Running)?;
            Session::put(store, id, &Activity::new(input.prompt.clone(), None), at)?;
        }
        "PostToolUse" => {
            status(State::Running)?;
        }
        "PermissionRequest" => {
            status(State::Waiting)?;
        }
        "Notification" => {
            // The spec's `elicitation_*`: the dialogs ask the user for input, while
            // `elicitation_complete` and `elicitation_response` end that wait, as `PostToolUse`
            // does after an approval. Other notifications (`permission_prompt` repeats
            // `PermissionRequest`, `idle_prompt` follows `Stop`) change nothing.
            match input.notification_type.as_deref() {
                Some("elicitation_dialog" | "elicitation_url_dialog") => {
                    status(State::Waiting)?;
                }
                Some("elicitation_complete" | "elicitation_response") => {
                    status(State::Running)?;
                }
                _ => {}
            }
        }
        "Stop" | "StopFailure" => {
            status(State::Idle)?;
            let branch = git_branch(input.cwd.as_deref());
            let facts = transcript(input);
            Session::put(
                store,
                id,
                &Context {
                    title: facts.title,
                    branch,
                },
                at,
            )?;
            let reply = input.last_assistant_message.clone();
            Session::put(store, id, &Activity::new(facts.last_prompt, reply), at)?;
        }
        "SessionEnd" => {
            status(State::Done)?;
        }
        other => bail!("unknown hook event {other:?}"),
    }
    Ok(())
}

/// Where `flow hook` logs its errors: `hook.log` next to the Store's root directory.
pub fn log_path(store: &Store) -> PathBuf {
    store.root().with_file_name("hook.log")
}

/// Appends one line to the hook log, naming the CDE it came from. Failing to log is ignored:
/// the hook has nowhere else to report and must not disturb the Session.
pub fn log_error(store: &Store, at: Timestamp, message: &str) {
    let path = log_path(store);
    let cde = HookEnv::from_env().ona_id.unwrap_or_else(|| "-".into());
    let line = format!(
        "{} {cde} flow hook: {}\n",
        store::format_at(at),
        message.replace('\n', " ")
    );
    let _ = append(&path, &line);
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// What the Session's transcript says; empty when the hook names none or it can't be read.
fn transcript(input: &HookInput) -> transcript::Facts {
    input
        .transcript_path
        .as_deref()
        .map(|path| transcript::read(Path::new(path)))
        .unwrap_or_default()
}

/// The CDE's name for `ona_id`, from `gitpod environment get`; `None` on any failure.
fn cde_name(ona_id: &str) -> Option<String> {
    let args = [
        "environment",
        "get",
        ona_id,
        "--field",
        "name",
        "--timeout",
        "3s",
    ];
    output("gitpod", &args, None)
}

/// The branch checked out in `cwd`; `None` outside a git repo or on a detached HEAD.
fn git_branch(cwd: Option<&str>) -> Option<String> {
    output("git", &["branch", "--show-current"], Some(Path::new(cwd?)))
}

/// A command's trimmed stdout if it exits 0 and prints something.
fn output(program: &str, args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}
