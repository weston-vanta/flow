use jiff::Timestamp;
use serde::Serialize;

use super::TaskView;
use crate::paths::StateDir;
use crate::render;
use crate::state::Store;
use crate::state::task::{Status, TaskRecord};

const WHAT_WIDTH: usize = 80;

#[derive(Debug, Serialize)]
struct StatusView<'a> {
    tasks: Vec<TaskView<'a>>,
    adopted: usize,
}

/// Open tasks, then their holds, then the number of adopted sessions in the cached inventories.
pub fn run(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let mut tasks = Store::open(state).list_tasks()?;
    let adopted = super::sessions::count_adopted(state, &tasks)?;
    tasks.retain(|(_, status)| status.state.is_open());
    if json {
        return super::print_json(&StatusView {
            tasks: super::task_views(&tasks),
            adopted,
        });
    }
    super::print(&render_text(&tasks, adopted, Timestamp::now()))
}

fn render_text(tasks: &[(TaskRecord, Status)], adopted: usize, now: Timestamp) -> String {
    let mut out = if tasks.is_empty() {
        "no open tasks\n".to_owned()
    } else {
        render::table(&super::task_rows(tasks, now))
    };
    let holds: Vec<Vec<String>> = tasks
        .iter()
        .filter_map(|(rec, status)| {
            let hold = status.hold.as_ref()?;
            Some(vec![
                rec.id.clone(),
                hold.on.to_string(),
                render::truncate(&hold.what, WHAT_WIDTH),
                render::relative(hold.since, now),
            ])
        })
        .collect();
    if !holds.is_empty() {
        out.push_str("holds:\n");
        out.push_str(&render::table(&holds));
    }
    out.push_str(&format!("adopted: {adopted} sessions\n"));
    out
}
