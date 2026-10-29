//! `flow watch`: blocks on the fleet and returns the first reason the supervisor should wake.
//! Each poll cycle reads every task's `status.json` and `events.jsonl`, reads the herdr waits
//! it keeps running per worker and hand session, checks tasks gone quiet, and classifies what
//! it saw. Absorbed observations go to `state/supervisor/watch.log`; the first wake is queued
//! and returned. Nothing here changes a task: the supervisor acts on what watch reports.
//!
//! `state/supervisor/watch-state.json` carries what one run hands the next: how far each
//! task's files were read, so a change made between runs still wakes the next one, and what
//! earlier wakes reported, so a condition that persists wakes once rather than on every turn
//! end. A reported worker status is news again once its task's event log grows, since herdr
//! reports only the current status and a worker that went blocked, working, and blocked again
//! between runs reads the same. An adopted session has no event log, so the same gap remains
//! for it: a repeat of its reported status between runs is absorbed until herdr reports a
//! different one. A task an earlier run never read is replayed from `pending` and an empty
//! event log, so a worker that finished or asked something before the watcher first saw its
//! task still wakes the supervisor; only the very first run, with no `watch-state.json` yet,
//! takes every task as it is. That run writes the file once a cycle has listed the tasks.
//!
//! A worker that stops, blocked or done in herdr's terms, after its task reported
//! `needs-decision` or `blocked` is absorbed: it stopped to wait on the hold that already woke
//! the supervisor. A task whose worker herdr last reported `working` does not count as quiet.
//!
//! Hand sessions are the herdr agents outside flow's herdr session on live hosts, as of the
//! cached inventory: sessions the user started by hand. A run first lists every live host's
//! agents into that cache, once, keeping a host's cached agents when its listing fails, and
//! waits on every hand session it then finds. Any of them turning `blocked` wakes the
//! supervisor; the ones in the follow list also wake it on `idle` and `done`. Whether a session
//! is followed is read every cycle.
//!
//! A run with nothing to watch, no open task, followed session or hand session, ends after its
//! first cycle rather than waiting for the heartbeat. A cycle that could not read the tasks or
//! the follow list counts as having something to watch. The heartbeat fires only for open
//! tasks, followed sessions or what could not be read: a run watching hand sessions alone ends
//! without waking once the heartbeat interval has passed, rather than waking the supervisor
//! hourly or outliving the Stop hook's timeout.
//!
//! An error after the lock is taken wakes the supervisor as `watch-failed` once per distinct
//! error; `run_retrying` keeps retrying a repeat of it in the background, holding the lock.

pub mod classify;
mod files;
pub mod lock;
mod panes;
pub mod queue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::adopt::{FollowList, Followed};
use crate::herdr::{self, Herdr};
use crate::inventory;
use crate::paths::StateDir;
use crate::runner::Runner;
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::events::EVENTS_FILE;
use crate::state::fs::{append_line, write_atomic};
use crate::state::task::{State, Status, TaskRecord};
use classify::{Observation, Reported, Thresholds, Verdict, classify};
use files::{Checked, NewEvents, Seen, WorktreeCheck, file_len, new_events};
use lock::Guard;
use panes::{PaneWait, Polled, WaitSpec, WaitTarget};

const SUPERVISOR_DIR: &str = "supervisor";
const WATCH_STATE_FILE: &str = "supervisor/watch-state.json";
const WATCH_LOG_FILE: &str = "supervisor/watch.log";
/// How often one quiet task's working directory is searched for writes.
const WORKTREE_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// `--once` runs this many cycles, one poll interval apart, so the herdr waits the first
/// starts can answer in the second.
const ONCE_CYCLES: u32 = 2;

#[derive(Debug, Clone)]
pub struct Settings {
    pub poll: Duration,
    pub heartbeat: Duration,
    pub once: bool,
    pub quiet_secs: u64,
    /// Whether the run first lists every live host's agents into the cached inventory.
    pub list_agents: bool,
}

/// Watches until something wakes the supervisor and returns the reason, already queued; with
/// `settings.once`, returns `None` when nothing did. `lock` is touched every cycle.
pub fn run(
    state: &StateDir,
    runner: Arc<dyn Runner>,
    settings: &Settings,
    lock: &Guard,
) -> anyhow::Result<Option<String>> {
    if settings.list_agents {
        list_agents(state, &runner);
    }
    let mut watcher = Watcher::new(state, runner, settings)?;
    let mut cycles = 0;
    loop {
        lock.touch()?;
        if let Some(reason) = watcher.cycle()? {
            return Ok(Some(reason));
        }
        if !watcher.watching {
            watcher.log("nothing to watch: no open tasks, followed or hand sessions; exiting");
            return Ok(None);
        }
        if !watcher.beating && watcher.started.elapsed() >= settings.heartbeat {
            watcher.log("heartbeat interval passed with only hand sessions watched; exiting");
            return Ok(None);
        }
        cycles += 1;
        if settings.once && cycles >= ONCE_CYCLES {
            return Ok(None);
        }
        thread::sleep(settings.poll);
    }
}

/// Refreshes every live host's cached agents with one herdr listing each, the way
/// `flow sessions` does. A host whose listing fails keeps its cached agents; failures are
/// logged and never end the run.
fn list_agents(state: &StateDir, runner: &Arc<dyn Runner>) {
    let found = Config::load(state).and_then(|config| Ok((config, inventory::live_hosts(state)?)));
    let (config, hosts) = match found {
        Ok(found) => found,
        Err(err) => {
            append_log(state, &format!("listing agents: {err:#}"));
            return;
        }
    };
    let listed = inventory::list_agents_on(
        state,
        runner,
        &config.herdr_sessions,
        &hosts,
        Timestamp::now(),
    );
    for (host, listed) in hosts.iter().zip(listed) {
        let problem = match listed {
            Ok(listed) => listed.error,
            Err(err) => Some(format!("{err:#}")),
        };
        if let Some(problem) = problem {
            append_log(
                state,
                &format!("listing agents on {}: {problem}", host.name),
            );
        }
    }
}

