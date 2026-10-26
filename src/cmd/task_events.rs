use jiff::Timestamp;
use serde::Serialize;

use crate::paths::StateDir;
use crate::render;
use crate::state::Store;
use crate::state::events::{self, Event};

const DATA_WIDTH: usize = 80;

#[derive(Debug, Serialize)]
struct EventsView<'a> {
    events: &'a [Event],
}

/// The task's events, oldest first, limited to the newest `last` when given.
pub fn run(state: &StateDir, id: &str, last: Option<usize>, json: bool) -> anyhow::Result<()> {
    let store = Store::open(state);
    store.task(id)?;
    let all = events::read_all(&store.task_dir(id)?)?;
    let skip = last.map_or(0, |n| all.len().saturating_sub(n));
    let shown = &all[skip..];
    if json {
        return super::print_json(&EventsView { events: shown });
    }
    super::print(&render_text(shown, Timestamp::now()))
}

/// `age  source  kind  data`, with `data` as one-line JSON cut to 80 characters.
fn render_text(events: &[Event], now: Timestamp) -> String {
    let rows: Vec<Vec<String>> = events
        .iter()
        .map(|ev| {
            vec![
                render::relative(ev.ts, now),
                ev.source.clone(),
                ev.kind.clone(),
                render::truncate(&ev.data.to_string(), DATA_WIDTH),
            ]
        })
        .collect();
    render::table(&rows)
}
