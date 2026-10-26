use jiff::Timestamp;
use serde::Serialize;

use super::TaskView;
use crate::paths::StateDir;
use crate::render;
use crate::state::Store;

#[derive(Debug, Serialize)]
struct ListView<'a> {
    tasks: Vec<TaskView<'a>>,
}

/// Tasks oldest first; `done` and `orphaned` ones only with `all`.
pub fn run(state: &StateDir, all: bool, json: bool) -> anyhow::Result<()> {
    let mut tasks = Store::open(state).list_tasks()?;
    if !all {
        tasks.retain(|(_, status)| status.state.is_open());
    }
    if json {
        return super::print_json(&ListView {
            tasks: super::task_views(&tasks),
        });
    }
    if tasks.is_empty() {
        return super::print(if all { "no tasks\n" } else { "no open tasks\n" });
    }
    super::print(&render::table(&super::task_rows(&tasks, Timestamp::now())))
}
