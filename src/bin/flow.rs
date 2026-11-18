//! `flow`: the CLI. Prints JSON on stdout; errors are `{error, help}` JSON with exit code 1
//! (error) or 2 (usage).

use std::io::Read as _;
use std::os::unix::process::ExitStatusExt as _;
use std::panic;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use jiff::Timestamp;
use serde::Serialize;

use flow::hook;
use flow::launch;
use flow::services::{self, Refusal};
use flow::session::{self, Session};
use flow::store::Store;
use flow::task;

/// The agent note, `docs/agent-note.md`, printed by `flow guide`.
const AGENT_NOTE: &str = include_str!("../../docs/agent-note.md");

/// Every Session on every CDE, with the PRs each one owns.
#[derive(Parser)]
#[command(name = "flow")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Read Sessions: Claude Code conversations on every CDE; start an IC Session.
    #[command(subcommand)]
    Session(SessionCommand),
    /// Read PRs: GitHub pull requests, each with its owner Session.
    #[command(subcommand)]
    Pr(PrCommand),
    /// Read Branches: git branches a Session created a worktree for; record their Jira issues.
    #[command(subcommand)]
    Branch(BranchCommand),
    /// Tasks: units of delegated work, each with a Task directory; create, join and read them.
    #[command(subcommand)]
    Task(TaskCommand),
    /// Create worktrees through `wt`, recording the calling Session as the owner.
    #[command(subcommand)]
    Wt(WtCommand),
    /// Print the agent note: how agents use flow.
    Guide,
    /// Record a Claude Code hook event read from stdin. Run by Claude Code, not by hand.
    #[command(hide = true)]
    Hook,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Print every Session as a JSON array.
    List,
    /// Print one Session as JSON.
    Get {
        /// The Claude session id; defaults to `$CLAUDE_CODE_SESSION_ID`, the calling Session.
        id: Option<String>,
    },
    /// Start an IC Session in a new, unfocused herdr tab on this CDE (or `--cde`), running
    /// `/flow:ic <brief>`. Prints `{id, name, cde, pane}`.
    Start {
        /// The Task slug: the Session's name and the tab's label (`[a-z][a-z0-9_-]`, at most 32).
        #[arg(long)]
        name: String,
        /// The brief file, on /efs-home.
        #[arg(long)]
        brief: PathBuf,
        /// The herdr workspace the tab goes in, created if missing; defaults to the calling
        /// Session's title, else its id. Set by the remote form, which has no calling Session.
        #[arg(long, hide = true)]
        workspace: Option<String>,
        /// Launch on this other running CDE, by name, over ssh; flow never starts a CDE.
        #[arg(long)]
        cde: Option<String>,
        /// The IC's Session id; flow chooses a fresh UUID by default.
        #[arg(long, hide = true)]
        id: Option<String>,
        /// The repo's main checkout, which must have a wt base; the IC starts there.
        repo: PathBuf,
    },
    /// Delete Sessions from the Store, for clearing test Sessions by hand. Prints
    /// `{"removed": [ids]}`. Removes nothing unless every Session exists and, without `--force`, is done.
    #[command(hide = true)]
    Remove {
        /// Remove Sessions in any status, not only done ones.
        #[arg(long)]
        force: bool,
        /// The Claude session ids to remove.
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PrCommand {
    /// Print every PR that has an owner as a JSON array; `owner` is the owning Session's id.
    List {
        /// Also print the PRs you authored that no Session owns, with `owner` null.
        #[arg(long)]
        all: bool,
    },
    /// Print one PR as JSON, owned or not.
    Get {
        /// The PR's key, `owner/repo#number`.
        key: String,
    },
}

#[derive(Subcommand)]
enum BranchCommand {
    /// Print every Branch whose owner is a listed Session as a JSON array; `at` is when its
    /// worktree was created and `jira` its Jira issue.
    List,
    /// Record the Jira issue a Branch implements.
    #[command(subcommand)]
    Jira(JiraCommand),
}

#[derive(Subcommand)]
enum JiraCommand {
    /// Record `<key>` as the Jira issue of the Branch checked out here (or `--branch`),
    /// replacing any earlier one. Checks neither ownership nor Jira.
    Set {
        /// The Jira key, such as `ABC-123`.
        key: String,
        /// A branch of the current directory's repository, instead of the one checked out.
        #[arg(long)]
        branch: Option<String>,
    },
    /// Unlink the Branch checked out here (or `--branch`) from its Jira issue.
    Clear {
        /// A branch of the current directory's repository, instead of the one checked out.
        #[arg(long)]
        branch: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Create a Task and its Task directory, `$FLOW_HOME/tasks/<slug>/`. Prints `{slug, dir}`.
    /// Joins no Session.
    Create {
        /// The Task slug: `[a-z][a-z0-9_-]`, at most 32 characters.
        slug: String,
    },
    /// Put the calling Session (`$CLAUDE_CODE_SESSION_ID`) on the Task, replacing any Task it
    /// was on.
    Join {
        /// The slug of a Task made with `flow task create`.
        slug: String,
    },
    /// Print every Task a listed Session is on as a JSON array, ordered by slug.
    List,
    /// Print one Task as JSON.
    Get {
        /// The Task slug.
        slug: String,
    },
    /// Record the Resources a Task points at: links to its Jira issues, docs and files.
    #[command(subcommand)]
    Resource(ResourceCommand),
}

#[derive(Subcommand)]
enum ResourceCommand {
    /// Add a Resource to the calling Session's Task (or `--task`), replacing the title of one
    /// with the same URL. Prints `{task, resources}`.
    Add {
        /// An absolute `https`, `http` or `file` URL.
        url: String,
        /// What to show instead of the URL.
        #[arg(long)]
        title: Option<String>,
        /// The Task's slug, instead of the calling Session's Task.
        #[arg(long)]
        task: Option<String>,
    },
    /// Remove the Resource with this URL from the calling Session's Task (or `--task`).
    /// Prints `{task, resources}`.
    Remove {
        /// The Resource's URL.
        url: String,
        /// The Task's slug, instead of the calling Session's Task.
        #[arg(long)]
        task: Option<String>,
    },
}

#[derive(Subcommand)]
enum WtCommand {
    /// Run `wt create <branch> [wt args]` and, if it succeeds inside a Claude Code Session,
    /// record that Session as the owner of the branch's PRs. Use it instead of `wt create`.
    // `--help` goes to `wt`; `flow help wt create` shows flow's help.
    #[command(disable_help_flag = true)]
    Create {
        /// The branch to create a worktree for.
        branch: String,
        /// Passed to `wt create` as they are.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> ExitCode {
    // A hook's facts are stamped with when the hook process started.
    let started = Timestamp::now();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return usage_error(err),
    };
    let store = Store::from_env();
    match cli.command {
        None => {
            print!("{}", Cli::command().render_help());
            ExitCode::SUCCESS
        }
        Some(Command::Session(SessionCommand::List)) => match services::session_list(&store) {
            Ok(sessions) => print_json(&sessions),
            Err(err) => store_error(&err),
        },
        Some(Command::Session(SessionCommand::Get { id })) => session_get(&store, id),
        Some(Command::Session(SessionCommand::Start {
            name,
            brief,
            workspace,
            cde,
            id,
            repo,
        })) => session_start(
            &store,
            launch::Request {
                name: &name,
                brief: &brief,
                workspace,
                cde,
                id,
                repo: &repo,
            },
        ),
        Some(Command::Session(SessionCommand::Remove { force, ids })) => {
            session_remove(&store, &ids, force)
        }
        Some(Command::Pr(PrCommand::List { all })) => match if all {
            services::pr_list_all(&store)
        } else {
            services::pr_list(&store)
        } {
            Ok(prs) => print_json(&prs),
            Err(err) => store_error(&err),
        },
        Some(Command::Pr(PrCommand::Get { key })) => match services::pr_get(&store, &key) {
            Ok(Some(pr)) => print_json(&pr),
            Ok(None) => error(
                1,
                &format!("no PR with key {key:?}"),
                "PR keys are `owner/repo#number`; run `flow pr list` to see owned PRs",
            ),
            Err(err) => store_error(&err),
        },
        Some(Command::Branch(BranchCommand::List)) => match services::branch_list(&store) {
            Ok(branches) => print_json(&branches),
            Err(err) => store_error(&err),
        },
        Some(Command::Branch(BranchCommand::Jira(JiraCommand::Set { key, branch }))) => {
            branch_jira(&store, branch.as_deref(), Some(&key))
        }
        Some(Command::Branch(BranchCommand::Jira(JiraCommand::Clear { branch }))) => {
            branch_jira(&store, branch.as_deref(), None)
        }
        Some(Command::Task(command)) => task(&store, command),
        Some(Command::Wt(WtCommand::Create { branch, args })) => wt_create(&store, &branch, &args),
        Some(Command::Guide) => {
            print!("{AGENT_NOTE}");
            ExitCode::SUCCESS
        }
        Some(Command::Hook) => run_hook(&store, started),
    }
}

fn task(store: &Store, command: TaskCommand) -> ExitCode {
    let root = task::root_from_env();
    match command {
        TaskCommand::Create { slug } if !task::is_slug(&slug) => error(
            2,
            &format!("{slug:?} is not a usable Task slug"),
            "use lowercase letters, digits, `-` and `_`, starting with a letter, at most 32 characters",
        ),
        TaskCommand::Create { slug } => match task::create(&root, &slug) {
            Ok(Some(dir)) => print_json(&serde_json::json!({ "slug": slug, "dir": dir })),
            Ok(None) => error(
                1,
                &format!("Task {slug:?} already exists"),
                "choose another slug; `flow task get <slug>` reads the existing Task",
            ),
            Err(err) => error(
                1,
                &format!("{err:#}"),
                "the Task directory could not be created",
            ),
        },
        TaskCommand::Join { slug } => {
            let Some(id) = std::env::var("CLAUDE_CODE_SESSION_ID")
                .ok()
                .filter(|id| !id.is_empty())
            else {
                return error(
                    1,
                    "CLAUDE_CODE_SESSION_ID is not set",
                    "run `flow task join` inside a Claude Code Session",
                );
            };
            if !task::exists(&root, &slug) {
                return no_task(&slug);
            }
            let facet = session::Task { slug: slug.clone() };
            match Session::put(store, &id, &facet, Timestamp::now()) {
                Ok(_) => print_json(&serde_json::json!({ "session": id, "task": slug })),
                Err(err) => store_error(&err),
            }
        }
        TaskCommand::List => match task::list(store, &root) {
            Ok(tasks) => print_json(&tasks),
            Err(err) => store_error(&err),
        },
        TaskCommand::Get { slug } => match task::get(store, &root, &slug) {
            Ok(Some(task)) => print_json(&task),
            Ok(None) => no_task(&slug),
            Err(err) => store_error(&err),
        },
        TaskCommand::Resource(command) => task_resource(store, &root, command),
    }
}

/// Runs `flow task resource add/remove`, printing `{task, resources}`.
fn task_resource(store: &Store, root: &std::path::Path, command: ResourceCommand) -> ExitCode {
    let (url, slug) = match &command {
        ResourceCommand::Add { url, task, .. } | ResourceCommand::Remove { url, task } => {
            (url, task.clone())
        }
    };
    if !task::is_resource_url(url) {
        return error(
            2,
            &format!("{url:?} is not an absolute https, http or file URL"),
            "pass the full URL, such as `https://example.com/doc` or `file:///efs-home/notes.md`",
        );
    }
    let slug = match slug {
        Some(slug) => slug,
        None => match calling_task(store) {
            Ok(Some(slug)) => slug,
            Ok(None) => {
                return error(
                    1,
                    "the calling Session is on no Task",
                    "pass --task <slug>, or run `flow task join <slug>` first",
                );
            }
            Err(err) => return store_error(&err),
        },
    };
    if !task::exists(root, &slug) {
        return no_task(&slug);
    }
    let resources = match command {
        ResourceCommand::Add { url, title, .. } => {
            task::add_resource(store, &slug, task::Resource { url, title })
        }
        ResourceCommand::Remove { url, .. } => task::remove_resource(store, &slug, &url),
    };
    match resources {
        Ok(resources) => print_json(&serde_json::json!({ "task": slug, "resources": resources })),
        Err(err) => store_error(&err),
    }
}

/// The slug of the Task the calling Session (`$CLAUDE_CODE_SESSION_ID`) is on, if any.
fn calling_task(store: &Store) -> anyhow::Result<Option<String>> {
    let Some(id) = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    Ok(services::session_get(store, &id)?.and_then(|session| session.task))
}

fn no_task(slug: &str) -> ExitCode {
    error(
        1,
        &format!("no Task {slug:?}"),
        "run `flow task create` first; `flow task list` lists Tasks with Sessions on them",
    )
}

/// Runs `flow wt create`: `wt`'s output goes straight through and its exit status is flow's.
/// If the worktree was created but ownership was not recorded, the `{error, help}` JSON goes
/// to stderr, since stdout and the exit status belong to `wt`.
fn wt_create(store: &Store, branch: &str, args: &[String]) -> ExitCode {
    let session = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty());
    match services::wt_create(store, branch, args, session.as_deref()) {
        Ok((status, report)) => {
            if let Some(err) = report {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "error": format!("{err:#}"),
                        "help": "run `flow wt create` inside the repository, whose origin remote must be a GitHub URL",
                    })
                );
            }
            match status.code() {
                Some(code) => ExitCode::from(code as u8),
                // Killed by a signal: report it the way a shell does.
                None => ExitCode::from(128 + status.signal().unwrap_or(0) as u8),
            }
        }
        Err(err) => error(1, &format!("{err:#}"), "`wt` must be on PATH"),
    }
}

