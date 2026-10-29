use std::path::Path;

use anyhow::Context;
use jiff::Timestamp;
use serde::Serialize;

use crate::paths::StateDir;
use crate::render;
use crate::state::Store;
use crate::state::task::{Status, TaskRecord};

const BRIEF_HEAD_LINES: usize = 10;

#[derive(Debug, Serialize)]
struct ShowView<'a> {
    task: &'a TaskRecord,
    status: &'a Status,
    brief: &'a str,
}

/// A key/value block for the task, then the head of its brief.
pub fn run(state: &StateDir, id: &str, json: bool) -> anyhow::Result<()> {
    // The brief's path is printed for the reader to open, so it is absolute and the same on
    // every host.
    let state = &state
        .canonical()
        .with_context(|| format!("resolving {}", state.root().display()))?;
    let store = Store::open(state);
    let (task, status) = store.task(id)?;
    let brief = store.brief(id)?;
    if json {
        return super::print_json(&ShowView {
            task: &task,
            status: &status,
            brief: &brief,
        });
    }
    let brief_path = store.brief_path(id)?;
    super::print(&render_text(
        &task,
        &status,
        &brief,
        &brief_path,
        Timestamp::now(),
    ))
}

fn render_text(
    task: &TaskRecord,
    status: &Status,
    brief: &str,
    brief_path: &Path,
    now: Timestamp,
) -> String {
    let mut fields = vec![
        ("id", task.id.clone()),
        ("title", task.title.clone()),
        ("state", status.state.to_string()),
        ("host", task.host.clone()),
        ("cwd", task.cwd.display().to_string()),
    ];
    if let Some(repo) = &task.repo {
        fields.push(("repo", repo.clone()));
    }
    fields.extend([
        ("harness", task.harness.to_string()),
        ("origin", task.origin.to_string()),
        ("created", render::relative(task.created, now)),
        ("last event", render::relative(status.last_event, now)),
    ]);
    if let Some(parent) = &task.parent {
        fields.push(("parent", parent.clone()));
    }
    if let Some(hold) = &status.hold {
        let since = render::relative(hold.since, now);
        fields.push(("hold", format!("{}: {} ({since})", hold.on, hold.what)));
    }
    if let Some(herdr) = &status.herdr {
        let target = format!(
            "{} {}/{} (workspace {}, {})",
            herdr.host, herdr.session, herdr.pane_id, herdr.workspace_id, herdr.agent
        );
        fields.push(("herdr", target));
    }
    let rows: Vec<Vec<String>> = fields
        .into_iter()
        .map(|(key, value)| vec![key.to_owned(), value])
        .collect();

    let mut out = render::table(&rows);
    out.push('\n');
    let lines: Vec<&str> = brief.lines().collect();
    for line in lines.iter().take(BRIEF_HEAD_LINES) {
        out.push_str(line);
        out.push('\n');
    }
    if lines.len() > BRIEF_HEAD_LINES {
        let more = lines.len() - BRIEF_HEAD_LINES;
        out.push_str(&format!(
            "… ({more} more lines in {})\n",
            brief_path.display()
        ));
    }
    out
}
