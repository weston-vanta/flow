//! `flow wake drain` and `flow wake ack`: the wakes `flow watch` queued, and the supervisor's
//! record of which it handled.

use jiff::Timestamp;

use crate::paths::StateDir;
use crate::render;
use crate::watch::queue;

/// Prints each unacknowledged wake as `seq  age  reason`, then the ack command that clears
/// them. Prints nothing when none are pending.
pub fn run_drain(state: &StateDir) -> anyhow::Result<()> {
    let wakes = queue::pending(state)?;
    let Some(last) = wakes.last() else {
        return Ok(());
    };
    let now = Timestamp::now();
    let rows: Vec<Vec<String>> = wakes
        .iter()
        .map(|wake| {
            vec![
                wake.seq.to_string(),
                render::relative(wake.ts, now),
                wake.reason.clone(),
            ]
        })
        .collect();
    let mut out = render::table(&rows);
    out.push_str(&format!(
        "WAKE_ACK_REQUIRED: flow wake ack --through {}\n",
        last.seq
    ));
    super::print(&out)
}

pub fn run_ack(state: &StateDir, through: u64) -> anyhow::Result<()> {
    queue::ack_through(state, through)
}
