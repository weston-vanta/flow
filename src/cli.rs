use clap::{Parser, Subcommand};

use flow::cmd;
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
    /// Show open tasks, their holds, and adopted sessions
    Status {
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
    /// Report and manage task state
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
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

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// Show a task's fields and the head of its brief
    Show {
        id: String,
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
    /// Print a task's brief in full
    Brief { id: String },
    /// List open tasks
    List {
        /// Include done and orphaned tasks
        #[arg(long)]
        all: bool,
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
    /// Show a task's event log
    Events {
        id: String,
        /// Show only the newest N events
        #[arg(long, value_name = "N")]
        last: Option<usize>,
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
}

pub fn run(cli: Cli, state: &StateDir) -> anyhow::Result<()> {
    match cli.command {
        Command::Status { json } => cmd::status::run(state, json),
        Command::Task { command } => run_task(command, state),
        Command::Dispatch
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

fn run_task(command: TaskCommand, state: &StateDir) -> anyhow::Result<()> {
    match command {
        TaskCommand::Show { id, json } => cmd::task_show::run(state, &id, json),
        TaskCommand::Brief { id } => cmd::task_brief::run(state, &id),
        TaskCommand::List { all, json } => cmd::task_list::run(state, all, json),
        TaskCommand::Events { id, last, json } => cmd::task_events::run(state, &id, last, json),
    }
}
