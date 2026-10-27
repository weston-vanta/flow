//! `flow dispatch`: creates a task and starts its worker in a herdr pane on the chosen host.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use super::Refused;
use crate::harness::handshake::{self, AgentStart};
use crate::harness::{ClaudeAdapter, HarnessAdapter, brief};
use crate::herdr::{Agent, Herdr};
use crate::host;
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::config::{Config, Host};
use crate::state::events::Event;
use crate::state::task::{Harness, HerdrTarget, Hold, HoldOn, State, TaskRecord};
use crate::state::{Store, ids};
use crate::worktree;

const EVENT_SOURCE: &str = "cli";
const OWNER: &str = "supervisor";

pub enum BriefSource {
    File(PathBuf),
    Text(String),
}

pub enum WorktreeSource {
    /// An existing worktree on the host.
    Path(PathBuf),
    /// A branch to create a worktree for with the configured worktree tool.
    Branch(String),
}

pub struct DispatchArgs {
    pub host: String,
    pub title: String,
    pub brief: BriefSource,
    pub criteria: String,
    pub worktree: WorktreeSource,
    pub parent: Option<String>,
    pub allow_shared_worktree: bool,
}

/// Creates the task, starts its worker, and prints `dispatched <id> on <host> (<pane_id>)`.
/// A failure once the task exists leaves it `blocked` with a supervisor hold naming the failure.
pub fn run(state: &StateDir, args: &DispatchArgs) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let host = host::find(&config, &args.host).map_err(|err| Refused(format!("{err:#}")))?;
    let store = Store::open(state);
    if let Some(parent) = &args.parent {
        store
            .task(parent)
            .map_err(|err| Refused(format!("--parent: {err:#}")))?;
    }
    if let WorktreeSource::Path(path) = &args.worktree
        && !path.is_absolute()
    {
        return Err(Refused(format!(
            "--worktree must be an absolute path on {}: {}",
            host.name,
            path.display()
        ))
        .into());
    }
    let body = match &args.brief {
        BriefSource::File(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        BriefSource::Text(text) => text.clone(),
    };
    let flow_bin = std::env::current_exe().context("locating the flow binary")?;

    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let worktree = match &args.worktree {
        WorktreeSource::Path(path) => path.clone(),
        WorktreeSource::Branch(branch) => worktree::create(
            runner.as_ref(),
            host,
            &config.monorepo.path,
            &config.worktree_tool,
            branch,
        )?,
    };
    let herdr = Herdr::new(runner);
    herdr.ensure_server(host)?;
    if !args.allow_shared_worktree {
        refuse_shared_worktree(&herdr.agents(host)?, host, &worktree)?;
    }

    let rec = TaskRecord {
        id: ids::new_task_id(Timestamp::now(), &mut rand::rng()),
        parent: args.parent.clone(),
        title: args.title.clone(),
        host: host.name.clone(),
        worktree,
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: OWNER.to_owned(),
    };
    store.create_task(
        rec.clone(),
        &brief::render(&rec, &body, &args.criteria, &flow_bin),
    )?;
    match start_worker(&store, &herdr, host, &rec, &flow_bin) {
        Ok((pane_id, start)) => {
            if let AgentStart::BlockedAtStart { .. } = start {
                eprintln!(
                    "worker in pane {pane_id} is blocked on something other than the trust \
                     dialog; see `flow task events {}`",
                    rec.id
                );
            }
            super::print(&format!(
                "dispatched {} on {} ({pane_id})\n",
                rec.id, host.name
            ))
        }
        Err(err) => {
            if let Err(record_err) = record_failure(&store, &rec.id, &err) {
                eprintln!("could not mark task {} blocked: {record_err:#}", rec.id);
            }
            Err(err)
        }
    }
}

/// Fails with `Refused` when an agent on `host` already works in `worktree`.
fn refuse_shared_worktree(agents: &[Agent], host: &Host, worktree: &Path) -> anyhow::Result<()> {
    let Some(agent) = agents.iter().find(|agent| agent.cwd == worktree) else {
        return Ok(());
    };
    let name = if agent.title.is_empty() {
        String::new()
    } else {
        format!("{:?} ", agent.title)
    };
    Err(Refused(format!(
        "agent {name}(pane {}) on {} already works in {}; pass --allow-shared-worktree to \
         dispatch there anyway",
        agent.pane_id,
        host.name,
        worktree.display()
    ))
    .into())
}

/// Opens a workspace for the task, records it as the task's herdr target, and launches the
/// worker there. Returns the worker's pane and how it started.
fn start_worker(
    store: &Store,
    herdr: &Herdr,
    host: &Host,
    rec: &TaskRecord,
    flow_bin: &Path,
) -> anyhow::Result<(String, AgentStart)> {
    let adapter = ClaudeAdapter;
    let launch = adapter.prepare(rec, &store.task_dir(&rec.id)?, flow_bin)?;
    let mut argv = Vec::new();
    if !launch.env.is_empty() {
        argv.push("env".to_owned());
        argv.extend(
            launch
                .env
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
    }
    argv.extend(launch.argv);

    let (workspace_id, pane_id) = herdr.create_workspace(host, &rec.worktree, &rec.id)?;
    store.set_status(&rec.id, |status| {
        status.state = State::Working;
        status.herdr = Some(HerdrTarget {
            host: host.name.clone(),
            workspace_id: workspace_id.clone(),
            pane_id: pane_id.clone(),
            agent: adapter.kind().to_owned(),
        });
    })?;
    append_event(
        store,
        &rec.id,
        "worker.started",
        json!({"pane_id": pane_id, "workspace_id": workspace_id, "argv": argv}),
    )?;
    herdr.pane_run(host, &pane_id, &argv)?;
    let start = handshake::await_start(herdr, host, &workspace_id, &pane_id)?;
    if let AgentStart::BlockedAtStart { pane_text } = &start {
        append_event(
            store,
            &rec.id,
            "worker.blocked_at_start",
            json!({ "pane_text": pane_text }),
        )?;
    }
    Ok((pane_id, start))
}

/// Logs the failure and holds the task for the supervisor, so a half-started task is visible.
fn record_failure(store: &Store, id: &str, err: &anyhow::Error) -> anyhow::Result<()> {
    let message = format!("{err:#}");
    append_event(store, id, "dispatch.failed", json!({ "error": message }))?;
    store.set_status(id, |status| {
        status.state = State::Blocked;
        status.hold = Some(Hold {
            on: HoldOn::Supervisor,
            what: format!("dispatch failed: {message}"),
            since: Timestamp::now(),
        });
    })
}

fn append_event(store: &Store, id: &str, kind: &str, data: impl Serialize) -> anyhow::Result<()> {
    let ev = Event {
        ts: Timestamp::now(),
        source: EVENT_SOURCE.to_owned(),
        kind: kind.to_owned(),
        data: serde_json::to_value(data)?,
    };
    store.append_event(id, &ev)
}
