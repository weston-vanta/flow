//! `flow dispatch`: creates a task and starts its worker in a herdr pane on the chosen CDE.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use super::Refused;
use crate::cde;
use crate::harness::{ClaudeAdapter, HarnessAdapter, brief};
use crate::herdr::{Agent, AgentStatus, Herdr, KEY_DOWN, KEY_ENTER};
use crate::paths::StateDir;
use crate::runner::{self, Runner};
use crate::state::config::{Cde, Config};
use crate::state::events::Event;
use crate::state::task::{Harness, HerdrTarget, Hold, HoldOn, State, TaskRecord};
use crate::state::{Store, ids};
use crate::worktree;

const EVENT_SOURCE: &str = "cli";
const OWNER: &str = "supervisor";
/// How long the worker gets to start, and again to settle after the trust dialog.
const START_TIMEOUT_MS: u64 = 60_000;
/// Enough of the pane to hold the whole trust dialog.
const DIALOG_LINES: u32 = 40;
/// The trust dialog lists two options, so reaching `Yes` never takes more moves than this.
const MAX_CURSOR_MOVES: usize = 3;

pub enum BriefSource {
    File(PathBuf),
    Text(String),
}

pub enum WorktreeSource {
    /// An existing worktree on the CDE.
    Path(PathBuf),
    /// A branch to create a worktree for with the configured worktree tool.
    Branch(String),
}

pub struct DispatchArgs {
    pub cde: String,
    pub title: String,
    pub brief: BriefSource,
    pub criteria: String,
    pub worktree: WorktreeSource,
    pub parent: Option<String>,
    pub allow_shared_worktree: bool,
}

