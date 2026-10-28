use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

use flow::cmd;
use flow::cmd::Refused;
use flow::cmd::dispatch::{BriefSource, DirSource, DispatchArgs, TaskSource};
use flow::cmd::task_write::ReportedState;
use flow::paths::StateDir;
use flow::state::task::HoldOn;

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
    /// Write state/config.toml from what this machine can tell, then refresh the inventory
    Init {
        /// Replace an existing state/config.toml
        #[arg(long)]
        force: bool,
    },
    /// Show open tasks, their holds, pending dispatch requests, and adopted sessions
    Status {
        /// Include done and orphaned tasks
        #[arg(long)]
        all: bool,
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
    Dispatch(DispatchCli),
    /// List hosts and run commands on them
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Show worktrees, agents, and PRs per host
    Inventory {
        /// Discover hosts from Ona (or the static list), probe each running one, and list open
        /// PRs before showing them
        #[arg(long)]
        refresh: bool,
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
    /// List herdr agents across hosts, with their tasks or as adopted sessions
    Sessions {
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
        /// Print each session's latest summary under its row
        #[arg(long)]
        summaries: bool,
    },
    /// Wake the supervisor when an adopted session changes state
    Follow {
        /// Host the session runs on
        host: String,
        /// herdr pane the session runs in, from `flow sessions --json`
        pane_id: String,
    },
    /// Stop waking the supervisor for an adopted session
    Unfollow {
        /// Host the session runs on
        host: String,
        /// herdr pane the session runs in
        pane_id: String,
    },
    /// Watch tasks and exit 2 with a reason when the supervisor should wake
    Watch {
        /// Wake after this many seconds even when nothing happened [default: 3600]
        #[arg(long, value_name = "SECS")]
        heartbeat: Option<u64>,
        /// Watch for one poll interval and exit 0 when nothing woke
        #[arg(long)]
        once: bool,
    },
    /// Drain and acknowledge queued wakes
    Wake {
        #[command(subcommand)]
        command: WakeCommand,
    },
    /// Stop hook that blocks ending a turn while work is unwatched
    Guard,
    /// Receive harness hook events
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// Send a message to a task's worker and log it
    Steer {
        id: String,
        /// The message, as the worker should read it
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Attach to the live supervisor session, or start one on this host and attach
    Up,
}

impl Command {
    /// Whether this is one of the supervisor's hooks in `.claude/settings.json`.
    pub fn is_supervisor_hook(&self) -> bool {
        self.supervisor_hook_name().is_some()
    }

    /// The subcommand words of this command when it is one of the supervisor's hooks.
    pub fn supervisor_hook_name(&self) -> Option<&'static str> {
        match self {
            Command::Guard => Some("guard"),
            Command::Watch { .. } => Some("watch"),
            Command::Hook {
                command: HookCommand::Supervisor,
            } => Some("hook supervisor"),
            _ => None,
        }
    }
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("dir_source")))]
pub struct DispatchCli {
    /// Pending dispatch request to dispatch, from `flow status`; it supplies the brief, and the
    /// title, host, and parent unless given
    #[arg(long, value_name = "REQ_ID")]
    request: Option<String>,
    /// Host to run the worker on [default: the request's host, else this machine]
    #[arg(long, value_name = "NAME")]
    host: Option<String>,
    /// Required unless --request supplies it
    #[arg(long)]
    title: Option<String>,
    /// File holding the ask, read on this machine
    #[arg(long, value_name = "FILE")]
    brief_file: Option<PathBuf>,
    /// The ask itself
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    brief: Option<String>,
    /// What must be true for the task to be done, often a `- ` list
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    criteria: String,
    /// Existing directory on the host to work in; with neither this nor --branch, the worker
    /// runs in the task's own work directory
    #[arg(long, value_name = "PATH", group = "dir_source")]
    dir: Option<PathBuf>,
    /// New branch to create a worktree for in the repo
    #[arg(long, value_name = "NAME", group = "dir_source")]
    branch: Option<String>,
    /// Configured repo to create the --branch worktree in; needed when several are configured
    #[arg(long, value_name = "NAME")]
    repo: Option<String>,
    /// Task this one was split from [default: the request's requesting task]
    #[arg(long, value_name = "ID")]
    parent: Option<String>,
    /// Dispatch even when another agent already works in the directory
    #[arg(long)]
    allow_shared_dir: bool,
}

