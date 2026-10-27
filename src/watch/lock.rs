//! The single-flight lock for `flow watch`: the directory `state/supervisor/watch.lock`, made
//! with mkdir, holding `owner.json`. The holder touches the directory every poll cycle, so a
//! lock whose holder vanished on another host goes stale by age.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::cde;
use crate::paths::StateDir;
use crate::state::fs::write_atomic;

const LOCK_DIR: &str = "supervisor/watch.lock";
const OWNER_FILE: &str = "owner.json";
/// A lock untouched this long is stale whoever holds it.
const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Who holds the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    pub host: String,
    pub started: Timestamp,
}

/// The held lock; dropping it removes the lock directory.
#[derive(Debug)]
pub struct Guard {
    dir: PathBuf,
}

/// Takes the lock for process `pid` on this host, or returns `None` when a live holder has it.
/// A stale lock, whose holder on this host is dead or which nobody touched for
/// `STALE_AFTER`, is removed and taken.
pub fn acquire(state: &StateDir, pid: u32) -> anyhow::Result<Option<Guard>> {
    let dir = state.root().join(LOCK_DIR);
    let parent = dir.parent().expect("the lock has a parent directory");
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let host = this_host();
    for _ in 0..2 {
        match fs::create_dir(&dir) {
            Ok(()) => {
                let guard = Guard { dir };
                let owner = Owner {
                    pid,
                    host,
                    started: Timestamp::now(),
                };
                let path = guard.dir.join(OWNER_FILE);
                write_atomic(&path, &serde_json::to_vec(&owner)?)
                    .with_context(|| format!("writing {}", path.display()))?;
                return Ok(Some(guard));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err).with_context(|| format!("creating {}", dir.display())),
        }
        if !is_stale(&dir, &host)? {
            return Ok(None);
        }
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {}", dir.display())),
        }
    }
    Ok(None)
}

/// Whether a live holder has the lock: it exists and is not stale. Reads only; never takes or
/// removes the lock.
pub fn held_live(state: &StateDir) -> anyhow::Result<bool> {
    let dir = state.root().join(LOCK_DIR);
    Ok(!is_stale(&dir, &this_host())?)
}

impl Guard {
    /// Marks the lock as still held, so it does not go stale by age.
    pub fn touch(&self) -> anyhow::Result<()> {
        File::open(&self.dir)
            .and_then(|dir| dir.set_modified(SystemTime::now()))
            .with_context(|| format!("touching {}", self.dir.display()))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The name `owner.json` records for this host: its Ona environment id, else its hostname.
fn this_host() -> String {
    cde::local_env_id().unwrap_or_else(|| {
        fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|name| name.trim().to_owned())
            .unwrap_or_default()
    })
}

/// Whether the lock at `dir` was left behind: untouched for `STALE_AFTER`, or held by a process
/// on `host` that no longer runs. A lock without a readable owner yet is being created and is
/// judged by age alone.
fn is_stale(dir: &Path, host: &str) -> anyhow::Result<bool> {
    let modified = match fs::metadata(dir) {
        Ok(meta) => meta.modified()?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    if age > STALE_AFTER {
        return Ok(true);
    }
    let owner: Option<Owner> = fs::read(dir.join(OWNER_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    Ok(owner.is_some_and(|owner| owner.host == host && !is_running(owner.pid)))
}

fn is_running(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("state")).unwrap();
        let state = StateDir::resolve(dir.path()).unwrap();
        (dir, state)
    }

    fn dead_pid() -> u32 {
        let mut child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn second_acquire_is_refused_while_held() {
        let (_dir, state) = state();
        let guard = acquire(&state, std::process::id()).unwrap();
        assert!(guard.is_some());
        assert!(acquire(&state, std::process::id()).unwrap().is_none());
        drop(guard);
        assert!(!state.root().join(LOCK_DIR).exists());
        assert!(acquire(&state, std::process::id()).unwrap().is_some());
    }

    #[test]
    fn lock_of_a_dead_pid_on_this_host_is_taken() {
        let (_dir, state) = state();
        std::mem::forget(acquire(&state, dead_pid()).unwrap().unwrap());
        assert!(acquire(&state, std::process::id()).unwrap().is_some());
    }

    #[test]
    fn untouched_lock_is_taken_whoever_holds_it() {
        let (_dir, state) = state();
        let dir = state.root().join(LOCK_DIR);
        fs::create_dir_all(&dir).unwrap();
        let owner = Owner {
            pid: 1,
            host: "another-host".to_owned(),
            started: Timestamp::now(),
        };
        fs::write(dir.join(OWNER_FILE), serde_json::to_vec(&owner).unwrap()).unwrap();
        assert!(acquire(&state, std::process::id()).unwrap().is_none());
        let old = SystemTime::now() - STALE_AFTER - Duration::from_secs(1);
        File::open(&dir).unwrap().set_modified(old).unwrap();
        assert!(acquire(&state, std::process::id()).unwrap().is_some());
    }

    #[test]
    fn held_live_is_true_while_a_live_pid_holds_it() {
        let (_dir, state) = state();
        let guard = acquire(&state, std::process::id()).unwrap().unwrap();
        assert!(held_live(&state).unwrap());
        assert!(state.root().join(LOCK_DIR).join(OWNER_FILE).exists());
        drop(guard);
        assert!(!held_live(&state).unwrap());
    }

    #[test]
    fn held_live_is_false_without_a_lock() {
        let (_dir, state) = state();
        assert!(!held_live(&state).unwrap());
        assert!(!state.root().join(LOCK_DIR).exists());
    }

    #[test]
    fn held_live_is_false_for_a_dead_pid_on_this_host() {
        let (_dir, state) = state();
        std::mem::forget(acquire(&state, dead_pid()).unwrap().unwrap());
        assert!(!held_live(&state).unwrap());
        assert!(state.root().join(LOCK_DIR).exists());
    }

    #[test]
    fn touch_keeps_the_lock_fresh() {
        let (_dir, state) = state();
        let guard = acquire(&state, std::process::id()).unwrap().unwrap();
        let dir = state.root().join(LOCK_DIR);
        let old = SystemTime::now() - STALE_AFTER - Duration::from_secs(1);
        File::open(&dir).unwrap().set_modified(old).unwrap();
        guard.touch().unwrap();
        assert!(!is_stale(&dir, "another-host").unwrap());
    }
}
