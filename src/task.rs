//! The Task: a unit of delegated work, named by its slug. A Task exists when its Task
//! directory, `$FLOW_HOME/tasks/<slug>/`, does (ADR-0005); a Session joins it by writing its
//! `task` facet through `flow task join`.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;
use serde_json::Value;

use crate::services;
use crate::store::{self, Store};

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
    pub resources: Vec<Value>,
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
    Ok(by_slug(store)?
        .into_iter()
        .filter(|(slug, _)| exists(root, slug))
        .map(|(slug, sessions)| task(root, slug, sessions))
        .collect())
}

/// `flow task get`: the Task `slug`, if its Task directory exists.
pub fn get(store: &Store, root: &Path, slug: &str) -> Result<Option<Task>> {
    if !exists(root, slug) {
        return Ok(None);
    }
    let sessions = by_slug(store)?.remove(slug).unwrap_or_default();
    Ok(Some(task(root, slug.to_owned(), sessions)))
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

fn task(root: &Path, slug: String, sessions: Vec<String>) -> Task {
    Task {
        dir: root.join(&slug),
        slug,
        sessions,
        resources: Vec::new(),
    }
}
