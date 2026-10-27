//! Live check of the herdr adapter against a real herdr on this machine, in the throwaway
//! session `flow-smoke`: start a server, create a workspace in a temp dir, start
//! `claude --version` in it, wait for `done`, then close the workspace and stop the server.
//!
//! Run with `FLOW_HERDR_LIVE=1 cargo run --example herdr_smoke`.

use std::sync::Arc;

use anyhow::Context;
use flow::herdr::{AgentStatus, Herdr};
use flow::runner::{LocalSshRunner, Runner, default_control_dir};
use flow::state::config::Cde;

const SESSION: &str = "flow-smoke";
const LOCAL_ENV_ID: &str = "smoke-local";

fn main() -> anyhow::Result<()> {
    if std::env::var("FLOW_HERDR_LIVE").as_deref() != Ok("1") {
        println!("skipped: set FLOW_HERDR_LIVE=1 to run against a live herdr");
        return Ok(());
    }
    let runner: Arc<dyn Runner> = Arc::new(LocalSshRunner::new(
        Some(LOCAL_ENV_ID.to_owned()),
        default_control_dir(),
    )?);
    let cde = Cde {
        name: "local".to_owned(),
        env_id: LOCAL_ENV_ID.to_owned(),
        ssh_alias: String::new(),
        local_marker_env: None,
    };
    let herdr = Herdr::with_session(runner.clone(), SESSION);
    let cwd = tempfile::tempdir().context("creating temp cwd")?;

    herdr.ensure_server(&cde)?;
    println!("server: running in session {SESSION}");
    let outcome = exercise(&herdr, &cde, cwd.path());

    let stop = runner.run(
        &cde,
        &["herdr", "--session", SESSION, "server", "stop"].map(str::to_owned),
        None,
    )?;
    println!("server stop: exit {}", stop.status);
    outcome
}

fn exercise(herdr: &Herdr, cde: &Cde, cwd: &std::path::Path) -> anyhow::Result<()> {
    let (workspace_id, pane_id) = herdr.create_workspace(cde, cwd, "flow-smoke")?;
    println!(
        "workspace: {workspace_id} pane: {pane_id} cwd: {}",
        cwd.display()
    );

    let started = herdr
        .start_agent(cde, &pane_id, "smoke", "claude", &["--version".to_owned()])
        .and_then(|()| {
            println!("agent start: ok");
            herdr.wait(cde, &pane_id, &[AgentStatus::Done], Some(60_000))
        })
        .map(|status| println!("wait: {}", status.as_str()));
    if let Err(err) = &started {
        println!("error: {err:#}");
    }
    match herdr.read(cde, &pane_id, 10) {
        Ok(text) => println!("pane text:\n{}", text.trim_end()),
        Err(err) => println!("read error: {err:#}"),
    }
    println!("agents: {:?}", herdr.agents(cde)?);

    herdr.close_workspace(cde, &workspace_id)?;
    println!("workspace {workspace_id}: closed");
    started
}
