//! The Jira issue: one issue in Jira, keyed by its Jira key (`ABC-123`), and its one facet,
//! `issue`, the facts the Jira poller last fetched. A Branch names the issue it implements
//! through its own `jira` facet; [`Jira`] joins the two for reads.
//!
//! The Jira poller, this module's Writer, searches every listed Branch's issue through `acli`
//! in one call per cycle and rewrites the `issue` facet of each key it searched for.

use std::collections::{BTreeSet, HashMap};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::services;
use crate::store::{self, Entity, Store};

/// How often `flowd` polls Jira.
pub const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The hard timeout on each `acli` call.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// The Store kind Jira issues are kept under.
pub const KIND: &str = "jira";

/// The name of the `issue` facet in the Store.
pub const ISSUE: &str = "issue";

/// Where a key's issue opens in the browser.
const BROWSE: &str = "https://vanta.atlassian.net/browse/";

/// The issue as Jira last reported it. Writer: the Jira poller in `flowd`. When `found` is
/// false the other fields are absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Issue {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The status's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The status category's key: `new`, `indeterminate` or `done`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The issue type's name.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// A Branch's Jira issue as the reads print it. Every field but `key` and `url` is null until
/// the poller first fetches the key; when it is not found, all but `found` and `at` stay null.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Jira {
    pub key: String,
    pub url: String,
    pub found: Option<bool>,
    /// When the facts were fetched, RFC 3339 as the Store carries it.
    pub at: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

impl Jira {
    /// The issue keyed `key`, with what `entity` (its `jira` record, if any) holds.
    pub fn new(key: &str, entity: Option<&Entity>) -> Result<Self> {
        let mut jira = Self {
            key: key.to_owned(),
            url: format!("{BROWSE}{key}"),
            found: None,
            at: None,
            summary: None,
            status: None,
            category: None,
            kind: None,
        };
        if let Some(facet) = entity.and_then(|e| e.facets.get(ISSUE)) {
            let issue: Issue = serde_json::from_value(facet.value.clone())
                .with_context(|| format!("jira {key} has a malformed {ISSUE} facet"))?;
            jira.found = Some(issue.found);
            jira.at = Some(store::format_at(facet.at));
            if issue.found {
                jira.summary = issue.summary;
                jira.status = issue.status;
                jira.category = issue.category;
                jira.kind = issue.kind;
            }
        }
        Ok(jira)
    }
}

/// What one poll cycle did: how many keys it searched for and how many Jira returned.
#[derive(Debug, Default)]
pub struct Cycle {
    pub searched: usize,
    pub found: usize,
}

/// One poll cycle: searches the Jira issue of every Branch `flow branch list` lists, with
/// `acli` killed after `timeout`, and writes each searched key's `issue` facet. Calls nothing
/// when no listed Branch has a Jira issue. A failed search writes nothing.
pub fn poll(store: &Store, timeout: Duration) -> Result<Cycle> {
    let keys: BTreeSet<String> = services::branch_list(store)?
        .into_iter()
        .filter_map(|b| b.jira.map(|j| j.key))
        .collect();
    if keys.is_empty() {
        return Ok(Cycle::default());
    }
    let mut issues = search(&keys, timeout)?;
    let at = Timestamp::now();
    let mut cycle = Cycle {
        searched: keys.len(),
        found: 0,
    };
    for key in &keys {
        let issue = issues.remove(key).unwrap_or_default();
        cycle.found += usize::from(issue.found);
        store.put_facet(KIND, key, ISSUE, &serde_json::to_value(issue)?, at)?;
    }
    Ok(cycle)
}

/// acli's error when none of a search's keys exists or can be read: it fails the search
/// rather than returning no items. Seen live from acli 1.3.39 as `✗ Error: failed to parse JQL
/// query: issue does not exist or you do not have permission to see it.` on stderr, exit 1.
const NONE_READABLE: &str = "issue does not exist or you do not have permission to see it";

/// Searches `keys` in one `acli` call and returns the issues Jira returned, by key.
fn search(keys: &BTreeSet<String>, timeout: Duration) -> Result<HashMap<String, Issue>> {
    let jql = format!(
        "key in ({})",
        keys.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    // coreutils `timeout` kills `acli` after the deadline and exits 124.
    let out = Command::new("timeout")
        .arg(timeout.as_secs().to_string())
        .args(["acli", "jira", "workitem", "search", "--jql", &jql])
        .args([
            "--fields",
            "key,summary,status,issuetype",
            "--paginate",
            "--json",
        ])
        .stdin(Stdio::null())
        .output()
        .context("run acli")?;
    if out.status.code() == Some(124) {
        bail!("acli jira workitem search timed out after {timeout:?}");
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        if stderr.contains(NONE_READABLE) {
            return Ok(HashMap::new());
        }
        bail!(
            "acli jira workitem search failed ({}): {}",
            out.status,
            stderr.lines().next().unwrap_or_default()
        );
    }
    let items: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
        .context("acli jira workitem search printed malformed JSON")?;
    let text = |item: &serde_json::Value, path| item.pointer(path)?.as_str().map(str::to_owned);
    items
        .iter()
        .map(|item| {
            let key = text(item, "/key").context("acli returned an issue with no key")?;
            let issue = Issue {
                found: true,
                summary: text(item, "/fields/summary"),
                status: text(item, "/fields/status/name"),
                category: text(item, "/fields/status/statusCategory/key"),
                kind: text(item, "/fields/issuetype/name"),
            };
            Ok((key, issue))
        })
        .collect()
}

/// Whether `key` is a Jira key: `[A-Z][A-Z0-9_]*-[1-9][0-9]*`.
pub fn is_key(key: &str) -> bool {
    let Some((project, number)) = key.split_once('-') else {
        return false;
    };
    let mut project = project.chars();
    project.next().is_some_and(|c| c.is_ascii_uppercase())
        && project.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && number.starts_with(|c: char| ('1'..='9').contains(&c))
        && number.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::is_key;

    #[test]
    fn keys() {
        for key in ["ABC-1", "A-10", "A1_B-123"] {
            assert!(is_key(key), "{key}");
        }
        for key in [
            "abc-1", "ABC-0", "ABC-01", "1A-1", "ABC", "ABC-", "-1", "ABC-1x", "AB-C-1",
        ] {
            assert!(!is_key(key), "{key}");
        }
    }
}
