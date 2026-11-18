//! The Task: a unit of delegated work, named by its slug. A Task exists when its Task
//! directory, `$FLOW_HOME/tasks/<slug>/`, does (ADR-0005); a Session joins it by writing its
//! `task` facet through `flow task join`. A Task points at Resources, kept as the
//! `resources` facet of its `task` record, whose one Writer is `flow task resource`.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::services;
use crate::store::{self, Store};

/// The Store kind of a Task's record, by slug.
pub const KIND: &str = "task";

/// The facet holding a Task's whole list of Resources.
const RESOURCES: &str = "resources";

/// The directory holding every Task directory, `$FLOW_HOME/tasks`.
pub fn root_from_env() -> PathBuf {
    store::home_from_env().join("tasks")
}

/// Whether `slug` is a usable Task slug: `[a-z][a-z0-9_-]{0,31}`.
pub fn is_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && slug.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A Task as `flow task list` and `get` print it.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub slug: String,
    pub dir: PathBuf,
    /// The ids of the listed Sessions on this Task, ordered by id.
    pub sessions: Vec<String>,
    pub resources: Vec<Resource>,
}

/// Something outside flow a Task points at, identified by its URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub url: String,
    pub title: Option<String>,
}

/// Whether `url` is an absolute `https`, `http` or `file` URL.
pub fn is_resource_url(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    if url.chars().any(char::is_whitespace) {
        return false;
    }
    match url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        Some(rest) => !rest.is_empty() && !rest.starts_with('/'),
        None => url
            .strip_prefix("file:///")
            .is_some_and(|path| !path.is_empty()),
    }
}

/// The Resources of Task `slug`, in the order they were first added.
pub fn resources(store: &Store, slug: &str) -> Result<Vec<Resource>> {
    let Some(facet) = store
        .get(KIND, slug)?
        .and_then(|mut entity| entity.facets.remove(RESOURCES))
    else {
        return Ok(Vec::new());
    };
    serde_json::from_value(facet.value)
        .with_context(|| format!("task {slug} has a malformed {RESOURCES} facet"))
}

/// `flow task resource add`: adds `resource` to Task `slug`, or replaces the title of the
/// one with its URL. Returns the new list.
pub fn add_resource(store: &Store, slug: &str, resource: Resource) -> Result<Vec<Resource>> {
    let mut list = resources(store, slug)?;
    match list.iter_mut().find(|r| r.url == resource.url) {
        Some(existing) => existing.title = resource.title,
        None => list.push(resource),
    }
    put_resources(store, slug, list)
}

/// `flow task resource remove`: removes the Resource with `url` from Task `slug`, if it has
/// one; removing the last one removes the `task` record. Returns the new list.
pub fn remove_resource(store: &Store, slug: &str, url: &str) -> Result<Vec<Resource>> {
    let mut list = resources(store, slug)?;
    let before = list.len();
    list.retain(|r| r.url != url);
    if list.len() == before {
        Ok(list)
    } else if list.is_empty() {
        store.remove(KIND, slug)?;
        Ok(list)
    } else {
        put_resources(store, slug, list)
    }
}

fn put_resources(store: &Store, slug: &str, list: Vec<Resource>) -> Result<Vec<Resource>> {
    let value = serde_json::to_value(&list)?;
    store.put_facet(KIND, slug, RESOURCES, &value, Timestamp::now())?;
    Ok(list)
}

/// `flow task create`: creates the Task directory of `slug` under `root`. `Ok(None)` if it
/// already exists. The slug must already be checked with [`is_slug`].
pub fn create(root: &Path, slug: &str) -> Result<Option<PathBuf>> {
    let dir = root.join(slug);
    fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    match fs::create_dir(&dir) {
        Ok(()) => Ok(Some(dir)),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err).with_context(|| format!("create {}", dir.display())),
    }
}

/// Whether the Task `slug` exists under `root`.
pub fn exists(root: &Path, slug: &str) -> bool {
    is_slug(slug) && root.join(slug).is_dir()
}

/// `flow task list`: every Task with a listed Session on it, ordered by slug.
pub fn list(store: &Store, root: &Path) -> Result<Vec<Task>> {
    by_slug(store)?
        .into_iter()
        .filter(|(slug, _)| exists(root, slug))
        .map(|(slug, sessions)| task(store, root, slug, sessions))
        .collect()
}

/// `flow task get`: the Task `slug`, if its Task directory exists.
pub fn get(store: &Store, root: &Path, slug: &str) -> Result<Option<Task>> {
    if !exists(root, slug) {
        return Ok(None);
    }
    let sessions = by_slug(store)?.remove(slug).unwrap_or_default();
    Ok(Some(task(store, root, slug.to_owned(), sessions)?))
}

/// The ids of the listed Sessions on each Task, by slug.
fn by_slug(store: &Store) -> Result<BTreeMap<String, Vec<String>>> {
    let mut by_slug: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session in services::session_list(store)? {
        if let Some(slug) = session.task {
            by_slug.entry(slug).or_default().push(session.id);
        }
    }
    Ok(by_slug)
}

fn task(store: &Store, root: &Path, slug: String, sessions: Vec<String>) -> Result<Task> {
    Ok(Task {
        dir: root.join(&slug),
        resources: resources(store, &slug)?,
        slug,
        sessions,
    })
}
