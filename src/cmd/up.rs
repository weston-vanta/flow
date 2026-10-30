//! `flow up`: keeps at most one live supervisor per state directory. Run from a shell pane inside
//! herdr, it focuses the supervisor the lock records on this host in this pane's herdr session,
//! or starts one here, after seeding `watch-state.json`, when none runs or its host is gone. It
//! refuses, asking no host, when the lock records a supervisor anywhere else.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, anyhow, bail};
use jiff::Timestamp;

use crate::harness::handshake::{self, AgentStart, StoppedEarly};
use crate::herdr::{self, Herdr};
use crate::host;
use crate::inventory;
use crate::paths::{STATE_ENV, StateDir};
use crate::render;
use crate::runner::{self, Runner};
use crate::state::Store;
use crate::state::config::Host;
use crate::state::task::HerdrTarget;
use crate::supervisor::{self, Lock, SUPERVISOR_CWD, SUPERVISOR_ENV, SUPERVISOR_NAME};
use crate::watch::{self, lock as watch_lock};

const AGENT_KIND: &str = "claude";
/// The supervisor's first prompt, fresh or resumed: its first turn runs the start procedure,
/// and that turn's end arms `flow watch`.
const START_PROMPT: &str = "Run the session start procedure in CLAUDE.md.";

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let Some(session) = herdr::invoking_session()? else {
        bail!("flow up runs from a shell pane inside herdr");
    };
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let live = inventory::live_hosts(state)?;
    let here = local_host(&live)?;
    let found = match supervisor::running(state)? {
        Some(lock) => recorded(state, &runner, here, &session, lock)?,
        None => None,
    };
    let herdr = Herdr::with_session(runner, &session);
    let pane_id = match found {
        Some(pane_id) => pane_id,
        None => start_here(state, &herdr, here)?,
    };
    if let Err(err) = herdr.focus_agent(here, &pane_id) {
        eprintln!("could not focus the supervisor's pane {pane_id}: {err:#}");
    }
    super::print(&format!("supervisor is in workspace {SUPERVISOR_NAME}\n"))
}

/// The pane of the running supervisor `lock` records, when it is here: on `here` in herdr session
/// `session`, where one listing finds it by its Claude Code session or its pane's label. `None`
/// when it runs nowhere there or its host is gone. Refuses for a lock anywhere else.
fn recorded(
    state: &StateDir,
    runner: &Arc<dyn Runner>,
    here: &Host,
    session: &str,
    lock: Lock,
) -> anyhow::Result<Option<String>> {
    if let Some(record) = inventory::load(state, &lock.host_env_id)?
        && record.gone
    {
        eprintln!(
            "supervisor host {} is gone; starting a new supervisor here",
            record.name
        );
        return Ok(None);
    }
    if lock.host_env_id != here.env_id || lock.herdr.session != session {
        bail!(
            "a supervisor is recorded on {} in herdr session {} (pane {}) since {} ago; run flow \
             down to stop it before starting one here",
            lock.herdr.host,
            lock.herdr.session,
            lock.herdr.pane_id,
            render::relative(lock.since, Timestamp::now())
        );
    }
    let tasks = Store::open(state).list_tasks()?;
    let Some(agent) = supervisor::find(runner, here, &lock, &[], &tasks)? else {
        return Ok(None);
    };
    eprintln!(
        "found the supervisor on {} (pane {})",
        here.name, agent.pane_id
    );
    if !lock.herdr.is(&here.name, &agent.session, &agent.pane_id) {
        supervisor::set_target(
            state,
            HerdrTarget {
                workspace_id: agent.workspace_id.clone(),
                pane_id: agent.pane_id.clone(),
                ..lock.herdr
            },
        )?;
    }
    Ok(Some(agent.pane_id))
}

/// Starts a supervisor on `here` in `herdr`'s session and returns its pane. Whatever holds the
/// watch lock outlived its supervisor, possibly on another host where it cannot be seen to be
/// dead, so the lock is cleared first.
fn start_here(state: &StateDir, herdr: &Herdr, here: &Host) -> anyhow::Result<String> {
    if watch_lock::clear(state)? {
        eprintln!("removed state/supervisor/watch.lock left by the previous supervisor");
    }
    watch::seed(state)?;
    // The launch line names the state path, since herdr hands panes the environment of
    // whoever started its server, which may hold another FLOW_STATE.
    let state_root = state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let lock = start(state, herdr, here, state_root.root())?;
    match &lock.claude_session {
        Some(session) => eprintln!(
            "resumed supervisor session {session} on {} (pane {})",
            here.name, lock.herdr.pane_id
        ),
        None => eprintln!(
            "started a fresh supervisor on {} (pane {})",
            here.name, lock.herdr.pane_id
        ),
    }
    Ok(lock.herdr.pane_id)
}

/// The host in `live` this process runs on.
fn local_host(live: &[Host]) -> anyhow::Result<&Host> {
    let local_env_id = host::local_env_id();
    live.iter()
        .find(|c| host::is_local(c, local_env_id.as_deref()))
        .ok_or_else(|| {
            anyhow!(
                "this machine is not a known host ($ONA_ID is {}); flow up starts the \
                 supervisor only on a known host; run `flow inventory --refresh` to discover \
                 hosts",
                local_env_id.as_deref().unwrap_or("unset")
            )
        })
}

