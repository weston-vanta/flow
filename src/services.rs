//! The reads the binaries call: the same functions back `flow` and `flowd`.

use std::collections::{HashMap, HashSet};
use std::process::{Command, ExitStatus};

use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;

use crate::branch;
use crate::pr::{self, Pr};
use crate::retention;
use crate::session::{self, Session};
use crate::store::Store;

/// The `help` of the `{error, help}` JSON both binaries return when a read of the Store fails.
pub const STORE_ERROR_HELP: &str = "the Store could not be read or holds a malformed facet; the error names the path ($FLOW_HOME/store, FLOW_HOME defaulting to /efs-home/flow)";

/// Every listed Session, ordered by id: see [`retention::is_listed`].
pub fn session_list(store: &Store) -> Result<Vec<Session>> {
    Ok(listing(store)?.0)
}

/// The Session with `id`, if any of its facets has been written.
pub fn session_get(store: &Store, id: &str) -> Result<Option<Session>> {
    store
        .get(session::KIND, id)?
        .as_ref()
        .map(Session::from_entity)
        .transpose()
}

/// Why `flow session remove` removed nothing.
#[derive(Debug)]
pub enum Refusal {
    /// No Session has this id.
    Missing(String),
    /// This Session's status is not done (or it has none), and the removal was not forced.
    NotDone(String),
}

/// `flow session remove`: deletes every Session in `ids` from the Store, or none of them if
/// any is missing or, unless `force`, not done. Leaves their PRs and Branches to the sweep.
pub fn session_remove(store: &Store, ids: &[String], force: bool) -> Result<Result<(), Refusal>> {
    for id in ids {
        let Some(session) = session_get(store, id)? else {
            return Ok(Err(Refusal::Missing(id.clone())));
        };
        let done = session
            .status
            .is_some_and(|s| s.value.state == session::State::Done);
        if !done && !force {
            return Ok(Err(Refusal::NotDone(id.clone())));
        }
    }
    for id in ids {
        store.remove(session::KIND, id)?;
    }
    Ok(Ok(()))
}

/// Every PR whose owner is a listed Session, ordered by key, each with its derived `owner`
/// and `jira`.
pub fn pr_list(store: &Store) -> Result<Vec<Pr>> {
    let mut prs = listing(store)?.1;
    join_jira(store, &mut prs)?;
    Ok(prs)
}

/// `flow pr list --all`: every stored PR, ordered by key; `owner` is null unless a listed
/// Session owns it.
pub fn pr_list_all(store: &Store) -> Result<Vec<Pr>> {
    let owned: HashMap<String, Option<String>> = listing(store)?
        .1
        .into_iter()
        .map(|pr| (pr.key, pr.owner))
        .collect();
    let mut prs = store
        .list(pr::KIND)?
        .iter()
        .map(|entity| {
            let mut pr = Pr::from_entity(entity)?;
            pr.owner = owned.get(&pr.key).cloned().flatten();
            Ok(pr)
        })
        .collect::<Result<Vec<_>>>()?;
    join_jira(store, &mut prs)?;
    Ok(prs)
}

/// Sets each PR's `jira` to its head Branch's Jira issue.
fn join_jira(store: &Store, prs: &mut [Pr]) -> Result<()> {
    let jiras = branch::jiras(store)?;
    for pr in prs {
        pr.jira = pr.branch_key().and_then(|key| jiras.get(&key).cloned());
    }
    Ok(())
}

/// `flow branch list`: every Branch whose owner is a listed Session, ordered by key.
pub fn branch_list(store: &Store) -> Result<Vec<branch::Branch>> {
    let sessions = session_list(store)?;
    let listed: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let mut branches = branch::list(store)?;
    branches.retain(|b| listed.contains(b.owner.as_str()));
    Ok(branches)
}

/// The listed Sessions and the PRs they own, under the listing rule at this moment.
fn listing(store: &Store) -> Result<(Vec<Session>, Vec<Pr>)> {
    let owners = branch::owners(store)?;
    let mut prs = Vec::new();
    for entity in store.list(pr::KIND)? {
        let mut pr = Pr::from_entity(&entity)?;
        pr.owner = pr.branch_key().and_then(|key| owners.get(&key).cloned());
        if pr.owner.is_some() {
            prs.push(pr);
        }
    }
    let reporters: HashSet<&str> = owners.values().map(String::as_str).collect();
    let now = Timestamp::now();
    let mut sessions = Vec::new();
    for entity in store.list(session::KIND)? {
        let session = Session::from_entity(&entity)?;
        if retention::is_listed(&session, &prs, reporters.contains(session.id.as_str()), now) {
            sessions.push(session);
        }
    }
    let listed: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    prs.retain(|pr| pr.owner.as_deref().is_some_and(|o| listed.contains(o)));
    Ok((sessions, prs))
}

/// The PR keyed `key` (`owner/repo#number`), owned or not, with its derived `owner` and
/// `jira`.
pub fn pr_get(store: &Store, key: &str) -> Result<Option<Pr>> {
    let Some(entity) = store.get(pr::KIND, key)? else {
        return Ok(None);
    };
    let mut pr = Pr::from_entity(&entity)?;
    if let Some(key) = pr.branch_key() {
        pr.owner = branch::owner(store, &key)?;
        pr.jira = branch::jiras(store)?.remove(&key);
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
    let key = branch::key(&origin_repo()?, branch);
    branch::put_reporter(store, &key, session, Timestamp::now())?;
    Ok(())
}

/// `flow branch jira set/clear`: records `jira` (a Jira key, or `None` to clear) on `branch`
/// in the current directory's repository, or on the branch checked out there. Returns the
/// Branch's key.
pub fn branch_jira(store: &Store, branch: Option<&str>, jira: Option<&str>) -> Result<String> {
    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => git(&["branch", "--show-current"])?,
    };
    if branch.is_empty() {
        bail!("no branch is checked out here (detached HEAD)");
    }
    let key = branch::key(&origin_repo()?, &branch);
    branch::put_jira(store, &key, jira, Timestamp::now())?;
    Ok(key)
}

/// The `owner/name` of the current directory's origin remote, as `flow wt create` records it.
fn origin_repo() -> Result<String> {
    let url = git(&["remote", "get-url", "origin"])?;
    branch::repo_from_remote(&url)
        .with_context(|| format!("cannot tell owner/name from the origin remote {url:?}"))
}

/// Runs `git <args>` in the current directory and returns its trimmed stdout.
fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
