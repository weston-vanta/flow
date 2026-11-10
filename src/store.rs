//! The Store: generic entities made of facets, kept as one JSON file per facet under a root
//! directory. It knows no kinds or facet names; the domain models give them meaning.
//!
//! Layout: `<root>/<kind>/<encoded id>/<facet>.json`, each file `{"at": ..., "value": ...}`.
//! Writes go in place with no lock and no temp file. The one rule every write obeys: a write
//! whose `at` is strictly older than the facet's current `at` is dropped. The read-compare-write
//! is not atomic, so two writers racing on one facet can still let the older one win; locks
//! were rejected until that is seen in practice.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the Store lives when `FLOW_STORE` is not set.
pub const DEFAULT_ROOT: &str = "/efs-home/flow/store";

/// How many facet-reading threads a list uses at most.
const LIST_PARALLELISM: usize = 16;

/// One Session, PR or Branch: its id and every facet written so far.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub id: String,
    pub facets: BTreeMap<String, Facet>,
}

/// A facet's value and when its fact was observed.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    pub at: Timestamp,
    pub value: Value,
}

/// What `put_facet` did with a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// The write replaced the facet (or created it).
    Applied,
    /// The facet already holds a fact observed later; the write was dropped.
    Stale,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The Store at `FLOW_STORE`, or at [`DEFAULT_ROOT`].
    pub fn from_env() -> Self {
        Self::new(std::env::var_os("FLOW_STORE").unwrap_or_else(|| DEFAULT_ROOT.into()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every entity of `kind` that has at least one facet, ordered by id.
    pub fn list(&self, kind: &str) -> Result<Vec<Entity>> {
        let dirs: Vec<_> = match fs::read_dir(self.root.join(kind)) {
            Ok(entries) => entries.collect::<Result<_, _>>()?,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let chunk = dirs.len().div_ceil(LIST_PARALLELISM).max(1);
        let mut entities = thread::scope(|scope| {
            let handles: Vec<_> = dirs
                .chunks(chunk)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|entry| {
                                let id = decode_id(&entry.file_name().to_string_lossy());
                                read_entity(&id, &entry.path())
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .filter_map(Result::transpose)
                .collect::<Result<Vec<_>>>()
        })?;
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    /// The entity of `kind` with `id`, if any facet of it has been written.
    pub fn get(&self, kind: &str, id: &str) -> Result<Option<Entity>> {
        read_entity(id, &self.entity_dir(kind, id))
    }

    /// Replaces facet `name` of entity `kind`/`id` with `value` observed at `at`, unless the
    /// facet already holds a fact observed strictly later. Equal or newer `at` applies.
    pub fn put_facet(
        &self,
        kind: &str,
        id: &str,
        name: &str,
        value: &Value,
        at: Timestamp,
    ) -> Result<Put> {
        let dir = self.entity_dir(kind, id);
        let path = dir.join(format!("{name}.json"));
        // A facet file that cannot be read has no `at` to compare with; the write replaces it.
        if let Ok(Some(current)) = read_facet(&path)
            && at < current.at
        {
            return Ok(Put::Stale);
        }
        let file = FacetFile {
            at: format_at(at),
            value: value.clone(),
        };
        fs::create_dir_all(&dir)
            .and_then(|_| fs::write(&path, serde_json::to_vec(&file)?))
            .with_context(|| format!("write {}", path.display()))?;
        Ok(Put::Applied)
    }

    /// Deletes entity `kind`/`id` and every facet of it. Removing a missing entity is not an
    /// error.
    pub fn remove(&self, kind: &str, id: &str) -> Result<()> {
        match fs::remove_dir_all(self.entity_dir(kind, id)) {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    fn entity_dir(&self, kind: &str, id: &str) -> PathBuf {
        self.root.join(kind).join(encode_id(id))
    }
}

/// Formats `at` the way the Store and the CLI's JSON carry it: RFC 3339 UTC with microseconds.
pub fn format_at(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

/// A facet as stored on disk.
#[derive(Serialize, Deserialize)]
struct FacetFile {
    at: String,
    value: Value,
}

fn read_entity(id: &str, dir: &Path) -> Result<Option<Entity>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", dir.display())),
    };
    let mut facets = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        if let Some(name) = file_name.to_str().and_then(|n| n.strip_suffix(".json"))
            && let Some(facet) = read_facet(&entry.path())?
        {
            facets.insert(name.to_owned(), facet);
        }
    }
    Ok((!facets.is_empty()).then(|| Entity {
        id: id.to_owned(),
        facets,
    }))
}

/// Reads one facet file, re-reading once if it fails to parse (caught mid-write). `None` if it
/// does not exist.
fn read_facet(path: &Path) -> Result<Option<Facet>> {
    let read = || -> Result<Option<Facet>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let file: FacetFile = serde_json::from_slice(&bytes)?;
        Ok(Some(Facet {
            at: file.at.parse()?,
            value: file.value,
        }))
    };
    read()
        .or_else(|_| read())
        .with_context(|| format!("read {}", path.display()))
}

/// Encodes an id into one path segment by escaping `%`, `/` and `:`.
fn encode_id(id: &str) -> String {
    id.replace('%', "%25")
        .replace('/', "%2F")
        .replace(':', "%3A")
}

fn decode_id(segment: &str) -> String {
    segment
        .replace("%2F", "/")
        .replace("%3A", ":")
        .replace("%25", "%")
}
