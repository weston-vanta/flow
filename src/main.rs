mod cli;

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use flow::paths::{PathError, StateDir};

use crate::cli::Cli;

const PATH_ERROR_EXIT: u8 = 3;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err:#}");
            if err.downcast_ref::<PathError>().is_some() {
                ExitCode::from(PATH_ERROR_EXIT)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    // The binary always runs from this repo's own build, so the build-time manifest dir is the repo root.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let state = StateDir::resolve(repo_root)?;
    cli::run(cli, &state)
}
