//! `flow watch`: blocks on the fleet and returns the first reason the supervisor should wake.
//! Each poll cycle reads every task's `status.json` and `events.jsonl`, reads the herdr waits
//! it keeps running per worker and followed pane, checks tasks gone quiet, and classifies what
//! it saw. Absorbed observations go to `state/supervisor/watch.log`; the first wake is queued
//! and returned. Nothing here changes a task: the supervisor acts on what watch reports.
//!
//! `state/supervisor/watch-state.json` carries what one run hands the next: how far each
//! task's files were read, so a change made between runs still wakes the next one, and what
//! earlier wakes reported, so a condition that persists wakes once rather than on every turn
//! end.

pub mod classify;
pub mod lock;
pub mod queue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::adopt::{FollowList, Followed};
use crate::herdr::{self, AgentStatus, Herdr, WaitEnd};
use crate::paths::StateDir;
use crate::runner::Runner;
use crate::state::Store;
use crate::state::config::{Cde, Config};
use crate::state::events::{EVENTS_FILE, Event};
use crate::state::fs::{append_line, write_atomic};
use crate::state::task::{State, Status, TaskRecord};
use classify::{Observation, Reported, Thresholds, Verdict, classify};
use lock::Guard;

const SUPERVISOR_DIR: &str = "supervisor";
const WATCH_STATE_FILE: &str = "supervisor/watch-state.json";
const WATCH_LOG_FILE: &str = "supervisor/watch.log";
/// How often one quiet task's worktree is searched for writes.
const WORKTREE_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How long a herdr wait that could not run waits before it is started again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// `--once` runs this many cycles, one poll interval apart, so the herdr waits the first
/// starts can answer in the second.
const ONCE_CYCLES: u32 = 2;
/// The exit code ssh uses for its own failures.
const SSH_FAILURE_STATUS: i32 = 255;
/// The worker statuses worth waking for.
const TASK_WAKE_STATUSES: &[AgentStatus] = &[AgentStatus::Blocked, AgentStatus::Done];
/// The adopted-session statuses worth waking for.
const ADOPTED_WAKE_STATUSES: &[AgentStatus] =
    &[AgentStatus::Idle, AgentStatus::Blocked, AgentStatus::Done];
/// The statuses a wait for a change from one of them ends on.
const WAITABLE_STATUSES: &[AgentStatus] = &[
    AgentStatus::Idle,
    AgentStatus::Working,
    AgentStatus::Blocked,
    AgentStatus::Done,
];

#[derive(Debug, Clone)]
pub struct Settings {
    pub poll: Duration,
    pub heartbeat: Duration,
    pub once: bool,
    pub quiet_secs: u64,
}

/// Watches until something wakes the supervisor and returns the reason, already queued; with
/// `settings.once`, returns `None` when nothing did. `lock` is touched every cycle.
pub fn run(
    state: &StateDir,
    runner: Arc<dyn Runner>,
    settings: &Settings,
    lock: &Guard,
) -> anyhow::Result<Option<String>> {
    let mut watcher = Watcher::new(state, runner, settings)?;
    let mut cycles = 0;
    loop {
        lock.touch()?;
        if let Some(reason) = watcher.cycle()? {
            return Ok(Some(reason));
        }
        cycles += 1;
        if settings.once && cycles >= ONCE_CYCLES {
            return Ok(None);
        }
        thread::sleep(settings.poll);
    }
}

/// What one watch run hands the next.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct WatchState {
    #[serde(flatten)]
    reported: Reported,
    /// Per task, the `last_event` of the quiet spell a wake already reported.
    #[serde(default)]
    quiet_reported: BTreeMap<String, Timestamp>,
    /// Per task, how far its files have been read.
    #[serde(default)]
    seen: BTreeMap<String, Seen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    state: State,
    /// Bytes of `events.jsonl` read.
    events: u64,
}

/// What handling an observation, absorbed or woken, moves forward.
enum Progress {
    Nothing,
    Seen {
        task: String,
        seen: Seen,
    },
    /// Recorded only when the observation wakes.
    QuietReported {
        task: String,
        last_event: Timestamp,
    },
}

