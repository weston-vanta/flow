//! Sibling-work requests a task files for the supervisor, at
//! `tasks/<task-id>/requests/<req-id>.json`, with the brief copied beside it as
//! `<req-id>.brief.md` so the supervisor can read it from any host. Dispatching a request renames
//! its JSON to `<req-id>.json.consumed` and leaves the brief in place. Request ids are unique
//! across all tasks, consumed requests included.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Store, fs, ids, read_json};

const REQUESTS_DIR: &str = "requests";
const CONSUMED_SUFFIX: &str = ".consumed";
const BRIEF_SUFFIX: &str = ".brief.md";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub title: String,
    pub brief_path: PathBuf,
    pub host: Option<String>,
    pub requested_by: String,
}

/// What a task asks for when it files a request.
#[derive(Debug, Clone, Copy)]
pub struct NewRequest<'a> {
    pub title: &'a str,
    pub host: Option<&'a str>,
    /// The brief's contents, copied into the request directory.
    pub brief: &'a [u8],
}

impl Store {
    /// Files `new` under task `from` with a fresh request id and returns the id and the request
    /// as written.
    pub fn add_request(&self, from: &str, new: &NewRequest) -> anyhow::Result<(String, Request)> {
        let mut rng = rand::rng();
        self.add_request_with_ids(from, new, || ids::new_task_id(Timestamp::now(), &mut rng))
    }

    /// `add_request` with candidate ids drawn from `next_id`. The id is chosen and the files
    /// created under the state-wide `requests.lock`, so concurrent writers in any task cannot
    /// pick the same id. The brief is written first, so a listed request always has its brief.
    fn add_request_with_ids(
        &self,
        from: &str,
        new: &NewRequest,
        mut next_id: impl FnMut() -> String,
    ) -> anyhow::Result<(String, Request)> {
        let requests = self.existing_task_dir(from)?.join(REQUESTS_DIR);
        std::fs::create_dir_all(&requests)
            .with_context(|| format!("creating {}", requests.display()))?;
        fs::with_lock(&self.requests_lock_target(), || {
            let id = loop {
                let id = next_id();
                if !self.request_id_taken(&id).map_err(io::Error::other)? {
                    break id;
                }
            };
            let brief_path = brief_path(&requests, &id);
            fs::write_new_atomic(&brief_path, new.brief)?;
            let req = Request {
                title: new.title.to_owned(),
                brief_path,
                host: new.host.map(str::to_owned),
                requested_by: from.to_owned(),
            };
            let mut bytes = serde_json::to_vec_pretty(&req)?;
            bytes.push(b'\n');
            fs::write_new_atomic(&request_path(&requests, &id), &bytes)?;
            Ok((id, req))
        })
        .with_context(|| format!("adding a request to task {from}"))
    }

    /// Locking this path takes `<state>/requests.lock`, which serializes request creation.
    fn requests_lock_target(&self) -> PathBuf {
        self.tasks.with_file_name(REQUESTS_DIR)
    }

