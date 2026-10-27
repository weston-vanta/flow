//! Live check of the herdr adapter against a real herdr on this machine, in the throwaway
//! session `flow-smoke`: start a server, create a workspace in a temp dir, launch an
//! interactive `claude` with `pane run`, accept the folder trust dialog, wait for `idle`, exit
//! it with `/exit`, then close the workspace and stop the server. No prompt reaches the model.
//!
//! Run with `FLOW_HERDR_LIVE=1 cargo run --example herdr_smoke`.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use flow::herdr::{AgentStatus, Herdr, KEY_DOWN, KEY_ENTER};
use flow::runner::{LocalSshRunner, Runner, default_control_dir};
use flow::state::config::Cde;

const SESSION: &str = "flow-smoke";
const LOCAL_ENV_ID: &str = "smoke-local";
/// The second option of Claude's "Is this a project you trust?" dialog; the first, which has
/// the cursor, is "No, exit".
const TRUST_DIALOG: &str = "Yes, I trust this folder";

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
    let outcome = herdr
        .create_workspace(&cde, cwd.path(), "flow-smoke")
        .and_then(|(workspace_id, pane_id)| {
            println!(
                "workspace: {workspace_id} pane: {pane_id} cwd: {}",
                cwd.path().display()
            );
            let outcome = exercise(&herdr, &cde, &pane_id);
            if let Err(err) = &outcome {
                println!("error: {err:#}");
            }
            herdr.close_workspace(&cde, &workspace_id)?;
            println!("workspace {workspace_id}: closed");
            outcome
        });

    let stop = runner.run(
        &cde,
        &["herdr", "--session", SESSION, "server", "stop"].map(str::to_owned),
        None,
    )?;
    println!("server stop: exit {}", stop.status);
    outcome
}

fn exercise(herdr: &Herdr, cde: &Cde, pane_id: &str) -> anyhow::Result<()> {
    herdr.pane_run(cde, pane_id, &["claude".to_owned()])?;
    println!("pane run: claude");

    if herdr.pane_wait_output(cde, pane_id, TRUST_DIALOG, 20_000)? {
        println!("trust dialog: shown");
        // Keys sent as soon as the dialog renders are dropped; once herdr reports the agent
        // blocked on it, Claude reads input.
        let status = herdr.wait(cde, pane_id, &[AgentStatus::Blocked], Some(10_000))?;
        println!("wait blocked: {status:?}; selecting \"{TRUST_DIALOG}\"");
        herdr.pane_send_keys(cde, pane_id, &[KEY_DOWN, KEY_ENTER])?;
    } else {
        println!("trust dialog: not shown within 20 s");
    }

    let status = herdr.wait(cde, pane_id, &[AgentStatus::Idle], Some(60_000))?;
    println!("wait idle: {status:?}");
    if status != Some(AgentStatus::Idle) {
        println!("agents: {:?}", herdr.agents(cde)?);
        println!("pane text:\n{}", herdr.read(cde, pane_id, 20)?.trim_end());
        bail!("claude did not become idle");
    }
    println!("agents: {:?}", herdr.agents(cde)?);

    // Claude exits on /exit, so herdr reports the agent gone rather than a final status.
    match herdr.prompt(cde, pane_id, "/exit", &[]) {
        Ok(status) => println!("prompt /exit: {status:?}"),
        Err(err) => println!("prompt /exit: {err:#}"),
    }
    match herdr.wait(cde, pane_id, &[AgentStatus::Done], Some(30_000)) {
        Ok(status) => println!("wait done: {status:?}"),
        Err(err) => println!("wait done: {err:#}"),
    }
    wait_until_gone(herdr, cde, pane_id, Duration::from_secs(10))?;
    println!("agent in {pane_id}: gone");
    Ok(())
}

fn wait_until_gone(herdr: &Herdr, cde: &Cde, pane_id: &str, limit: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + limit;
    while herdr
        .agents(cde)?
        .iter()
        .any(|agent| agent.pane_id == pane_id)
    {
        if Instant::now() >= deadline {
            bail!(
                "agent in {pane_id} still running {} s after /exit",
                limit.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}
