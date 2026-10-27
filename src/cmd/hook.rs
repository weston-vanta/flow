//! `flow hook claude`: the receiver Claude Code worker hooks call with their event on stdin.

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
pub fn run_claude(state: &StateDir, id: &str, mut input: impl Read) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .context("reading hook input from stdin")?;
    let hook: HookInput = serde_json::from_slice(&bytes).context("parsing hook input")?;
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