struct Watcher<'a> {
    state: &'a StateDir,
    store: Store,
    config: Config,
    runner: Arc<dyn Runner>,
    settings: Settings,
    memory: WatchState,
    saved: WatchState,
    waits: BTreeMap<String, PaneWait>,
    quiet_checked: HashMap<String, Instant>,
    /// Problems already logged, so a persistent one is logged once per run.
    logged: HashSet<String>,
    started: Instant,
}

impl<'a> Watcher<'a> {
    fn new(
        state: &'a StateDir,
        runner: Arc<dyn Runner>,
        settings: &Settings,
    ) -> anyhow::Result<Watcher<'a>> {
        let supervisor = state.root().join(SUPERVISOR_DIR);
        std::fs::create_dir_all(&supervisor)
            .with_context(|| format!("creating {}", supervisor.display()))?;
        let memory = load_watch_state(state)?;
        Ok(Watcher {
            state,
            store: Store::open(state),
            config: Config::load(state)?,
            runner,
            settings: settings.clone(),
            saved: memory.clone(),
            memory,
            waits: BTreeMap::new(),
            quiet_checked: HashMap::new(),
            logged: HashSet::new(),
            started: Instant::now(),
        })
    }

    /// One poll cycle: the wake reason when something woke the supervisor.
    fn cycle(&mut self) -> anyhow::Result<Option<String>> {
        let tasks = self.store.list_tasks().unwrap_or_else(|err| {
            self.log(&format!("reading tasks: {err:#}"));
            Vec::new()
        });
        let follow = FollowList::load(self.state).map_or_else(
            |err| {
                self.log(&format!("{err:#}"));
                Vec::new()
            },
            |list| list.follow,
        );
        let mut observed = self.read_task_files(&tasks);
        self.reconcile_waits(&tasks, &follow);
        observed.extend(self.poll_waits());
        observed.extend(self.quiet_tasks(&tasks));
        if self.started.elapsed() >= self.settings.heartbeat {
            observed.push((Observation::Heartbeat, Progress::Nothing));
        }
        let thresholds = Thresholds {
            quiet_secs: self.settings.quiet_secs,
            follow,
        };
        for (obs, progress) in observed {
            let verdict = self
                .memory
                .reported
                .review(&obs, classify(&obs, &thresholds));
            match verdict {
                Verdict::Absorb => {
                    self.advance(progress, false);
                    self.log(&format!("absorb {}", serde_json::to_string(&obs)?));
                }
                Verdict::Wake(reason) => {
                    self.advance(progress, true);
                    queue::push(self.state, &reason, &obs)?;
                    self.log(&format!("wake {reason}"));
                    self.save()?;
                    return Ok(Some(reason));
                }
            }
        }
        self.save()?;
        Ok(None)
    }

    fn advance(&mut self, progress: Progress, woke: bool) {
        match progress {
            Progress::Nothing => {}
            Progress::Seen { task, seen } => {
                self.memory.seen.insert(task, seen);
            }
            Progress::QuietReported { task, last_event } => {
                if woke {
                    self.memory.quiet_reported.insert(task, last_event);
                }
            }
        }
    }

    /// Status changes and appended events since each task's files were last read. A task
    /// read for the first time is taken as it is; a closed task whose closing was read is
    /// passed over.
    fn read_task_files(&mut self, tasks: &[(TaskRecord, Status)]) -> Vec<(Observation, Progress)> {
        let mut observed = Vec::new();
        for (rec, status) in tasks {
            let seen = self.memory.seen.get(&rec.id).copied();
            if seen.is_some_and(|seen| !seen.state.is_open() && seen.state == status.state) {
                continue;
            }
            let events_path = match self.store.task_dir(&rec.id) {
                Ok(dir) => dir.join(EVENTS_FILE),
                Err(err) => {
                    self.log(&format!("{err:#}"));
                    continue;
                }
            };
            let Some(mut seen) = seen else {
                let baseline = Seen {
                    state: status.state,
                    events: file_len(&events_path),
                };
                self.memory.seen.insert(rec.id.clone(), baseline);
                continue;
            };
            if seen.state != status.state {
                let obs = Observation::StatusChanged {
                    task: rec.id.clone(),
                    from: seen.state,
                    to: status.state,
                };
                seen.state = status.state;
                let task = rec.id.clone();
                observed.push((obs, Progress::Seen { task, seen }));
            }
            match new_events(&events_path, seen.events) {
                Ok(NewEvents::Lines(lines)) => {
                    for (kind, end) in lines {
                        seen.events = end;
                        let task = rec.id.clone();
                        let obs = Observation::EventAppended {
                            task: task.clone(),
                            kind,
                        };
                        observed.push((obs, Progress::Seen { task, seen }));
                    }
                }
                Ok(NewEvents::Shrunk(len)) => {
                    seen.events = len;
                    self.memory.seen.insert(rec.id.clone(), seen);
                }
                Err(err) => self.log(&format!("{err:#}")),
            }
        }
        observed
    }

    /// Keeps one wait per open task with a herdr target and one per followed pane, dropping
    /// waits whose task closed, whose target moved, or which were unfollowed.
    fn reconcile_waits(&mut self, tasks: &[(TaskRecord, Status)], follow: &[Followed]) {
        let mut wanted: BTreeMap<String, WaitSpec> = BTreeMap::new();
        for (rec, status) in tasks {
            let Some(target) = status.herdr.as_ref().filter(|_| status.state.is_open()) else {
                continue;
            };
            wanted.insert(
                rec.id.clone(),
                WaitSpec {
                    target: WaitTarget::Task(rec.id.clone()),
                    cde: target.cde.clone(),
                    session: herdr::SESSION.to_owned(),
                    pane_id: target.pane_id.clone(),
                },
            );
        }
        for followed in follow {
            let spec = WaitSpec {
                target: WaitTarget::Adopted,
                cde: followed.cde.clone(),
                session: followed.session.clone(),
                pane_id: followed.pane_id.clone(),
            };
            wanted.insert(spec.key(), spec);
        }
        self.waits
            .retain(|key, wait| wanted.get(key).is_some_and(|spec| *spec == wait.spec));
        for (key, spec) in wanted {
            if self.waits.contains_key(&key) {
                continue;
            }
            let Some(cde) = self.config.cdes.iter().find(|cde| cde.name == spec.cde) else {
                self.log(&format!(
                    "{key}: no CDE named {:?} in config.toml",
                    spec.cde
                ));
                continue;
            };
            let herdr = Herdr::with_session(self.runner.clone(), &spec.session);
            self.waits
                .insert(key, PaneWait::new(spec, cde.clone(), herdr));
        }
    }

    /// Starts the waits not running and reads the ones that ended. A status not worth waking
    /// for is logged rather than observed.
    fn poll_waits(&mut self) -> Vec<(Observation, Progress)> {
        let timeout_ms = u64::try_from(self.settings.heartbeat.as_millis()).unwrap_or(u64::MAX);
        let mut observed = Vec::new();
        let mut notes = Vec::new();
        for wait in self.waits.values_mut() {
            let reported = &mut self.memory.reported;
            let obs = match wait.poll(timeout_ms) {
                Polled::Running => continue,
                Polled::Reached(status) => {
                    reported.reachable(&wait.cde.name);
                    reported.agent_seen(&wait.spec.key(), status);
                    if !wait.spec.wake_statuses().contains(&status) {
                        notes.push(format!("{}: agent {}", wait.spec.key(), status.as_str()));
                        continue;
                    }
                    wait.spec.observe(status)
                }
                Polled::TimedOut => {
                    reported.reachable(&wait.cde.name);
                    continue;
                }
                Polled::Gone => {
                    reported.reachable(&wait.cde.name);
                    wait.spec.gone()
                }
                Polled::Unreachable => Observation::CdeUnreachable {
                    cde: wait.cde.name.clone(),
                },
                Polled::Failed(problem) => {
                    notes.push(problem);
                    continue;
                }
            };
            observed.push((obs, Progress::Nothing));
        }
        for note in notes {
            self.log(&note);
        }
        observed
    }

    /// A `Quiet` observation for each working or pending task without a hold whose last event
    /// is older than the quiet threshold, unless a wake already reported that quiet spell. The
    /// task's worktree is searched for writes after the last event, at most once per
    /// `WORKTREE_CHECK_INTERVAL`.
    fn quiet_tasks(&mut self, tasks: &[(TaskRecord, Status)]) -> Vec<(Observation, Progress)> {
        let now = Timestamp::now();
        let mut observed = Vec::new();
        for (rec, status) in tasks {
            if !matches!(status.state, State::Working | State::Pending) || status.hold.is_some() {
                continue;
            }
            let since_secs =
                u64::try_from(now.as_second() - status.last_event.as_second()).unwrap_or(0);
            if since_secs < self.settings.quiet_secs
                || self.memory.quiet_reported.get(&rec.id) == Some(&status.last_event)
                || self
                    .quiet_checked
                    .get(&rec.id)
                    .is_some_and(|at| at.elapsed() < WORKTREE_CHECK_INTERVAL)
            {
                continue;
            }
            self.quiet_checked.insert(rec.id.clone(), Instant::now());
            let worktree_written = match self.worktree_written(rec, status.last_event) {
                Ok(written) => written,
                Err(cde) => {
                    observed.push((Observation::CdeUnreachable { cde }, Progress::Nothing));
                    continue;
                }
            };
            observed.push((
                Observation::Quiet {
                    task: rec.id.clone(),
                    since_secs,
                    worktree_written,
                },
                Progress::QuietReported {
                    task: rec.id.clone(),
                    last_event: status.last_event,
                },
            ));
        }
        observed
    }

    /// Whether any file in the task's worktree, `.git` aside, changed after `since`; `false`
    /// for a task without a worktree or on an unknown CDE. `Err` names the CDE when it could not
    /// be reached.
    fn worktree_written(&mut self, rec: &TaskRecord, since: Timestamp) -> Result<bool, String> {
        if rec.worktree.as_os_str().is_empty() {
            return Ok(false);
        }
        let Some(cde) = self.config.cdes.iter().find(|cde| cde.name == rec.cde) else {
            return Ok(false);
        };
        let argv = worktree_write_argv(&rec.worktree, since);
        match self.runner.run(cde, &argv, None) {
            Ok(output) if output.status == SSH_FAILURE_STATUS && output.stdout.is_empty() => {
                Err(cde.name.clone())
            }
            Ok(output) => {
                self.memory.reported.reachable(&cde.name);
                Ok(!output.stdout.trim().is_empty())
            }
            Err(err) => {
                let name = cde.name.clone();
                self.log(&format!("checking {}: {err:#}", rec.worktree.display()));
                Err(name)
            }
        }
    }

    fn save(&mut self) -> anyhow::Result<()> {
        if self.memory == self.saved {
            return Ok(());
        }
        let path = self.state.root().join(WATCH_STATE_FILE);
        let mut bytes = serde_json::to_vec_pretty(&self.memory)?;
        bytes.push(b'\n');
        write_atomic(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        self.saved = self.memory.clone();
        Ok(())
    }

    /// Appends `message` to the watch log. Absorbed observations and wakes are each logged;
    /// any other message once per run.
    fn log(&mut self, message: &str) {
        let repeatable = message.starts_with("absorb ") || message.starts_with("wake ");
        if !repeatable && !self.logged.insert(message.to_owned()) {
            return;
        }
        let path = self.state.root().join(WATCH_LOG_FILE);
        let line = format!("{} {message}\n", Timestamp::now());
        // The log is for diagnosis; failing to write it must not stop the watch.
        let _ = append_line(&path, line.as_bytes());
    }
}

