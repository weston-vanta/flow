//! `flow hook claude` and `flow hook supervisor`: the receivers Claude Code hooks call with
//! their event on stdin, for workers and for the supervisor.

use std::io::Read;
use std::path::PathBuf;

use anyhow::Context;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::paths::StateDir;
use crate::state::Store;
use crate::state::events::Event;
use crate::state::task::ClaudeSession;
use crate::supervisor;

const EVENT_SOURCE: &str = "hook";

/// The fields of a Claude Code hook's stdin that flow keeps; the rest are ignored.
#[derive(Debug, Deserialize)]
struct HookInput {
    hook_event_name: String,
    session_id: String,
    transcript_path: PathBuf,
    cwd: PathBuf,
}

/// Appends `hook.<event lowercased>` to the task's events and, on `SessionStart`, records the
/// session in `status.json`. Prints nothing, so the hook passes no output back to the worker.
pub fn run_claude(state: &StateDir, id: &str, input: impl Read) -> anyhow::Result<()> {
    let hook: HookInput = read_input(input)?;
    let store = Store::open(state);
    let ev = Event {
        ts: Timestamp::now(),
        source: EVENT_SOURCE.to_owned(),
        kind: format!("hook.{}", hook.hook_event_name.to_lowercase()),
        data: json!({
            "session_id": hook.session_id,
            "transcript_path": hook.transcript_path,
            "cwd": hook.cwd,
        }),
    };
    store.append_event(id, &ev)?;
    if hook.hook_event_name == "SessionStart" {
        store.set_status(id, |status| {
            status.claude_session = Some(ClaudeSession {
                session_id: hook.session_id,
                transcript_path: hook.transcript_path,
            });
        })?;
    }
    Ok(())
}

/// The fields of the supervisor's hook input that flow reads.
#[derive(Debug, Deserialize)]
struct SupervisorHookInput {
    hook_event_name: String,
    session_id: String,
}

/// On the supervisor's `SessionStart`, records its session id in the supervisor lock so the
/// next `flow up` resumes it. Other events, sessions that are not the supervisor (`is_supervisor`
/// false: another session started in the repo), and a missing lock change nothing. Prints
/// nothing, since `SessionStart` output would become session context.
pub fn run_supervisor(
    state: &StateDir,
    is_supervisor: bool,
    input: impl Read,
) -> anyhow::Result<()> {
    let hook: SupervisorHookInput = read_input(input)?;
    if !is_supervisor || hook.hook_event_name != "SessionStart" {
        return Ok(());
    }
    supervisor::record_session(state, &hook.session_id)?;
    Ok(())
}

fn read_input<T: serde::de::DeserializeOwned>(mut input: impl Read) -> anyhow::Result<T> {
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .context("reading hook input from stdin")?;
    serde_json::from_slice(&bytes).context("parsing hook input")
}