/// A herdr agent outside flow's herdr session on a live host, as of the cached inventory.
struct HandSession {
    host: String,
    session: String,
    pane_id: String,
    title: String,
}

fn hand_sessions(state: &StateDir) -> anyhow::Result<Vec<HandSession>> {
    Ok(inventory::load_all(state)?
        .into_iter()
        .filter(|inv| !inv.gone)
        .flat_map(|inv| {
            let host = inv.name;
            inv.agents
                .into_iter()
                .filter(|agent| agent.session != herdr::SESSION)
                .map(move |agent| HandSession {
                    host: host.clone(),
                    session: agent.session,
                    pane_id: agent.pane_id,
                    title: agent.title,
                })
        })
        .collect())
}

/// How a watch that fails after taking the lock retries.
#[derive(Debug, Clone)]
pub struct Retry {
    /// The delay before the first retry; each later one doubles, up to `heartbeat`.
    pub first: Duration,
    /// The longest delay, and how long a reported error stays reported: the same error wakes
    /// again once this long has passed since its wake.
    pub heartbeat: Duration,
}

/// Runs `attempt`, a watch holding `lock`, until it returns, and returns what it returned.
/// When it fails with an error that is news, the error's wake is queued (see `failure_wake`)
/// and returned. An error is news unless the newest queued wake reported the same error less
/// than `retry.heartbeat` ago. Otherwise the run keeps the lock, which keeps `flow guard`
/// satisfied, logs the error, and retries silently after a delay that starts at `retry.first`
/// and doubles up to the heartbeat, touching the lock meanwhile.
pub fn run_retrying(
    state: &StateDir,
    lock: &Guard,
    retry: &Retry,
    mut attempt: impl FnMut() -> anyhow::Result<Option<String>>,
) -> Option<String> {
    let mut delay = retry.first;
    loop {
        let err = match attempt() {
            Ok(watched) => return watched,
            Err(err) => err,
        };
        let error = one_line(&err);
        // An unreadable queue counts as no earlier wake: waking too often beats never waking.
        let latest = queue::latest(state).ok().flatten();
        if failure_is_news(latest.as_ref(), &error, Timestamp::now(), retry.heartbeat) {
            return Some(failure_wake_for(state, error));
        }
        append_log(
            state,
            &format!(
                "watch failed again, retrying in {}s: {error}",
                delay.as_secs_f64()
            ),
        );
        sleep_holding(lock, delay);
        delay = (delay * 2).min(retry.heartbeat);
    }
}

/// Whether the watch error `error` should wake the supervisor at `now`, given the newest
/// queued wake.
fn failure_is_news(
    latest: Option<&queue::Wake>,
    error: &str,
    now: Timestamp,
    heartbeat: Duration,
) -> bool {
    let Some(wake) = latest else {
        return true;
    };
    let Observation::WatchFailed { error: reported } = &wake.obs else {
        return true;
    };
    let age_ms = now.as_millisecond() - wake.ts.as_millisecond();
    reported != error || age_ms >= i64::try_from(heartbeat.as_millis()).unwrap_or(i64::MAX)
}

/// Sleeps for `delay`, touching `lock` at least once a minute so it does not go stale. A
/// failed touch is left for the next attempt's own touch to report.
fn sleep_holding(lock: &Guard, delay: Duration) {
    const TOUCH_EVERY: Duration = Duration::from_secs(60);
    let end = Instant::now() + delay;
    loop {
        let _ = lock.touch();
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(TOUCH_EVERY));
    }
}

fn one_line(err: &anyhow::Error) -> String {
    format!("{err:#}").replace('\n', " ")
}

/// The wake for an error that ended a run holding the lock: queued when the queue can be
/// written, and returned as the reason line either way.
pub fn failure_wake(state: &StateDir, err: &anyhow::Error) -> String {
    failure_wake_for(state, one_line(err))
}

fn failure_wake_for(state: &StateDir, error: String) -> String {
    let obs = Observation::WatchFailed { error };
    let Verdict::Wake(reason) = classify(&obs, &Thresholds::default()) else {
        unreachable!("a watch failure always wakes");
    };
    match queue::push(state, &reason, &obs) {
        Ok(_) => reason,
        Err(queue_err) => format!("{reason} (not queued: {queue_err:#})").replace('\n', " "),
    }
}

/// A `Quiet` observation for `task`, recording its quiet spell when it wakes.
fn quiet(
    task: &str,
    status: &Status,
    since_secs: u64,
    worktree_written: bool,
) -> (Observation, Progress) {
    (
        Observation::Quiet {
            task: task.to_owned(),
            since_secs,
            worktree_written,
        },
        Progress::QuietReported {
            task: task.to_owned(),
            last_event: status.last_event,
        },
    )
}

/// The task's state when `obs` is its worker stopping, blocked or done, after the task already
/// reported `needs-decision` or `blocked`: the worker stopped to wait for the answer, and the
/// status change already woke the supervisor.
fn escalated(obs: &Observation, tasks: &[(TaskRecord, Status)]) -> Option<State> {
    let Observation::AgentState {
        task,
        status: herdr::AgentStatus::Blocked | herdr::AgentStatus::Done,
    } = obs
    else {
        return None;
    };
    tasks
        .iter()
        .find(|(rec, _)| rec.id == *task)
        .map(|(_, status)| status.state)
        .filter(|state| matches!(state, State::NeedsDecision | State::Blocked))
}

