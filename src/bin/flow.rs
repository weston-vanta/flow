//! `flow`: the CLI. Prints JSON on stdout; errors are `{error, help}` JSON with exit code 1
//! (error) or 2 (usage).

use std::io::Read as _;
use std::os::unix::process::ExitStatusExt as _;
use std::panic;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use jiff::Timestamp;
use serde::Serialize;

use flow::hook;
use flow::services;
use flow::store::Store;

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
    /// Read Sessions: Claude Code conversations on every CDE.
    #[command(subcommand)]
    Session(SessionCommand),
    /// Read PRs: GitHub pull requests, each with its owner Session.
    #[command(subcommand)]
    Pr(PrCommand),
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
}

#[derive(Subcommand)]
enum PrCommand {
    /// Print every PR that has an owner as a JSON array; `owner` is the owning Session's id.
    List,
    /// Print one PR as JSON, owned or not.
    Get {
        /// The PR's key, `owner/repo#number`.
        key: String,
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
        Some(Command::Pr(PrCommand::List)) => match services::pr_list(&store) {
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
        Some(Command::Wt(WtCommand::Create { branch, args })) => wt_create(&store, &branch, &args),
        Some(Command::Guide) => {
            print!("{AGENT_NOTE}");
            ExitCode::SUCCESS
        }
        Some(Command::Hook) => run_hook(&store, started),
    }
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
