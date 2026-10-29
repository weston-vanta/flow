//! Subcommand implementations. Read commands render compact text, or raw serde JSON of the same
//! data when asked.

pub mod dispatch;
pub mod guard;
pub mod hook;
pub mod init;
pub mod inventory;
pub mod run;
pub mod sessions;
pub mod status;
pub mod steer;
pub mod sync;
pub mod task_adopt;
pub mod task_close;
pub mod task_events;
pub mod task_show;
pub mod task_write;
pub mod up;
pub mod wake;
pub mod watch;

use std::io::{self, BufRead, IsTerminal, Write};

use jiff::Timestamp;
use serde::Serialize;

use crate::render;
use crate::runner;
use crate::state::task::{Status, TaskRecord};

const NO_PANE: &str = "-";
/// Under the fake runner, makes commands treat stdin and stdout as a terminal when set.
const FAKE_TTY_VAR: &str = "FLOW_FAKE_TTY";

/// A request flow turns down as given, such as an unknown host or a directory another agent
/// works in. The binary exits 2 for it, as for a usage error.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

/// One task as `--json` output shows it.
#[derive(Debug, Serialize)]
struct TaskView<'a> {
    task: &'a TaskRecord,
    status: &'a Status,
}

fn task_views(tasks: &[(TaskRecord, Status)]) -> Vec<TaskView<'_>> {
    tasks
        .iter()
        .map(|(task, status)| TaskView { task, status })
        .collect()
}

/// `id  state  host  pane  age  title`, aged by creation time; `pane` is the herdr pane running
/// the task's worker, or `-` when it has none.
fn task_rows(tasks: &[(TaskRecord, Status)], now: Timestamp) -> Vec<Vec<String>> {
    tasks
        .iter()
        .map(|(rec, status)| {
            vec![
                rec.id.clone(),
                status.state.to_string(),
                rec.host.clone(),
                status
                    .herdr
                    .as_ref()
                    .map_or_else(|| NO_PANE.to_owned(), |target| target.pane_id.clone()),
                render::relative(rec.created, now),
                rec.title.clone(),
            ]
        })
        .collect()
}

/// Whether stdout is a terminal, or the fake runner is told to treat it as one.
fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal() || fake_tty()
}

/// Whether someone is at a terminal to answer a question: stdin and stdout are both terminals,
/// or the fake runner is told to treat them as such.
fn interactive() -> bool {
    (io::stdin().is_terminal() && io::stdout().is_terminal()) || fake_tty()
}

fn fake_tty() -> bool {
    runner::fake_runner_active() && std::env::var_os(FAKE_TTY_VAR).is_some()
}

/// The answer to applying a plan already printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Consent {
    Given,
    Declined,
    /// Nobody is at a terminal to ask and `--yes` was not passed.
    NoTerminal,
}

/// Asks `Proceed? [y/N]` after a blank line unless `yes`, reading the answer from stdin; only
/// `y` or `yes` gives consent.
fn confirm(yes: bool) -> anyhow::Result<Consent> {
    if yes {
        return Ok(Consent::Given);
    }
    if !interactive() {
        return Ok(Consent::NoTerminal);
    }
    print("\nProceed? [y/N] ")?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    let given = matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
    Ok(if given {
        Consent::Given
    } else {
        Consent::Declined
    })
}

fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    let mut out = serde_json::to_string(value)?;
    out.push('\n');
    print(&out)
}

/// Writes `out` to stdout. A reader that stops early (`| head`) is not an error.
fn print(out: &str) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(out.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => Err(err.into()),
        _ => Ok(()),
    }
}
