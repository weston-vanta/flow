//! `flow up`: attaches to the live supervisor, or starts one on this host and attaches to it. A
//! live supervisor is found in the herdr session its lock records, whatever the configured one; a
//! new one starts in the configured session. One `flow down` stopped is not looked for: a new one
//! starts here at once, resuming its Claude Code session. When the lock's pane holds an agent in
//! the supervisor's workspace that cannot be confirmed as the supervisor, it refuses rather than
//! start a second supervisor on the same session; `flow down --force` or `--forget`, as the user
//! tells which it is, clears the way.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};

use crate::harness::handshake::{self, AgentStart, StoppedEarly};
use crate::herdr::{self, Agent, Herdr};
use crate::host;
use crate::inventory;
use crate::paths::{STATE_ENV, StateDir};
use crate::runner::{self, Runner};
use crate::state::config::{Config, Host};
use crate::state::task::HerdrTarget;
use crate::supervisor::{self, Identity, Lock, SUPERVISOR_CWD, SUPERVISOR_ENV, SUPERVISOR_NAME};
use crate::watch::lock as watch_lock;

const AGENT_KIND: &str = "claude";
/// The supervisor's first prompt, fresh or resumed: its first turn runs the start procedure,
/// and that turn's end arms `flow watch`.
const START_PROMPT: &str = "Run the session start procedure in CLAUDE.md.";

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let hosts = inventory::hosts(state)?;
    let live = inventory::live_hosts(state)?;
    if let Some(lock) = supervisor::load(state)?.filter(|lock| lock.stopped.is_none()) {
        let herdr = Herdr::with_session(runner.clone(), &lock.herdr.session);
        if let Some(host) = live_host(&hosts, &live, &herdr, &lock)? {
            eprintln!(
                "attaching to the supervisor on {} (pane {})",
                host.name, lock.herdr.pane_id
            );
            return attach(&lock, host);
        }
    }
    let herdr = Herdr::with_session(runner, &config.herdr_session);
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
    attach(&lock, here)
}

/// The host running the supervisor `lock` names, when its pane holds the supervisor (see
/// `Lock::identify`). When it holds nothing, or clearly another agent, the lock's workspace is
/// closed there when it is still the supervisor's, so a supervisor that did not come up leaves no
/// workspace behind. Fails, starting nothing, when the pane holds an agent that may be the
/// supervisor, since a second one would resume the same session. Fails when that host cannot be
/// reached, since the supervisor may well be alive there; a gone host is not asked, since the
/// supervisor went with it.
fn live_host<'a>(
    hosts: &'a [Host],
    live: &[Host],
    herdr: &Herdr,
    lock: &Lock,
) -> anyhow::Result<Option<&'a Host>> {
    let Some(host) = hosts.iter().find(|host| host.env_id == lock.host_env_id) else {
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
    // No server means no workspaces left to close either.
    let Some(mut agents) = agents else {
        return Ok(None);
    };
    supervisor::look_up_workspace_labels(herdr, host, &mut agents)?;
    let in_pane = agents
        .iter()
        .find(|agent| agent.pane_id == lock.herdr.pane_id);
    if let Some(agent) = in_pane {
        match lock.identify(agent) {
            Identity::Supervisor => return Ok(Some(host)),
            Identity::Unconfirmed => bail!(
                "{}. flow up started nothing, since a second supervisor would resume the same \
                 session. {}",
                supervisor::unconfirmed(lock, &host.name, agent),
                supervisor::UNCONFIRMED_WAY_OUT
            ),
            Identity::Other => {}
        }
    }
    close_left_workspace(herdr, host, lock, &agents);
    Ok(None)
}

/// Closes the workspace `lock` names on `host` when it is still there under the supervisor's
/// label and none of `agents`, the session's, runs in it. Failures are reported and do not stop
/// the start.
fn close_left_workspace(herdr: &Herdr, host: &Host, lock: &Lock, agents: &[Agent]) {
    let workspace_id = &lock.herdr.workspace_id;
    if let Some(agent) = agents
        .iter()
        .find(|agent| agent.workspace_id == *workspace_id)
    {
        eprintln!(
            "left workspace {workspace_id} on {} open: pane {} runs {}",
            host.name,
            agent.pane_id,
            supervisor::not_the_supervisor(lock, agent)
        );
        return;
    }
    let closed = herdr.workspaces(host).and_then(|workspaces| {
        let left = workspaces
            .iter()
            .any(|workspace| lock.names_workspace(workspace));
        if left {
            herdr.close_workspace(host, workspace_id)
        } else {
            Ok(false)
        }
    });
    match closed {
        Ok(true) => eprintln!(
            "closed workspace {workspace_id} on {}, left by the previous supervisor",
            host.name
        ),
        Ok(false) => {}
        Err(err) => eprintln!(
            "could not close workspace {workspace_id} on {}, left by the previous supervisor: \
             {err:#}",
            host.name
        ),
    }
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
    // A resume Claude Code cannot find exits at once, which this stops on instead of waiting
    // out the start timeout.
    let started = handshake::await_start(
        herdr,
        here,
        &workspace_id,
        &pane_id,
        Some(handshake::session_missing),
    );
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

/// Runs the attach command in place of this process, so it owns the terminal. When stdout is
/// not a terminal, which the attach command needs, it prints the attach command to run from one
/// and returns. Under the fake runner it prints `attach: <argv>` in place of running it.
fn attach(lock: &Lock, host: &Host) -> anyhow::Result<()> {
    let local = host::is_local(host, host::local_env_id().as_deref());
    let control_dir = runner::default_control_dir();
    let argv = supervisor::attach_argv(lock, host, local, &control_dir);
    if !super::stdout_is_terminal() {
        // The printed ssh command names a socket in the control dir, which is gone after a
        // reboot.
        if !local {
            runner::ensure_control_dir(&control_dir)?;
        }
        let command = argv
            .iter()
            .map(|word| herdr::shell_word(word))
            .collect::<Vec<_>>()
            .join(" ");
        return super::print(&format!(
            "stdout is not a terminal, so flow did not attach; to attach, run: {command}\n"
        ));
    }
    if runner::fake_runner_active() {
        return super::print(&format!("attach: {}\n", argv.join(" ")));
    }
    if !local {
        runner::ensure_control_dir(&control_dir)?;
    }
    let (program, args) = argv.split_first().expect("the attach argv names a program");
    let err = Command::new(program).args(args).exec();
    Err(err).with_context(|| format!("running `{}`", argv.join(" ")))
}
