//! `flow up`: attaches to the live supervisor, or starts one on this CDE and attaches to it.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail};

use super::attach;
use crate::cde;
use crate::harness::handshake::{self, AgentStart};
use crate::herdr::{self, Herdr};
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::config::{Cde, Config};
use crate::state::task::HerdrTarget;
use crate::supervisor::{self, Lock, SUPERVISOR_CWD, SUPERVISOR_ENV, SUPERVISOR_NAME};

const AGENT_KIND: &str = "claude";

pub fn run(state: &StateDir) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let herdr = Herdr::new(runner);
    if let Some(lock) = supervisor::load(state)?
        && let Some(host) = live_host(&config, &herdr, &lock)?
    {
        return attach::exec(&lock, host);
    }
    let here = local_cde(&config)?;
    let lock = start(state, &herdr, here)?;
    attach::exec(&lock, here)
}

/// The CDE running the supervisor `lock` names, when its pane still holds an agent. Fails when
/// that CDE cannot be reached, since the supervisor may well be alive there.
fn live_host<'a>(
    config: &'a Config,
    herdr: &Herdr,
    lock: &Lock,
) -> anyhow::Result<Option<&'a Cde>> {
    let Some(host) = attach::host(config, lock) else {
        eprintln!(
            "supervisor host {} is not a CDE in config.toml; starting a new supervisor here",
            lock.host_env_id
        );
        return Ok(None);
    };
    let agents = match herdr.agents_if_running(host) {
        Ok(agents) => agents,
        Err(err) if herdr::is_unreachable(&err) => bail!(
            "supervisor host {} unreachable; fix connectivity or remove \
             state/supervisor/lock.json to start fresh: {err:#}",
            host.name
        ),
        Err(err) => return Err(err),
    };
    let live = agents.is_some_and(|agents| {
        agents
            .iter()
            .any(|agent| agent.pane_id == lock.herdr.pane_id)
    });
    Ok(live.then_some(host))
}

fn local_cde(config: &Config) -> anyhow::Result<&Cde> {
    let local_env_id = cde::local_env_id();
    config
        .cdes
        .iter()
        .find(|c| cde::is_local(c, local_env_id.as_deref()))
        .ok_or_else(|| {
            anyhow!(
                "this machine is not a CDE in config.toml ($ONA_ID is {}); flow up starts the \
                 supervisor only on a configured CDE",
                local_env_id.as_deref().unwrap_or("unset")
            )
        })
}

/// Opens the supervisor's workspace on `here`, takes the lock for it, and launches Claude
/// Code there, resuming the recorded session when there is one.
fn start(state: &StateDir, herdr: &Herdr, here: &Cde) -> anyhow::Result<Lock> {
    herdr.ensure_server(here)?;
    let (workspace_id, pane_id) =
        herdr.create_workspace(here, Path::new(SUPERVISOR_CWD), SUPERVISOR_NAME)?;
    // Taken before Claude Code starts, so its SessionStart hook finds the lock to record the
    // session in.
    let lock = supervisor::take(
        state,
        &here.env_id,
        HerdrTarget {
            cde: here.name.clone(),
            workspace_id: workspace_id.clone(),
            pane_id: pane_id.clone(),
            agent: AGENT_KIND.to_owned(),
        },
    )?;
    herdr.pane_run(here, &pane_id, &launch_argv(lock.claude_session.as_deref()))?;
    if let AgentStart::BlockedAtStart { .. } =
        handshake::await_start(herdr, here, &workspace_id, &pane_id)?
    {
        eprintln!(
            "supervisor in pane {pane_id} is blocked on a prompt other than the trust dialog; \
             answer it once attached"
        );
    }
    Ok(lock)
}

/// `env FLOW_SUPERVISOR=1 claude --name flow-supervisor [--resume <session>]`.
fn launch_argv(session: Option<&str>) -> Vec<String> {
    let marker = format!("{SUPERVISOR_ENV}=1");
    let mut argv = vec!["env", &marker, AGENT_KIND, "--name", SUPERVISOR_NAME];
    if let Some(session) = session {
        argv.extend(["--resume", session]);
    }
    argv.into_iter().map(str::to_owned).collect()
}