impl TryFrom<DispatchCli> for DispatchArgs {
    type Error = anyhow::Error;

    fn try_from(cli: DispatchCli) -> anyhow::Result<DispatchArgs> {
        let source = match (cli.request, cli.brief_file, cli.brief) {
            (Some(req_id), None, None) => TaskSource::Request {
                req_id,
                title: cli.title,
            },
            (Some(_), _, _) => {
                return Err(Refused(
                    "--brief and --brief-file do not apply with --request: the request supplies \
                     the brief"
                        .to_owned(),
                )
                .into());
            }
            (None, file, text) => {
                let brief = match (file, text) {
                    (Some(path), None) => BriefSource::File(path),
                    (None, Some(text)) => BriefSource::Text(text),
                    _ => {
                        return Err(Refused(
                            "pass exactly one of --request, --brief-file, and --brief".to_owned(),
                        )
                        .into());
                    }
                };
                let title = cli.title.ok_or_else(|| {
                    Refused("--title is required unless --request supplies it".to_owned())
                })?;
                TaskSource::Given { title, brief }
            }
        };
        let dir = match (cli.dir, cli.branch, cli.repo) {
            (None, Some(branch), repo) => DirSource::Branch { branch, repo },
            (_, None, Some(_)) => {
                return Err(Refused("--repo only applies with --branch".to_owned()).into());
            }
            (Some(path), None, None) => DirSource::Path(path),
            (None, None, None) => DirSource::TaskWorkDir,
            (Some(_), Some(_), _) => anyhow::bail!("pass at most one of --dir and --branch"),
        };
        Ok(DispatchArgs {
            host: cli.host,
            source,
            criteria: cli.criteria,
            dir,
            parent: cli.parent,
            allow_shared_dir: cli.allow_shared_dir,
        })
    }
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
    /// Report the task working, blocked, or needing a decision
    Status {
        id: String,
        /// working, blocked, or needs-decision
        state: ReportedState,
        /// What changed; for blocked and needs-decision, what the supervisor must answer
        #[arg(
            long,
            required_if_eq_any([("state", "blocked"), ("state", "needs-decision")])
        )]
        message: Option<String>,
    },
    /// Finish the task with a report
    Done {
        id: String,
        /// File to keep as the task's report.md
        #[arg(long, value_name = "FILE")]
        report: PathBuf,
    },
    /// Ask the supervisor to dispatch sibling work
    RequestDispatch {
        id: String,
        #[arg(long)]
        title: String,
        /// File holding the brief for the new task
        #[arg(long, value_name = "FILE")]
        brief: PathBuf,
        /// Host to run the new task on
        #[arg(long, value_name = "NAME")]
        host: Option<String>,
    },
    /// Answer the task's hold and lift it
    Decide {
        id: String,
        #[arg(long)]
        by: Party,
        #[arg(long, value_name = "TEXT")]
        answer: String,
    },
    /// Hold the task until someone answers
    Hold {
        id: String,
        /// Who must answer
        #[arg(long)]
        on: Party,
        /// What they must answer
        #[arg(long, value_name = "TEXT")]
        what: String,
    },
    /// Mark the task orphaned: its work will not finish
    Orphan {
        id: String,
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    /// Close the task's herdr workspace, keeping its record, working directory and branch
    Close {
        id: String,
        /// Close it even while the task is still open: not done or orphaned
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// List known hosts from the cache, gone ones last, marking the one this runs on
    List {
        /// Print raw JSON instead of the compact view
        #[arg(long)]
        json: bool,
    },
    /// Run a command on a host, passing its output and exit code through
    Run {
        name: String,
        #[arg(last = true, required = true, value_name = "ARGV")]
        argv: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    /// Record a Claude Code hook event read as JSON from stdin
    Claude {
        #[arg(long, value_name = "ID")]
        task: String,
    },
    /// Record the supervisor's Claude Code session from its SessionStart hook on stdin
    Supervisor,
}

#[derive(Debug, Subcommand)]
pub enum WakeCommand {
    /// Print the queued wakes not yet acknowledged
    Drain,
    /// Acknowledge every queued wake up to and including N
    Ack {
        #[arg(long, value_name = "N")]
        through: u64,
    },
}

/// Who answers a hold or made a decision.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Party {
    User,
    Supervisor,
}

impl From<Party> for HoldOn {
    fn from(party: Party) -> HoldOn {
        match party {
            Party::User => HoldOn::User,
            Party::Supervisor => HoldOn::Supervisor,
        }
    }
}

/// Runs `cli`; `repo_root` is this flow checkout.
pub fn run(cli: Cli, state: &StateDir, repo_root: &Path) -> anyhow::Result<ExitCode> {
    let done = match cli.command {
        Command::Init { force } => cmd::init::run(state, repo_root, force),
        Command::Host { command } => return run_host(command, state),
        Command::Status { all, json } => cmd::status::run(state, all, json),
        Command::Task { command } => run_task(command, state),
        Command::Hook {
            command: HookCommand::Claude { task },
        } => cmd::hook::run_claude(state, &task, std::io::stdin().lock()),
        Command::Hook {
            command: HookCommand::Supervisor,
        } => {
            cmd::hook::run_supervisor(state, std::io::stdin().lock());
            Ok(())
        }
        Command::Dispatch(args) => cmd::dispatch::run(state, &args.try_into()?),
        Command::Inventory { refresh, json } => cmd::inventory::run(state, refresh, json),
        Command::Sessions { json, summaries } => cmd::sessions::run(state, json, summaries),
        Command::Follow { host, pane_id } => cmd::sessions::run_follow(state, &host, &pane_id),
        Command::Unfollow { host, pane_id } => cmd::sessions::run_unfollow(state, &host, &pane_id),
        Command::Watch { heartbeat, once } => return cmd::watch::run(state, heartbeat, once),
        Command::Wake { command } => match command {
            WakeCommand::Drain => cmd::wake::run_drain(state),
            WakeCommand::Ack { through } => cmd::wake::run_ack(state, through),
        },
        Command::Guard => return cmd::guard::run(state),
        Command::Steer { id, text } => cmd::steer::run(state, &id, &text),
        Command::Up => cmd::up::run(state),
    };
    done.map(|()| ExitCode::SUCCESS)
}

fn run_host(command: HostCommand, state: &StateDir) -> anyhow::Result<ExitCode> {
    match command {
        HostCommand::List { json } => cmd::host::run_list(state, json).map(|()| ExitCode::SUCCESS),
        HostCommand::Run { name, argv } => {
            let status = cmd::host::run_command(state, &name, &argv)?;
            Ok(ExitCode::from(u8::try_from(status).unwrap_or(u8::MAX)))
        }
    }
}

fn run_task(command: TaskCommand, state: &StateDir) -> anyhow::Result<()> {
    match command {
        TaskCommand::Show { id, json } => cmd::task_show::run(state, &id, json),
        TaskCommand::Brief { id } => cmd::task_brief::run(state, &id),
        TaskCommand::Events { id, last, json } => cmd::task_events::run(state, &id, last, json),
        TaskCommand::Status {
            id,
            state: reported,
            message,
        } => cmd::task_write::run_status(state, &id, reported, message.as_deref()),
        TaskCommand::Done { id, report } => cmd::task_write::run_done(state, &id, &report),
        TaskCommand::RequestDispatch {
            id,
            title,
            brief,
            host,
        } => cmd::task_write::run_request_dispatch(state, &id, &title, &brief, host.as_deref()),
        TaskCommand::Decide { id, by, answer } => {
            cmd::task_write::run_decide(state, &id, by.into(), &answer)
        }
        TaskCommand::Hold { id, on, what } => {
            cmd::task_write::run_hold(state, &id, on.into(), &what)
        }
        TaskCommand::Orphan { id, reason } => cmd::task_write::run_orphan(state, &id, &reason),
        TaskCommand::Close { id, force } => cmd::task_close::run(state, &id, force),
    }
}
