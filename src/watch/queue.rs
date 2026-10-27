//! The wake queue, `state/supervisor/wake-queue.jsonl`: every wake `flow watch` delivered, with
//! a sequence number, so a wake survives a supervisor that stopped before handling it.
//! `state/supervisor/wake-ack` holds the highest sequence number the supervisor handled.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::classify::Observation;
use crate::paths::StateDir;
use crate::state::fs::{append_line_holding_lock, with_lock, write_atomic};

const QUEUE_FILE: &str = "supervisor/wake-queue.jsonl";
const ACK_FILE: &str = "supervisor/wake-ack";

/// One line of the wake queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    pub seq: u64,
    pub ts: Timestamp,
    pub reason: String,
    pub obs: Observation,
}

/// Appends a wake for `reason` and returns its sequence number, one more than the last one
/// queued (the first is 1).
pub fn push(state: &StateDir, reason: &str, obs: &Observation) -> anyhow::Result<u64> {
    let path = queue_path(state);
    let dir = path
        .parent()
        .expect("the queue file has a parent directory");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    with_lock(&path, || {
        let seq = read_queue(&path)
            .map_err(io::Error::other)?
            .last()
            .map_or(1, |wake| wake.seq + 1);
        let wake = Wake {
            seq,
            ts: Timestamp::now(),
            reason: reason.to_owned(),
            obs: obs.clone(),
        };
        let mut line = serde_json::to_vec(&wake)?;
        line.push(b'\n');
        append_line_holding_lock(&path, &line)?;
        Ok(seq)
    })
    .with_context(|| format!("appending to {}", path.display()))
}

/// The queued wakes after the acknowledged one, oldest first.
pub fn pending(state: &StateDir) -> anyhow::Result<Vec<Wake>> {
    let acked = acked(state)?;
    let mut wakes = read_queue(&queue_path(state))?;
    wakes.retain(|wake| wake.seq > acked);
    Ok(wakes)
}

/// Records every wake up to and including `seq` as handled.
pub fn ack_through(state: &StateDir, seq: u64) -> anyhow::Result<()> {
    let path = ack_path(state);
    let dir = path.parent().expect("the ack file has a parent directory");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    write_atomic(&path, format!("{seq}\n").as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// The highest acknowledged sequence number; 0 before any ack.
fn acked(state: &StateDir) -> anyhow::Result<u64> {
    let path = ack_path(state);
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .trim()
            .parse()
            .with_context(|| format!("parsing {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

/// Every wake in the queue. A last line that does not parse is a crashed writer's partial
/// append and is skipped.
fn read_queue(path: &Path) -> anyhow::Result<Vec<Wake>> {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut wakes = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        match serde_json::from_str(line) {
            Ok(wake) => wakes.push(wake),
            Err(_) if i + 1 == lines.len() => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("parsing {} line {}", path.display(), i + 1));
            }
        }
    }
    Ok(wakes)
}

fn queue_path(state: &StateDir) -> PathBuf {
    state.root().join(QUEUE_FILE)
}

fn ack_path(state: &StateDir) -> PathBuf {
    state.root().join(ACK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("state")).unwrap();
        let state = StateDir::resolve(dir.path()).unwrap();
        (dir, state)
    }

    fn reasons(wakes: &[Wake]) -> Vec<(u64, &str)> {
        wakes
            .iter()
            .map(|wake| (wake.seq, wake.reason.as_str()))
            .collect()
    }

    #[test]
    fn push_increments_seq() {
        let (_dir, state) = state();
        assert_eq!(
            push(&state, "heartbeat", &Observation::Heartbeat).unwrap(),
            1
        );
        assert_eq!(
            push(&state, "heartbeat", &Observation::Heartbeat).unwrap(),
            2
        );
        let pending = pending(&state).unwrap();
        assert_eq!(reasons(&pending), [(1, "heartbeat"), (2, "heartbeat")]);
        assert_eq!(pending[0].obs, Observation::Heartbeat);
    }

    #[test]
    fn pending_excludes_acked() {
        let (_dir, state) = state();
        let obs = Observation::CdeUnreachable {
            cde: "cde-b".to_owned(),
        };
        for reason in ["first", "second", "third"] {
            push(&state, reason, &obs).unwrap();
        }
        ack_through(&state, 2).unwrap();
        assert_eq!(reasons(&pending(&state).unwrap()), [(3, "third")]);
        ack_through(&state, 3).unwrap();
        assert!(pending(&state).unwrap().is_empty());
        assert_eq!(push(&state, "fourth", &obs).unwrap(), 4);
    }

    #[test]
    fn pending_is_empty_without_a_queue() {
        let (_dir, state) = state();
        assert!(pending(&state).unwrap().is_empty());
    }
}
