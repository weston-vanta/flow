//! `flow session start`: launches an IC Session in a new, unfocused herdr tab on this CDE.
//! It is a command, not a Writer: the IC's own hooks put the IC in the Store.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;

use crate::hook;
use crate::services::STORE_ERROR_HELP;
use crate::session;
use crate::store::Store;

/// How long `herdr agent start` may wait for Claude to be ready, in milliseconds.
const AGENT_START_TIMEOUT_MS: &str = "60000";

/// What to launch. Every field is a plain value, so the remote form can pass them over ssh.
pub struct Launch<'a> {
    /// The IC's Claude session id, a UUID.
    pub id: &'a str,
    /// The Task slug: Claude's `--name`, herdr's agent name and the tab's label.
    pub name: &'a str,
    /// The label of the herdr workspace the tab goes in, found or created.
    pub workspace: &'a str,
    pub brief: &'a Path,
    /// The repo's main checkout, the IC's cwd.
    pub repo: &'a Path,
}

/// What `flow session start` prints on success.
#[derive(Debug, Serialize)]
pub struct Started {
    pub id: String,
    pub name: String,
    /// This CDE's name; `None` if it could not be resolved.
    pub cde: Option<String>,
    pub pane: String,
}

/// Why a launch stopped: printed as `{error, help}` with exit 1.
#[derive(Debug)]
pub struct Failure {
    pub error: String,
    pub help: String,
}

fn fail(error: impl Into<String>, help: impl Into<String>) -> Failure {
    Failure {
        error: error.into(),
        help: help.into(),
    }
}

/// A fresh version 4 UUID, from the kernel.
pub fn new_id() -> std::io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_owned())
}

/// Whether `id` is a UUID in its hyphenated form, as `claude --session-id` requires.
pub fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Whether `name` is usable as a herdr agent name: `[a-z][a-z0-9_-]{0,31}`.
pub fn is_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// The workspace label for ICs launched by the Session `manager`: its title in the Store,
/// else its id.
pub fn workspace_for(store: &Store, manager: &str) -> anyhow::Result<String> {
    let session = store
        .get(session::KIND, manager)?
        .as_ref()
        .map(session::Session::from_entity)
        .transpose()?;
    let title = session.and_then(|s| s.context).and_then(|c| c.value.title);
    Ok(title.unwrap_or_else(|| manager.to_owned()))
}

/// Checks the repo, brief and id, then opens the tab, starts Claude in it and sends
/// `/flow:ic <brief>`. Stops at the first failure, leaving any pane as it is.
pub fn start(store: &Store, launch: &Launch) -> Result<Started, Failure> {
    let repo = checkout(launch.repo)?;
    let brief = launch.brief.canonicalize().map_err(|err| {
        fail(
            format!("cannot read the brief {}: {err}", launch.brief.display()),
            "write the brief under /efs-home first and pass its path with --brief",
        )
    })?;
    match store.get(session::KIND, launch.id) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Err(fail(
                format!("Session id {} is already in use", launch.id),
                "leave out --id to have flow choose a fresh one",
            ));
        }
        Err(err) => return Err(fail(format!("{err:#}"), STORE_ERROR_HELP)),
    }
    let repo = repo.to_string_lossy();
    let list = herdr(&["workspace", "list"])?;
    let workspace = list["result"]["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|w| w["label"] == launch.workspace)
        .and_then(|w| w["workspace_id"].as_str());
    let created = match workspace {
        Some(id) => herdr(&[
            "tab",
            "create",
            "--workspace",
            id,
            "--cwd",
            &repo,
            "--label",
            launch.name,
            "--no-focus",
        ])?,
        None => herdr(&[
            "workspace",
            "create",
            "--cwd",
            &repo,
            "--label",
            launch.workspace,
            "--no-focus",
        ])?,
    };
    let pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .ok_or_else(|| fail("herdr returned no pane id for the new tab", HERDR_BUG_HELP))?
        .to_owned();
    herdr(&[
        "agent",
        "start",
        launch.name,
        "--kind",
        "claude",
        "--pane",
        &pane,
        "--timeout",
        AGENT_START_TIMEOUT_MS,
        "--",
        "--session-id",
        launch.id,
        "--name",
        launch.name,
    ])
    .map_err(|f| in_pane(f, &pane))?;
    let prompt = format!("/flow:ic {}", brief.display());
    herdr(&["agent", "prompt", &pane, &prompt]).map_err(|f| in_pane(f, &pane))?;
    let cde = std::env::var("ONA_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .and_then(|id| hook::cde_name(&id));
    Ok(Started {
        id: launch.id.to_owned(),
        name: launch.name.to_owned(),
        cde,
        pane,
    })
}

const HERDR_BUG_HELP: &str = "herdr's output was not what flow expects; check `herdr --version`";

/// The repo dir made absolute, if it exists and has a wt base beside it.
fn checkout(repo: &Path) -> Result<PathBuf, Failure> {
    let repo = repo.canonicalize().map_err(|err| {
        fail(
            format!("repo dir {} cannot be used: {err}", repo.display()),
            "pass the repo's main checkout, for example /workspaces/obsidian",
        )
    })?;
    let mut base = repo.clone().into_os_string();
    base.push(".wt/.base/.git");
    if !Path::new(&base).exists() {
        return Err(fail(
            format!("{} has no wt base", repo.display()),
            "run `wt base-init` in the repo first; the IC creates its worktrees with `flow wt create`",
        ));
    }
    Ok(repo)
}

/// Says which pane a failure left behind.
fn in_pane(mut failure: Failure, pane: &str) -> Failure {
    failure.error = format!("{} (pane {pane}, left as it is)", failure.error);
    failure
}

/// Runs `herdr <args>` and returns its JSON output, or a failure built from herdr's error.
fn herdr(args: &[&str]) -> Result<Value, Failure> {
    let out = Command::new("herdr")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| fail(format!("cannot run herdr: {err}"), "herdr must be on PATH"))?;
    if !out.status.success() {
        let report: Value = serde_json::from_slice(&out.stderr).unwrap_or_default();
        let code = report["error"]["code"].as_str().unwrap_or_default();
        let message = report["error"]["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_owned());
        let command = format!("herdr {} {}", args[0], args[1]);
        return Err(match code {
            "server_not_running" => fail(
                format!("no herdr server is running: {message}"),
                "start herdr yourself in a terminal on this CDE; flow never starts a herdr server",
            ),
            "agent_not_ready" => fail(
                format!("Claude is blocked at a dialog: {message}"),
                "open the pane and answer the dialog yourself; flow sends no keys",
            ),
            "agent_pane_busy" => fail(
                format!("the new pane is busy: {message}"),
                "the pane's shell was not at its prompt; check the pane and launch again",
            ),
            _ => fail(
                format!("{command} failed: {message}"),
                "check the pane and herdr's message; flow does not retry",
            ),
        });
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|err| fail(format!("herdr printed no JSON: {err}"), HERDR_BUG_HELP))
}