/// `find` printing the first file under `worktree`, `.git` aside, modified after `since`.
fn worktree_write_argv(worktree: &Path, since: Timestamp) -> Vec<String> {
    let since = since.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
    [
        "find",
        &worktree.to_string_lossy(),
        "-path",
        "*/.git",
        "-prune",
        "-o",
        "-type",
        "f",
        "-newermt",
        &since,
        "-print",
        "-quit",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Whose agent a wait watches: a task's worker or a followed adopted session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WaitTarget {
    Task(String),
    Adopted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WaitSpec {
    target: WaitTarget,
    cde: String,
    session: String,
    pane_id: String,
}

impl WaitSpec {
    /// The key `classify::pane_key` gives this pane's observations.
    fn key(&self) -> String {
        match &self.target {
            WaitTarget::Task(task) => task.clone(),
            WaitTarget::Adopted => format!("{}/{}/{}", self.cde, self.session, self.pane_id),
        }
    }

    fn wake_statuses(&self) -> &'static [AgentStatus] {
        match self.target {
            WaitTarget::Task(_) => TASK_WAKE_STATUSES,
            WaitTarget::Adopted => ADOPTED_WAKE_STATUSES,
        }
    }

    fn observe(&self, status: AgentStatus) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentState {
                task: task.clone(),
                status,
            },
            WaitTarget::Adopted => self.adopted(status),
        }
    }

    /// herdr no longer knows the pane. An adopted session's agent then reads as `unknown`.
    fn gone(&self) -> Observation {
        match &self.target {
            WaitTarget::Task(task) => Observation::AgentGone { task: task.clone() },
            WaitTarget::Adopted => self.adopted(AgentStatus::Unknown),
        }
    }

    fn adopted(&self, status: AgentStatus) -> Observation {
        Observation::AdoptedChanged {
            cde: self.cde.clone(),
            session: self.session.clone(),
            pane_id: self.pane_id.clone(),
            status,
        }
    }
}