/// A working-directory check under way for a quiet task.
struct RunningCheck {
    check: WorktreeCheck,
    host: String,
    /// The task's `last_event` when the check started.
    last_event: Timestamp,
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
    /// The known hosts as of this run's start.
    hosts: Vec<Host>,
    runner: Arc<dyn Runner>,
    settings: Settings,
    memory: WatchState,
    saved: WatchState,
    /// Whether a task this run has no `seen` entry for is replayed from `pending`, because an
    /// earlier run saved state and so would have read it had it existed then.
    replay_unseen: bool,
    /// Whether `watch-state.json` exists for this run. The very first run writes it only after
    /// a cycle that listed the tasks, so a first run that could not list them leaves the next
    /// run a first run too, rather than one that replays every task it finds.
    state_saved: bool,
    /// The hand sessions as of this run's start.
    hands: Vec<HandSession>,
    /// Whether the last cycle found anything to watch: an open task, a followed or hand
    /// session, or a task list or follow list it could not read.
    watching: bool,
    /// Whether the heartbeat fires: the last cycle found something to watch besides hand
    /// sessions.
    beating: bool,
    /// Whether `unreachable` entries no wait targets were dropped yet this run.
    pruned: bool,
    waits: BTreeMap<String, PaneWait>,
    quiet_checked: HashMap<String, Instant>,
    worktree_checks: BTreeMap<String, RunningCheck>,
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
        let (memory, replay_unseen) = load_watch_state(state)?;
        Ok(Watcher {
            state,
            store: Store::open(state),
            hosts: inventory::hosts(state)?,
            hands: hand_sessions(state)?,
            runner,
            settings: settings.clone(),
            saved: memory.clone(),
            memory,
            replay_unseen,
            state_saved: replay_unseen,
            watching: true,
            beating: true,
            pruned: false,
            waits: BTreeMap::new(),
            quiet_checked: HashMap::new(),
            worktree_checks: BTreeMap::new(),
            logged: HashSet::new(),
            started: Instant::now(),
        })
    }

    /// One poll cycle: the wake reason when something woke the supervisor.
    fn cycle(&mut self) -> anyhow::Result<Option<String>> {
        let mut unread = false;
        let listed = self.store.list_tasks();
        let tasks_listed = listed.is_ok();
        let tasks = listed.unwrap_or_else(|err| {
            self.log(&format!("reading tasks: {err:#}"));
            unread = true;
            Vec::new()
        });
        let follow = FollowList::load(self.state).map_or_else(
            |err| {
                self.log(&format!("{err:#}"));
                unread = true;
                Vec::new()
            },
            |list| list.follow,
        );
        // What could not be read may well be open, so an error never ends the run as idle.
        self.beating =
            unread || !follow.is_empty() || tasks.iter().any(|(_, status)| status.state.is_open());
        self.watching = self.beating || !self.hands.is_empty();
        if !self.beating && self.watching {
            let n = self.hands.len();
            let plural = if n == 1 { "" } else { "s" };
            self.log(&format!(
                "watching {n} hand session{plural} for blocked only; heartbeat off"
            ));
        }
        let mut observed = self.read_task_files(&tasks);
        self.reconcile_waits(&tasks, &follow);
        if !self.pruned {
            self.prune_unreachable();
            self.pruned = true;
        }
        observed.extend(self.poll_waits());
        observed.extend(self.quiet_tasks(&tasks));
        if self.beating && self.started.elapsed() >= self.settings.heartbeat {
            observed.push((Observation::Heartbeat, Progress::Nothing));
        }
        let thresholds = Thresholds {
            quiet_secs: self.settings.quiet_secs,
            follow,
        };
        for (obs, progress) in observed {
            if let Some(state) = escalated(&obs, &tasks) {
                self.advance(progress, false);
                self.log(&format!(
                    "absorb {} (task is {state})",
                    serde_json::to_string(&obs)?
                ));
                continue;
            }
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
                    if let Observation::AgentState { task, .. } | Observation::AgentGone { task } =
                        &obs
                    {
                        self.record_report_events(task);
                    }
                    queue::push(self.state, &reason, &obs)?;
                    self.log(&format!("wake {reason}"));
                    self.save(tasks_listed)?;
                    return Ok(Some(reason));
                }
            }
        }
        self.save(tasks_listed)?;
        Ok(None)
    }

    /// Notes how long the task's event log was when a wake reported its worker, so new events
    /// make the same status news again.
    fn record_report_events(&mut self, task: &str) {
        match self.store.task_dir(task) {
            Ok(dir) => {
                let events = file_len(&dir.join(EVENTS_FILE));
                self.memory.reported.record_events(task, events);
            }
            Err(err) => self.log(&format!("{err:#}")),
        }
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

    /// Drops hosts reported unreachable that no wait of this run targets: nothing here would
    /// hear that such a host answers again, so its next outage would be absorbed.
    fn prune_unreachable(&mut self) {
        let targeted: HashSet<&str> = self
            .waits
            .values()
            .map(|wait| wait.host.name.as_str())
            .collect();
        self.memory
            .reported
            .unreachable
            .retain(|host| targeted.contains(host.as_str()));
    }

    /// Status changes and appended events since each task's files were last read. A task
    /// read for the first time is replayed from `pending` and an empty event log, unless this
    /// is the very first run, which takes it as it is; a closed task whose closing was read is
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
            let mut seen = match seen {
                Some(seen) => seen,
                None if self.replay_unseen => {
                    let replay = Seen {
                        state: State::Pending,
                        events: 0,
                    };
                    self.memory.seen.insert(rec.id.clone(), replay);
                    replay
                }
                None => {
                    let baseline = Seen {
                        state: status.state,
                        events: file_len(&events_path),
                    };
                    self.memory.seen.insert(rec.id.clone(), baseline);
                    continue;
                }
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
                    if let Some((_, end)) = lines.last() {
                        self.memory.reported.events_grew(&rec.id, *end);
                    }
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

    /// Keeps one wait per open task with a herdr target and one per hand session or followed
    /// pane, dropping waits whose task closed or whose target moved. A hand session's wait
    /// starts afresh when it is followed or unfollowed, since that changes what it ends on.
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
                    host: target.host.clone(),
                    session: herdr::SESSION.to_owned(),
                    pane_id: target.pane_id.clone(),
                },
            );
        }
        for hand in &self.hands {
            let followed = follow.iter().any(|followed| {
                followed.host == hand.host
                    && followed.session == hand.session
                    && followed.pane_id == hand.pane_id
            });
            let spec = WaitSpec {
                target: WaitTarget::Adopted {
                    followed,
                    title: hand.title.clone(),
                },
                host: hand.host.clone(),
                session: hand.session.clone(),
                pane_id: hand.pane_id.clone(),
            };
            wanted.insert(spec.key(), spec);
        }
        for followed in follow {
            let spec = WaitSpec {
                target: WaitTarget::Adopted {
                    followed: true,
                    title: String::new(),
                },
                host: followed.host.clone(),
                session: followed.session.clone(),
                pane_id: followed.pane_id.clone(),
            };
            wanted.entry(spec.key()).or_insert(spec);
        }
        self.waits
            .retain(|key, wait| wanted.get(key).is_some_and(|spec| *spec == wait.spec));
        for (key, spec) in wanted {
            if self.waits.contains_key(&key) {
                continue;
            }
            let Some(host) = self.hosts.iter().find(|host| host.name == spec.host) else {
                self.log(&format!("{key}: no known host named {:?}", spec.host));
                continue;
            };
            let herdr = Herdr::with_session(self.runner.clone(), &spec.session);
            self.waits
                .insert(key, PaneWait::new(spec, host.clone(), herdr));
        }
    }

    /// Starts the waits not running and reads the ones that ended. A status not worth waking
    /// for, or an unreachable host seen only by an unfollowed hand session's wait, is logged
    /// rather than observed.
    fn poll_waits(&mut self) -> Vec<(Observation, Progress)> {
        let timeout_ms = u64::try_from(self.settings.heartbeat.as_millis()).unwrap_or(u64::MAX);
        let mut observed = Vec::new();
        let mut notes = Vec::new();
        for wait in self.waits.values_mut() {
            let reported = &mut self.memory.reported;
            let obs = match wait.poll(timeout_ms) {
                Polled::Running => continue,
                Polled::Reached(status) => {
                    reported.reachable(&wait.host.name);
                    reported.agent_seen(&wait.spec.key(), status);
                    if !wait.spec.wake_statuses().contains(&status) {
                        notes.push(format!("{}: agent {}", wait.spec.key(), status.as_str()));
                        continue;
                    }
                    wait.spec.observe(status)
                }
                Polled::TimedOut => {
                    reported.reachable(&wait.host.name);
                    continue;
                }
                Polled::Gone => {
                    reported.reachable(&wait.host.name);
                    wait.spec.gone()
                }
                Polled::Unreachable if wait.spec.unfollowed_hand() => {
                    notes.push(format!(
                        "{}: host {} unreachable",
                        wait.spec.key(),
                        wait.host.name
                    ));
                    continue;
                }
                Polled::Unreachable => Observation::HostUnreachable {
                    host: wait.host.name.clone(),
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

    /// `Quiet` observations for working or pending tasks without a hold whose last event is
    /// older than the quiet threshold and whose worker herdr does not report `working`, unless
    /// a wake already reported that quiet spell. Each
    /// such task's working directory is searched for writes after its last event, at most once per
    /// `WORKTREE_CHECK_INTERVAL`, by a check that runs across cycles; the observation comes
    /// when it ends, and only if the task is still quiet the same way.
    fn quiet_tasks(&mut self, tasks: &[(TaskRecord, Status)]) -> Vec<(Observation, Progress)> {
        let now = Timestamp::now();
        let mut observed = Vec::new();
        let mut ended = Vec::new();
        for (task, running) in &mut self.worktree_checks {
            match running.check.poll() {
                Checked::Running => {}
                checked => ended.push((task.clone(), checked)),
            }
        }
        for (task, checked) in ended {
            let running = self
                .worktree_checks
                .remove(&task)
                .expect("an ended check is running");
            let still_quiet = tasks
                .iter()
                .find(|(rec, status)| rec.id == task && status.last_event == running.last_event)
                .and_then(|(_, status)| Some((status, self.quiet_since(&task, status, now)?)));
            let Some((status, since_secs)) = still_quiet else {
                continue;
            };
            let worktree_written = match checked {
                Checked::Running => unreachable!("only ended checks are handled"),
                Checked::Written(written) => {
                    self.memory.reported.reachable(&running.host);
                    written
                }
                Checked::Unreachable => {
                    let host = running.host;
                    observed.push((Observation::HostUnreachable { host }, Progress::Nothing));
                    continue;
                }
                Checked::TimedOut => {
                    self.log(&format!("{task}: working-directory check timed out"));
                    false
                }
                Checked::Failed(problem) => {
                    self.log(&format!(
                        "{task}: working-directory check failed: {problem}"
                    ));
                    false
                }
            };
            observed.push(quiet(&task, status, since_secs, worktree_written));
        }
        for (rec, status) in tasks {
            let Some(since_secs) = self.quiet_since(&rec.id, status, now) else {
                continue;
            };
            if self.memory.quiet_reported.get(&rec.id) == Some(&status.last_event)
                || self.worktree_checks.contains_key(&rec.id)
                || self
                    .quiet_checked
                    .get(&rec.id)
                    .is_some_and(|at| at.elapsed() < WORKTREE_CHECK_INTERVAL)
            {
                continue;
            }
            self.quiet_checked.insert(rec.id.clone(), Instant::now());
            let host = self.hosts.iter().find(|host| host.name == rec.host);
            let Some(host) = host else {
                observed.push(quiet(&rec.id, status, since_secs, false));
                continue;
            };
            match WorktreeCheck::start(&*self.runner, host, &rec.cwd, status.last_event) {
                Ok(check) => {
                    let running = RunningCheck {
                        check,
                        host: host.name.clone(),
                        last_event: status.last_event,
                    };
                    self.worktree_checks.insert(rec.id.clone(), running);
                }
                Err(err) => {
                    let host = host.name.clone();
                    self.log(&format!("checking {}: {err:#}", rec.cwd.display()));
                    observed.push((Observation::HostUnreachable { host }, Progress::Nothing));
                }
            }
        }
        observed
    }

    /// How long the task has been quiet, when it may count as quiet at all: working or
    /// pending, without a hold, not reported `working` by herdr, and past the threshold.
    fn quiet_since(&self, task: &str, status: &Status, now: Timestamp) -> Option<u64> {
        if !matches!(status.state, State::Working | State::Pending) || status.hold.is_some() {
            return None;
        }
        if self.waits.get(task).is_some_and(PaneWait::working) {
            return None;
        }
        let since_secs =
            u64::try_from(now.as_second() - status.last_event.as_second()).unwrap_or(0);
        (since_secs >= self.settings.quiet_secs).then_some(since_secs)
    }

    /// Writes what changed since the last save; `tasks_listed` says whether this cycle read
    /// the task list, without which a run that never saved writes nothing yet.
    fn save(&mut self, tasks_listed: bool) -> anyhow::Result<()> {
        if self.state_saved && self.memory == self.saved {
            return Ok(());
        }
        if !self.state_saved && !tasks_listed {
            return Ok(());
        }
        write_watch_state(self.state, &self.memory)?;
        self.saved = self.memory.clone();
        self.state_saved = true;
        Ok(())
    }

    /// Appends `message` to the watch log. Absorbed observations and wakes are each logged;
    /// any other message once per run.
    fn log(&mut self, message: &str) {
        let repeatable = message.starts_with("absorb ") || message.starts_with("wake ");
        if !repeatable && !self.logged.insert(message.to_owned()) {
            return;
        }
        append_log(self.state, message);
    }
}

/// Appends `message` to the watch log, ignoring a failure to write it: the log is for
/// diagnosis and must not stop the watch.
fn append_log(state: &StateDir, message: &str) {
    let path = state.root().join(WATCH_LOG_FILE);
    let line = format!("{} {message}\n", Timestamp::now());
    let _ = append_line(&path, line.as_bytes());
}

fn write_watch_state(state: &StateDir, memory: &WatchState) -> anyhow::Result<()> {
    let path = state.root().join(WATCH_STATE_FILE);
    let mut bytes = serde_json::to_vec_pretty(memory)?;
    bytes.push(b'\n');
    write_atomic(&path, &bytes).with_context(|| format!("writing {}", path.display()))
}

/// The state the last run saved and `true`, or the default and `false` when there is none. A
/// file that does not parse is renamed aside as `watch-state.json.corrupt-<ts>`, reported on
/// stderr, and replaced by the default, so one bad write cannot stop every later watch; it
/// counts as no saved state, since replaying every task it no longer records would wake once
/// per task ever closed.
fn load_watch_state(state: &StateDir) -> anyhow::Result<(WatchState, bool)> {
    let path = state.root().join(WATCH_STATE_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok((WatchState::default(), false));
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    match serde_json::from_slice(&bytes) {
        Ok(memory) => Ok((memory, true)),
        Err(err) => {
            let mut aside = path.clone().into_os_string();
            aside.push(format!(
                ".corrupt-{}",
                Timestamp::now().strftime("%Y%m%dT%H%M%SZ")
            ));
            std::fs::rename(&path, &aside)
                .with_context(|| format!("moving {} aside", path.display()))?;
            eprintln!(
                "{} did not parse ({err}); moved it to {} and starting afresh",
                path.display(),
                Path::new(&aside).display()
            );
            Ok((WatchState::default(), false))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::inventory::HostInventory;
    use crate::runner::{FakeRunner, Scripted};
    use crate::state::events::Event;
    use crate::state::task::{Harness, HerdrTarget};
    use classify::DEFAULT_QUIET_SECS;

    const CONFIG: &str = r#"
[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
"#;
    const TASK: &str = "aaaaaaab";
    const IDLE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"idle","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const BLOCKED: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"blocked","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const WORKING: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"working","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const DONE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"done","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:wait"}"#;
    const SERVER_RUNNING: &str = "status: running\nversion: 0.9.5\nendpoint_compatible: yes\n\
         private_protocol: 22\nprivate_protocol_compatible: yes\n\
         socket: /home/dev/.config/herdr/sessions/flow/herdr.sock\n";
    const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
    const DEFAULT_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"idle","cwd":"/work/repo","pane_id":"w1:p1","terminal_title":"✳ Tidy docs","terminal_title_stripped":"Tidy docs","workspace_id":"w1"}],"type":"agent_list"}}"#;
    const TIDY_DOCS_BLOCKED: &str = r#"adopted-blocked: host-a w1:p1 "Tidy docs""#;

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
            let host = Host {
                name: "host-a".to_owned(),
                env_id: "env-0001".to_owned(),
                ssh_alias: "host-a.example".to_owned(),
                local_marker_env: None,
            };
            inventory::save(&state, &HostInventory::new(&host, Timestamp::now())).unwrap();
            Fixture { _dir: dir, state }
        }

        /// A working task on `host-a`, its worker in `w1:p1` when `with_pane`.
        fn task(&self, with_pane: bool) {
            let store = Store::open(&self.state);
            let rec = TaskRecord {
                id: TASK.to_owned(),
                parent: None,
                title: "Build feature".to_owned(),
                host: "host-a".to_owned(),
                cwd: PathBuf::from("/work/repo.wt/feature"),
                repo: Some("example-repo".to_owned()),
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
            };
            store.create_task(rec, "brief").unwrap();
            store
                .set_status(TASK, |status| {
                    status.state = State::Working;
                    status.herdr = with_pane.then(|| HerdrTarget {
                        host: "host-a".to_owned(),
                        workspace_id: "w1".to_owned(),
                        pane_id: "w1:p1".to_owned(),
                        agent: "claude".to_owned(),
                    });
                })
                .unwrap();
        }

        fn host_b(&self) -> Host {
            Host {
                name: "host-b".to_owned(),
                env_id: "env-0002".to_owned(),
                ssh_alias: "host-b.example".to_owned(),
                local_marker_env: None,
            }
        }

        /// Adds an agent in `session`'s pane `pane_id` to the cached inventory of `env_id`.
        fn agent(&self, env_id: &str, session: &str, pane_id: &str, title: &str) {
            let mut inv = inventory::load(&self.state, env_id).unwrap().unwrap();
            inv.agents.push(herdr::Agent {
                pane_id: pane_id.to_owned(),
                workspace_id: pane_id.split(':').next().unwrap().to_owned(),
                agent: "claude".to_owned(),
                status: herdr::AgentStatus::Idle,
                cwd: PathBuf::from("/work/repo"),
                title: title.to_owned(),
                session: session.to_owned(),
                agent_session_id: None,
            });
            inventory::save(&self.state, &inv).unwrap();
        }

        /// A hand session titled "Tidy docs" in herdr's `default` session, pane `w1:p1` on
        /// `host-a`.
        fn hand(&self) {
            self.agent("env-0001", "default", "w1:p1", "Tidy docs");
        }

        fn log(&self) -> String {
            std::fs::read_to_string(self.state.root().join(WATCH_LOG_FILE)).unwrap_or_default()
        }

        fn follow(&self) {
            FollowList::update(&self.state, |list| {
                list.follow.push(Followed {
                    host: "host-a".to_owned(),
                    pane_id: "w1:p1".to_owned(),
                    session: "default".to_owned(),
                });
            })
            .unwrap();
        }

        fn watcher(&self, fake: &Arc<FakeRunner>, quiet_secs: u64) -> Watcher<'_> {
            let settings = Settings {
                quiet_secs,
                ..settings(Duration::from_secs(3600))
            };
            Watcher::new(&self.state, fake.clone(), &settings).unwrap()
        }
    }

    fn settings(heartbeat: Duration) -> Settings {
        Settings {
            poll: Duration::from_millis(10),
            heartbeat,
            once: false,
            quiet_secs: DEFAULT_QUIET_SECS,
            list_agents: false,
        }
    }

    fn on(host: &str, argv: &[&str], stdout: &str) -> Scripted {
        Scripted {
            host: Some(host.to_owned()),
            argv: Some(argv.iter().map(|word| (*word).to_owned()).collect()),
            stdout: stdout.to_owned(),
            ..Scripted::default()
        }
    }

    fn host_b_unreachable() -> Scripted {
        Scripted {
            host: Some("host-b".to_owned()),
            status: 255,
            stderr: "ssh: connect to host host-b.example port 22: Connection refused".to_owned(),
            repeat: true,
            ..Scripted::default()
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
        cycles(watcher, 2)
    }

    /// Runs `n` cycles, letting every wait and check started end between them, and returns the
    /// first wake.
    fn cycles(watcher: &mut Watcher, n: usize) -> Option<String> {
        for _ in 0..n {
            if let Some(reason) = watcher.cycle().unwrap() {
                return Some(reason);
            }
            for wait in watcher.waits.values_mut() {
                if let Some(child) = &mut wait.child {
                    child.wait().unwrap();
                }
            }
            for running in watcher.worktree_checks.values_mut() {
                running.check.finish();
            }
        }
        None
    }

    fn hand_keys(watcher: &Watcher) -> Vec<String> {
        watcher
            .hands
            .iter()
            .map(|hand| {
                format!(
                    "{}/{}/{} {}",
                    hand.host, hand.session, hand.pane_id, hand.title
                )
            })
            .collect()
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
            Some(r#"adopted-idle: host-a w1:p1 "w1:p1""#)
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
    fn unfollowed_hand_session_wakes_once_when_blocked_until_it_leaves_blocked() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![
            answer(0, BLOCKED),
            answer(0, BLOCKED),
            answer(0, WORKING),
            answer(0, BLOCKED),
        ]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut first).as_deref(), Some(TIDY_DOCS_BLOCKED));
        drop(first);
        let queued = queue::latest(&fx.state).unwrap().unwrap();
        assert_eq!(queued.reason, TIDY_DOCS_BLOCKED);

        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(cycles(&mut second, 6).as_deref(), Some(TIDY_DOCS_BLOCKED));
        assert_eq!(
            wait_untils(&runner),
            [
                vec!["blocked"],
                vec!["blocked"],
                vec!["idle", "working", "done"],
                vec!["idle", "blocked", "done"],
            ]
        );
        assert!(runner.calls()[0].argv.contains(&"default".to_owned()));
    }

    #[test]
    fn unfollowed_hand_session_idle_working_done_or_gone_is_absorbed() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![
            answer(0, IDLE),
            answer(0, WORKING),
            answer(0, DONE),
            answer(1, NOT_FOUND),
        ]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(cycles(&mut watcher, 8), None);
        assert_eq!(runner.calls().len(), 4);
        assert!(queue::latest(&fx.state).unwrap().is_none());
        let log = fx.log();
        assert!(
            log.lines().any(|line| line.contains("absorb")
                && line.contains(r#""type":"adopted-changed""#)
                && line.contains(r#""status":"unknown""#)),
            "{log}"
        );
    }

    #[test]
    fn unfollowed_hand_session_on_an_unreachable_host_is_not_reported() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![answer(255, "")]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut watcher), None);
        assert!(watcher.memory.reported.unreachable.is_empty());
        assert!(queue::latest(&fx.state).unwrap().is_none());
    }

    #[test]
    fn followed_hand_session_wakes_on_idle_with_its_title() {
        let fx = Fixture::new();
        fx.hand();
        fx.follow();
        let runner = fake(vec![answer(0, IDLE)]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut watcher).as_deref(),
            Some(r#"adopted-idle: host-a w1:p1 "Tidy docs""#)
        );
        assert_eq!(wait_untils(&runner), [vec!["idle", "blocked", "done"]]);
    }

    #[test]
    fn heartbeat_is_off_while_only_hand_sessions_are_watched() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![Scripted {
            repeat: true,
            ..answer(0, WORKING)
        }]);
        let mut watcher =
            Watcher::new(&fx.state, runner.clone(), &settings(Duration::ZERO)).unwrap();
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.watching);
        assert!(!watcher.beating);
        drop(watcher);
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        assert_eq!(
            run(&fx.state, runner.clone(), &settings(Duration::ZERO), &lock).unwrap(),
            None
        );
        drop(lock);
        let log = fx.log();
        assert_eq!(
            log.matches("watching 1 hand session for blocked only; heartbeat off")
                .count(),
            2,
            "{log}"
        );
        assert!(log.contains("only hand sessions watched"), "{log}");
        assert!(queue::latest(&fx.state).unwrap().is_none());

        fx.task(true);
        let mut watcher =
            Watcher::new(&fx.state, runner.clone(), &settings(Duration::ZERO)).unwrap();
        assert_eq!(watcher.cycle().unwrap().as_deref(), Some("heartbeat"));
        assert!(watcher.beating);
    }

    #[test]
    fn hand_sessions_exclude_flows_own_herdr_session_and_gone_hosts() {
        let fx = Fixture::new();
        fx.agent("env-0001", herdr::SESSION, "w1:p1", "supervisor");
        let mut gone = HostInventory::new(&fx.host_b(), Timestamp::now());
        gone.gone = true;
        inventory::save(&fx.state, &gone).unwrap();
        fx.agent("env-0002", "default", "w5:p1", "Upgrade toolchain");
        let runner = fake(Vec::new());
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert!(watcher.hands.is_empty());
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(!watcher.watching);
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn listing_keeps_an_unreachable_hosts_cached_agents_and_the_run() {
        let fx = Fixture::new();
        inventory::save(
            &fx.state,
            &HostInventory::new(&fx.host_b(), Timestamp::now()),
        )
        .unwrap();
        fx.agent("env-0002", "default", "w5:p1", "Upgrade toolchain");
        let host_b_before = inventory::load(&fx.state, "env-0002").unwrap();
        let runner = fake(vec![
            host_b_unreachable(),
            on(
                "host-a",
                &["--session", "flow", "status", "server"],
                SERVER_RUNNING,
            ),
            on("host-a", &["--session", "flow", "agent", "list"], NO_AGENTS),
            on(
                "host-a",
                &["--session", "default", "agent", "list"],
                DEFAULT_AGENTS,
            ),
            Scripted {
                repeat: true,
                ..on("host-a", &["agent", "wait"], WORKING)
            },
        ]);
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let once = Settings {
            once: true,
            list_agents: true,
            ..settings(Duration::from_secs(3600))
        };

        assert_eq!(run(&fx.state, runner.clone(), &once, &lock).unwrap(), None);

        assert_eq!(
            inventory::load(&fx.state, "env-0002").unwrap(),
            host_b_before
        );
        let host_a = inventory::load(&fx.state, "env-0001").unwrap().unwrap();
        let titles: Vec<&str> = host_a.agents.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["Tidy docs"]);
        assert!(
            fx.log().contains("host unreachable: host-b"),
            "{}",
            fx.log()
        );
        let listings = runner
            .calls()
            .iter()
            .filter(|call| call.argv.windows(2).any(|w| w == ["agent", "list"]))
            .count();
        assert_eq!(listings, 2);
        let watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            hand_keys(&watcher),
            [
                "host-a/default/w1:p1 Tidy docs",
                "host-b/default/w5:p1 Upgrade toolchain",
            ]
        );
    }

    #[test]
    fn blocked_worker_wakes_again_once_its_task_logged_events() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(vec![
            answer(0, BLOCKED),
            answer(0, BLOCKED),
            answer(0, BLOCKED),
        ]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut first).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
        drop(first);
        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut second), None);
        drop(second);

        let ev = Event {
            ts: Timestamp::now(),
            source: "cli".to_owned(),
            kind: "status.report".to_owned(),
            data: serde_json::json!({}),
        };
        Store::open(&fx.state).append_event(TASK, &ev).unwrap();
        let mut third = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut third).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
    }

    #[test]
    fn task_first_seen_after_an_earlier_run_replays_its_status() {
        let fx = Fixture::new();
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );
        assert!(fx.state.root().join(WATCH_STATE_FILE).exists());

        fx.task(false);
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let mut next = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(next.cycle().unwrap().as_deref(), Some("done: aaaaaaab"));
        let mut after = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(after.cycle().unwrap(), None);
    }

    #[test]
    fn first_run_ever_takes_tasks_as_they_are() {
        let fx = Fixture::new();
        fx.task(false);
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );
    }

    #[test]
    fn first_run_whose_listing_fails_stays_a_first_run() {
        let fx = Fixture::new();
        fx.task(false);
        let status = Store::open(&fx.state)
            .task_dir(TASK)
            .unwrap()
            .join("status.json");
        let good = std::fs::read(&status).unwrap();
        std::fs::write(&status, "{not json").unwrap();
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );
        assert!(!fx.state.root().join(WATCH_STATE_FILE).exists());

        std::fs::write(&status, good).unwrap();
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let mut next = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert!(!next.replay_unseen);
        assert_eq!(next.cycle().unwrap(), None);
        assert!(fx.state.root().join(WATCH_STATE_FILE).exists());
    }

    #[test]
    fn unreachable_hosts_no_wait_targets_are_forgotten_at_run_start() {
        let fx = Fixture::new();
        fx.task(true);
        let supervisor = fx.state.root().join(SUPERVISOR_DIR);
        std::fs::create_dir_all(&supervisor).unwrap();
        let saved = serde_json::json!({"unreachable": ["host-a", "host-b"]});
        std::fs::write(fx.state.root().join(WATCH_STATE_FILE), saved.to_string()).unwrap();
        let runner = fake(vec![answer(255, "")]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);
        let unreachable: Vec<&str> = watcher
            .memory
            .reported
            .unreachable
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(unreachable, ["host-a"]);
    }

    #[test]
    fn worker_stopping_after_it_escalated_is_absorbed() {
        for (state, answer_json) in [(State::NeedsDecision, DONE), (State::Blocked, BLOCKED)] {
            let fx = Fixture::new();
            fx.task(true);
            Store::open(&fx.state)
                .set_status(TASK, |status| status.state = state)
                .unwrap();
            let runner = fake(vec![answer(0, answer_json), answer(0, answer_json)]);
            assert_eq!(
                two_cycles(&mut fx.watcher(&runner, DEFAULT_QUIET_SECS)),
                None
            );

            let ev = Event {
                ts: Timestamp::now(),
                source: "cli".to_owned(),
                kind: "hold.set".to_owned(),
                data: serde_json::json!({}),
            };
            Store::open(&fx.state).append_event(TASK, &ev).unwrap();
            assert_eq!(
                two_cycles(&mut fx.watcher(&runner, DEFAULT_QUIET_SECS)),
                None
            );
            let log = std::fs::read_to_string(fx.state.root().join(WATCH_LOG_FILE)).unwrap();
            assert!(
                log.lines().any(|line| line.contains("absorb")
                    && line.contains(r#""type":"agent-state""#)
                    && line.contains(&format!("task is {state}"))),
                "{log}"
            );
        }
    }

    #[test]
    fn quiet_is_not_reported_while_herdr_reports_the_worker_working() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(vec![answer(0, WORKING), answer(0, "")]);
        let mut watcher = fx.watcher(&runner, 0);
        assert_eq!(two_cycles(&mut watcher), None);
        assert_eq!(runner.calls().len(), 2);
    }

    #[test]
    fn unreadable_task_or_follow_list_keeps_the_run_watching() {
        let fx = Fixture::new();
        fx.task(false);
        let status = Store::open(&fx.state)
            .task_dir(TASK)
            .unwrap()
            .join("status.json");
        std::fs::write(&status, "{not json").unwrap();
        let runner = fake(Vec::new());
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.watching);

        let fx = Fixture::new();
        let supervisor = fx.state.root().join(SUPERVISOR_DIR);
        std::fs::create_dir_all(&supervisor).unwrap();
        std::fs::write(supervisor.join("follow.json"), "{not json").unwrap();
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.watching);
    }

    #[test]
    fn a_watch_error_is_news_unless_the_newest_wake_just_reported_it() {
        let heartbeat = Duration::from_secs(3600);
        let at = Timestamp::now();
        let wake = |obs: Observation| queue::Wake {
            seq: 1,
            ts: at,
            reason: String::new(),
            obs,
        };
        let failed = wake(Observation::WatchFailed {
            error: "disk full".to_owned(),
        });
        let soon = at + jiff::SignedDuration::from_secs(60);
        let later = at + jiff::SignedDuration::from_secs(3600);
        assert!(failure_is_news(None, "disk full", soon, heartbeat));
        assert!(!failure_is_news(
            Some(&failed),
            "disk full",
            soon,
            heartbeat
        ));
        assert!(failure_is_news(
            Some(&failed),
            "no such file",
            soon,
            heartbeat
        ));
        assert!(failure_is_news(
            Some(&failed),
            "disk full",
            later,
            heartbeat
        ));
        let heartbeat_wake = wake(Observation::Heartbeat);
        assert!(failure_is_news(
            Some(&heartbeat_wake),
            "disk full",
            soon,
            heartbeat
        ));
    }

    #[test]
    fn unparsable_watch_state_is_set_aside() {
        let fx = Fixture::new();
        let supervisor = fx.state.root().join(SUPERVISOR_DIR);
        std::fs::create_dir_all(&supervisor).unwrap();
        std::fs::write(fx.state.root().join(WATCH_STATE_FILE), "{not json").unwrap();
        let mut watcher = fx.watcher(&fake(Vec::new()), DEFAULT_QUIET_SECS);
        assert_eq!(watcher.memory, WatchState::default());
        assert!(!watcher.replay_unseen);
        assert_eq!(watcher.cycle().unwrap(), None);
        let mut names: Vec<String> = std::fs::read_dir(&supervisor)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(names[0], "watch-state.json");
        assert!(
            names[1].starts_with("watch-state.json.corrupt-"),
            "{names:?}"
        );
        let aside = std::fs::read_to_string(supervisor.join(&names[1])).unwrap();
        assert_eq!(aside, "{not json");
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
        let reason = two_cycles(&mut first).unwrap();
        assert!(reason.starts_with("quiet: aaaaaaab "), "{reason}");
        assert_eq!(
            runner.calls()[0].argv[..4],
            ["timeout", "30", "find", "/work/repo.wt/feature"]
        );

        let mut second = fx.watcher(&runner, 0);
        assert_eq!(two_cycles(&mut second), None);
        assert_eq!(runner.calls().len(), 1);
    }

    #[test]
    fn quiet_task_writing_its_worktree_is_absorbed() {
        let fx = Fixture::new();
        fx.task(false);
        let runner = fake(vec![answer(0, "/work/repo.wt/feature/src/lib.rs\n")]);
        assert_eq!(two_cycles(&mut fx.watcher(&runner, 0)), None);
        let log = std::fs::read_to_string(fx.state.root().join(WATCH_LOG_FILE)).unwrap();
        assert!(
            log.lines().any(|line| line.contains("absorb")
                && line.contains(r#""type":"quiet""#)
                && line.contains(r#""worktree_written":true"#)),
            "{log}"
        );
    }
}
