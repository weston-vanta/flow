//! `flow up`: runs the supervisor in the herdr pane it is typed in, replacing its own process with
//! Claude Code there, unless herdr on this host shows the supervisor the lock records running in
//! another pane. It asks no other host: a lock naming one only earns a warning.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use jiff::Timestamp;

use crate::herdr::{self, Agent, Herdr, InvokingPane};
use crate::host;
use crate::inventory;
use crate::observe;
use crate::paths::{STATE_ENV, StateDir};
use crate::render;
use crate::runner::{self, Runner};
use crate::state::config::Host;
use crate::state::task::HerdrTarget;
use crate::supervisor::{self, Lock, SUPERVISOR_CWD, SUPERVISOR_ENV, SUPERVISOR_NAME};
use crate::watch::{self, lock as watch_lock};

const AGENT_KIND: &str = "claude";
/// The supervisor's first prompt, fresh or resumed: its first turn runs the start procedure,
/// and that turn's end arms `flow watch`.
const START_PROMPT: &str = "Run the session start procedure in CLAUDE.md.";

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let Some(pane) = herdr::invoking_pane()? else {
        bail!("flow up runs from a shell pane inside herdr");
    };
    let live = inventory::live_hosts(state)?;
    let here = local_host(&live)?;
    if let Some(lock) = supervisor::load(state)? {
        if lock.host_env_id == here.env_id {
            let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
            if let Some((agent, label)) = running_here(runner, here, &lock)? {
                return super::print(&format!(
                    "the supervisor is running in pane {} (workspace {label})\n",
                    agent.pane_id
                ));
            }
        } else if !inventory::load(state, &lock.host_env_id)?.is_some_and(|record| record.gone) {
            eprintln!(
                "the previous supervisor was recorded on {} since {} ago; if it is still running \
                 there, exit it",
                lock.herdr.host,
                render::relative(lock.since, Timestamp::now())
            );
        }
    }
    start_here(state, here, pane)
}

/// The agent herdr shows on `here`, in the lock's herdr session, reporting the lock's Claude Code
/// session, with its workspace's label, or its id when herdr lists no label for it.
fn running_here(
    runner: Arc<dyn Runner>,
    here: &Host,
    lock: &Lock,
) -> anyhow::Result<Option<(Agent, String)>> {
    let Some(session) = lock.claude_session.as_deref() else {
        return Ok(None);
    };
    let herdr = Herdr::with_session(runner, &lock.herdr.session);
    let agents = herdr.agents_if_running(here)?.unwrap_or_default();
    let Some(agent) = agents
        .into_iter()
        .find(|agent| agent.agent_session_id.as_deref() == Some(session))
    else {
        return Ok(None);
    };
    let label = herdr
        .workspaces_if_running(here)?
        .unwrap_or_default()
        .into_iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)
        .map_or_else(|| agent.workspace_id.clone(), |workspace| workspace.label);
    Ok(Some((agent, label)))
}

/// Takes the lock for `pane` on `here`, clears a watch lock its predecessor left, seeds
/// `watch-state.json`, and replaces this process with the supervisor. A recorded Claude Code
/// session is resumed only while its transcript exists, since `claude --resume` of a missing one
/// exits at once.
fn start_here(state: &StateDir, here: &Host, pane: InvokingPane) -> anyhow::Result<()> {
    let lock = supervisor::take(
        state,
        &here.env_id,
        HerdrTarget {
            host: here.name.clone(),
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
            agent: AGENT_KIND.to_owned(),
            session: pane.session,
        },
    )?;
    if watch_lock::clear(state)? {
        eprintln!("removed state/supervisor/watch.lock left by the previous supervisor");
    }
    watch::seed(state)?;
    let resume = match lock.claude_session.as_deref() {
        Some(session) => resumable(state, session)?,
        None => None,
    };
    match &resume {
        Some(session) => eprintln!(
            "resuming supervisor session {session} in pane {} on {}",
            lock.herdr.pane_id, here.name
        ),
        None => eprintln!(
            "starting a fresh supervisor in pane {} on {}",
            lock.herdr.pane_id, here.name
        ),
    }
    // The launch line names the state path, since the pane's environment may hold another
    // FLOW_STATE.
    let state_root = state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    exec(&launch_argv(state_root.root(), resume.as_deref())?)
}

/// `session` when Claude Code keeps its transcript, else `None` after dropping it from the lock.
fn resumable(state: &StateDir, session: &str) -> anyhow::Result<Option<String>> {
    let transcript =
        observe::transcript_in(&observe::claude_home()?, Path::new(SUPERVISOR_CWD), session);
    if transcript.exists() {
        return Ok(Some(session.to_owned()));
    }
    eprintln!(
        "no transcript of supervisor session {session} at {}; starting a fresh supervisor \
         session",
        transcript.display()
    );
    supervisor::set_session(state, None)?;
    Ok(None)
}

/// Replaces this process with `argv`, run from `SUPERVISOR_CWD`; returns only on failure. Under
/// the fake runner it prints `exec: <argv>` instead.
fn exec(argv: &[String]) -> anyhow::Result<()> {
    if runner::fake_runner_active() {
        return super::print(&format!("exec: {}\n", argv.join(" ")));
    }
    let err = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(SUPERVISOR_CWD)
        .exec();
    Err(err).with_context(|| format!("running `{}` in {SUPERVISOR_CWD}", argv.join(" ")))
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

/// `env FLOW_SUPERVISOR=1 FLOW_STATE=<state_root> claude --name flow-supervisor
/// [--resume <session>] "<START_PROMPT>"`.
fn launch_argv(state_root: &Path, session: Option<&str>) -> anyhow::Result<Vec<String>> {
    let state_root = state_root
        .to_str()
        .with_context(|| format!("state path is not UTF-8: {}", state_root.display()))?;
    let marker = format!("{SUPERVISOR_ENV}=1");
    let state = format!("{STATE_ENV}={state_root}");
    let mut argv = vec![
        "env",
        &marker,
        &state,
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