/// Creates the task, starts its worker, and prints `dispatched <id> on <cde> (<pane_id>)`.
/// A failure once the task exists leaves it `blocked` with a supervisor hold naming the failure.
pub fn run(state: &StateDir, args: &DispatchArgs) -> anyhow::Result<()> {
    let config = Config::load(state)?;
    let cde = cde::find(&config, &args.cde).map_err(|err| Refused(format!("{err:#}")))?;
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
            cde.name,
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
        WorktreeSource::Branch(branch) => {
            worktree::create(runner.as_ref(), cde, &config.worktree_tool, branch)?
        }
    };
    let herdr = Herdr::new(runner);
    herdr.ensure_server(cde)?;
    if !args.allow_shared_worktree {
        refuse_shared_worktree(&herdr.agents(cde)?, cde, &worktree)?;
    }

    let rec = TaskRecord {
        id: ids::new_task_id(Timestamp::now(), &mut rand::rng()),
        parent: args.parent.clone(),
        title: args.title.clone(),
        cde: cde.name.clone(),
        worktree,
        harness: Harness::Claude,
        created: Timestamp::now(),
        owner: OWNER.to_owned(),
    };
    store.create_task(
        rec.clone(),
        &brief::render(&rec, &body, &args.criteria, &flow_bin),
    )?;
    match start_worker(&store, &herdr, cde, &rec, &flow_bin) {
        Ok((pane_id, start)) => {
            if let WorkerStart::BlockedAtStart { .. } = start {
                eprintln!(
                    "worker in pane {pane_id} is blocked on something other than the trust \
                     dialog; see `flow task events {}`",
                    rec.id
                );
            }
            super::print(&format!(
                "dispatched {} on {} ({pane_id})\n",
                rec.id, cde.name
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

/// Fails with `Refused` when an agent on `cde` already works in `worktree`.
fn refuse_shared_worktree(agents: &[Agent], cde: &Cde, worktree: &Path) -> anyhow::Result<()> {
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
        cde.name,
        worktree.display()
    ))
    .into())
}

/// Opens a workspace for the task, records it as the task's herdr target, and launches the
/// worker there. Returns the worker's pane and how it started.
fn start_worker(
    store: &Store,
    herdr: &Herdr,
    cde: &Cde,
    rec: &TaskRecord,
    flow_bin: &Path,
) -> anyhow::Result<(String, WorkerStart)> {
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

    let (workspace_id, pane_id) = herdr.create_workspace(cde, &rec.worktree, &rec.id)?;
    store.set_status(&rec.id, |status| {
        status.state = State::Working;
        status.herdr = Some(HerdrTarget {
            cde: cde.name.clone(),
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
    herdr.pane_run(cde, &pane_id, &argv)?;
    let start = await_worker_start(herdr, cde, &workspace_id, &pane_id)?;
    if let WorkerStart::BlockedAtStart { pane_text } = &start {
        append_event(
            store,
            &rec.id,
            "worker.blocked_at_start",
            json!({ "pane_text": pane_text }),
        )?;
    }
    Ok((pane_id, start))
}

#[derive(Debug)]
enum WorkerStart {
    /// herdr sees the worker idle or working.
    Running,
    /// The worker stopped on a prompt other than the trust dialog, showing `pane_text`; no keys
    /// were sent.
    BlockedAtStart { pane_text: String },
}

/// Waits for the worker just launched in `pane_id` to come up. Claude shows a folder-trust
/// dialog in a directory it has not seen, and herdr reports it `blocked`; keys sent before then
/// are lost. When the blocked pane shows that dialog, this selects its `Yes` option and waits
/// again.
fn await_worker_start(
    herdr: &Herdr,
    cde: &Cde,
    workspace_id: &str,
    pane_id: &str,
) -> anyhow::Result<WorkerStart> {
    let wait = |until: &[AgentStatus]| -> anyhow::Result<AgentStatus> {
        match herdr.wait(cde, pane_id, until, Some(START_TIMEOUT_MS))? {
            Some(status) => Ok(status),
            None => bail!(
                "worker in pane {pane_id} on {} did not start within {} s; workspace \
                 {workspace_id} is left open for inspection",
                cde.name,
                START_TIMEOUT_MS / 1000
            ),
        }
    };
    let status = wait(&[
        AgentStatus::Blocked,
        AgentStatus::Idle,
        AgentStatus::Working,
    ])?;
    if status != AgentStatus::Blocked {
        return Ok(WorkerStart::Running);
    }
    let text = herdr.read(cde, pane_id, DIALOG_LINES)?;
    let Some(keys) = trust_dialog_keys(&text) else {
        return Ok(WorkerStart::BlockedAtStart { pane_text: text });
    };
    herdr.pane_send_keys(cde, pane_id, &keys)?;
    wait(&[AgentStatus::Idle, AgentStatus::Working])?;
    Ok(WorkerStart::Running)
}

/// The keys that accept Claude's folder-trust dialog shown in `text`: enough `down`s to move
/// the cursor (`❯` or `>` starting a line) onto the option containing `Yes`, then `enter`.
/// `None` when `text` is not that dialog or the cursor cannot reach `Yes` by moving down.
pub fn trust_dialog_keys(text: &str) -> Option<Vec<&'static str>> {
    if !text.to_lowercase().contains("trust") {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // The pane holds earlier output too, so the dialog's cursor is the last one on screen.
    let cursor = lines.iter().rposition(|line| is_cursor_line(line))?;
    let yes = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("Yes"))
        .min_by_key(|(i, _)| i.abs_diff(cursor))?
        .0;
    if yes < cursor {
        return None;
    }
    let moves = lines[cursor + 1..=yes]
        .iter()
        .filter(|line| !line.is_empty())
        .count();
    if moves > MAX_CURSOR_MOVES {
        return None;
    }
    let mut keys = vec![KEY_DOWN; moves];
    keys.push(KEY_ENTER);
    Some(keys)
}

/// A selection cursor: `❯` or `>` alone or followed by a space, so a `>_ claude` prompt line
/// does not count.
fn is_cursor_line(line: &str) -> bool {
    ["❯", ">"].iter().any(|marker| {
        line.strip_prefix(marker)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOG_ON_NO: &str = "\
=> /workspaces/acme-worktrees/search-index
>_ claude
 Accessing workspace:

 /workspaces/acme-worktrees/search-index

 Quick safety check: Is this a project you created or one you trust?

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel
";

    #[test]
    fn trust_dialog_keys_moves_cursor_to_yes() {
        assert_eq!(
            trust_dialog_keys(DIALOG_ON_NO),
            Some(vec![KEY_DOWN, KEY_ENTER])
        );
    }

    #[test]
    fn trust_dialog_keys_confirms_when_cursor_is_on_yes() {
        let text =
            " Do you trust the files in this folder?\n\n > 1. Yes, proceed\n   2. No, exit\n";
        assert_eq!(trust_dialog_keys(text), Some(vec![KEY_ENTER]));
    }

    #[test]
    fn trust_dialog_keys_ignores_other_prompts() {
        assert_eq!(
            trust_dialog_keys("Choose a theme:\n ❯ Dark\n   Light\n"),
            None
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n   Yes\n ❯ No\n"),
            None,
            "Yes above the cursor would need an up key"
        );
        assert_eq!(
            trust_dialog_keys("Do you trust this folder?\n>_ claude\n   Yes\n"),
            None,
            "a shell prompt is not a cursor"
        );
    }
}
