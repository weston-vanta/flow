//! Sibling-work requests a task files for the supervisor, at
//! `tasks/<task-id>/requests/<req-id>.json`. Dispatching one renames it to `<req-id>.json.consumed`.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Store, ids, read_json, write_json};

const REQUESTS_DIR: &str = "requests";
const CONSUMED_SUFFIX: &str = ".consumed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub title: String,
    pub brief_path: PathBuf,
    pub cde: Option<String>,
    pub requested_by: String,
}

impl Store {
    /// Writes `req` under task `from` with a fresh request id and returns the id.
    pub fn add_request(&self, from: &str, req: &Request) -> anyhow::Result<String> {
        let dir = self.existing_task_dir(from)?;
        let requests = dir.join(REQUESTS_DIR);
        std::fs::create_dir_all(&requests)
            .with_context(|| format!("creating {}", requests.display()))?;
        let mut rng = rand::rng();
        let (id, path) = loop {
            let id = ids::new_task_id(Timestamp::now(), &mut rng);
            let path = request_path(&requests, &id);
            if !path.exists() && !consumed_path(&path).exists() {
                break (id, path);
            }
        };
        write_json(&path, req).with_context(|| format!("writing {}", path.display()))?;
        Ok(id)
    }

    /// Every request not yet consumed, across all tasks, ordered by task then request id.
    pub fn pending_requests(&self) -> anyhow::Result<Vec<(String, Request)>> {
        let mut out = Vec::new();
        for task in self.task_ids()? {
            let requests = self.tasks.join(&task).join(REQUESTS_DIR);
            for id in request_ids(&requests)? {
                out.push((id.clone(), read_json(&request_path(&requests, &id))?));
            }
        }
        Ok(out)
    }

    /// Marks request `req_id` dispatched so it no longer lists as pending.
    pub fn consume_request(&self, req_id: &str) -> anyhow::Result<()> {
        if !ids::is_task_id(req_id) {
            bail!("invalid request id: {req_id:?}");
        }
        for task in self.task_ids()? {
            let path = request_path(&self.tasks.join(&task).join(REQUESTS_DIR), req_id);
            if path.is_file() {
                let consumed = consumed_path(&path);
                return std::fs::rename(&path, &consumed)
                    .with_context(|| format!("renaming {}", path.display()));
            }
        }
        bail!("no pending request: {req_id}")
    }
}

fn request_path(requests: &Path, id: &str) -> PathBuf {
    requests.join(format!("{id}.json"))
}

fn consumed_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(CONSUMED_SUFFIX);
    PathBuf::from(name)
}

/// Ids of the `<req-id>.json` files in `requests`, sorted. A missing directory has none.
fn request_ids(requests: &Path) -> anyhow::Result<Vec<String>> {
    let entries = match std::fs::read_dir(requests) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("listing {}", requests.display())),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", requests.display()))?;
        if let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            && ids::is_task_id(id)
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}
