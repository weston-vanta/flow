//! `flow watch [--heartbeat SECS] [--once]`: the supervisor's Stop hook. Blocks until something
//! is actionable, then prints the reason on stdout and stderr and exits 2, which Claude Code
//! delivers to the supervisor as hook feedback. Exits 0 silently when another watch holds the
//! lock, and under `--once` when nothing woke. Outside the supervisor session the binary exits 0
//! before this runs.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::watch::classify::DEFAULT_QUIET_SECS;
use crate::watch::{self, Settings, lock};

/// Overrides the poll interval, in milliseconds.
const POLL_VAR: &str = "FLOW_WATCH_POLL_MS";
const DEFAULT_POLL: Duration = Duration::from_secs(2);
const DEFAULT_HEARTBEAT_SECS: u64 = 3600;
const WAKE_EXIT: u8 = 2;

pub fn run(state: &StateDir, heartbeat: Option<u64>, once: bool) -> anyhow::Result<ExitCode> {
    let Some(lock) = lock::acquire(state, std::process::id())? else {
        return Ok(ExitCode::SUCCESS);
    };
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let settings = Settings {
        poll: poll_interval()?,
        heartbeat: Duration::from_secs(heartbeat.unwrap_or(DEFAULT_HEARTBEAT_SECS)),
        once,
        quiet_secs: DEFAULT_QUIET_SECS,
    };
    let Some(reason) = watch::run(state, runner, &settings, &lock)? else {
        return Ok(ExitCode::SUCCESS);
    };
    super::print(&format!("{reason}\n"))?;
    eprintln!("{reason}");
    Ok(ExitCode::from(WAKE_EXIT))
}

fn poll_interval() -> anyhow::Result<Duration> {
    match std::env::var(POLL_VAR) {
        Ok(ms) => {
            let ms = ms
                .parse()
                .with_context(|| format!("parsing ${POLL_VAR} {ms:?} as milliseconds"))?;
            Ok(Duration::from_millis(ms))
        }
        Err(_) => Ok(DEFAULT_POLL),
    }
}
