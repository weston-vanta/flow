//! `flow guard`: the supervisor's blocking Stop hook, the turn-end backstop. Exits 2 with a
//! reason on stdout and stderr, which Claude Code delivers to the supervisor, while wakes are
//! unhandled, or while tasks are open and no `flow watch` holds the lock. Exits 0 otherwise.
//! Outside the supervisor session the binary exits 0 before this runs.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;

use crate::paths::StateDir;
use crate::state::Store;
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
    let input = read_input();
    let wakes = queue::pending(state)?.len();
    if wakes > 0 {
        return block_on_wakes(wakes);
    }
    let open = Store::open(state)
        .list_tasks()?
        .iter()
        .filter(|(_, status)| status.state.is_open())
        .count();
    if open == 0 || input.stop_hook_active {
        return Ok(ExitCode::SUCCESS);
    }
    match await_watcher(state, grace()?)? {
        Grace::Watcher => Ok(ExitCode::SUCCESS),
        Grace::Wakes(wakes) => block_on_wakes(wakes),
        Grace::Neither => block(&format!(
            "flow: work under way ({open} tasks) and no watcher is live; run `flow watch` in the background before ending the turn"
        )),
    }
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

/// What the guard saw within its grace.
enum Grace {
    /// The watch lock is held live.
    Watcher,
    /// This many wakes are queued: a watcher took the lock, queued them and exited.
    Wakes(usize),
    /// Neither, once the grace ran out.
    Neither,
}

/// Waits up to `grace` for the watch lock to be held live or for a wake to be queued.
fn await_watcher(state: &StateDir, grace: Duration) -> anyhow::Result<Grace> {
    let deadline = Instant::now() + grace;
    loop {
        if lock::held_live(state)? {
            return Ok(Grace::Watcher);
        }
        let wakes = queue::pending(state)?.len();
        if wakes > 0 {
            return Ok(Grace::Wakes(wakes));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Grace::Neither);
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

/// Blocks so the queued wakes carry the next turn.
fn block_on_wakes(wakes: usize) -> anyhow::Result<ExitCode> {
    block(&format!(
        "flow: {wakes} unhandled wakes; run flow wake drain"
    ))
}

fn block(reason: &str) -> anyhow::Result<ExitCode> {
    super::print(&format!("{reason}\n"))?;
    eprintln!("{reason}");
    Ok(ExitCode::from(BLOCK_EXIT))
}
