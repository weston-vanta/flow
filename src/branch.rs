//! The Branch: a git branch in a GitHub repository, keyed `owner/repo:branch`, and its one
//! facet, `reporter`: the Session that most recently created a worktree for it.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::store::{Entity, Put, Store};

/// The Store kind Branches are kept under.
pub const KIND: &str = "branch";

const REPORTER: &str = "reporter";

/// The Session that most recently created a worktree for a Branch; it owns the PRs whose
/// head is that Branch. Writer: `flow wt create`, as a blind write, so the most recent
/// reporter wins through the Store's `at` rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reporter {
    pub session: String,
}

/// The key of branch `branch` in repository `repo` (`owner/name`).
pub fn key(repo: &str, branch: &str) -> String {
    format!("{repo}:{branch}")
}

/// Records `session` as the reporter of the Branch keyed `key`, observed at `at`.
pub fn put_reporter(store: &Store, key: &str, session: &str, at: Timestamp) -> Result<Put> {
    let value = serde_json::to_value(Reporter {
        session: session.to_owned(),
    })?;
    store.put_facet(KIND, key, REPORTER, &value, at)
}

/// The Session reported for the Branch keyed `key`, if any.
pub fn owner(store: &Store, key: &str) -> Result<Option<String>> {
    match store.get(KIND, key)? {
        Some(entity) => reporter(&entity),
        None => Ok(None),
    }
}

/// Every reported Branch's key mapped to its reporter Session.
pub fn owners(store: &Store) -> Result<HashMap<String, String>> {
    let mut owners = HashMap::new();
    for entity in store.list(KIND)? {
        if let Some(session) = reporter(&entity)? {
            owners.insert(entity.id, session);
        }
    }
    Ok(owners)
}

fn reporter(entity: &Entity) -> Result<Option<String>> {
    let Some(facet) = entity.facets.get(REPORTER) else {
        return Ok(None);
    };
    let reporter: Reporter = serde_json::from_value(facet.value.clone())
        .with_context(|| format!("branch {} has a malformed {REPORTER} facet", entity.id))?;
    Ok(Some(reporter.session))
}

/// The `owner/name` a git remote URL points at, for the scp-like ssh form
/// `git@host:owner/name(.git)` and the URL forms `ssh://…/owner/name(.git)` and
/// `https://…/owner/name(.git)`. `None` for anything else.
pub fn repo_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?.1,
        None => url.split_once(':')?.1,
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::repo_from_remote;

    #[test]
    fn remote_forms() {
        for url in [
            "git@github.com:acme/widgets.git",
            "ssh://git@github.com/acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://user:token@github.com/acme/widgets.git\n",
        ] {
            assert_eq!(
                repo_from_remote(url).as_deref(),
                Some("acme/widgets"),
                "{url}"
            );
        }
        for url in ["/srv/git/widgets.git", "https://github.com/acme"] {
            assert_eq!(repo_from_remote(url), None, "{url}");
        }
    }
}
