//! The PR: a GitHub pull request, keyed `owner/repo#number`, and its one facet, `github`.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::session::Stamped;
use crate::store::Entity;

/// The Store kind PRs are kept under.
pub const KIND: &str = "pr";

/// The name of the `github` facet in the Store.
pub const GITHUB: &str = "github";

/// The key of PR `number` in repository `repo` (`owner/name`).
pub fn key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}

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
    /// Required checks only, reruns deduplicated by name. Required means named by the
    /// default branch's rulesets, whatever the PR's base, so a stacked PR is judged by the
    /// bar it must clear to land. An open PR on the default branch also lists each
    /// required check that has yet to report, as an `EXPECTED` status.
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
    #[serde(default)]
    pub changed_files: u64,
    /// The latest reviews that approved or requested changes.
    #[serde(default)]
    pub reviews: Vec<Review>,
    /// The Feedback: unresolved, not-outdated review threads, each by its first comment.
    #[serde(default)]
    pub threads: Vec<Thread>,
    /// When the oldest default-branch commit the head lacks landed there; `None` unless the
    /// PR is open, based on the default branch, behind it by at least one commit, and not
    /// queued.
    #[serde(default)]
    pub behind_since: Option<String>,
    /// The PR's place in its repository's merge queue; `None` unless it is queued.
    #[serde(default)]
    pub merge_queue: Option<MergeQueue>,
}

/// A queued PR's place in the merge queue of its base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueue {
    /// Counted from 1, the PR that merges next.
    pub position: u64,
    /// How many PRs the queue holds.
    pub size: u64,
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
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum ReviewRequest {
    Login {
        login: String,
        #[serde(default)]
        as_code_owner: bool,
    },
    Team {
        team: String,
        #[serde(default)]
        as_code_owner: bool,
    },
}

/// A reviewer's latest review that approved or requested changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub login: String,
    /// `APPROVED` or `CHANGES_REQUESTED`.
    pub state: String,
    /// The slugs of the teams the review was given on behalf of.
    pub teams: Vec<String>,
}

/// A review thread, by its first comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub login: String,
    pub path: String,
    /// `None` for a comment on the whole file.
    pub line: Option<u64>,
    /// The comment as plain text, in full.
    pub body: String,
    pub url: String,
    pub created_at: String,
}

/// A required check, with GitHub's raw `status` and `conclusion`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    /// The check run's `detailsUrl` or the commit status's `targetUrl`.
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackPosition {
    /// The GitHub stack's number, shared by every PR in it. Facets written before it was
    /// fetched lack it until their PR is next fetched.
    #[serde(default)]
    pub number: u64,
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
