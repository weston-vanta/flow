//! `flow`: the CLI. Prints JSON on stdout; errors are `{error, help}` JSON with exit code 1
//! (error) or 2 (usage).

use std::io::Read as _;
use std::panic;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use jiff::Timestamp;
use serde::Serialize;

use flow::hook::{self, HookEnv};
use flow::services;
use flow::store::Store;

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
        Some(Command::Hook) => run_hook(&store, started),
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
            .and_then(|_| hook::handle_hook(store, &stdin, &HookEnv::from_env(), started))
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
    error(
        1,
        &format!("{err:#}"),
        "the Store could not be read or holds a malformed facet; the error names the path (FLOW_STORE, default /efs-home/flow/store)",
    )
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
