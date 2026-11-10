//! The PR: a GitHub pull request, keyed `owner/repo#number`, and its one facet, `github`.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::session::Stamped;
use crate::store::Entity;

/// The Store kind PRs are kept under.
pub const KIND: &str = "pr";

/// The name of the `github` facet in the Store.
pub const GITHUB: &str = "github";

/// A PR as the CLI prints it. `owner` is derived at read time, never stored: the `reporter`
/// Session of the Branch keyed `<repo>:<headRefName>`, or `None`.
#[derive(Debug, Clone, Serialize)]
pub struct Pr {
    pub key: String,
    pub owner: Option<String>,
    pub github: Option<Stamped<Github>>,
}

/// The PR as GitHub reports it, raw. Writer: the PR Writer in `flowd`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Github {
    /// `owner/name`.
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`.
    pub mergeable: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or `None`.
    #[serde(default)]
    pub review_decision: Option<String>,
    /// Pending review requests.
    #[serde(default)]
    pub review_requests: Vec<ReviewRequest>,
    /// Required checks only, reruns deduplicated by name.
    #[serde(default)]
    pub checks: Vec<Check>,
    /// The PR's place in its GitHub stack, `None` outside a stack.
    #[serde(default)]
    pub stack: Option<StackPosition>,
    pub updated_at: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    /// How many review threads are not resolved.
    #[serde(default)]
    pub unresolved_review_threads: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// A pending review request, for a person or a team.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReviewRequest {
    Login { login: String },
    Team { team: String },
}

/// A required check, with GitHub's raw `status` and `conclusion`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackPosition {
    pub position: u32,
}

impl Pr {
    /// The PR stored as `entity`, with no `owner` yet.
    pub fn from_entity(entity: &Entity) -> Result<Self> {
        let github = match entity.facets.get(GITHUB) {
            None => None,
            Some(facet) => Some(Stamped {
                at: facet.at,
                value: serde_json::from_value(facet.value.clone())
                    .with_context(|| format!("pr {} has a malformed {GITHUB} facet", entity.id))?,
            }),
        };
        Ok(Self {
            key: entity.id.clone(),
            owner: None,
            github,
        })
    }

    /// The key of the Branch whose reporter owns this PR, once its `github` facet is known.
    pub fn branch_key(&self) -> Option<String> {
        let github = &self.github.as_ref()?.value;
        Some(crate::branch::key(&github.repo, &github.head_ref_name))
    }
}