/// Runs `flow branch jira set/clear`, printing `{branch, jira}`: the Branch's key and the
/// recorded `{key}` or null.
fn branch_jira(store: &Store, branch: Option<&str>, jira: Option<&str>) -> ExitCode {
    if let Some(key) = jira
        && !flow::jira::is_key(key)
    {
        return error(
            2,
            &format!("{key:?} is not a Jira key"),
            "a Jira key is the project key, a dash and the issue number, such as `ABC-123`",
        );
    }
    match services::branch_jira(store, branch, jira) {
        Ok(branch) => print_json(&serde_json::json!({
            "branch": branch,
            "jira": jira.map(|key| serde_json::json!({ "key": key })),
        })),
        Err(err) => error(
            1,
            &format!("{err:#}"),
            "run it inside the repository, whose origin remote must be a GitHub URL, with the branch checked out or named with --branch",
        ),
    }
}

fn session_get(store: &Store, id: Option<String>) -> ExitCode {
    let id = id.or_else(|| {
        std::env::var("CLAUDE_CODE_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty())
    });
    let Some(id) = id else {
        return error(
            2,
            "no Session id given and CLAUDE_CODE_SESSION_ID is not set",
            "pass a Session id: `flow session get <id>`; `flow session list` lists them",
        );
    };
    match services::session_get(store, &id) {
        Ok(Some(session)) => print_json(&session),
        Ok(None) => error(
            1,
            &format!("no Session with id {id:?}"),
            "run `flow session list` to see every Session",
        ),
        Err(err) => store_error(&err),
    }
}

fn session_start(store: &Store, request: launch::Request) -> ExitCode {
    match launch::session_start(store, request) {
        Ok(outcome) => {
            println!("{}", outcome.output);
            ExitCode::from(outcome.code)
        }
        Err(failure) => error(failure.code, &failure.error, &failure.help),
    }
}

fn session_remove(store: &Store, ids: &[String], force: bool) -> ExitCode {
    match services::session_remove(store, ids, force) {
        Ok(Ok(())) => print_json(&serde_json::json!({ "removed": ids })),
        Ok(Err(Refusal::Missing(id))) => error(
            1,
            &format!("no Session with id {id:?}; nothing was removed"),
            "run `flow session list` to see every Session",
        ),
        Ok(Err(Refusal::NotDone(id))) => error(
            1,
            &format!("Session {id:?} is not done; nothing was removed"),
            "pass --force to remove Sessions in any status",
        ),
        Err(err) => store_error(&err),
    }
}

/// Runs `flow hook`: always exits 0 and prints nothing; every failure, including a panic,
/// becomes one line in the hook log.
fn run_hook(store: &Store, started: Timestamp) -> ExitCode {
    let log_store = store.clone();
    panic::set_hook(Box::new(move |info| {
        hook::log_error(&log_store, started, &format!("panic: {info}"));
    }));
    let result = panic::catch_unwind(|| {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .map_err(anyhow::Error::from)
            .and_then(|_| hook::handle_hook(store, &stdin, started))
    });
    if let Ok(Err(err)) = result {
        hook::log_error(store, started, &format!("{err:#}"));
    }
    ExitCode::SUCCESS
}

fn print_json(value: &impl Serialize) -> ExitCode {
    match serde_json::to_string(value) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => error(1, &err.to_string(), "this is a bug in flow"),
    }
}

fn store_error(err: &anyhow::Error) -> ExitCode {
    error(1, &format!("{err:#}"), services::STORE_ERROR_HELP)
}

fn error(code: u8, message: &str, help: &str) -> ExitCode {
    println!("{}", serde_json::json!({ "error": message, "help": help }));
    ExitCode::from(code)
}

/// Prints clap's `--help` output as is; any other parse failure becomes `{error, help}` JSON
/// on stdout with exit code 2.
fn usage_error(err: clap::Error) -> ExitCode {
    if err.kind() == ErrorKind::DisplayHelp {
        print!("{err}");
        return ExitCode::SUCCESS;
    }
    let rendered = err.render().to_string();
    let message = rendered
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ");
    error(2, message, "run `flow --help` to list commands")
}
