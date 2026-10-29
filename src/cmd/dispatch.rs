//! `flow dispatch`: creates a task and starts its worker in a herdr pane on the chosen host, in
//! an existing directory, a new worktree of a configured repo, or the task's own work directory;
//! from a brief given here or from a pending dispatch request, which it consumes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
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
use crate::inventory;
use crate::paths::{STATE_ENV, StateDir};
use crate::runner::{self, Runner};
use crate::state::config::{Config, Host, Repo};
use crate::state::events::Event;
use crate::state::task::{Harness, HerdrTarget, Hold, HoldOn, Origin, State, TaskRecord};
use crate::state::{Store, ids};
use crate::supervisor;
use crate::worktree;

const EVENT_SOURCE: &str = "cli";
const OWNER: &str = "supervisor";
/// Printed after a dispatch from outside the supervisor's session: `flow watch` starts when a
/// supervisor turn ends, so none may be watching the new task yet.
const HAND_DISPATCH_NOTE: &str = "note: no supervisor turn is watching; the supervisor hears of \
     this task when it next ends a turn\n";
/// The task's own working directory, inside its task directory, for work outside any repo.
const WORK_DIR: &str = "work";

#[derive(Clone)]
pub enum BriefSource {
    File(PathBuf),
    Text(String),
}

/// Where the task's title and brief come from.
pub enum TaskSource {
    Given {
        title: String,
        brief: BriefSource,
    },
    /// A pending dispatch request, which supplies the brief, and the title, host, and parent
    /// unless given; `title` overrides the request's.
    Request {
        req_id: String,
        title: Option<String>,
    },
}

/// Where the worker runs.
pub enum DirSource {
    /// An existing directory on the host.
    Path(PathBuf),
    /// A new branch to create a worktree for in `repo`, or in the only configured repo.
    Branch {
        branch: String,
        repo: Option<String>,
    },
    /// `<task_dir>/work` on the shared volume, created for the task.
    TaskWorkDir,
}

pub struct DispatchArgs {
    /// The host to run on; the request's host, else the machine flow runs on, when unset.
    pub host: Option<String>,
    pub source: TaskSource,
    pub criteria: String,
    pub dir: DirSource,
    pub parent: Option<String>,
    pub allow_shared_dir: bool,
}