    /// Whether any task holds request `id`, pending or consumed, or a brief under that id.
    fn request_id_taken(&self, id: &str) -> anyhow::Result<bool> {
        for task in self.task_ids()? {
            let requests = self.tasks.join(&task).join(REQUESTS_DIR);
            let path = request_path(&requests, id);
            if path.exists() || consumed_path(&path).exists() || brief_path(&requests, id).exists()
            {
                return Ok(true);
            }
        }
        Ok(false)
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

fn brief_path(requests: &Path, id: &str) -> PathBuf {
    requests.join(format!("{id}{BRIEF_SUFFIX}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::task::{Harness, TaskRecord};

    fn store_with_tasks(ids: &[&str]) -> (tempfile::TempDir, Store) {
        let state = tempfile::tempdir().unwrap();
        let store = Store {
            tasks: state.path().join("tasks"),
        };
        for id in ids {
            let rec = TaskRecord {
                id: (*id).to_owned(),
                parent: None,
                title: "Example task".to_owned(),
                host: "host-a".to_owned(),
                cwd: PathBuf::from("/workspaces/example-repo.wt/example"),
                repo: Some("example-repo".to_owned()),
                harness: Harness::Claude,
                created: Timestamp::now(),
                owner: "supervisor".to_owned(),
            };
            store.create_task(rec, "brief").unwrap();
        }
        (state, store)
    }

    fn request(title: &str) -> NewRequest<'_> {
        NewRequest {
            title,
            host: None,
            brief: b"Update the example docs.\n",
        }
    }

    /// Hands out `ids` in order, then panics: a test that asks for more has looped on a taken id.
    fn fixed_ids<'a>(ids: &'a [&'a str]) -> impl FnMut() -> String + 'a {
        let mut ids = ids.iter();
        move || (*ids.next().expect("ran out of candidate ids")).to_owned()
    }

    #[test]
    fn request_ids_are_unique_across_tasks() {
        let (_state, store) = store_with_tasks(&["aaaaaaab", "aaaaaaac"]);
        let first = store
            .add_request_with_ids("aaaaaaab", &request("First"), fixed_ids(&["aaaaaaaz"]))
            .unwrap()
            .0;
        store.consume_request(&first).unwrap();
        let reused = store
            .add_request_with_ids(
                "aaaaaaab",
                &request("Second"),
                fixed_ids(&["aaaaaaaz", "aaaaaaay"]),
            )
            .unwrap()
            .0;

        let sibling = store
            .add_request_with_ids(
                "aaaaaaac",
                &request("Third"),
                fixed_ids(&["aaaaaaaz", "aaaaaaay", "aaaaaaax"]),
            )
            .unwrap()
            .0;

        assert_eq!(
            (first.as_str(), reused.as_str(), sibling.as_str()),
            ("aaaaaaaz", "aaaaaaay", "aaaaaaax")
        );
        store.consume_request(&sibling).unwrap();
        let pending = store.pending_requests().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "aaaaaaay");
        assert_eq!(pending[0].1.requested_by, "aaaaaaab");
    }

    #[test]
    fn request_create_does_not_overwrite_existing() {
        let (_state, store) = store_with_tasks(&["aaaaaaab"]);
        let requests = store.tasks.join("aaaaaaab").join(REQUESTS_DIR);
        std::fs::create_dir_all(&requests).unwrap();
        let existing = request_path(&requests, "aaaaaaaz");
        std::fs::write(&existing, "{\"title\": \"already here\"}\n").unwrap();

        let id = store
            .add_request_with_ids(
                "aaaaaaab",
                &request("New"),
                fixed_ids(&["aaaaaaaz", "aaaaaaay"]),
            )
            .unwrap()
            .0;

        assert_eq!(id, "aaaaaaay");
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "{\"title\": \"already here\"}\n"
        );
        assert_eq!(
            read_json::<Request>(&request_path(&requests, &id))
                .unwrap()
                .title,
            "New"
        );
    }

    #[test]
    fn request_id_skips_a_leftover_brief() {
        let (_state, store) = store_with_tasks(&["aaaaaaab"]);
        let requests = store.tasks.join("aaaaaaab").join(REQUESTS_DIR);
        std::fs::create_dir_all(&requests).unwrap();
        std::fs::write(brief_path(&requests, "aaaaaaaz"), "left behind\n").unwrap();

        let (id, req) = store
            .add_request_with_ids(
                "aaaaaaab",
                &request("New"),
                fixed_ids(&["aaaaaaaz", "aaaaaaay"]),
            )
            .unwrap();

        assert_eq!(id, "aaaaaaay");
        assert_eq!(req.brief_path, brief_path(&requests, "aaaaaaay"));
        assert_eq!(
            std::fs::read_to_string(&req.brief_path).unwrap(),
            "Update the example docs.\n"
        );
    }
}
