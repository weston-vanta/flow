use clap::{Parser, Subcommand};

use flow::paths::StateDir;

#[derive(Debug, Parser)]
#[command(
    name = "flow",
    version,
    about = "Supervise agent sessions across cloud dev environments"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show the state directory and open work
    Status,
    /// Report and manage task state
    Task,
    /// Start a worker session for a new task
    Dispatch,
    /// List CDEs and run commands on them
    Cde,
    /// Show worktrees, agents, and PRs per CDE
    Inventory,
    /// List agent sessions across CDEs
    Sessions,
    /// Watch tasks and exit when the supervisor should wake
    Watch,
    /// Drain and acknowledge queued wakes
    Wake,
    /// Stop hook that blocks ending a turn while work is unwatched
    Guard,
    /// Receive harness hook events
    Hook,
    /// Start or reattach the supervisor session
    Up,
    /// Attach to the supervisor session
    Attach,
}

pub fn run(cli: Cli, state: &StateDir) -> anyhow::Result<()> {
    match cli.command {
        Command::Status => {
            println!("state: {}", state.root().display());
            Ok(())
        }
        Command::Task
        | Command::Dispatch
        | Command::Cde
        | Command::Inventory
        | Command::Sessions
        | Command::Watch
        | Command::Wake
        | Command::Guard
        | Command::Hook
        | Command::Up
        | Command::Attach => anyhow::bail!("not implemented"),
    }
}