/// One herdr `agent wait` kept running on a pane.
struct PaneWait {
    spec: WaitSpec,
    cde: Cde,
    herdr: Herdr,
    child: Option<Child>,
    /// The status the last wait ended on; the next one waits for a change from it when it is
    /// one worth waking for, since that was already observed.
    settled: Option<AgentStatus>,
    retry_at: Option<Instant>,
    /// Set once herdr no longer knows the pane; nothing more is asked about it.
    gone: bool,
}

enum Polled {
    Running,
    Reached(AgentStatus),
    TimedOut,
    Gone,
    Unreachable,
    Failed(String),
}

impl PaneWait {
    fn new(spec: WaitSpec, cde: Cde, herdr: Herdr) -> PaneWait {
        PaneWait {
            spec,
            cde,
            herdr,
            child: None,
            settled: None,
            retry_at: None,
            gone: false,
        }
    }

    /// The statuses the next wait ends on.
    fn until(&self) -> Vec<AgentStatus> {
        let wake = self.spec.wake_statuses();
        match self.settled {
            Some(settled) if wake.contains(&settled) => WAITABLE_STATUSES
                .iter()
                .copied()
                .filter(|status| *status != settled)
                .collect(),
            _ => wake.to_vec(),
        }
    }

    /// Starts the wait when none runs and it is not backing off, or reads the running one.
    fn poll(&mut self, timeout_ms: u64) -> Polled {
        if self.gone {
            return Polled::Running;
        }
        let Some(child) = &mut self.child else {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return Polled::Running;
            }
            let until = self.until();
            return match self.herdr.spawn_wait(
                &self.cde,
                &self.spec.pane_id,
                &until,
                Some(timeout_ms),
            ) {
                Ok(child) => {
                    self.child = Some(child);
                    self.retry_at = None;
                    Polled::Running
                }
                Err(_) => self.back_off(Polled::Unreachable),
            };
        };
        let exit = match child.try_wait() {
            Ok(None) => return Polled::Running,
            Ok(Some(exit)) => exit,
            Err(err) => return self.back_off(Polled::Failed(format!("waiting on herdr: {err}"))),
        };
        let mut stdout = String::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_string(&mut stdout);
        }
        self.child = None;
        match herdr::wait_end(exit.code(), &stdout) {
            WaitEnd::Reached(status) => {
                self.settled = Some(status);
                Polled::Reached(status)
            }
            WaitEnd::TimedOut => Polled::TimedOut,
            WaitEnd::Gone => {
                self.gone = true;
                Polled::Gone
            }
            WaitEnd::Unreachable => self.back_off(Polled::Unreachable),
            WaitEnd::Failed(problem) => self.back_off(Polled::Failed(format!(
                "{}: herdr wait on {} {problem}",
                self.spec.key(),
                self.cde.name
            ))),
        }
    }

    fn back_off(&mut self, polled: Polled) -> Polled {
        self.child = None;
        self.retry_at = Some(Instant::now() + RETRY_AFTER);
        polled
    }
}

