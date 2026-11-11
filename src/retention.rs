//! Retention: the read-time listing rule that hides done Sessions, and the sweep that removes
//! old Sessions, PRs and Branches from the Store. Neither ever changes a status.

use std::collections::HashSet;

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};

use crate::branch;
use crate::pr::{self, Pr, PrState};
use crate::session::{self, Session, State};
use crate::store::{Entity, Store};

/// How long a done Session that owns no open PR stays listed after its last news.
pub const LISTED_FOR: SignedDuration = SignedDuration::from_hours(24);

/// How long a done Session that reports no Branch (it never created a worktree, or every
/// Branch it did was since taken over) stays listed after its `status.at`.
pub const LISTED_WITHOUT_WORKTREE_FOR: SignedDuration = SignedDuration::from_mins(5);

/// How old an entity's newest facet must be before the sweep removes it.
pub const KEPT_FOR: SignedDuration = SignedDuration::from_hours(30 * 24);

/// Whether `session` is listed at `now`, given `prs` with their derived owners and whether
/// it `reports_branch`. A non-done Session always is. A done one that reports no Branch is
/// until [`LISTED_WITHOUT_WORKTREE_FOR`] after its `status.at`. Any other done one is while
/// it owns an open PR, else until [`LISTED_FOR`] after the later of its `status.at` and the
/// newest `github.at` of the PRs it owns.
pub fn is_listed(session: &Session, prs: &[Pr], reports_branch: bool, now: Timestamp) -> bool {
    let Some(status) = &session.status else {
        return true;
    };
    if status.value.state != State::Done {
        return true;
    }
    if !reports_branch {
        return now.duration_since(status.at) < LISTED_WITHOUT_WORKTREE_FOR;
    }
    let mut last = status.at;
    let owned = prs
        .iter()
        .filter(|pr| pr.owner.as_deref() == Some(session.id.as_str()))
        .filter_map(|pr| pr.github.as_ref());
    for github in owned {
        if github.value.state == PrState::Open {
            return true;
        }
        last = last.max(github.at);
    }
    now.duration_since(last) < LISTED_FOR
}

/// Removes every Session, PR and Branch whose newest facet is older than [`KEPT_FOR`] at
/// `now`, except a Branch at the head of an open PR and a Session that owns an open PR.
/// Returns how many were removed.
pub fn sweep(store: &Store, now: Timestamp) -> Result<usize> {
    let owners = branch::owners(store)?;
    let prs = store.list(pr::KIND)?;
    let mut kept_branches = HashSet::new();
    let mut kept_sessions = HashSet::new();
    for entity in &prs {
        let pr = Pr::from_entity(entity)?;
        let open = pr
            .github
            .as_ref()
            .is_some_and(|g| g.value.state == PrState::Open);
        if let (true, Some(key)) = (open, pr.branch_key()) {
            if let Some(owner) = owners.get(&key) {
                kept_sessions.insert(owner.clone());
            }
            kept_branches.insert(key);
        }
    }

    let old = |entity: &Entity| {
        let newest = entity.facets.values().map(|f| f.at).max();
        newest.is_some_and(|at| now.duration_since(at) > KEPT_FOR)
    };
    let mut removed = 0;
    for (kind, entities, kept) in [
        (session::KIND, store.list(session::KIND)?, kept_sessions),
        (pr::KIND, prs, HashSet::new()),
        (branch::KIND, store.list(branch::KIND)?, kept_branches),
    ] {
        for entity in entities {
            if old(&entity) && !kept.contains(&entity.id) {
                store.remove(kind, &entity.id)?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}
