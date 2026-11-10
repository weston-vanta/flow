//! The Session: one Claude Code conversation, keyed by its Claude session id, and its facets.
//! Every Session facet has one Writer, `flow hook`.

use anyhow::{Context as _, Result};
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};

use crate::store::{self, Entity, Put, Store};

/// The Store kind Sessions are kept under.
pub const KIND: &str = "session";

/// A Session as the CLI prints it: a facet never written is `None` (`null` in JSON).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub origin: Option<Stamped<Origin>>,
    pub status: Option<Stamped<Status>>,
    pub context: Option<Stamped<Context>>,
}

/// A facet's fields with the time its fact was observed, serialised as `{at, ...fields}`.
#[derive(Debug, Clone, Serialize)]
pub struct Stamped<T> {
    #[serde(serialize_with = "serialize_at")]
    pub at: Timestamp,
    #[serde(flatten)]
    pub value: T,
}

/// Where a Session runs. Writer: `flow hook`, on every `SessionStart`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Origin {
    /// The CDE's name, resolved from `cde_id` through the gitpod CLI; `None` if that failed.
    pub cde: Option<String>,
    /// `ONA_ID`.
    pub cde_id: Option<String>,
    /// `HERDR_PANE_ID`.
    pub pane: Option<String>,
    /// `HERDR_SOCKET_PATH`, raw.
    pub herdr_socket: Option<String>,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
}

/// What a Session is doing. Writer: `flow hook`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Waiting,
    Idle,
    Done,
}

/// What a Session is working on. Writer: `flow hook`, on `SessionStart`, `Stop` and
/// `StopFailure`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Context {
    pub title: Option<String>,
    /// The branch checked out in the Session's cwd; `None` outside git or when detached.
    pub branch: Option<String>,
}

/// A Session facet: its name in the Store and its type.
pub trait SessionFacet: Serialize + DeserializeOwned {
    const NAME: &'static str;
}

impl SessionFacet for Origin {
    const NAME: &'static str = "origin";
}

impl SessionFacet for Status {
    const NAME: &'static str = "status";
}

impl SessionFacet for Context {
    const NAME: &'static str = "context";
}

impl Session {
    pub fn from_entity(entity: &Entity) -> Result<Self> {
        Ok(Self {
            id: entity.id.clone(),
            origin: facet(entity)?,
            status: facet(entity)?,
            context: facet(entity)?,
        })
    }

    /// Replaces facet `F` of Session `id`, unless it holds a fact observed after `at`.
    pub fn put<F: SessionFacet>(store: &Store, id: &str, value: &F, at: Timestamp) -> Result<Put> {
        store.put_facet(KIND, id, F::NAME, &serde_json::to_value(value)?, at)
    }
}

fn facet<F: SessionFacet>(entity: &Entity) -> Result<Option<Stamped<F>>> {
    let Some(facet) = entity.facets.get(F::NAME) else {
        return Ok(None);
    };
    let value = serde_json::from_value(facet.value.clone())
        .with_context(|| format!("session {} has a malformed {} facet", entity.id, F::NAME))?;
    Ok(Some(Stamped {
        at: facet.at,
        value,
    }))
}

fn serialize_at<S: Serializer>(at: &Timestamp, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&store::format_at(*at))
}
