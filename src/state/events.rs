use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use anyhow::Context;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

const EVENTS_FILE: &str = "events.jsonl";

/// One line of a task's `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    pub source: String,
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Appends `ev` as one line to `<dir>/events.jsonl` under the file's lock.
pub fn append(dir: &Path, ev: &Event) -> anyhow::Result<()> {
    let path = dir.join(EVENTS_FILE);
    let mut line = serde_json::to_vec(ev)?;
    line.push(b'\n');
    super::fs::with_lock(&path, || {
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&line)?;
        file.sync_all()
    })
    .with_context(|| format!("appending to {}", path.display()))
}

/// Every event in `<dir>/events.jsonl`, oldest first. A missing file has no events.
pub fn read_all(dir: &Path) -> anyhow::Result<Vec<Event>> {
    let path = dir.join(EVENTS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| {
            serde_json::from_str(line)
                .with_context(|| format!("parsing {} line {}", path.display(), i + 1))
        })
        .collect()
}