impl Drop for PaneWait {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

enum NewEvents {
    /// Each complete new line's event kind and the byte offset just after it.
    Lines(Vec<(String, u64)>),
    /// The file is shorter than what was read; this is its length.
    Shrunk(u64),
}

/// The complete lines of `events.jsonl` after byte `offset`. A line that does not parse reads
/// as an event of empty kind.
fn new_events(path: &Path, offset: u64) -> anyhow::Result<NewEvents> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(NewEvents::Lines(Vec::new()));
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let len = file.metadata()?.len();
    if len < offset {
        return Ok(NewEvents::Shrunk(len));
    }
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_to_end(&mut bytes))
        .with_context(|| format!("reading {}", path.display()))?;
    let mut lines = Vec::new();
    let mut end = offset;
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        end += line.len() as u64;
        if line.trim_ascii().is_empty() {
            continue;
        }
        let kind = serde_json::from_slice::<Event>(line)
            .map(|ev| ev.kind)
            .unwrap_or_default();
        lines.push((kind, end));
    }
    Ok(NewEvents::Lines(lines))
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

fn load_watch_state(state: &StateDir) -> anyhow::Result<WatchState> {
    let path = state.root().join(WATCH_STATE_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(WatchState::default()),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::runner::{FakeRunner, Scripted};
    use crate::state::task::{Harness, HerdrTarget};
    use classify::DEFAULT_QUIET_SECS;

    const CONFIG: &str = r#"
worktree_tool = { create = ["wt", "create", "{branch}"] }

[monorepo]
path = "/work/repo"
default_branch = "main"

[[cdes]]
name = "cde-a"
env_id = "env-0001"
ssh_alias = "cde-a.example"
"#;
    const TASK: &str = "aaaaaaab";
    const IDLE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"idle","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:wait"}"#;

    struct Fixture {
        _dir: tempfile::TempDir,
        state: StateDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("state");
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("config.toml"), CONFIG).unwrap();
            let state = StateDir::resolve(dir.path()).unwrap();
            Fixture { _dir: dir, state }
        }

        /// A working task on `cde-a`, its worker in `w1:p1` when `with_pane`.
        fn task(&self, with_pane: bool) {
            let store = Store::open(&self.state);
            let rec = TaskRecord {
                id: TASK.to_owned(),
                parent: None,
                title: "Build feature".to_owned(),
                cde: "cde-a".to_owned(),
                worktree: PathBuf::from("/work/repo.wt/feature"),
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
            };
            store.create_task(rec, "brief").unwrap();
            store
                .set_status(TASK, |status| {
                    status.state = State::Working;
                    status.herdr = with_pane.then(|| HerdrTarget {
                        cde: "cde-a".to_owned(),
                        workspace_id: "w1".to_owned(),
                        pane_id: "w1:p1".to_owned(),
                        agent: "claude".to_owned(),
                    });
                })
                .unwrap();
        }

        fn follow(&self) {
            FollowList::update(&self.state, |list| {
                list.follow.push(Followed {
                    cde: "cde-a".to_owned(),
                    pane_id: "w1:p1".to_owned(),
                    session: "default".to_owned(),
                });
            })
            .unwrap();
        }

        fn watcher(&self, fake: &Arc<FakeRunner>, quiet_secs: u64) -> Watcher<'_> {
            let settings = Settings {
                poll: Duration::from_millis(10),
                heartbeat: Duration::from_secs(3600),
                once: false,
                quiet_secs,
            };
            Watcher::new(&self.state, fake.clone(), &settings).unwrap()
        }
    }

    fn fake(script: Vec<Scripted>) -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(script))
    }

    fn answer(status: i32, stdout: &str) -> Scripted {
        Scripted {
            status,
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    /// Runs a cycle, lets every wait it started end, and runs another.
    fn two_cycles(watcher: &mut Watcher) -> Option<String> {
        if let Some(reason) = watcher.cycle().unwrap() {
            return Some(reason);
        }
        for wait in watcher.waits.values_mut() {
            if let Some(child) = &mut wait.child {
                child.wait().unwrap();
            }
        }
        watcher.cycle().unwrap()
    }

    fn wait_untils(fake: &FakeRunner) -> Vec<Vec<String>> {
        fake.calls()
            .into_iter()
            .map(|call| {
                call.argv
                    .windows(2)
                    .filter(|pair| pair[0] == "--until")
                    .map(|pair| pair[1].clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn status_change_between_runs_wakes_the_next_run() {
        let fx = Fixture::new();
        fx.task(false);
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );

        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::NeedsDecision)
            .unwrap();
        let mut next = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            next.cycle().unwrap().as_deref(),
            Some("needs-decision: aaaaaaab")
        );
        let mut after = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(after.cycle().unwrap(), None);
    }

    #[test]
    fn followed_session_wakes_once_then_waits_for_a_change() {
        let fx = Fixture::new();
        fx.follow();
        let runner = fake(vec![answer(0, IDLE), answer(0, IDLE), answer(0, IDLE)]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut first).as_deref(),
            Some("adopted-idle: cde-a/w1:p1")
        );
        drop(first);

        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut second), None);
        assert_eq!(second.cycle().unwrap(), None);
        assert_eq!(
            wait_untils(&runner),
            [
                vec!["idle", "blocked", "done"],
                vec!["idle", "blocked", "done"],
                vec!["working", "blocked", "done"],
            ]
        );
        assert!(runner.calls()[0].argv.contains(&"default".to_owned()));
    }

    #[test]
    fn vanished_worker_wakes_and_is_not_asked_again() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(vec![answer(1, NOT_FOUND)]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut watcher).as_deref(),
            Some("agent-gone: aaaaaaab")
        );
        assert_eq!(watcher.cycle().unwrap(), None);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn quiet_task_wakes_once_per_quiet_spell() {
        let fx = Fixture::new();
        fx.task(false);
        let runner = fake(vec![answer(0, "")]);
        let mut first = fx.watcher(&runner, 0);
        let reason = first.cycle().unwrap().unwrap();
        assert!(reason.starts_with("quiet: aaaaaaab "), "{reason}");
        let argv = &runner.calls()[0].argv;
        assert_eq!(argv[..2], ["find", "/work/repo.wt/feature"]);
        assert!(argv.contains(&"-newermt".to_owned()));

        let mut second = fx.watcher(&runner, 0);
        assert_eq!(second.cycle().unwrap(), None);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn quiet_task_writing_its_worktree_is_absorbed() {
        let fx = Fixture::new();
        fx.task(false);
        let runner = fake(vec![answer(0, "/work/repo.wt/feature/src/lib.rs\n")]);
        assert_eq!(fx.watcher(&runner, 0).cycle().unwrap(), None);
        let log = std::fs::read_to_string(fx.state.root().join(WATCH_LOG_FILE)).unwrap();
        assert!(
            log.lines().any(|line| line.contains("absorb")
                && line.contains(r#""type":"quiet""#)
                && line.contains(r#""worktree_written":true"#)),
            "{log}"
        );
    }
}
