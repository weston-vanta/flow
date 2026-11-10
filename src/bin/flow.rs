//! `flow`: the CLI. Prints JSON on stdout; errors are `{error, help}` JSON with exit code 1
//! (error) or 2 (usage).

use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};

/// Every Session on every CDE, with the PRs each one owns.
#[derive(Parser)]
#[command(name = "flow")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return usage_error(err),
    };
    match cli.command {
        None => {
            print!("{}", Cli::command().render_help());
            ExitCode::SUCCESS
        }
    }
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
    let body = serde_json::json!({
        "error": message,
        "help": "run `flow --help` to list commands",
    });
    println!("{body}");
    ExitCode::from(2)
}
