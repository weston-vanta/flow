mod cli;

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use flow::cmd::Refused;
use flow::paths::{PathError, STATE_ENV, StateDir};
use flow::supervisor;

use crate::cli::Cli;

const REFUSED_EXIT: u8 = 2;
const PATH_ERROR_EXIT: u8 = 3;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            if err.downcast_ref::<Refused>().is_some() {
                ExitCode::from(REFUSED_EXIT)
            } else if err.downcast_ref::<PathError>().is_some() {
                ExitCode::from(PATH_ERROR_EXIT)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    // The repo's hooks run in every Claude Code session started in it; outside the supervisor
    // they do nothing, even where no state directory exists yet.
    if cli.command.is_supervisor_hook() && !supervisor::is_supervisor_session() {
        return Ok(ExitCode::SUCCESS);
    }
    // The binary always runs from this repo's own build, so the build-time manifest dir is the repo root.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let state = match StateDir::resolve(repo_root) {
        Ok(state) => state,
        Err(err) => {
            if let Some(name) = cli.command.supervisor_hook_name() {
                eprintln!("flow {name}: {}", hook_state_error(&err));
                return Ok(ExitCode::from(PATH_ERROR_EXIT));
            }
            return Err(err.into());
        }
    };
    cli::run(cli, &state, repo_root)
}

/// Why a supervisor hook found no state directory, naming where the path came from: a
/// `FLOW_STATE` left in herdr's environment otherwise breaks every hook unnoticed.
fn hook_state_error(err: &PathError) -> String {
    let PathError::Missing(path) = err;
    let source = if std::env::var_os(STATE_ENV).is_some() {
        format!("from ${STATE_ENV}")
    } else {
        format!("the repo's state link; ${STATE_ENV} is unset")
    };
    format!(
        "state directory not found: {} ({source}); the supervisor's hooks do nothing until \
         {STATE_ENV} names the state directory",
        path.display()
    )
}
