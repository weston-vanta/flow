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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the Store lives when `FLOW_STORE` is not set.
pub const DEFAULT_ROOT: &str = "/efs-home/flow/store";

/// How many facet-reading threads a list uses at most.
const LIST_PARALLELISM: usize = 16;

/// How long to wait before re-reading a facet file that failed to parse (caught mid-write).
const REREAD_DELAY: Duration = Duration::from_millis(50);

/// The longest encoded id the Store can keep: one path segment, at most 255 bytes.
const MAX_ENCODED_ID: usize = 255;

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
        match std::env::var_os("FLOW_STORE") {
            Some(root) if !root.is_empty() => Self::new(root),
            _ => Self::new(DEFAULT_ROOT),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every entity of `kind` that has at least one facet, ordered by id.
    pub fn list(&self, kind: &str) -> Result<Vec<Entity>> {
        let kind_dir = self.root.join(kind);
        let entries = match fs::read_dir(&kind_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err).with_context(|| format!("read {}", kind_dir.display())),
        };
        let mut dirs = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("read {}", kind_dir.display()))?;
            let name = entry.file_name();
            // Anything that is not an encoded id was not written by the Store; skip it.
            if let Some(id) = name.to_str().and_then(decode_id) {
                dirs.push((id, entry.path()));
            }
        }

        let next = AtomicUsize::new(0);
        let workers = dirs.len().clamp(1, LIST_PARALLELISM);
        let results: Vec<Result<Option<Entity>>> = thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some((id, dir)) = dirs.get(i) else { break };
                            out.push(read_entity(id, dir));
                        }
                        out
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("list worker panicked"))
                .collect()
        });

        let mut entities = Vec::new();
        for result in results {
            if let Some(entity) = result? {
                entities.push(entity);
            }
        }
        entities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entities)
    }

    /// The entity of `kind` with `id`, if any facet of it has been written.
    pub fn get(&self, kind: &str, id: &str) -> Result<Option<Entity>> {
        if id.is_empty() {
            return Ok(None);
        }
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
        if id.is_empty() {
            bail!("cannot write facet {name} of a {kind} with an empty id");
        }
        let encoded = encode_id(id).len();
        if encoded > MAX_ENCODED_ID {
            bail!(
                "cannot write facet {name} of {kind} {id:?}: the id encodes to {encoded} bytes, more than the Store's limit of {MAX_ENCODED_ID}"
            );
        }
        // `at` is kept to the microsecond, so compare at the precision it is stored with.
        let at = Timestamp::from_microsecond(at.as_microsecond())?;
        let dir = self.entity_dir(kind, id);
        let path = dir.join(format!("{name}.json"));
        match read_facet(&path) {
            Ok(Some(current)) if at < current.at => return Ok(Put::Stale),
            Ok(_) => {}
            // A facet file that stays unparseable has no `at` to compare with; the write
            // replaces it rather than leaving the facet unwritable for good.
            Err(ReadError::Corrupt(_)) => {}
            Err(ReadError::Io(err)) => return Err(err),
        }
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let file = FacetFile {
            at: format_at(at),
            value: value.clone(),
        };
        let bytes = serde_json::to_vec(&file)?;
        fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
        Ok(Put::Applied)
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

enum ReadError {
    Io(anyhow::Error),
    Corrupt(anyhow::Error),
}

impl From<ReadError> for anyhow::Error {
    fn from(err: ReadError) -> Self {
        match err {
            ReadError::Io(err) | ReadError::Corrupt(err) => err,
        }
    }
}

fn read_entity(id: &str, dir: &Path) -> Result<Option<Entity>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {}", dir.display())),
    };
    let mut facets = BTreeMap::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", dir.display()))?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        if let Some(facet) = read_facet(&entry.path())? {
            facets.insert(name.to_owned(), facet);
        }
    }
    if facets.is_empty() {
        return Ok(None);
    }
    Ok(Some(Entity {
        id: id.to_owned(),
        facets,
    }))
}

/// Reads one facet file, re-reading once if it fails to parse. `None` if it does not exist.
fn read_facet(path: &Path) -> Result<Option<Facet>, ReadError> {
    let mut retried = false;
    loop {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(ReadError::Io(
                    anyhow!(err).context(format!("read {}", path.display())),
                ));
            }
        };
        match parse_facet(&bytes) {
            Ok(facet) => return Ok(Some(facet)),
            Err(_) if !retried => {
                retried = true;
                thread::sleep(REREAD_DELAY);
            }
            Err(err) => {
                return Err(ReadError::Corrupt(
                    err.context(format!("parse {}", path.display())),
                ));
            }
        }
    }
}

/// Parses the first JSON document in a facet file. Bytes after it are ignored: two writers
/// racing on one file each write their whole document from offset 0, so the file always
/// starts with the last writer's complete document, possibly followed by the tail of a
/// longer, earlier one.
fn parse_facet(bytes: &[u8]) -> Result<Facet> {
    let file: FacetFile = serde_json::Deserializer::from_slice(bytes)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty facet file"))??;
    let at = file
        .at
        .parse()
        .with_context(|| format!("bad at {:?}", file.at))?;
    Ok(Facet {
        at,
        value: file.value,
    })
}

/// Bytes kept as they are in an encoded id; every other byte becomes `%XX`.
fn is_plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.#@+~,=".contains(&byte)
}

/// Encodes an id into one path segment: `/`, `:`, `%` and any other byte outside a small
/// plain set become `%XX`, and so does a leading `.`, so no id can name `.`, `..` or a
/// hidden file.
fn encode_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for (i, &byte) in id.as_bytes().iter().enumerate() {
        if is_plain(byte) && !(i == 0 && byte == b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The id an encoded path segment stands for, or `None` if it is not one `encode_id` makes.
fn decode_id(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = segment.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let id = String::from_utf8(out).ok()?;
    (!id.is_empty() && encode_id(&id) == segment).then_some(id)
}
