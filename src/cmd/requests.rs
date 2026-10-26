use serde::Serialize;

use crate::paths::StateDir;
use crate::state::Store;
use crate::state::requests::Request;

const NO_CDE: &str = "-";

#[derive(Debug, Serialize)]
struct RequestView<'a> {
    id: &'a str,
    #[serde(flatten)]
    request: &'a Request,
}

/// Pending dispatch requests across all tasks: `req-id  from-task  cde  title`.
pub fn run_list(state: &StateDir, json: bool) -> anyhow::Result<()> {
    let requests = Store::open(state).pending_requests()?;
    if json {
        let views: Vec<_> = requests
            .iter()
            .map(|(id, request)| RequestView { id, request })
            .collect();
        return super::print_json(&views);
    }
    super::print(&render_text(&requests))
}

/// Marks a request dispatched so it stops listing.
pub fn run_consume(state: &StateDir, req_id: &str) -> anyhow::Result<()> {
    Store::open(state).consume_request(req_id)
}

fn render_text(requests: &[(String, Request)]) -> String {
    if requests.is_empty() {
        return "no pending requests\n".to_owned();
    }
    let rows: Vec<Vec<String>> = requests
        .iter()
        .map(|(id, req)| {
            vec![
                id.clone(),
                req.requested_by.clone(),
                req.cde.clone().unwrap_or_else(|| NO_CDE.to_owned()),
                req.title.clone(),
            ]
        })
        .collect();
    crate::render::table(&rows)
}
