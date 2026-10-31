//! `flow watch`: blocks on the fleet and returns the first reason the supervisor should wake.
//! Each poll cycle reads every task's `status.json` and `events.jsonl`, reads the herdr waits
//! it keeps running per worker and hand session, checks tasks gone quiet, and classifies what
//! it saw. Absorbed observations go to `state/supervisor/watch.log`; the first wake is queued
//! and returned. Nothing here changes a task's state: the supervisor acts on what watch
//! reports. It writes a `turn.end` event for an adopted task whose agent goes from `working` to
//! `idle`, standing in for the Stop hook a session started by hand lacks, so its quiet check
//! counts from the end of its last turn as a dispatched worker's does; and the run-start listing
//! retires the herdr target of a task whose pane herdr reused, as every listing does.
//!
//! `state/supervisor/watch-state.json` carries what one run hands the next: how far each task's
//! files were read, so a change made between runs still wakes the next one, and what earlier
//! wakes reported, so a condition that persists wakes once rather than on every turn end. A wait
//! on a pane whose status a wake reported ends only on another status first, so a session still
//! elsewhere when a run starts is seen leaving it; one that left and came back between runs reads
//! the same. A pane reported gone wakes once, and its next occupant is news. A task no run has
//! read is replayed from `pending` and an empty event log, so a worker that finished or asked
//! something before the watcher first saw its task still wakes the supervisor; `flow up` seeds
//! the file with every existing task read, so the first run does not replay history. A file that
//! does not parse is logged and replaced. An adopted task's turn that ends while no run is up is
//! not logged.
//!
//! A worker that stops, blocked or done in herdr's terms, after its task reported
//! `needs-decision` or `blocked` is absorbed: it stopped to wait on the hold that already woke
//! the supervisor. A task whose worker herdr last reported `working` does not count as quiet.
//!
//! Hand sessions are the herdr agents on live hosts, in any herdr session flow lists, as of the
//! cached inventory, other than the supervisor, each with its owner
//! (see `owner::owner`) as of the listing. The observed ones are the sessions the user started
//! by hand. One a task owns,
//! adopted ones included, is waited on as that task's worker, in the herdr session it was
//! started in, and not as a hand session. A run first lists every live host's agents into that
//! cache, in parallel, without starting any herdr server, and giving each host at most
//! `Settings::list_timeout`. A host whose listing fails keeps its cached agents. The run waits on
//! every hand session it then finds, except on hosts that ssh could not reach, that run no herdr
//! server, or that did not answer in time: those wait for the next listing. A host whose earlier
//! listing has still not returned is not asked again in the same run. Any hand session turning
//! `blocked` wakes the supervisor; the ones in the follow list also wake it on `idle` and `done`.
//! Whether a session is followed is read every cycle.
//!
//! A run with nothing to watch, no open task, followed session or hand session, ends after its
//! first cycle rather than waiting for the heartbeat. A cycle that could not read the tasks or
//! the follow list counts as having something to watch. The heartbeat fires only for open
//! tasks, followed sessions or what could not be read, timed from when the run found one. A run
//! watching hand sessions alone lists the hosts again every heartbeat interval, so a session
//! started after the run began is watched within the hour, and ends without waking after
//! `Settings::hand_max`, a day by default, rather than waking the supervisor hourly; the Stop
//! hook's timeout is set longer than that.
//!
//! An error after the lock is taken wakes the supervisor as `watch-failed` once per distinct
//! error; `run_retrying` keeps retrying a repeat of it in the background, holding the lock. A run
//! whose lock is removed or taken over, as `flow up` does, ends quietly at its next touch,
//! retrying or not, so a watcher on another host ends on its own.

pub mod classify;
mod files;
pub mod lock;
mod panes;
pub mod queue;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::herdr::{self, Agent, Herdr};
use crate::inventory::{self, FlowServer};
use crate::observe::{FollowList, Followed};
use crate::owner::{self, Owner};
use crate::paths::StateDir;
use crate::runner::Runner;
use crate::state::Store;
use crate::state::config::{Config, Host};
use crate::state::events::{EVENTS_FILE, Event};
use crate::state::fs::{append_line, write_atomic};
use crate::state::task::{Origin, State, Status, TaskRecord};
use crate::supervisor;
use classify::{Observation, Reported, Thresholds, Verdict, classify};
use files::{Checked, NewEvents, Seen, WorktreeCheck, file_len, new_events};
use lock::Guard;
use panes::{PaneWait, Polled, WaitSpec, WaitTarget};

const SUPERVISOR_DIR: &str = "supervisor";
const WATCH_STATE_FILE: &str = "supervisor/watch-state.json";
const WATCH_LOG_FILE: &str = "supervisor/watch.log";
/// The source of the events the watcher appends.
const EVENT_SOURCE: &str = "watch";
/// The event the watcher appends when an adopted task's turn ends.
const TURN_END: &str = "turn.end";
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
    /// How long a run watching only hand sessions nobody follows lasts before it ends without
    /// waking, in place of the heartbeat.
    pub hand_max: Duration,
    /// How long listing the hosts' agents may take; a host not answered by then is left out.
    pub list_timeout: Duration,
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
    if settings.list_agents {
        watcher.refresh_hands(lock);
    }
    let mut cycles = 0;
    loop {
        if let Err(err) = lock.touch() {
            if is_lost(&err) {
                watcher.log(&format!("{err:#}; exiting"));
                return Ok(None);
            }
            return Err(err);
        }
        if let Some(reason) = watcher.cycle()? {
            return Ok(Some(reason));
        }
        if !watcher.watching {
            watcher.log("nothing to watch: no open tasks, followed or hand sessions; exiting");
            return Ok(None);
        }
        if !watcher.beating && watcher.started.elapsed() >= settings.hand_max {
            watcher.log("only hand sessions watched for the hand maximum; exiting");
            return Ok(None);
        }
        if watcher.relist_due() {
            watcher.refresh_hands(lock);
        }
        cycles += 1;
        if settings.once && cycles >= ONCE_CYCLES {
            return Ok(None);
        }
        thread::sleep(settings.poll);
    }
}

/// A herdr agent on a live host, as of the cached inventory, other than the supervisor, with its
/// owner as of the listing.
struct HandSession {
    host: String,
    agent: Agent,
    listed: Owner,
}

impl HandSession {
    /// Its owner as listed, unless it was listed as observed and a task has claimed it since, as
    /// one adopted after the listing.
    fn owner(&self, tasks: &[(TaskRecord, Status)]) -> Owner {
        if self.listed == Owner::Observed {
            owner::owner(&self.host, &self.agent, None, tasks)
        } else {
            self.listed.clone()
        }
    }

    /// Whether `owner`, not the task whose pane this is, rests on evidence: another owner, a
    /// reported id, or a workspace label that was looked up. An unknown label proves nothing.
    fn shows_reuse(&self, owner: &Owner) -> bool {
        *owner != Owner::Observed
            || self.agent.agent_session_id.is_some()
            || self.agent.workspace_label.is_some()
    }

    fn is_in(&self, host: &str, session: &str, pane_id: &str) -> bool {
        self.host == host && self.agent.session == session && self.agent.pane_id == pane_id
    }
}

