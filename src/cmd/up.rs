//! `flow up`: attaches to the live supervisor, or starts one on this host and attaches to it.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};

use super::attach;
use crate::harness::handshake::{self, AgentStart};
use crate::herdr::{self, Herdr};
use crate::host;
use crate::inventory;
use crate::paths::{STATE_ENV, StateDir};
use crate::runner::{self, Runner};
use crate::state::config::Host;
use crate::state::task::HerdrTarget;
use crate::supervisor::{self, Lock, SUPERVISOR_CWD, SUPERVISOR_ENV, SUPERVISOR_NAME};
use crate::watch::lock as watch_lock;

const AGENT_KIND: &str = "claude";
/// The supervisor's first prompt, fresh or resumed: its first turn runs the start procedure,
/// and that turn's end arms `flow watch`.
const START_PROMPT: &str = "Run the session start procedure in CLAUDE.md.";

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let herdr = Herdr::new(runner);
    let hosts = inventory::hosts(state)?;
    let live = inventory::live_hosts(state)?;
    if let Some(lock) = supervisor::load(state)?
        && let Some(host) = live_host(&hosts, &live, &herdr, &lock)?
    {
        return attach::exec(&lock, host);
    }
    let here = local_host(&live)?;
    // No supervisor is live, so whatever holds the watch lock is a watcher that outlived its
    // supervisor, possibly on another host where it cannot be seen to be dead.
    if watch_lock::clear(state)? {
        eprintln!("removed state/supervisor/watch.lock left by the previous supervisor");
    }
    // The launch line names the state path, since herdr hands panes the environment of
    // whoever started its server, which may hold another FLOW_STATE.
    let state_root = state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let lock = start(state, &herdr, here, state_root.root())?;
    attach::exec(&lock, here)
}

/// The host running the supervisor `lock` names, when its pane still holds an agent. Fails when
/// that host cannot be reached, since the supervisor may well be alive there; a gone host is
/// not asked, since the supervisor went with it.
fn live_host<'a>(
    hosts: &'a [Host],
    live: &[Host],
    herdr: &Herdr,
    lock: &Lock,
) -> anyhow::Result<Option<&'a Host>> {
    let Some(host) = attach::host(hosts, lock) else {
        eprintln!(
            "supervisor host {} is not a known host; starting a new supervisor here",
            lock.host_env_id
        );
        return Ok(None);
    };
    if !live.contains(host) {
        eprintln!(
            "supervisor host {} is gone; starting a new supervisor here",
            host.name
        );
        return Ok(None);
    }
    let agents = match herdr.agents_if_running(host) {
        Ok(agents) => agents,
        Err(err) if herdr::is_unreachable(&err) => bail!(
            "supervisor host {} unreachable: {err:#}\nrunning or attaching to the supervisor \
             from this machine needs the operator's Ona login (`gitpod login`) and `gitpod \
             environment ssh-config` here; set them up, or run `flow up` where they are",
            host.name
        ),
        Err(err) => return Err(err),
    };
    // herdr reuses pane ids after a server restart, so the pane alone could name a worker.
    let live = agents.is_some_and(|agents| {
        agents.iter().any(|agent| {
            agent.pane_id == lock.herdr.pane_id && agent.cwd == Path::new(SUPERVISOR_CWD)
        })
    });
    Ok(live.then_some(host))
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

/// Starts the supervisor on `here`, resuming the recorded session when there is one. When a
/// resume does not come up because Claude Code cannot find the session, the session is dropped
/// from the lock and the start retried once without it, since it would otherwise fail every
/// later `flow up` the same way. Any other failed resume keeps the session and leaves its
/// workspace open for inspection.
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
    let pane_text = match herdr.pane_read(here, &lock.herdr.pane_id, handshake::PANE_LINES) {
        Ok(text) => text,
        Err(read_err) => {
            eprintln!(
                "could not read pane {} on {}: {read_err:#}",
                lock.herdr.pane_id, here.name
            );
            String::new()
        }
    };
    if !handshake::session_missing(&pane_text) {
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
        },
    )?;
    herdr.pane_run(
        here,
        &pane_id,
        &launch_argv(state_root, lock.claude_session.as_deref())?,
    )?;
    let started = handshake::await_start(herdr, here, &workspace_id, &pane_id);
    Ok((lock, started))
}

fn finish(lock: Lock, started: AgentStart) -> anyhow::Result<Lock> {
    if let AgentStart::BlockedAtStart { .. } = started {
        eprintln!(
            "supervisor in pane {} is blocked on a prompt other than the trust dialog; \
             answer it once attached",
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
