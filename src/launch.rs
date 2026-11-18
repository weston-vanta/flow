//! `flow session start`: launches an IC Session in a new, unfocused herdr tab on this CDE.
//! It is a command, not a Writer: the IC's own hooks put the IC in the Store.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;

use crate::cde;
use crate::services::STORE_ERROR_HELP;
use crate::session;
use crate::store::Store;
use crate::task;

/// How long `herdr agent start` may wait for Claude to be ready, in milliseconds.
const AGENT_START_TIMEOUT_MS: &str = "60000";

/// What `flow session start` was asked for, as parsed: the id and workspace may be left out.
pub struct Request<'a> {
    /// The Task slug.
    pub task: &'a str,
    pub workspace: Option<String>,
    /// The CDE to launch on over ssh; `None` for this one.
    pub cde: Option<String>,
    pub id: Option<String>,
    pub repo: &'a Path,
}

/// What to launch. Every field is a plain value, so the remote form can pass them over ssh.
struct Launch<'a> {
    /// The IC's Claude session id, a UUID.
    pub id: &'a str,
    /// The Task slug: Claude's `--name`, herdr's agent name, the tab's label and the
    /// argument of `/flow:ic`.
    pub task: &'a str,
    /// The label of the herdr workspace the tab goes in, found or created.
    pub workspace: &'a str,
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

/// Why a launch stopped: printed as `{error, help}` with exit `code`, 1 (error) or 2 (usage).
#[derive(Debug)]
pub struct Failure {
    pub code: u8,
    pub error: String,
    pub help: String,
}

fn fail(error: impl Into<String>, help: impl Into<String>) -> Failure {
    Failure {
        code: 1,
        error: error.into(),
        help: help.into(),
    }
}

fn usage(error: impl Into<String>, help: impl Into<String>) -> Failure {
    Failure {
        code: 2,
        ..fail(error, help)
    }
}

/// What a launch ended with: the JSON to print and the exit code.
pub struct Outcome {
    pub code: u8,
    pub output: Value,
}

/// Runs `flow session start`: checks the slug and id (choosing a fresh id if none is given),
/// defaults the workspace to the calling Session's, then launches here or on `request.cde`.
pub fn session_start(store: &Store, request: Request) -> Result<Outcome, Failure> {
    let slug = request.task;
    if !task::is_slug(slug) {
        return Err(usage(
            format!("--task {slug:?} is not a usable Task slug"),
            "use lowercase letters, digits, `-` and `_`, starting with a letter, at most 32 characters",
        ));
    }
    let workspace = match request.workspace {
        Some(workspace) => workspace,
        None => match std::env::var("CLAUDE_CODE_SESSION_ID") {
            Ok(manager) if !manager.is_empty() => workspace_for(store, &manager)
                .map_err(|err| fail(format!("{err:#}"), STORE_ERROR_HELP))?,
            _ => {
                return Err(usage(
                    "no --workspace given and CLAUDE_CODE_SESSION_ID is not set",
                    "run it from a Manager Session",
                ));
            }
        },
    };
    let id = match request.id {
        Some(id) if is_uuid(&id) => id,
        Some(id) => {
            return Err(usage(
                format!("--id {id:?} is not a UUID"),
                "leave out --id to have flow choose one",
            ));
        }
        None => new_id().map_err(|err| {
            fail(
                format!("cannot generate a Session id: {err}"),
                "this is a bug in flow",
            )
        })?,
    };
    let launch = Launch {
        id: &id,
        task: slug,
        workspace: &workspace,
        repo: request.repo,
    };
    match request.cde {
        Some(cde) => start_remote(&cde, &launch),
        None => {
            let started = start(store, &launch)?;
            let output = serde_json::to_value(started)
                .map_err(|err| fail(err.to_string(), "this is a bug in flow"))?;
            Ok(Outcome { code: 0, output })
        }
    }
}

/// A fresh version 4 UUID, from the kernel.
fn new_id() -> std::io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_owned())
}

/// Whether `id` is a UUID in its hyphenated form, as `claude --session-id` requires.
fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// The workspace label for ICs launched by the Session `manager`: its title in the Store,
/// else its id.
fn workspace_for(store: &Store, manager: &str) -> anyhow::Result<String> {
    let session = store
        .get(session::KIND, manager)?
        .as_ref()
        .map(session::Session::from_entity)
        .transpose()?;
    let title = session.and_then(|s| s.context).and_then(|c| c.value.title);
    Ok(title.unwrap_or_else(|| manager.to_owned()))
}

