//! Subcommand implementations. Read commands render compact text, or raw serde JSON of the same
//! data when asked.

pub mod dispatch;
pub mod guard;
pub mod hook;
pub mod host;
pub mod init;
pub mod inventory;
pub mod sessions;
pub mod status;
pub mod steer;
pub mod task_brief;
pub mod task_close;
pub mod task_events;
pub mod task_show;
pub mod task_write;
pub mod up;
pub mod wake;
pub mod watch;

use std::io::{self, Write};

use jiff::Timestamp;
use serde::Serialize;

use crate::render;
use crate::state::task::{Status, TaskRecord};

const NO_PANE: &str = "-";

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
