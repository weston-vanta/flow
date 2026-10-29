//! `flow watch [--heartbeat SECS] [--once]`: the supervisor's Stop hook. Blocks until something
//! is actionable, then prints the reason on stdout and stderr and exits 2, which Claude Code
//! delivers to the supervisor as hook feedback. Exits 0 silently when another watch holds the
//! lock, under `--once` when nothing woke, when there is nothing to watch, and when only hand
//! sessions were watched for a heartbeat interval. Each run first lists every live host's agents
//! once, retries included, to find the hand sessions to watch. Once the lock is
//! held, an error wakes the supervisor as `watch-failed: <error>` rather than failing the hook,
//! which would leave no watcher and nobody told. Each distinct error wakes once: a watch that
//! hits the error the newest wake already reported keeps the lock and retries in the
//! background with backoff, waking again only if the error changes or is still there a
//! heartbeat after its wake; when a retry succeeds the watch goes on as usual. `--once` reports
//! any error at once and does not retry. Outside the supervisor session the binary exits 0
//! before this runs.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::watch::classify::DEFAULT_QUIET_SECS;
use crate::watch::{self, Retry, Settings, lock};

/// Overrides the poll interval, in milliseconds.
const POLL_VAR: &str = "FLOW_WATCH_POLL_MS";
const DEFAULT_POLL: Duration = Duration::from_secs(2);
/// Overrides the delay before a failed watch first retries, in milliseconds.
const RETRY_VAR: &str = "FLOW_WATCH_RETRY_MS";
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const DEFAULT_HEARTBEAT_SECS: u64 = 3600;
const WAKE_EXIT: u8 = 2;

pub fn run(state: &StateDir, heartbeat: Option<u64>, once: bool) -> anyhow::Result<ExitCode> {
    let Some(lock) = lock::acquire(state, std::process::id())? else {
        return Ok(ExitCode::SUCCESS);
    };
    let heartbeat = Duration::from_secs(heartbeat.unwrap_or(DEFAULT_HEARTBEAT_SECS));
    let watched = if once {
        watch_holding(state, heartbeat, true, true, &lock)
            .unwrap_or_else(|err| Some(watch::failure_wake(state, &err)))
    } else {
        match env_millis(RETRY_VAR, DEFAULT_RETRY) {
            Ok(first) => {
                let retry = Retry { first, heartbeat };
                let mut first_attempt = true;
                watch::run_retrying(state, &lock, &retry, || {
                    let list_agents = std::mem::take(&mut first_attempt);
                    watch_holding(state, heartbeat, false, list_agents, &lock)
                })
            }
            Err(err) => Some(watch::failure_wake(state, &err)),
        }
    };
    let Some(reason) = watched else {
        return Ok(ExitCode::SUCCESS);
    };
    super::print(&format!("{reason}\n"))?;
    eprintln!("{reason}");
    Ok(ExitCode::from(WAKE_EXIT))
}

fn watch_holding(
    state: &StateDir,
    heartbeat: Duration,
    once: bool,
    list_agents: bool,
    lock: &lock::Guard,
) -> anyhow::Result<Option<String>> {
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let settings = Settings {
        poll: env_millis(POLL_VAR, DEFAULT_POLL)?,
        heartbeat,
        once,
        quiet_secs: DEFAULT_QUIET_SECS,
        list_agents,
    };
    watch::run(state, runner, &settings, lock)
}

/// The duration in milliseconds that `$var` sets, or `default` when it is unset.
fn env_millis(var: &str, default: Duration) -> anyhow::Result<Duration> {
    match std::env::var(var) {
        Ok(ms) => {
            let ms = ms
                .parse()
                .with_context(|| format!("parsing ${var} {ms:?} as milliseconds"))?;
            Ok(Duration::from_millis(ms))
        }
        Err(_) => Ok(default),
    }
}