/// Checks the repo, the Task's brief and the id, then opens the tab, starts Claude in it and
/// sends `/flow:ic <slug>`. Stops at the first failure, leaving any pane as it is.
fn start(store: &Store, launch: &Launch) -> Result<Started, Failure> {
    let repo = checkout(launch.repo)?;
    let brief = task::root_from_env().join(launch.task).join("brief.md");
    if !brief.is_file() {
        return Err(fail(
            format!(
                "Task {} has no brief: {} is missing",
                launch.task,
                brief.display()
            ),
            "create the Task with `flow task create` and write its brief.md first, as /flow:go does",
        ));
    }
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
    // A new tab is labelled with the slug. A new workspace takes the workspace's label, and
    // its first tab is renamed to the slug below.
    let (mut args, label) = match workspace {
        Some(id) => (vec!["tab", "create", "--workspace", id], launch.task),
        None => (vec!["workspace", "create"], launch.workspace),
    };
    args.extend(["--cwd", &repo, "--label", label, "--no-focus"]);
    let created = herdr(&args)?;
    let pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .ok_or_else(|| fail("herdr returned no pane id for the new tab", HERDR_BUG_HELP))?
        .to_owned();
    if workspace.is_none() {
        let tab = created["result"]["tab"]["tab_id"].as_str().ok_or_else(|| {
            fail(
                "herdr returned no tab id for the new workspace",
                HERDR_BUG_HELP,
            )
        })?;
        herdr(&["tab", "rename", tab, launch.task]).map_err(|f| in_pane(f, &pane))?;
    }
    herdr(&[
        "agent",
        "start",
        launch.task,
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
        launch.task,
    ])
    .map_err(|f| in_pane(f, &pane))?;
    let prompt = format!("/flow:ic {}", launch.task);
    herdr(&["agent", "prompt", &pane, &prompt]).map_err(|f| in_pane(f, &pane))?;
    Ok(Started {
        id: launch.id.to_owned(),
        name: launch.task.to_owned(),
        cde: cde::this_name(),
        pane,
    })
}

/// The installed `flow` every CDE shares, which a remote launch runs.
const INSTALLED_FLOW: &str = "/efs-home/flow/bin/flow";

/// Launches on the CDE named `cde`: resolves its environment id through `gitpod`, checks it
/// is running (it never starts one), then runs the local form of `flow session start` there
/// over ssh with every value explicit. The target's `{error, help}` and exit code pass through
/// unchanged; on success `cde` is set to `cde`.
fn start_remote(cde: &str, launch: &Launch) -> Result<Outcome, Failure> {
    let env_id = running_environment(cde)?;
    let repo = std::path::absolute(launch.repo).unwrap_or_else(|_| launch.repo.to_owned());
    let words = [
        INSTALLED_FLOW,
        "session",
        "start",
        "--id",
        launch.id,
        "--task",
        launch.task,
        "--workspace",
        launch.workspace,
        &repo.to_string_lossy(),
    ]
    .map(shell_quote)
    .join(" ");
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
        .arg(format!("{env_id}.ona.environment"))
        .arg(format!("zsh -lc {}", shell_quote(&words)))
        .stdin(Stdio::null())
        .output()
        .map_err(|err| fail(format!("cannot run ssh: {err}"), "ssh must be on PATH"))?;
    let code = out.status.code().unwrap_or(-1);
    match serde_json::from_slice::<Value>(&out.stdout) {
        Ok(mut output) if matches!(code, 0..=2) && output.is_object() => {
            if code == 0 {
                output["cde"] = cde.into();
            }
            Ok(Outcome {
                code: code as u8,
                output,
            })
        }
        _ => Err(fail(
            format!(
                "ssh to CDE {cde} ({env_id}) failed with exit code {code}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            "check that the CDE is reachable (`gitpod environment ssh-config`) and flow is installed on /efs-home",
        )),
    }
}

/// The environment id of the CDE named `cde`, if `gitpod environment list` shows it running.
fn running_environment(cde: &str) -> Result<String, Failure> {
    let out = Command::new("gitpod")
        .args(["environment", "list", "-o", "json"])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| {
            fail(
                format!("cannot run gitpod: {err}"),
                "gitpod must be on PATH",
            )
        })?;
    let listed: Value = serde_json::from_slice(&out.stdout)
        .ok()
        .filter(|_| out.status.success())
        .ok_or_else(|| {
            fail(
                format!(
                    "gitpod environment list failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                "log in with `gitpod login` on this CDE",
            )
        })?;
    let env = listed
        .as_array()
        .into_iter()
        .flatten()
        .find(|env| env["metadata"]["name"] == cde)
        .ok_or_else(|| {
            fail(
                format!("no CDE named {cde:?}"),
                "pass a name from `gitpod environment list`",
            )
        })?;
    let phase = env["status"]["phase"].as_str().unwrap_or_default();
    if phase != "ENVIRONMENT_PHASE_RUNNING" {
        return Err(fail(
            format!("CDE {cde} is not running ({phase})"),
            "start it yourself or pick a running CDE; flow never starts a CDE",
        ));
    }
    env["id"].as_str().map(str::to_owned).ok_or_else(|| {
        fail(
            format!("gitpod listed CDE {cde} with no id"),
            "check `gitpod --version`",
        )
    })
}

/// `word` in single quotes, safe for a POSIX shell.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
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
    // Some commands (such as `tab rename`) may print nothing on success.
    if out.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|err| fail(format!("herdr printed no JSON: {err}"), HERDR_BUG_HELP))
}
