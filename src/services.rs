//! The reads the binaries call: the same functions back `flow` and `flowd`.

use std::process::{Command, ExitStatus};

use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;

use crate::branch;
use crate::pr::{self, Pr};
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

/// Every PR that has an owner, ordered by key, each with its derived `owner`. No listing
/// rule yet.
pub fn pr_list(store: &Store) -> Result<Vec<Pr>> {
    let owners = branch::owners(store)?;
    let mut prs = Vec::new();
    for entity in store.list(pr::KIND)? {
        let mut pr = Pr::from_entity(&entity)?;
        pr.owner = pr.branch_key().and_then(|key| owners.get(&key).cloned());
        if pr.owner.is_some() {
            prs.push(pr);
        }
    }
    Ok(prs)
}

/// The PR keyed `key` (`owner/repo#number`), owned or not, with its derived `owner`.
pub fn pr_get(store: &Store, key: &str) -> Result<Option<Pr>> {
    let Some(entity) = store.get(pr::KIND, key)? else {
        return Ok(None);
    };
    let mut pr = Pr::from_entity(&entity)?;
    if let Some(key) = pr.branch_key() {
        pr.owner = branch::owner(store, &key)?;
    }
    Ok(Some(pr))
}

/// `flow wt create`: runs `wt create <branch> <args>` with inherited stdio and returns its
/// exit status. Only if it succeeds and `session` is given, records `session` as the
/// reporter of the Branch `<owner/name of the origin remote>:<branch>`. A failure to record
/// it is returned beside the status, which stays `wt`'s.
pub fn wt_create(
    store: &Store,
    branch: &str,
    args: &[String],
    session: Option<&str>,
) -> Result<(ExitStatus, Option<anyhow::Error>)> {
    let status = Command::new("wt")
        .arg("create")
        .arg(branch)
        .args(args)
        .status()
        .context("run wt")?;
    let report = match (status.success(), session) {
        (true, Some(session)) => report_branch(store, branch, session)
            .context("wt create succeeded, but the Session was not recorded as the branch's owner")
            .err(),
        _ => None,
    };
    Ok((status, report))
}

fn report_branch(store: &Store, branch: &str, session: &str) -> Result<()> {
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .context("run git remote get-url origin")?;
    if !out.status.success() {
        bail!(
            "git remote get-url origin failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&out.stdout);
    let repo = branch::repo_from_remote(&url).with_context(|| {
        format!(
            "cannot tell owner/name from the origin remote {:?}",
            url.trim()
        )
    })?;
    branch::put_reporter(
        store,
        &branch::key(&repo, branch),
        session,
        Timestamp::now(),
    )?;
    Ok(())
}
