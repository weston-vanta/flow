//! The Jira issue: one issue in Jira, keyed by its Jira key (`ABC-123`), and its one facet,
//! `issue`, the facts the Jira poller last fetched. A Branch names the issue it implements
//! through its own `jira` facet; [`Jira`] joins the two for reads.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::store::{self, Entity};

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
    pub summary: Option<String>,
    /// The status's name.
    pub status: Option<String>,
    /// The status category's key: `new`, `indeterminate` or `done`.
    pub category: Option<String>,
    /// The issue type's name.
    #[serde(rename = "type")]
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
