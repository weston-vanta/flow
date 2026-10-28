use jiff::Timestamp;
use serde::Serialize;

use super::TaskView;
use crate::paths::StateDir;
use crate::render;
use crate::state::Store;
use crate::state::requests::Request;
use crate::state::task::{Status, TaskRecord};

const WHAT_WIDTH: usize = 80;
const NO_HOST: &str = "-";

#[derive(Debug, Serialize)]
struct StatusView<'a> {
    tasks: Vec<TaskView<'a>>,
    requests: Vec<RequestView<'a>>,
    adopted: usize,
}

#[derive(Debug, Serialize)]
struct RequestView<'a> {
    id: &'a str,
    #[serde(flatten)]
    request: &'a Request,
}

/// Open tasks, or with `all` every task, oldest first; then their holds, then pending dispatch
/// requests, then the number of adopted sessions in the cached inventories.
pub fn run(state: &StateDir, all: bool, json: bool) -> anyhow::Result<()> {
    let store = Store::open(state);
    let mut tasks = store.list_tasks()?;
    let requests: Vec<(String, Request)> = store
        .pending_request_reads()?
        .into_iter()
        .filter_map(|(id, request)| match request {
            Ok(request) => Some((id, request)),
            Err(err) => {
                eprintln!("skipping request {id}: {err:#}");
                None
            }
        })
        .collect();
    let adopted = super::sessions::count_adopted(state, &tasks)?;
    if !all {
        tasks.retain(|(_, status)| status.state.is_open());
    }
    if json {
        return super::print_json(&StatusView {
            tasks: super::task_views(&tasks),
            requests: requests
                .iter()
                .map(|(id, request)| RequestView { id, request })
                .collect(),
            adopted,
        });
    }
    super::print(&render_text(
        &tasks,
        all,
        &requests,
        adopted,
        Timestamp::now(),
    ))
}

fn render_text(
    tasks: &[(TaskRecord, Status)],
    all: bool,
    requests: &[(String, Request)],
    adopted: usize,
    now: Timestamp,
) -> String {
    let mut out = if tasks.is_empty() {
        if all { "no tasks\n" } else { "no open tasks\n" }.to_owned()
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
    if !requests.is_empty() {
        out.push_str("requests:\n");
        out.push_str(&render::table(&request_rows(requests)));
    }
    out.push_str(&format!("adopted: {adopted} sessions\n"));
    out
}

/// `req-id  from-task  host  title`, `-` for a request that names no host.
fn request_rows(requests: &[(String, Request)]) -> Vec<Vec<String>> {
    requests
        .iter()
        .map(|(id, req)| {
            vec![
                id.clone(),
                req.requested_by.clone(),
                req.host.clone().unwrap_or_else(|| NO_HOST.to_owned()),
                req.title.clone(),
            ]
        })
        .collect()
}