/// Creates the task, starts its worker, and prints `dispatched <id> on <host> (<pane_id>)`,
/// with a note when run outside the supervisor's session. A dispatched request is consumed once
/// the worker has started, before anything is printed; when that fails the worker keeps
/// running, and the failure is reported first and exits 1.
/// A failure once the task exists leaves it `blocked` with a supervisor hold naming the failure.
pub fn run(state: &StateDir, args: &DispatchArgs) -> anyhow::Result<ExitCode> {
    // Workers get task paths (work dir, settings, brief) under the physical shared-volume path:
    // the repo's `state` symlink exists only where this checkout does, and a path through it
    // would put the worker inside this repo.
    let state = &state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let config = Config::load(state)?;
    let store = Store::open(state);
    let ask = Ask::resolve(&store, args)?;
    let hosts = inventory::hosts(state)?;
    let host = match &ask.host {
        Some(name) => host::find(&hosts, name),
        None => host::local(&hosts),
    }
    .map_err(|err| Refused(format!("{err:#}")))?;
    if !inventory::live_hosts(state)?.contains(host) {
        return Err(Refused(format!("host {} is gone", host.name)).into());
    }
    if let Some(parent) = &ask.parent {
        store
            .task(parent)
            .map_err(|err| Refused(format!("--parent: {err:#}")))?;
    }
    let dir = WorkDir::resolve(&args.dir, &config, host)?;
    let body = match &ask.brief {
        BriefSource::File(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        BriefSource::Text(text) => text.clone(),
    };
    let flow_bin = std::env::current_exe().context("locating the flow binary")?;
    let id = ids::new_task_id(Timestamp::now(), &mut rand::rng());

    let runner: Arc<dyn Runner> = Arc::from(runner::from_env()?);
    let cwd = match &dir {
        WorkDir::Existing(path) => path.to_path_buf(),
        WorkDir::Worktree { repo, branch } => {
            worktree::create(runner.as_ref(), host, repo, branch)?
        }
        WorkDir::Task => store.task_dir(&id)?.join(WORK_DIR),
    };
    let herdr = Herdr::with_session(runner, &config.herdr_session);
    herdr.ensure_server(host)?;
    if !args.allow_shared_dir {
        refuse_shared_dir(&herdr.agents(host)?, host, &cwd)?;
    }

    let rec = TaskRecord {
        id,
        parent: ask.parent.clone(),
        title: ask.title.clone(),
        host: host.name.clone(),
        cwd,
        repo: match &dir {
            WorkDir::Worktree { repo, .. } => Some(repo.name.clone()),
            WorkDir::Existing(_) | WorkDir::Task => None,
        },
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: OWNER.to_owned(),
        origin: Origin::Dispatched,
    };
    store.create_task(
        rec.clone(),
        &brief::render(&rec, &body, &args.criteria, &flow_bin.display().to_string()),
    )?;
    let started = create_work_dir(&dir, &rec.cwd)
        .and_then(|()| start_worker(&store, state.root(), &herdr, host, &rec, &flow_bin));
    match started {
        Ok((pane_id, start)) => {
            if let AgentStart::BlockedAtStart { .. } = start {
                eprintln!(
                    "worker in pane {pane_id} is blocked on something other than the trust \
                     dialog; see `flow task events {}`",
                    rec.id
                );
            }
            let code = match ask
                .request
                .as_ref()
                .map_or(Ok(()), |request| consume(&store, request, &rec.id))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("{err:#}");
                    ExitCode::FAILURE
                }
            };
            super::print(&format!(
                "dispatched {} on {} ({pane_id})\n",
                rec.id, host.name
            ))?;
            if !supervisor::is_supervisor_session() {
                super::print(HAND_DISPATCH_NOTE)?;
            }
            Ok(code)
        }
        Err(err) => {
            if let Err(record_err) = record_failure(&store, &rec.id, &err) {
                eprintln!("could not mark task {} blocked: {record_err:#}", rec.id);
            }
            Err(err)
        }
    }
}

/// The task's title, brief, host, and parent, from the arguments over the request's.
struct Ask {
    title: String,
    brief: BriefSource,
    host: Option<String>,
    parent: Option<String>,
    request: Option<RequestRef>,
}

/// A pending request this dispatch consumes once its worker starts.
struct RequestRef {
    req_id: String,
    requested_by: String,
}

impl Ask {
    /// Fails with `Refused` when the request is unknown or already consumed.
    fn resolve(store: &Store, args: &DispatchArgs) -> anyhow::Result<Ask> {
        let (req_id, title) = match &args.source {
            TaskSource::Given { title, brief } => {
                return Ok(Ask {
                    title: title.clone(),
                    brief: brief.clone(),
                    host: args.host.clone(),
                    parent: args.parent.clone(),
                    request: None,
                });
            }
            TaskSource::Request { req_id, title } => (req_id, title),
        };
        let (_, request) = store
            .pending_requests()?
            .into_iter()
            .find(|(id, _)| id == req_id)
            .ok_or_else(|| Refused(format!("no pending request: {req_id}")))?;
        Ok(Ask {
            title: title.clone().unwrap_or(request.title),
            brief: BriefSource::File(request.brief_path),
            host: args.host.clone().or(request.host),
            parent: Some(
                args.parent
                    .clone()
                    .unwrap_or_else(|| request.requested_by.clone()),
            ),
            request: Some(RequestRef {
                req_id: req_id.clone(),
                requested_by: request.requested_by,
            }),
        })
    }
}

/// Marks the request dispatched as task `id` and logs it on the requesting task.
fn consume(store: &Store, request: &RequestRef, id: &str) -> anyhow::Result<()> {
    let req_id = &request.req_id;
    store.consume_request(req_id).with_context(|| {
        format!(
            "task {id} is dispatched and its worker started, but request {req_id} was not \
             consumed"
        )
    })?;
    append_event(
        store,
        &request.requested_by,
        "request.consumed",
        json!({"req_id": req_id, "task": id}),
    )
    .with_context(|| {
        format!(
            "task {id} is dispatched and request {req_id} consumed, but logging it on task {} \
             failed",
            request.requested_by
        )
    })
}

