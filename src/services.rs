//! The reads the binaries call: the same functions back `flow` and `flowd`.

use anyhow::Result;

use crate::session::{self, Session};
use crate::store::Store;

/// Every Session in the Store, ordered by id. No listing rule yet.
pub fn session_list(store: &Store) -> Result<Vec<Session>> {
    store
        .list(session::KIND)?
        .iter()
        .map(Session::from_entity)
        .collect()
}

/// The Session with `id`, if any of its facets has been written.
pub fn session_get(store: &Store, id: &str) -> Result<Option<Session>> {
    store
        .get(session::KIND, id)?
        .as_ref()
        .map(Session::from_entity)
        .transpose()
}
