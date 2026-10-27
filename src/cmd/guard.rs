//! `flow guard`: the supervisor's blocking Stop hook, the turn-end backstop. Exits 2 with a
//! reason on stdout and stderr, which Claude Code delivers to the supervisor, while wakes are
//! unhandled, or while tasks are open and no `flow watch` holds the lock. Exits 0 otherwise,
//! and at once in a session that is not the supervisor.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;

use crate::paths::StateDir;
use crate::state::Store;
use crate::supervisor;
use crate::watch::{lock, queue};

/// Overrides how long to wait for a watcher to take the lock, in milliseconds.
const GRACE_VAR: &str = "FLOW_GUARD_GRACE_MS";
/// The async watch hook starts alongside this one, so its lock may not exist yet.
const DEFAULT_GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(250);
const BLOCK_EXIT: u8 = 2;

/// The field of the Stop hook's stdin the guard reads; the rest are ignored.
#[derive(Debug, Default, Deserialize)]
struct StopInput {
    #[serde(default)]
    stop_hook_active: bool,
}

pub fn run(state: &StateDir) -> anyhow::Result<ExitCode> {
    if !supervisor::is_supervisor_session() {
        return Ok(ExitCode::SUCCESS);
    }
    let input = read_input();
    let wakes = queue::pending(state)?.len();
    if wakes > 0 {
        return block(&format!(
            "flow: {wakes} unhandled wakes; run flow wake drain"
        ));
    }
    let open = Store::open(state)
        .list_tasks()?
        .iter()
        .filter(|(_, status)| status.state.is_open())
        .count();
    if open == 0 || input.stop_hook_active {
        return Ok(ExitCode::SUCCESS);
    }
    if watcher_appears(state, grace()?)? {
        return Ok(ExitCode::SUCCESS);
    }
    block(&format!(
        "flow: work under way ({open} tasks) and no watcher is live; run `flow watch` in the background before ending the turn"
    ))
}

/// The hook input on stdin. A terminal, an unreadable stream or input that does not parse
/// counts as no input, so the guard also runs by hand.
fn read_input() -> StopInput {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return StopInput::default();
    }
    let mut bytes = Vec::new();
    if stdin.read_to_end(&mut bytes).is_err() {
        return StopInput::default();
    }
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Whether the watch lock is held live now or becomes so within `grace`.
fn watcher_appears(state: &StateDir, grace: Duration) -> anyhow::Result<bool> {
    let deadline = Instant::now() + grace;
    loop {
        if lock::held_live(state)? {
            return Ok(true);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        thread::sleep(left.min(POLL));
    }
}

fn grace() -> anyhow::Result<Duration> {
    match std::env::var(GRACE_VAR) {
        Ok(ms) => {
            let ms = ms
                .parse()
                .with_context(|| format!("parsing ${GRACE_VAR} {ms:?} as milliseconds"))?;
            Ok(Duration::from_millis(ms))
        }
        Err(_) => Ok(DEFAULT_GRACE),
    }
}

fn block(reason: &str) -> anyhow::Result<ExitCode> {
    super::print(&format!("{reason}\n"))?;
    eprintln!("{reason}");
    Ok(ExitCode::from(BLOCK_EXIT))
}