/// `DirSource` checked against the config before anything runs.
enum WorkDir<'a> {
    Existing(&'a Path),
    Worktree { repo: &'a Repo, branch: &'a str },
    Task,
}

impl<'a> WorkDir<'a> {
    /// Fails with `Refused` for a relative `--dir` or a repo the config cannot resolve.
    fn resolve(source: &'a DirSource, config: &'a Config, host: &Host) -> anyhow::Result<Self> {
        Ok(match source {
            DirSource::Path(path) if !path.is_absolute() => {
                return Err(Refused(format!(
                    "--dir must be an absolute path on {}: {}",
                    host.name,
                    path.display()
                ))
                .into());
            }
            DirSource::Path(path) => WorkDir::Existing(path),
            DirSource::Branch { branch, repo } => WorkDir::Worktree {
                repo: config
                    .repo(repo.as_deref())
                    .map_err(|err| Refused(format!("{err:#}")))?,
                branch,
            },
            DirSource::TaskWorkDir => WorkDir::Task,
        })
    }
}

/// Creates the task's work directory when the worker runs there. It lives inside the task's
/// directory, so it can be made only once the task exists.
fn create_work_dir(dir: &WorkDir, cwd: &Path) -> anyhow::Result<()> {
    match dir {
        WorkDir::Task => {
            std::fs::create_dir_all(cwd).with_context(|| format!("creating {}", cwd.display()))
        }
        WorkDir::Existing(_) | WorkDir::Worktree { .. } => Ok(()),
    }
}

/// Fails with `Refused` when an agent on `host` already works in `dir`.
fn refuse_shared_dir(agents: &[Agent], host: &Host, dir: &Path) -> anyhow::Result<()> {
    let Some(agent) = agents.iter().find(|agent| agent.cwd == dir) else {
        return Ok(());
    };
    let name = if agent.title.is_empty() {
        String::new()
    } else {
        format!("{:?} ", agent.title)
    };
    Err(Refused(format!(
        "agent {name}(pane {}) on {} already works in {}; pass --allow-shared-dir to \
         dispatch into that directory anyway",
        agent.pane_id,
        host.name,
        dir.display()
    ))
    .into())
}

/// Opens a workspace for the task, records it as the task's herdr target, and launches the
/// worker there as `env FLOW_STATE=<state_root> [<harness env>] <harness argv>`: herdr hands
/// panes the environment of whoever started its server, which may hold another FLOW_STATE.
/// Returns the worker's pane and how it started.
fn start_worker(
    store: &Store,
    state_root: &Path,
    herdr: &Herdr,
    host: &Host,
    rec: &TaskRecord,
    flow_bin: &Path,
) -> anyhow::Result<(String, AgentStart)> {
    let adapter = ClaudeAdapter;
    let launch = adapter.prepare(rec, &store.task_dir(&rec.id)?, flow_bin)?;
    let state_root = state_root
        .to_str()
        .with_context(|| format!("state path is not UTF-8: {}", state_root.display()))?;
    let mut argv = vec!["env".to_owned(), format!("{STATE_ENV}={state_root}")];
    argv.extend(
        launch
            .env
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    argv.extend(launch.argv);

    let (workspace_id, pane_id) = herdr.create_workspace(host, &rec.cwd, &rec.title)?;
    store.set_status(&rec.id, |status| {
        status.state = State::Working;
        status.herdr = Some(HerdrTarget {
            host: host.name.clone(),
            workspace_id: workspace_id.clone(),
            pane_id: pane_id.clone(),
            agent: adapter.kind().to_owned(),
            session: herdr.session().to_owned(),
        });
    })?;
    append_event(
        store,
        &rec.id,
        "worker.started",
        json!({"pane_id": pane_id, "workspace_id": workspace_id, "argv": argv}),
    )?;
    herdr.pane_run(host, &pane_id, &argv)?;
    let start = handshake::await_start(herdr, host, &workspace_id, &pane_id, None)?;
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