/// Starts the supervisor on `here`, resuming the recorded session. When Claude Code cannot find
/// that session, it is dropped and the start retried once without it; any other failed resume
/// keeps it and leaves the workspace open for inspection.
fn start(state: &StateDir, herdr: &Herdr, here: &Host, state_root: &Path) -> anyhow::Result<Lock> {
    herdr.ensure_server(here)?;
    let (lock, started) = launch(state, herdr, here, state_root)?;
    let err = match started {
        Ok(started) => return finish(lock, started),
        Err(err) => err,
    };
    let Some(session) = &lock.claude_session else {
        return Err(err);
    };
    // `launch` stops early only on `session_missing`.
    if err.downcast_ref::<StoppedEarly>().is_none()
        && !pane_shows_session_missing(herdr, here, &lock)
    {
        return Err(err);
    }
    eprintln!("{err:#}");
    eprintln!("resume of session {session} failed; starting a fresh supervisor session");
    if let Err(err) = herdr.close_workspace(here, &lock.herdr.workspace_id) {
        eprintln!(
            "could not close workspace {} on {}: {err:#}",
            lock.herdr.workspace_id, here.name
        );
    }
    supervisor::set_session(state, None)?;
    let (lock, started) = launch(state, herdr, here, state_root)?;
    finish(lock, started?)
}

/// Whether the supervisor's pane in `lock` shows that Claude Code could not find the session to
/// resume. A pane that cannot be read does not.
fn pane_shows_session_missing(herdr: &Herdr, here: &Host, lock: &Lock) -> bool {
    match herdr.pane_read(here, &lock.herdr.pane_id, handshake::PANE_LINES) {
        Ok(text) => handshake::session_missing(&text),
        Err(err) => {
            eprintln!(
                "could not read pane {} on {}: {err:#}",
                lock.herdr.pane_id, here.name
            );
            false
        }
    }
}

/// Opens the supervisor's workspace on `here`, takes the lock for it, and launches Claude Code
/// there with the lock's session and `state_root` as its state directory. The handshake's
/// outcome comes back separately so a failed resume can be retried.
fn launch(
    state: &StateDir,
    herdr: &Herdr,
    here: &Host,
    state_root: &Path,
) -> anyhow::Result<(Lock, anyhow::Result<AgentStart>)> {
    let (workspace_id, pane_id) =
        herdr.create_workspace(here, Path::new(SUPERVISOR_CWD), SUPERVISOR_NAME)?;
    // Taken before Claude Code starts, so its SessionStart hook finds the lock to record the
    // session in.
    let lock = supervisor::take(
        state,
        &here.env_id,
        HerdrTarget {
            host: here.name.clone(),
            workspace_id: workspace_id.clone(),
            pane_id: pane_id.clone(),
            agent: AGENT_KIND.to_owned(),
            session: herdr.session().to_owned(),
        },
    )?;
    herdr.pane_run(
        here,
        &pane_id,
        &launch_argv(state_root, lock.claude_session.as_deref())?,
    )?;
    let since = Instant::now();
    // A resume Claude Code cannot find exits at once, which this stops on instead of waiting
    // out the start timeout.
    let started = handshake::await_start(
        herdr,
        here,
        &workspace_id,
        &pane_id,
        Some(handshake::session_missing),
    )
    .and_then(|started| {
        if let AgentStart::Running = started {
            record_session(state, herdr, here, &pane_id, since)?;
        }
        Ok(started)
    });
    Ok((lock, started))
}

/// Records the Claude Code session herdr reports for the new supervisor in `pane_id` in the
/// lock, once it does, so the supervisor is known by its session from the start.
fn record_session(
    state: &StateDir,
    herdr: &Herdr,
    here: &Host,
    pane_id: &str,
    since: Instant,
) -> anyhow::Result<()> {
    match handshake::await_session(herdr, here, pane_id, since)? {
        Some(session) => {
            supervisor::set_session(state, Some(&session))?;
        }
        None => eprintln!(
            "herdr reports no Claude Code session for the supervisor in pane {pane_id} on {}; \
             until it does, flow knows the supervisor only by that pane and its {SUPERVISOR_NAME} \
             label, so check its Claude Code integration (`herdr integration install claude`)",
            here.name
        ),
    }
    Ok(())
}

fn finish(lock: Lock, started: AgentStart) -> anyhow::Result<Lock> {
    if let AgentStart::BlockedAtStart { .. } = started {
        eprintln!(
            "supervisor in pane {} is blocked on a prompt other than the trust dialog; \
             answer it in its pane",
            lock.herdr.pane_id
        );
    }
    Ok(lock)
}

/// `env FLOW_STATE=<state_root> FLOW_SUPERVISOR=1 claude --name flow-supervisor
/// [--resume <session>] "<START_PROMPT>"`.
fn launch_argv(state_root: &Path, session: Option<&str>) -> anyhow::Result<Vec<String>> {
    let state_root = state_root
        .to_str()
        .with_context(|| format!("state path is not UTF-8: {}", state_root.display()))?;
    let state = format!("{STATE_ENV}={state_root}");
    let marker = format!("{SUPERVISOR_ENV}=1");
    let mut argv = vec![
        "env",
        &state,
        &marker,
        AGENT_KIND,
        "--name",
        SUPERVISOR_NAME,
    ];
    if let Some(session) = session {
        argv.extend(["--resume", session]);
    }
    argv.push(START_PROMPT);
    Ok(argv.into_iter().map(str::to_owned).collect())
}