/// The agents in the cached inventory other than the supervisor, owned as of one reading of the
/// lock and the tasks. Unreadable tasks own nothing here; `HandSession::owner` reads them again.
fn hand_sessions(state: &StateDir) -> anyhow::Result<Vec<HandSession>> {
    let lock = supervisor::load(state)?;
    let tasks = Store::open(state).list_tasks().unwrap_or_default();
    let mut hands = Vec::new();
    for inv in inventory::load_all(state)?
        .into_iter()
        .filter(|inv| !inv.gone)
    {
        for agent in inv.agents {
            match owner::owner(&inv.name, &agent, lock.as_ref(), &tasks) {
                Owner::Supervisor => {}
                listed => hands.push(HandSession {
                    host: inv.name.clone(),
                    agent,
                    listed,
                }),
            }
        }
    }
    Ok(hands)
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
        if is_lost(&err) {
            append_log(state, &format!("{err:#}; exiting"));
            return None;
        }
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
        if let Err(lost) = sleep_holding(lock, delay) {
            append_log(state, &format!("{lost:#}; exiting"));
            return None;
        }
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

/// Sleeps for `delay`, touching `lock` at least once a minute so it does not go stale, and
/// fails with why as soon as the lock was removed or taken over. Any other failed touch is left
/// for the next attempt's own touch to report.
fn sleep_holding(lock: &Guard, delay: Duration) -> anyhow::Result<()> {
    const TOUCH_EVERY: Duration = Duration::from_secs(60);
    let end = Instant::now() + delay;
    loop {
        if let Err(err) = lock.touch()
            && is_lost(&err)
        {
            return Err(err);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        thread::sleep(left.min(TOUCH_EVERY));
    }
}

/// Whether `err` says the run's lock was removed or taken over, as `flow up` does, so the run is
/// to end without a word to the supervisor.
fn is_lost(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| cause.is::<lock::Lost>())
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
    /// The hand sessions as of the last listing.
    hands: Vec<HandSession>,
    /// Whether the last cycle found anything to watch: an open task, a followed or hand
    /// session, or a task list or follow list it could not read.
    watching: bool,
    /// Whether the heartbeat fires: the last cycle found something to watch besides hand
    /// sessions.
    beating: bool,
    /// When `beating` last turned true; the heartbeat is timed from it.
    beating_since: Option<Instant>,
    /// When the hosts' agents were last listed, or the run started.
    listed_at: Instant,
    /// The hosts whose listing thread has not returned yet.
    listing: Arc<Mutex<HashSet<String>>>,
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
        let memory = load_watch_state(state)?;
        let hands = hand_sessions(state)?;
        Ok(Watcher {
            state,
            store: Store::open(state),
            hosts: inventory::hosts(state)?,
            hands,
            runner,
            settings: settings.clone(),
            saved: memory.clone(),
            memory,
            watching: true,
            beating: true,
            beating_since: None,
            listed_at: Instant::now(),
            listing: Arc::default(),
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
        let tasks = self.store.list_tasks().unwrap_or_else(|err| {
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
        if !self.beating {
            self.beating_since = None;
        } else if self.beating_since.is_none() {
            self.beating_since = Some(Instant::now());
        }
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
        if self
            .beating_since
            .is_some_and(|since| since.elapsed() >= self.settings.heartbeat)
        {
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

    /// Whether a run watching only hand sessions is due to list the hosts again: a heartbeat
    /// interval has passed since the last listing.
    fn relist_due(&self) -> bool {
        self.watching && !self.beating && self.listed_at.elapsed() >= self.settings.heartbeat
    }

    /// Lists every live host's agents into the cached inventory and takes the hand sessions
    /// from it, leaving out the hosts `list_agents` says not to watch.
    fn refresh_hands(&mut self, lock: &Guard) {
        let skipped = self.list_agents(lock);
        self.listed_at = Instant::now();
        match hand_sessions(self.state) {
            Ok(hands) => {
                self.hands = hands
                    .into_iter()
                    .filter(|hand| !skipped.contains(&hand.host))
                    .collect();
            }
            Err(err) => self.log(&format!("{err:#}")),
        }
    }

    /// Refreshes every live host's cached agents with one herdr listing each, the way
    /// `flow sessions` does but never starting a herdr server, and returns the hosts whose hand
    /// sessions are not to be watched: those that ssh could not reach, that run no herdr server,
    /// whose listing did not end within `Settings::list_timeout`, or whose listing from an
    /// earlier call has not returned, which are not asked again. Hosts are listed in parallel,
    /// each on a thread left to finish on its own once the timeout passes; `lock` is touched
    /// before and as each listing ends. A host whose listing fails keeps its cached agents;
    /// failures are logged and never end the run.
    fn list_agents(&mut self, lock: &Guard) -> Vec<String> {
        let state = self.state;
        let found = Config::load(state).and_then(|config| {
            let sessions = config.listed_sessions(&supervisor::herdr_session(state)?);
            Ok((sessions, inventory::live_hosts(state)?))
        });
        let (sessions, hosts) = match found {
            Ok(found) => found,
            Err(err) => {
                append_log(state, &format!("listing agents: {err:#}"));
                return Vec::new();
            }
        };
        let _ = lock.touch();
        let timeout = self.settings.list_timeout;
        let now = Timestamp::now();
        let (sender, receiver) = mpsc::channel();
        let mut skipped = Vec::new();
        let mut results: Vec<Option<anyhow::Result<inventory::Listed>>> =
            hosts.iter().map(|_| None).collect();
        for (index, host) in hosts.iter().enumerate() {
            let started = self
                .listing
                .lock()
                .expect("listing set lock")
                .insert(host.name.clone());
            if !started {
                skipped.push(host.name.clone());
                self.log(&format!(
                    "listing agents on {}: an earlier listing has not returned; skipping it",
                    host.name
                ));
                continue;
            }
            let sender = sender.clone();
            let state = state.clone();
            let runner = self.runner.clone();
            let sessions = sessions.clone();
            let host = host.clone();
            let listing = self.listing.clone();
            thread::spawn(move || {
                let listed = inventory::list_agents(
                    &state,
                    &runner,
                    &sessions,
                    FlowServer::LeaveStopped,
                    &host,
                    now,
                );
                listing.lock().expect("listing set lock").remove(&host.name);
                let _ = sender.send((index, listed));
            });
        }
        drop(sender);
        let deadline = Instant::now() + timeout;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok((index, listed)) = receiver.recv_timeout(left) else {
                break;
            };
            let _ = lock.touch();
            results[index] = Some(listed);
        }
        for (host, listed) in hosts.iter().zip(results) {
            if skipped.contains(&host.name) {
                continue;
            }
            let not_watched = "; not watching its sessions until the next listing";
            let problem = match listed {
                None => {
                    skipped.push(host.name.clone());
                    Some(format!(
                        "timed out after {}s{not_watched}",
                        timeout.as_secs_f64()
                    ))
                }
                Some(Ok(listed)) if listed.no_server || listed.unreachable => {
                    skipped.push(host.name.clone());
                    let problem = listed.error.as_deref().unwrap_or("no herdr server runs");
                    Some(format!("{problem}{not_watched}"))
                }
                Some(Ok(listed)) => listed.error,
                Some(Err(err)) => Some(format!("{err:#}")),
            };
            if let Some(problem) = problem {
                append_log(
                    state,
                    &format!("listing agents on {}: {problem}", host.name),
                );
            }
        }
        skipped
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
    /// read for the first time is replayed from `pending` and an empty event log; a closed task
    /// whose closing was read is passed over. New events drop the task's pane report, so its next
    /// wait ends on the status it holds, as after a steer. An open task's `target.retired` event
    /// reads as its agent gone.
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
            let mut seen = *self.memory.seen.entry(rec.id.clone()).or_insert(Seen {
                state: State::Pending,
                events: 0,
            });
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
                    if !lines.is_empty() {
                        self.memory.reported.agents_reported.remove(&rec.id);
                    }
                    for (kind, end) in lines {
                        seen.events = end;
                        let task = rec.id.clone();
                        let obs = if kind == owner::TARGET_RETIRED && status.state.is_open() {
                            Observation::AgentGone { task: task.clone() }
                        } else {
                            Observation::EventAppended {
                                task: task.clone(),
                                kind,
                            }
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

    /// Keeps one wait per open task's target unless an agent listed there is shown to be another's
    /// (see `HandSession::shows_reuse`), and one
    /// per observed or followed pane no task waits on. A wait whose spec changed, as on following
    /// or unfollowing, starts afresh.
    fn reconcile_waits(&mut self, tasks: &[(TaskRecord, Status)], follow: &[Followed]) {
        let owned: Vec<(&HandSession, Owner)> = self
            .hands
            .iter()
            .map(|hand| (hand, hand.owner(tasks)))
            .collect();
        let mut wanted: BTreeMap<String, WaitSpec> = BTreeMap::new();
        let mut reused = Vec::new();
        for (rec, status) in tasks {
            let Some(target) = status.herdr.as_ref().filter(|_| status.state.is_open()) else {
                continue;
            };
            let task = Owner::Task(rec.id.clone());
            if owned.iter().any(|(hand, owner)| {
                hand.is_in(&target.host, &target.session, &target.pane_id)
                    && *owner != task
                    && hand.shows_reuse(owner)
            }) {
                reused.push(format!(
                    "pane {} on {} (herdr session {}) no longer runs task {}'s worker: herdr \
                     reused the pane id, so flow no longer watches it as the task's",
                    target.pane_id, target.host, target.session, rec.id
                ));
                continue;
            }
            wanted.insert(
                rec.id.clone(),
                WaitSpec {
                    target: WaitTarget::Task {
                        id: rec.id.clone(),
                        adopted: rec.origin == Origin::Adopted,
                    },
                    host: target.host.clone(),
                    session: target.session.clone(),
                    pane_id: target.pane_id.clone(),
                },
            );
        }
        let claimed: Vec<(String, String, String)> = wanted
            .values()
            .map(|spec| {
                (
                    spec.host.clone(),
                    spec.session.clone(),
                    spec.pane_id.clone(),
                )
            })
            .collect();
        let claimed = |host: &str, session: &str, pane_id: &str| {
            claimed
                .iter()
                .any(|(h, s, p)| h == host && s == session && p == pane_id)
        };
        for (hand, owner) in &owned {
            let agent = &hand.agent;
            if *owner != Owner::Observed || claimed(&hand.host, &agent.session, &agent.pane_id) {
                continue;
            }
            let followed = follow
                .iter()
                .any(|followed| followed.is(&hand.host, &agent.session, &agent.pane_id));
            let spec = WaitSpec {
                target: WaitTarget::Observed {
                    followed,
                    title: agent.title.clone(),
                },
                host: hand.host.clone(),
                session: agent.session.clone(),
                pane_id: agent.pane_id.clone(),
            };
            wanted.insert(spec.key(), spec);
        }
        for followed in follow {
            let held = owned.iter().any(|(hand, owner)| {
                hand.is_in(&followed.host, &followed.session, &followed.pane_id)
                    && *owner != Owner::Observed
            });
            if held || claimed(&followed.host, &followed.session, &followed.pane_id) {
                continue;
            }
            let spec = WaitSpec {
                target: WaitTarget::Observed {
                    followed: true,
                    title: String::new(),
                },
                host: followed.host.clone(),
                session: followed.session.clone(),
                pane_id: followed.pane_id.clone(),
            };
            wanted.entry(spec.key()).or_insert(spec);
        }
        for message in reused {
            self.log(&message);
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
            let reported = self.memory.reported.status(&key);
            let wait = PaneWait::new(spec, host.clone(), herdr, reported);
            self.waits.insert(key, wait);
        }
    }

    /// Starts the waits not running and reads the ones that ended. A status not worth waking
    /// for, or an unreachable host seen only by an unfollowed hand session's wait, is logged
    /// rather than observed.
    fn poll_waits(&mut self) -> Vec<(Observation, Progress)> {
        let timeout_ms = u64::try_from(self.settings.heartbeat.as_millis()).unwrap_or(u64::MAX);
        let mut observed = Vec::new();
        let mut notes = Vec::new();
        let mut turn_ends = Vec::new();
        for wait in self.waits.values_mut() {
            let reported = &mut self.memory.reported;
            let obs = match wait.poll(timeout_ms) {
                Polled::Running => continue,
                Polled::Reached(status) => {
                    reported.reachable(&wait.host.name);
                    reported.agent_seen(&wait.spec.key(), status);
                    if wait.turn_ended() {
                        turn_ends.push(wait.spec.key());
                    }
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
                    if matches!(wait.spec.target, WaitTarget::Observed { .. }) {
                        reported.agent_seen(&wait.spec.key(), herdr::AgentStatus::Unknown);
                    }
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
        for task in turn_ends {
            self.log_turn_end(&task);
        }
        observed
    }

    /// Appends a `turn.end` event to an adopted task whose agent went from `working` to `idle`,
    /// as a dispatched worker's Stop hook would, so its quiet clock starts from the turn's end.
    fn log_turn_end(&mut self, task: &str) {
        let ev = Event {
            ts: Timestamp::now(),
            source: EVENT_SOURCE.to_owned(),
            kind: TURN_END.to_owned(),
            data: serde_json::json!({ "status": herdr::AgentStatus::Idle.as_str() }),
        };
        if let Err(err) = self.store.append_event(task, &ev) {
            self.log(&format!("{task}: logging the turn end: {err:#}"));
        }
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

    /// Writes what changed since the last save.
    fn save(&mut self) -> anyhow::Result<()> {
        if self.memory == self.saved {
            return Ok(());
        }
        write_watch_state(self.state, &self.memory)?;
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

/// The state the last run saved. With none, or one that does not parse, which is logged, it
/// writes and returns `seeded_state`; when the tasks cannot be read for that, an empty state.
fn load_watch_state(state: &StateDir) -> anyhow::Result<WatchState> {
    let path = state.root().join(WATCH_STATE_FILE);
    let reseed = || match seeded_state(state) {
        Ok(memory) => {
            write_watch_state(state, &memory)?;
            Ok(memory)
        }
        Err(err) => {
            append_log(state, &format!("seeding {}: {err:#}", path.display()));
            Ok(WatchState::default())
        }
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return reseed(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_slice(&bytes).or_else(|err| {
        let problem = format!(
            "{} did not parse ({err}); seeding it afresh",
            path.display()
        );
        append_log(state, &problem);
        reseed()
    })
}

/// A watch state with every closed task read as it is now; open tasks are left unread, so a
/// run replays their logs.
fn seeded_state(state: &StateDir) -> anyhow::Result<WatchState> {
    let store = Store::open(state);
    let mut memory = WatchState::default();
    for (rec, status) in store.list_tasks()? {
        if status.state.is_open() {
            continue;
        }
        let seen = Seen {
            state: status.state,
            events: file_len(&store.task_dir(&rec.id)?.join(EVENTS_FILE)),
        };
        memory.seen.insert(rec.id, seen);
    }
    Ok(memory)
}

/// Writes `seeded_state` to `watch-state.json` unless the file exists. `flow up` calls it when
/// it starts a supervisor.
pub fn seed(state: &StateDir) -> anyhow::Result<()> {
    if state.root().join(WATCH_STATE_FILE).exists() {
        return Ok(());
    }
    let supervisor = state.root().join(SUPERVISOR_DIR);
    std::fs::create_dir_all(&supervisor)
        .with_context(|| format!("creating {}", supervisor.display()))?;
    write_watch_state(state, &seeded_state(state)?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::inventory::HostInventory;
    use crate::runner::{FakeRunner, Scripted};
    use crate::state::events::Event;
    use crate::state::task::{ClaudeSession, Harness, HerdrTarget};
    use classify::DEFAULT_QUIET_SECS;

    const CONFIG: &str = r#"
herdr_sessions = ["flow", "default"]

[[repos]]
name = "example-repo"
path = "/work/repo"
default_branch = "main"
"#;
    const TASK: &str = "aaaaaaab";
    /// The workspace label dispatch gives `TASK`'s workspace.
    const TASK_LABEL: &str = "Build feature · aaaaaaab";
    const IDLE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"idle","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const BLOCKED: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"blocked","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const WORKING: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"working","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const DONE: &str = r#"{"id":"cli:agent:wait","result":{"agent":{"agent":"claude","agent_status":"done","pane_id":"w1:p1","workspace_id":"w1"},"type":"agent_info"}}"#;
    const NOT_FOUND: &str = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:wait"}"#;
    const NO_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
    const DEFAULT_AGENTS: &str = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"idle","cwd":"/work/repo","pane_id":"w1:p1","terminal_title":"✳ Tidy docs","terminal_title_stripped":"Tidy docs","workspace_id":"w1"}],"type":"agent_list"}}"#;
    const TIDY_DOCS_BLOCKED: &str = r#"observed-blocked: host-a w1:p1 "Tidy docs""#;

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
                origin: Origin::Dispatched,
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
                        session: "flow".to_owned(),
                    });
                })
                .unwrap();
        }

        /// The working task `TASK` adopted from the hand session in `default`'s pane `w1:p1` on
        /// `host-a`.
        fn adopted_task(&self) {
            let store = Store::open(&self.state);
            let rec = TaskRecord {
                id: TASK.to_owned(),
                parent: None,
                title: "Tidy docs".to_owned(),
                host: "host-a".to_owned(),
                cwd: PathBuf::from("/work/repo"),
                repo: None,
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
                origin: Origin::Adopted,
            };
            store.create_task(rec, "brief").unwrap();
            store
                .set_status(TASK, |status| {
                    status.state = State::Working;
                    status.herdr = Some(HerdrTarget {
                        host: "host-a".to_owned(),
                        workspace_id: "w1".to_owned(),
                        pane_id: "w1:p1".to_owned(),
                        agent: "claude".to_owned(),
                        session: "default".to_owned(),
                    });
                })
                .unwrap();
        }

        fn events(&self) -> Vec<Event> {
            crate::state::events::read_all(&Store::open(&self.state).task_dir(TASK).unwrap())
                .unwrap()
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
                workspace_label: None,
            });
            inventory::save(&self.state, &inv).unwrap();
        }

        /// Records the cached agent in pane `pane_id` on `env_id` under workspace label `label`,
        /// as a listing that needed it would.
        fn labelled(&self, env_id: &str, pane_id: &str, label: &str) {
            let mut inv = inventory::load(&self.state, env_id).unwrap().unwrap();
            for agent in inv.agents.iter_mut().filter(|a| a.pane_id == pane_id) {
                agent.workspace_label = Some(label.to_owned());
            }
            inventory::save(&self.state, &inv).unwrap();
        }

        /// Records that herdr reports Claude Code session `id` for the cached agent in pane
        /// `pane_id` of `env_id`.
        fn reports_session(&self, env_id: &str, pane_id: &str, id: &str) {
            let mut inv = inventory::load(&self.state, env_id).unwrap().unwrap();
            let agent = inv
                .agents
                .iter_mut()
                .find(|agent| agent.pane_id == pane_id)
                .unwrap();
            agent.agent_session_id = Some(id.to_owned());
            inventory::save(&self.state, &inv).unwrap();
        }

        /// Adds the supervisor's agent, reporting the Claude Code session `s-sup` that
        /// `supervisor_in` records, in `session`'s pane `pane_id` on `host-a` to the cached
        /// inventory.
        fn supervisor_agent(&self, session: &str, pane_id: &str) {
            self.agent("env-0001", session, pane_id, "flow-supervisor");
            self.reports_session("env-0001", pane_id, "s-sup");
        }

        /// Records Claude Code session `id` as task `TASK`'s.
        fn task_session(&self, id: &str) {
            Store::open(&self.state)
                .set_status(TASK, |status| {
                    status.claude_session = Some(ClaudeSession {
                        session_id: id.to_owned(),
                        transcript_path: PathBuf::from("/home/dev/.claude/projects/x/y.jsonl"),
                    });
                })
                .unwrap();
        }

        /// A hand session titled "Tidy docs" in herdr's `default` session, pane `w1:p1` on
        /// `host-a`.
        fn hand(&self) {
            self.agent("env-0001", "default", "w1:p1", "Tidy docs");
        }

        /// Records the supervisor as running in `session`'s pane `pane_id` on `host-a`, with
        /// Claude Code session `s-sup`.
        fn supervisor_in(&self, session: &str, pane_id: &str) {
            let target = HerdrTarget {
                host: "host-a".to_owned(),
                workspace_id: "w1".to_owned(),
                pane_id: pane_id.to_owned(),
                agent: "claude".to_owned(),
                session: session.to_owned(),
            };
            supervisor::take(&self.state, "env-0001", target).unwrap();
            supervisor::set_session(&self.state, Some("s-sup")).unwrap();
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
                    title: None,
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
            hand_max: Duration::from_secs(86_400),
            list_timeout: Duration::from_secs(60),
        }
    }

    /// A runner that answers like `inner`, except that any `run` on `slow_host` takes `delay`
    /// and then fails as unreachable, so the listing it holds up writes nothing once it ends.
    struct SlowRunner {
        inner: Arc<FakeRunner>,
        slow_host: &'static str,
        delay: Duration,
        slow_calls: std::sync::atomic::AtomicUsize,
    }

    impl SlowRunner {
        fn new(inner: Arc<FakeRunner>, slow_host: &'static str, delay: Duration) -> SlowRunner {
            SlowRunner {
                inner,
                slow_host,
                delay,
                slow_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn slow_calls(&self) -> usize {
            self.slow_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Runner for SlowRunner {
        fn run(
            &self,
            host: &Host,
            argv: &[String],
            stdin: Option<&[u8]>,
        ) -> anyhow::Result<crate::runner::Output> {
            if host.name == self.slow_host {
                self.slow_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                thread::sleep(self.delay);
                anyhow::bail!("host unreachable: {} (test delay)", host.name);
            }
            self.inner.run(host, argv, stdin)
        }

        fn spawn(&self, host: &Host, argv: &[String]) -> anyhow::Result<std::process::Child> {
            self.inner.spawn(host, argv)
        }
    }

    fn hand_listing(default_agents: &str) -> Vec<Scripted> {
        vec![
            Scripted {
                repeat: true,
                ..on("host-a", &["--session", "flow", "agent", "list"], NO_AGENTS)
            },
            Scripted {
                repeat: true,
                ..on(
                    "host-a",
                    &["--session", "default", "agent", "list"],
                    default_agents,
                )
            },
        ]
    }

    fn no_server(host: &str, session: &str) -> Scripted {
        Scripted {
            status: 1,
            stderr: include_str!("../../tests/fixtures/herdr/agent_list_server_not_running.json")
                .to_owned(),
            ..on(host, &["--session", session, "agent", "list"], "")
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
                    hand.host, hand.agent.session, hand.agent.pane_id, hand.agent.title
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
        let runner = fake(vec![answer(0, IDLE), answer(0, WORKING)]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut first).as_deref(),
            Some(r#"observed-idle: host-a w1:p1 "w1:p1""#)
        );
        drop(first);

        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut second), None);
        assert_eq!(
            wait_untils(&runner),
            [
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
                vec!["idle", "working", "done"],
                vec!["idle", "blocked", "done"],
            ]
        );
        assert!(runner.calls()[0].argv.contains(&"default".to_owned()));
    }

    fn task_workspaces(label: &str) -> String {
        format!(
            r#"{{"id":"cli:workspace:list","result":{{"type":"workspace_list","workspaces":[{{"workspace_id":"w1","label":"{label}"}}]}}}}"#
        )
    }

    /// Listings of host-a in which `TASK`'s pane in `flow` holds an agent herdr reports no id
    /// for, with `workspaces` answering its workspace list, and waits that see it working.
    fn listing_idless_in_task_pane(workspaces: Scripted) -> Arc<FakeRunner> {
        fake(vec![
            Scripted {
                repeat: true,
                ..on(
                    "host-a",
                    &["--session", "flow", "agent", "list"],
                    DEFAULT_AGENTS,
                )
            },
            Scripted {
                repeat: true,
                ..workspaces
            },
            Scripted {
                repeat: true,
                ..on(
                    "host-a",
                    &["--session", "default", "agent", "list"],
                    NO_AGENTS,
                )
            },
            Scripted {
                repeat: true,
                ..on("host-a", &["agent", "wait"], WORKING)
            },
        ])
    }

    #[test]
    fn an_open_task_whose_target_a_listing_retires_wakes_once() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = listing_idless_in_task_pane(on(
            "host-a",
            &["--session", "flow", "workspace", "list"],
            &task_workspaces("Notes"),
        ));
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let once = Settings {
            once: true,
            list_agents: true,
            ..settings(Duration::from_secs(3600))
        };

        assert_eq!(
            run(&fx.state, runner.clone(), &once, &lock)
                .unwrap()
                .as_deref(),
            Some("agent-gone: aaaaaaab")
        );
        assert_eq!(Store::open(&fx.state).task(TASK).unwrap().1.herdr, None);
        assert_eq!(run(&fx.state, runner, &once, &lock).unwrap(), None);
    }

    #[test]
    fn a_failed_label_listing_keeps_the_tasks_wait() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = listing_idless_in_task_pane(Scripted {
            status: 2,
            stderr: "workspace list failed".to_owned(),
            ..on("host-a", &["--session", "flow", "workspace", "list"], "")
        });
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let listing = Settings {
            list_agents: true,
            ..settings(Duration::from_secs(3600))
        };
        let mut watcher = Watcher::new(&fx.state, runner, &listing).unwrap();
        watcher.refresh_hands(&lock);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
        assert!(Store::open(&fx.state).task(TASK).unwrap().1.herdr.is_some());
        assert!(!fx.log().contains("reused"), "{}", fx.log());
    }

    #[test]
    fn a_dispatched_tasks_pane_under_another_label_is_not_the_tasks_wait() {
        let fx = Fixture::new();
        fx.hand();
        fx.labelled("env-0001", "w1:p1", "My notes");
        fx.task(true);
        Store::open(&fx.state)
            .set_status(TASK, |status| {
                if let Some(target) = &mut status.herdr {
                    target.session = "default".to_owned();
                }
            })
            .unwrap();
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(
            watcher.waits.keys().collect::<Vec<_>>(),
            ["host-a/default/w1:p1"]
        );
    }

    #[test]
    fn an_adopted_pane_is_watched_as_its_task_only() {
        let fx = Fixture::new();
        fx.hand();
        fx.labelled("env-0001", "w1:p1", TASK_LABEL);
        fx.follow();
        fx.task(true);
        Store::open(&fx.state)
            .set_status(TASK, |status| {
                if let Some(target) = &mut status.herdr {
                    target.session = "default".to_owned();
                }
            })
            .unwrap();
        let runner = fake(vec![answer(0, BLOCKED)]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(
            two_cycles(&mut watcher).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(
            calls[0]
                .argv
                .windows(2)
                .any(|pair| pair == ["--session", "default"]),
            "{calls:?}"
        );
    }

    #[test]
    fn a_done_adopted_pane_still_targeted_is_not_observed() {
        let fx = Fixture::new();
        fx.hand();
        fx.reports_session("env-0001", "w1:p1", "s-hand");
        fx.adopted_task();
        fx.task_session("s-hand");
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let runner = fake(Vec::new());
        seed(&fx.state).unwrap();
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert!(watcher.waits.is_empty());
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn a_done_tasks_pane_reused_by_a_hand_session_is_observed() {
        let fx = Fixture::new();
        fx.hand();
        fx.reports_session("env-0001", "w1:p1", "s-hand");
        fx.adopted_task();
        fx.task_session("s-worker");
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        seed(&fx.state).unwrap();
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(
            watcher.waits.keys().collect::<Vec<_>>(),
            ["host-a/default/w1:p1"]
        );
    }

    #[test]
    fn an_open_tasks_pane_reused_by_a_hand_session_is_observed_and_logged() {
        let fx = Fixture::new();
        fx.hand();
        fx.reports_session("env-0001", "w1:p1", "s-hand");
        fx.adopted_task();
        fx.task_session("s-worker");
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);
        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(
            watcher.waits.keys().collect::<Vec<_>>(),
            ["host-a/default/w1:p1"]
        );
        let reused = "pane w1:p1 on host-a (herdr session default) no longer runs task \
                      aaaaaaab's worker: herdr reused the pane id, so flow no longer watches it as \
                      the task's";
        assert_eq!(fx.log().matches(reused).count(), 1, "{}", fx.log());
    }

    #[test]
    fn a_worker_whose_session_changed_since_the_listing_keeps_its_wait() {
        let fx = Fixture::new();
        fx.hand();
        fx.reports_session("env-0001", "w1:p1", "s-old");
        fx.adopted_task();
        fx.task_session("s-old");
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);

        fx.task_session("s-new");
        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
        assert!(!fx.log().contains("reused"), "{}", fx.log());
    }

    #[test]
    fn a_worker_listed_before_its_session_was_recorded_keeps_its_wait() {
        let fx = Fixture::new();
        fx.hand();
        fx.adopted_task();
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);

        fx.task_session("s-worker");
        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
    }

    #[test]
    fn a_worker_herdr_reports_no_id_for_keeps_its_wait_after_its_session_is_recorded() {
        let fx = Fixture::new();
        fx.hand();
        fx.adopted_task();
        fx.task_session("s-worker");
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
        assert!(!fx.log().contains("reused"), "{}", fx.log());
    }

    #[test]
    fn an_open_tasks_pane_running_its_session_is_the_tasks_wait_only() {
        let fx = Fixture::new();
        fx.hand();
        fx.reports_session("env-0001", "w1:p1", "s-worker");
        fx.adopted_task();
        fx.task_session("s-worker");
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(watcher.waits.keys().collect::<Vec<_>>(), [TASK]);
        assert!(!fx.log().contains("reused"), "{}", fx.log());
    }

    #[test]
    fn an_agent_with_no_session_in_the_supervisors_pane_is_the_supervisor_under_any_label() {
        let fx = Fixture::new();
        fx.agent("env-0001", "default", "w1:p1", "flow-supervisor");
        fx.labelled("env-0001", "w1:p1", "Notes");
        fx.supervisor_in("default", "w1:p1");
        let runner = fake(Vec::new());

        let watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert!(watcher.hands.is_empty());
    }

    #[test]
    fn another_session_in_the_supervisors_pane_is_a_hand_session() {
        let fx = Fixture::new();
        fx.agent("env-0001", "default", "w1:p1", "Tidy docs");
        fx.supervisor_in("default", "w1:p1");
        fx.reports_session("env-0001", "w1:p1", "s-hand");
        let runner = fake(Vec::new());

        let watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        let hands: Vec<&str> = watcher
            .hands
            .iter()
            .map(|h| h.agent.pane_id.as_str())
            .collect();
        assert_eq!(hands, ["w1:p1"]);
    }

    #[test]
    fn an_open_tasks_worker_in_the_supervisors_pane_is_not_taken_for_the_supervisor() {
        let fx = Fixture::new();
        fx.supervisor_agent("default", "w1:p1");
        fx.supervisor_in("default", "w1:p1");
        fx.reports_session("env-0001", "w1:p1", "s-worker");
        fx.task(false);
        fx.task_session("s-worker");
        let runner = fake(Vec::new());

        let watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        let hands: Vec<&str> = watcher
            .hands
            .iter()
            .map(|h| h.agent.pane_id.as_str())
            .collect();
        assert_eq!(hands, ["w1:p1"]);
    }

    #[test]
    fn a_worker_the_supervisor_and_a_hand_session_share_one_herdr_session() {
        let fx = Fixture::new();
        fx.supervisor_agent("default", "w1:p1");
        fx.supervisor_in("default", "w1:p1");
        fx.agent("env-0001", "default", "w2:p1", "aaaaaaab");
        fx.labelled("env-0001", "w2:p1", TASK_LABEL);
        fx.task(true);
        Store::open(&fx.state)
            .set_status(TASK, |status| {
                if let Some(target) = &mut status.herdr {
                    target.session = "default".to_owned();
                    target.pane_id = "w2:p1".to_owned();
                }
            })
            .unwrap();
        fx.agent("env-0001", "default", "w3:p1", "Tidy docs");
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(
            watcher.waits.keys().collect::<Vec<_>>(),
            ["aaaaaaab", "host-a/default/w3:p1"]
        );
        let waited: Vec<String> = runner
            .calls()
            .into_iter()
            .filter_map(|call| {
                let at = call.argv.iter().position(|word| word == "wait")?;
                call.argv.get(at + 1).cloned()
            })
            .collect();
        assert_eq!(waited, ["w2:p1", "w3:p1"]);
    }

    #[test]
    fn a_flow_session_task_does_not_hide_a_hand_session_in_the_same_pane_id() {
        let fx = Fixture::new();
        fx.hand();
        fx.task(true);
        let runner = fake(vec![Scripted {
            repeat: true,
            ..on("host-a", &["agent", "wait"], WORKING)
        }]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(watcher.cycle().unwrap(), None);

        assert_eq!(
            watcher.waits.keys().collect::<Vec<_>>(),
            ["aaaaaaab", "host-a/default/w1:p1"]
        );
    }

    fn waits_answer(statuses: &[&str]) -> Vec<Scripted> {
        statuses
            .iter()
            .map(|stdout| on("host-a", &["agent", "wait"], stdout))
            .collect()
    }

    #[test]
    fn an_adopted_task_going_idle_after_working_logs_a_turn_end_without_waking() {
        let fx = Fixture::new();
        fx.adopted_task();
        let runner = fake(waits_answer(&[WORKING, IDLE, WORKING]));
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(cycles(&mut watcher, 4), None);

        let turn_ends: Vec<Event> = fx
            .events()
            .into_iter()
            .filter(|ev| ev.kind == "turn.end")
            .collect();
        assert_eq!(turn_ends.len(), 1, "{turn_ends:?}");
        assert_eq!(turn_ends[0].source, "watch");
        assert_eq!(turn_ends[0].data, serde_json::json!({"status": "idle"}));
        assert_eq!(
            wait_untils(&runner),
            [
                vec!["working", "blocked", "done"],
                vec!["idle", "blocked", "done"]
            ]
        );
        assert_eq!(cycles(&mut watcher, 1), None);
        assert!(
            fx.log()
                .lines()
                .any(|line| line.contains("absorb") && line.contains(r#""kind":"turn.end""#)),
            "{}",
            fx.log()
        );
    }

    #[test]
    fn an_adopted_task_first_read_idle_logs_nothing() {
        let fx = Fixture::new();
        fx.adopted_task();
        let runner = fake(waits_answer(&[IDLE]));
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(two_cycles(&mut watcher), None);

        assert!(!fx.events().iter().any(|ev| ev.kind == "turn.end"));
    }

    #[test]
    fn an_adopted_task_quiet_after_its_turn_ended_wakes() {
        let fx = Fixture::new();
        fx.adopted_task();
        let runner = fake(waits_answer(&[WORKING, IDLE]));
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(cycles(&mut first, 4), None);
        drop(first);
        let turn_end = fx
            .events()
            .into_iter()
            .find(|ev| ev.kind == "turn.end")
            .unwrap();

        let mut script = waits_answer(&[IDLE]);
        script.push(on("host-a", &["find"], ""));
        let runner = fake(script);
        let mut second = fx.watcher(&runner, 0);

        let reason = two_cycles(&mut second).unwrap();
        assert!(reason.starts_with("quiet: aaaaaaab "), "{reason}");
        let find = runner
            .calls()
            .into_iter()
            .find(|call| call.argv.contains(&"find".to_owned()))
            .unwrap();
        let since = turn_end.ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(find.argv.contains(&since), "{:?}", find.argv);
    }

    #[test]
    fn a_dispatched_task_going_idle_logs_nothing() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(waits_answer(&[WORKING, IDLE]));
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);

        assert_eq!(cycles(&mut watcher, 4), None);

        assert!(!fx.events().iter().any(|ev| ev.kind == "turn.end"));
        assert_eq!(
            wait_untils(&runner),
            [vec!["blocked", "done"], vec!["idle", "blocked", "done"]]
        );
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
                && line.contains(r#""type":"observed-changed""#)
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
            Some(r#"observed-idle: host-a w1:p1 "Tidy docs""#)
        );
        assert_eq!(wait_untils(&runner), [vec!["idle", "blocked", "done"]]);
    }

    #[test]
    fn heartbeat_is_off_while_only_hand_sessions_are_watched() {
        let fx = Fixture::new();
        fx.hand();
        let mut script = hand_listing(DEFAULT_AGENTS);
        script.push(Scripted {
            repeat: true,
            ..answer(0, WORKING)
        });
        let runner = fake(script);
        let mut watcher =
            Watcher::new(&fx.state, runner.clone(), &settings(Duration::ZERO)).unwrap();
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.watching);
        assert!(!watcher.beating);
        drop(watcher);
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let past_heartbeat = Settings {
            once: true,
            ..settings(Duration::ZERO)
        };
        assert_eq!(
            run(&fx.state, runner.clone(), &past_heartbeat, &lock).unwrap(),
            None
        );
        assert!(!fx.log().contains("exiting"), "{}", fx.log());
        let past_hand_max = Settings {
            hand_max: Duration::ZERO,
            ..settings(Duration::ZERO)
        };
        assert_eq!(
            run(&fx.state, runner.clone(), &past_hand_max, &lock).unwrap(),
            None
        );
        drop(lock);
        let log = fx.log();
        assert_eq!(
            log.matches("watching 1 hand session for blocked only; heartbeat off")
                .count(),
            3,
            "{log}"
        );
        assert_eq!(
            log.matches("only hand sessions watched for the hand maximum; exiting")
                .count(),
            1,
            "{log}"
        );
        assert!(queue::latest(&fx.state).unwrap().is_none());

        fx.task(true);
        let mut watcher =
            Watcher::new(&fx.state, runner.clone(), &settings(Duration::ZERO)).unwrap();
        assert_eq!(watcher.cycle().unwrap().as_deref(), Some("heartbeat"));
        assert!(watcher.beating);
    }

    #[test]
    fn hand_sessions_exclude_the_supervisor_and_gone_hosts() {
        let fx = Fixture::new();
        fx.supervisor_agent("flow", "w1:p1");
        fx.supervisor_in("flow", "w1:p1");
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
    fn listing_never_starts_herdr_and_skips_a_host_without_a_server() {
        let fx = Fixture::new();
        fx.hand();
        let before = std::fs::read(fx.state.root().join("hosts/env-0001.json")).unwrap();
        let runner = fake(vec![
            no_server("host-a", "flow"),
            no_server("host-a", "default"),
        ]);
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let listing = Settings {
            once: true,
            list_agents: true,
            ..settings(Duration::from_secs(3600))
        };

        assert_eq!(
            run(&fx.state, runner.clone(), &listing, &lock).unwrap(),
            None
        );

        let argvs: Vec<Vec<String>> = runner.calls().into_iter().map(|call| call.argv).collect();
        assert_eq!(
            argvs,
            [
                ["herdr", "--session", "flow", "agent", "list"].map(str::to_owned),
                ["herdr", "--session", "default", "agent", "list"].map(str::to_owned),
            ]
        );
        let after = std::fs::read(fx.state.root().join("hosts/env-0001.json")).unwrap();
        assert_eq!(after, before);
        assert!(fx.log().contains("nothing to watch"), "{}", fx.log());
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
            .filter(|call| call.host == "host-a")
            .filter(|call| call.argv.windows(2).any(|w| w == ["agent", "list"]))
            .count();
        assert_eq!(listings, 2);
        let host_b_calls = runner
            .calls()
            .iter()
            .filter(|call| call.host == "host-b")
            .count();
        assert_eq!(host_b_calls, 1, "{:?}", runner.calls());
    }

    #[test]
    fn hand_session_that_left_blocked_between_runs_wakes_when_blocked_again() {
        for between in [IDLE, WORKING] {
            let fx = Fixture::new();
            fx.hand();
            let runner = fake(vec![
                answer(0, BLOCKED),
                answer(0, between),
                answer(0, BLOCKED),
            ]);
            let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
            assert_eq!(two_cycles(&mut first).as_deref(), Some(TIDY_DOCS_BLOCKED));
            drop(first);

            let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
            assert_eq!(cycles(&mut second, 4).as_deref(), Some(TIDY_DOCS_BLOCKED));
            assert_eq!(wait_untils(&runner)[1], ["idle", "working", "done"]);
        }
    }

    #[test]
    fn hand_session_gone_while_blocked_lets_the_panes_next_occupant_wake() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![
            answer(0, BLOCKED),
            answer(1, NOT_FOUND),
            answer(0, BLOCKED),
        ]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut first).as_deref(), Some(TIDY_DOCS_BLOCKED));
        drop(first);
        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut second), None);
        assert!(second.memory.reported.agents_reported.is_empty());
        drop(second);

        let mut third = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut third).as_deref(), Some(TIDY_DOCS_BLOCKED));
        assert_eq!(
            wait_untils(&runner),
            [
                vec!["blocked"],
                vec!["idle", "working", "done"],
                vec!["blocked"],
            ]
        );
    }

    #[test]
    fn following_an_idle_hand_session_mid_run_restarts_its_wait_and_wakes_once() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![
            answer(0, WORKING),
            answer(0, IDLE),
            answer(0, WORKING),
        ]);
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);
        fx.follow();
        assert_eq!(
            cycles(&mut watcher, 2).as_deref(),
            Some(r#"observed-idle: host-a w1:p1 "Tidy docs""#)
        );
        assert_eq!(cycles(&mut watcher, 2), None);
        assert_eq!(
            wait_untils(&runner),
            [
                vec!["blocked"],
                vec!["idle", "blocked", "done"],
                vec!["working", "blocked", "done"],
            ]
        );
    }

    #[test]
    fn heartbeat_is_timed_from_when_the_run_started_beating() {
        let fx = Fixture::new();
        fx.hand();
        let runner = fake(vec![Scripted {
            repeat: true,
            ..answer(0, WORKING)
        }]);
        let heartbeat = Duration::from_millis(300);
        let mut watcher = Watcher::new(&fx.state, runner.clone(), &settings(heartbeat)).unwrap();
        assert_eq!(watcher.cycle().unwrap(), None);
        thread::sleep(heartbeat + Duration::from_millis(50));
        assert_eq!(watcher.cycle().unwrap(), None);

        fx.task(true);
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.beating);
        thread::sleep(heartbeat + Duration::from_millis(50));
        assert_eq!(watcher.cycle().unwrap().as_deref(), Some("heartbeat"));
    }

    #[test]
    fn hand_only_run_lists_hosts_again_each_heartbeat_interval() {
        let fx = Fixture::new();
        fx.hand();
        let both = r#"{"id":"cli:agent:list","result":{"agents":[{"agent":"claude","agent_status":"idle","cwd":"/work/repo","pane_id":"w1:p1","terminal_title_stripped":"Tidy docs","workspace_id":"w1"},{"agent":"claude","agent_status":"working","cwd":"/work/app","pane_id":"w1:p2","terminal_title_stripped":"Fix links","workspace_id":"w1"}],"type":"agent_list"}}"#;
        let mut script = hand_listing(both);
        script.push(Scripted {
            repeat: true,
            ..answer(0, WORKING)
        });
        let runner = fake(script);
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let mut watcher =
            Watcher::new(&fx.state, runner.clone(), &settings(Duration::ZERO)).unwrap();
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.relist_due());
        watcher.refresh_hands(&lock);
        assert_eq!(
            hand_keys(&watcher),
            [
                "host-a/default/w1:p1 Tidy docs",
                "host-a/default/w1:p2 Fix links"
            ]
        );
        assert_eq!(watcher.cycle().unwrap(), None);
        assert!(watcher.waits.contains_key("host-a/default/w1:p2"));

        fx.task(true);
        assert_eq!(watcher.cycle().unwrap().as_deref(), Some("heartbeat"));
        assert!(!watcher.relist_due());
    }

    #[test]
    fn a_hung_host_cannot_hold_up_the_listing_past_its_timeout() {
        let fx = Fixture::new();
        inventory::save(
            &fx.state,
            &HostInventory::new(&fx.host_b(), Timestamp::now()),
        )
        .unwrap();
        fx.agent("env-0002", "default", "w5:p1", "Upgrade toolchain");
        let slow = Arc::new(SlowRunner::new(
            fake(hand_listing(DEFAULT_AGENTS)),
            "host-b",
            Duration::from_secs(3),
        ));
        let lock = lock::acquire(&fx.state, std::process::id())
            .unwrap()
            .unwrap();
        let bounded = Settings {
            list_timeout: Duration::from_millis(200),
            ..settings(Duration::from_secs(3600))
        };
        let mut watcher = Watcher::new(&fx.state, slow.clone(), &bounded).unwrap();

        let started = Instant::now();
        watcher.refresh_hands(&lock);

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(hand_keys(&watcher), ["host-a/default/w1:p1 Tidy docs"]);
        assert!(
            fx.log()
                .contains("listing agents on host-b: timed out after 0.2s"),
            "{}",
            fx.log()
        );

        watcher.refresh_hands(&lock);
        watcher.refresh_hands(&lock);
        assert_eq!(slow.slow_calls(), 1);
        assert_eq!(hand_keys(&watcher), ["host-a/default/w1:p1 Tidy docs"]);
        let still = "listing agents on host-b: an earlier listing has not returned; skipping it";
        assert_eq!(fx.log().matches(still).count(), 1, "{}", fx.log());
    }

    #[test]
    fn a_status_an_earlier_run_reported_is_waited_on_for_a_change() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(vec![answer(0, BLOCKED), answer(0, IDLE)]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut first).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
        drop(first);
        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(two_cycles(&mut second), None);
        assert_eq!(
            wait_untils(&runner),
            [vec!["blocked", "done"], vec!["idle", "working", "done"]]
        );
    }

    #[test]
    fn a_worker_blocked_again_after_a_steer_between_runs_wakes_the_next_run() {
        let fx = Fixture::new();
        fx.task(true);
        let runner = fake(vec![answer(0, BLOCKED), answer(0, BLOCKED)]);
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut first).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
        drop(first);

        let steer = Event {
            ts: Timestamp::now(),
            source: "supervisor".to_owned(),
            kind: "steer".to_owned(),
            data: serde_json::json!({"text": "go on"}),
        };
        Store::open(&fx.state).append_event(TASK, &steer).unwrap();
        let mut second = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(
            two_cycles(&mut second).as_deref(),
            Some("agent-blocked: aaaaaaab")
        );
        assert_eq!(
            wait_untils(&runner),
            [vec!["blocked", "done"], vec!["blocked", "done"]]
        );
    }

    #[test]
    fn a_task_first_seen_replays_its_status() {
        let fx = Fixture::new();
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );
        fx.task(false);
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let mut first = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(first.cycle().unwrap().as_deref(), Some("done: aaaaaaab"));
        let mut next = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(next.cycle().unwrap(), None);
    }

    #[test]
    fn a_seeded_state_leaves_open_tasks_to_be_replayed() {
        let fx = Fixture::new();
        fx.task(false);
        assert!(seeded_state(&fx.state).unwrap().seen.is_empty());
        let runner = fake(Vec::new());
        let mut watcher = fx.watcher(&runner, DEFAULT_QUIET_SECS);
        assert_eq!(watcher.cycle().unwrap(), None);
        assert_eq!(watcher.memory.seen[TASK].state, State::Working);
    }

    /// Writes a watch state with every task read as it is now.
    fn mark_read(fx: &Fixture) {
        let store = Store::open(&fx.state);
        let mut memory = WatchState::default();
        for (rec, status) in store.list_tasks().unwrap() {
            let events = file_len(&store.task_dir(&rec.id).unwrap().join(EVENTS_FILE));
            let seen = Seen {
                state: status.state,
                events,
            };
            memory.seen.insert(rec.id, seen);
        }
        std::fs::create_dir_all(fx.state.root().join(SUPERVISOR_DIR)).unwrap();
        write_watch_state(&fx.state, &memory).unwrap();
    }

    #[test]
    fn a_seeded_watch_state_takes_existing_tasks_as_they_are() {
        let fx = Fixture::new();
        fx.task(false);
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        seed(&fx.state).unwrap();
        let path = fx.state.root().join(WATCH_STATE_FILE);
        let seeded = std::fs::read(&path).unwrap();
        let runner = fake(Vec::new());
        assert_eq!(
            fx.watcher(&runner, DEFAULT_QUIET_SECS).cycle().unwrap(),
            None
        );

        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Working)
            .unwrap();
        seed(&fx.state).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), seeded);
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
            mark_read(&fx);
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
    fn unparsable_watch_state_is_logged_and_seeded_afresh() {
        let fx = Fixture::new();
        fx.task(false);
        Store::open(&fx.state)
            .set_status(TASK, |status| status.state = State::Done)
            .unwrap();
        let supervisor = fx.state.root().join(SUPERVISOR_DIR);
        std::fs::create_dir_all(&supervisor).unwrap();
        let path = fx.state.root().join(WATCH_STATE_FILE);
        std::fs::write(&path, "{not json").unwrap();
        let mut watcher = fx.watcher(&fake(Vec::new()), DEFAULT_QUIET_SECS);
        assert!(fx.log().contains("did not parse"), "{}", fx.log());
        let saved: WatchState = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.seen[TASK].state, State::Done);
        assert_eq!(watcher.cycle().unwrap(), None);
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
