//! `flow hook`: turns one Claude Code hook event into Session facet writes.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use jiff::Timestamp;
use serde::Deserialize;

use crate::session::{self, Activity, Context, Origin, Session, SessionFacet, State, Status};
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

/// Writes the facets one hook event implies, each stamped `at` (when the hook process
/// started). Events flow does not use are an error, so they show up in the log.
pub fn handle_hook(store: &Store, stdin: &str, at: Timestamp) -> Result<()> {
    let input: HookInput = serde_json::from_str(stdin)?;
    let id = input.session_id.as_str();
    // Only a change of state is written, so `status.at` is when the Session entered its state.
    let status = |state| -> Result<()> {
        let status = serde_json::to_value(Status { state })?;
        let stored = store.get(session::KIND, id).ok().flatten();
        let stored = stored.as_ref().and_then(|e| e.facets.get(Status::NAME));
        if stored.map(|f| &f.value) != Some(&status) {
            Session::put(store, id, &Status { state }, at)?;
        }
        Ok(())
    };
    let transcript = || {
        input
            .transcript_path
            .as_deref()
            .map(|path| transcript::read(Path::new(path)))
            .unwrap_or_default()
    };
    let branch = || git_branch(input.cwd.as_deref());
    match input.hook_event_name.as_str() {
        "SessionStart" => {
            // Status first: it is cheap, so it lands even if the gitpod call below
            // outlives the hook's timeout.
            status(State::Idle)?;
            let env = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
            let cde_id = env("ONA_ID");
            let origin = Origin {
                cde: cde_id.as_deref().and_then(cde_name),
                cde_id,
                pane: env("HERDR_PANE_ID"),
                herdr_socket: env("HERDR_SOCKET_PATH"),
                cwd: input.cwd.clone(),
                transcript_path: input.transcript_path.clone(),
            };
            Session::put(store, id, &origin, at)?;
            let context = Context {
                title: transcript().title,
                branch: branch(),
            };
            Session::put(store, id, &context, at)?;
        }
        "UserPromptSubmit" => {
            status(State::Running)?;
            Session::put(store, id, &Activity::new(input.prompt.clone(), None), at)?;
        }
        "PostToolUse" => status(State::Running)?,
        "PermissionRequest" => status(State::Waiting)?,
        // The dialogs ask the user for input; `elicitation_complete` and
        // `elicitation_response` end that wait, as `PostToolUse` does after an approval. Other
        // notifications (`permission_prompt` repeats `PermissionRequest`, `idle_prompt`
        // follows `Stop`) change nothing.
        "Notification" => match input.notification_type.as_deref() {
            Some("elicitation_dialog" | "elicitation_url_dialog") => status(State::Waiting)?,
            Some("elicitation_complete" | "elicitation_response") => status(State::Running)?,
            _ => {}
        },
        "Stop" | "StopFailure" => {
            status(State::Idle)?;
            let facts = transcript();
            let context = Context {
                title: facts.title,
                branch: branch(),
            };
            Session::put(store, id, &context, at)?;
            let reply = input.last_assistant_message.clone();
            Session::put(store, id, &Activity::new(facts.last_prompt, reply), at)?;
        }
        "SessionEnd" => status(State::Done)?,
        other => bail!("unknown hook event {other:?}"),
    }
    Ok(())
}

/// Appends one line to `hook.log`, next to the Store's root directory. Failing to log is
/// ignored: the hook has nowhere else to report and must not disturb the Session.
pub fn log_error(store: &Store, at: Timestamp, message: &str) {
    let line = format!("{} flow hook: {message}\n", store::format_at(at));
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store.root().with_file_name("hook.log"))
        .and_then(|mut file| file.write_all(line.as_bytes()));
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
    output(Command::new("gitpod").args(args))
}

/// The branch checked out in `cwd`; `None` outside a git repo or on a detached HEAD.
fn git_branch(cwd: Option<&str>) -> Option<String> {
    output(
        Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(cwd?),
    )
}

/// A command's trimmed stdout if it exits 0 and prints something.
fn output(cmd: &mut Command) -> Option<String> {
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}
