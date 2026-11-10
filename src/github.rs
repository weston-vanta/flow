//! The PR Writer: polls GitHub through `gh api graphql` and writes each PR's `github` facet.
//! All of flow's knowledge of GitHub's GraphQL schema is in this module.
//!
//! A cycle has two steps. Discovery searches for every PR the user authored, open or closed
//! in the last [`RECENT_DAYS`] days. Detail fetches every open PR, plus each closed or merged
//! PR whose `updatedAt` differs from the stored one, in per-repo batches of at most
//! [`BATCH`] PRs, [`PARALLEL`] batches at a time. A batch that fails writes nothing.

use std::collections::{BTreeMap, HashMap};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::pr::{self, Check, Github, PrState, ReviewRequest, StackPosition};
use crate::store::Store;

/// How often `flowd` polls.
pub const INTERVAL: Duration = Duration::from_secs(60);
/// The hard timeout on each `gh` call.
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// Closed and merged PRs are discovered for this many days after they close.
const RECENT_DAYS: i64 = 30;
/// At most this many PRs per detail query, to stay under GitHub's 10 s query timeout. Real
/// obsidian batches of 8, with their 140-230 check contexts each, failed with HTTP 502/504.
const BATCH: usize = 4;
/// At most this many detail queries run at once.
const PARALLEL: usize = 4;

/// What one poll cycle did.
#[derive(Debug, Default)]
pub struct Cycle {
    /// How many PRs were written.
    pub written: usize,
    /// The GraphQL rate-limit points the successful calls cost.
    pub cost: u64,
    /// A failed discovery (which skips detail) or failed batches.
    pub errors: Vec<anyhow::Error>,
}

/// One poll cycle: discovery, then detail, with `gh` killed after `timeout`.
pub fn poll_prs(store: &Store, timeout: Duration) -> Cycle {
    let gh = Gh {
        timeout,
        cost: Default::default(),
    };
    let mut cycle = Cycle::default();
    match plan(store, &gh) {
        Err(err) => cycle
            .errors
            .push(err.context("discovery failed; detail skipped")),
        Ok(batches) => {
            // Each of up to PARALLEL threads takes every PARALLEL-th batch.
            let results: Vec<Result<usize>> = thread::scope(|scope| {
                let workers: Vec<_> = (0..PARALLEL)
                    .map(|i| {
                        let (batches, gh) = (&batches, &gh);
                        scope.spawn(move || {
                            batches
                                .iter()
                                .skip(i)
                                .step_by(PARALLEL)
                                .map(|(repo, numbers)| {
                                    write_batch(store, gh, repo, numbers).with_context(|| {
                                        format!("batch {repo} {numbers:?} failed; nothing written")
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .flat_map(|worker| worker.join().unwrap())
                    .collect()
            });
            for result in results {
                match result {
                    Ok(written) => cycle.written += written,
                    Err(err) => cycle.errors.push(err),
                }
            }
        }
    }
    cycle.cost = *gh.cost.lock().unwrap();
    cycle
}

/// Discovers the user's PRs and returns the batches to fetch: `(owner/name, numbers)`.
fn plan(store: &Store, gh: &Gh) -> Result<Vec<(String, Vec<u64>)>> {
    let found = discover(gh)?;
    let stored: HashMap<String, Option<String>> = store
        .list(pr::KIND)?
        .into_iter()
        .map(|entity| {
            let updated_at = entity
                .facets
                .get(pr::GITHUB)
                .and_then(|facet| facet.value.get("updatedAt")?.as_str().map(str::to_owned));
            (entity.id, updated_at)
        })
        .collect();
    let mut by_repo: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for pr in found {
        let repo = pr.repository.name_with_owner;
        let stored = stored.get(&pr::key(&repo, pr.number)).cloned().flatten();
        if pr.state == PrState::Open || stored.as_ref() != Some(&pr.updated_at) {
            by_repo.entry(repo).or_default().push(pr.number);
        }
    }
    Ok(by_repo
        .into_iter()
        .flat_map(|(repo, numbers)| {
            numbers
                .chunks(BATCH)
                .map(|chunk| (repo.clone(), chunk.to_vec()))
                .collect::<Vec<_>>()
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    number: u64,
    state: PrState,
    updated_at: String,
    repository: Repository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    #[serde(default)]
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

/// Every PR the user authored that is open, or closed in the last [`RECENT_DAYS`] days: two
/// searches in one query, paged until both are exhausted.
fn discover(gh: &Gh) -> Result<Vec<Found>> {
    let since =
        (Timestamp::now() - SignedDuration::from_hours(24 * RECENT_DAYS)).strftime("%Y-%m-%d");
    let mut pending = vec![
        ("open", "is:pr author:@me is:open".to_owned(), None),
        (
            "closed",
            format!("is:pr author:@me is:closed closed:>={since}"),
            None,
        ),
    ];
    let mut found = Vec::new();
    while !pending.is_empty() {
        let query: String = pending
            .iter()
            .map(|(alias, search, after)| {
                format!(
                    "{alias}: search(type: ISSUE, first: 100, query: {}{}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ ... on PullRequest {{ number state updatedAt repository {{ nameWithOwner }} }} }} }} ",
                    literal(search),
                    after_arg(after.as_deref()),
                )
            })
            .collect();
        let data = gh.query(&query)?;
        let mut more = Vec::new();
        for (alias, search, _) in pending {
            let page: Connection<Found> = field(&data, &[alias])?;
            found.extend(page.nodes);
            if page.page_info.has_next_page {
                more.push((alias, search, page.page_info.end_cursor));
            }
        }
        pending = more;
    }
    Ok(found)
}

/// The PR fields fetched in detail, other than check contexts.
const PR_FIELDS: &str = "number url title state isDraft headRefName headRefOid baseRefName \
    mergeable reviewDecision updatedAt additions deletions stackEntry { position } \
    reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug } } } } \
    reviewThreads(first: 100) { nodes { isResolved } }";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u64,
    url: String,
    title: String,
    state: PrState,
    is_draft: bool,
    head_ref_name: String,
    head_ref_oid: String,
    base_ref_name: String,
    mergeable: String,
    review_decision: Option<String>,
    updated_at: String,
    additions: u64,
    deletions: u64,
    stack_entry: Option<StackPosition>,
    review_requests: Connection<RequestNode>,
    review_threads: Connection<Thread>,
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestNode {
    requested_reviewer: Option<Reviewer>,
}

#[derive(Deserialize)]
struct Reviewer {
    login: Option<String>,
    slug: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    is_resolved: bool,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Connection<Context>,
}

/// One entry of a commit's check rollup: a check run, or a commit status.
#[derive(Deserialize)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
enum Context {
    CheckRun {
        name: String,
        status: Option<String>,
        conclusion: Option<String>,
        database_id: Option<u64>,
        is_required: bool,
    },
    StatusContext {
        context: String,
        state: String,
        is_required: bool,
    },
}

/// The check-context selection for PR `number`, from the page after `after`.
fn contexts(number: u64, after: Option<&str>) -> String {
    format!(
        "statusCheckRollup {{ contexts(first: 100{}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ __typename \
         ... on CheckRun {{ name status conclusion databaseId isRequired(pullRequestNumber: {number}) }} \
         ... on StatusContext {{ context state isRequired(pullRequestNumber: {number}) }} }} }} }}",
        after_arg(after)
    )
}

/// Fetches PRs `numbers` of `repo` in one query, pages through their check contexts, and
/// writes them all, stamped with the time the fetch began. Returns how many were written.
fn write_batch(store: &Store, gh: &Gh, repo: &str, numbers: &[u64]) -> Result<usize> {
    let at = Timestamp::now();
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow!("repo {repo:?} is not owner/name"))?;
    let in_repo = |prs: String| {
        format!(
            "repo: repository(owner: {}, name: {}) {{ {prs} }}",
            literal(owner),
            literal(name)
        )
    };
    let selections: String = numbers
        .iter()
        .map(|n| {
            format!(
                "p{n}: pullRequest(number: {n}) {{ {PR_FIELDS} {} }} ",
                contexts(*n, None)
            )
        })
        .collect();
    let data = gh.query(&in_repo(selections))?;
    let mut prs: Vec<RawPr> = numbers
        .iter()
        .map(|n| field(&data, &["repo", &format!("p{n}")]))
        .collect::<Result<_>>()?;

    loop {
        let more: String = prs
            .iter()
            .filter_map(|pr| {
                let page = &pr.status_check_rollup.as_ref()?.contexts.page_info;
                page.has_next_page.then(|| {
                    format!(
                        "p{n}: pullRequest(number: {n}) {{ {} }} ",
                        contexts(pr.number, page.end_cursor.as_deref()),
                        n = pr.number
                    )
                })
            })
            .collect();
        if more.is_empty() {
            break;
        }
        let data = gh.query(&in_repo(more))?;
        for pr in &mut prs {
            let Some(rollup) = &mut pr.status_check_rollup else {
                continue;
            };
            if !rollup.contexts.page_info.has_next_page {
                continue;
            }
            let page: Connection<Context> = field(
                &data,
                &[
                    "repo",
                    &format!("p{}", pr.number),
                    "statusCheckRollup",
                    "contexts",
                ],
            )?;
            rollup.contexts.nodes.extend(page.nodes);
            rollup.contexts.page_info = page.page_info;
        }
    }

    let githubs: Vec<Github> = prs.into_iter().map(|raw| github(repo, raw)).collect();
    for github in &githubs {
        let key = pr::key(repo, github.number);
        store.put_facet(
            pr::KIND,
            &key,
            pr::GITHUB,
            &serde_json::to_value(github)?,
            at,
        )?;
    }
    Ok(githubs.len())
}

fn github(repo: &str, raw: RawPr) -> Github {
    let contexts = raw
        .status_check_rollup
        .map(|rollup| rollup.contexts.nodes)
        .unwrap_or_default();
    Github {
        repo: repo.to_owned(),
        number: raw.number,
        url: raw.url,
        title: raw.title,
        state: raw.state,
        is_draft: raw.is_draft,
        head_ref_name: raw.head_ref_name,
        head_ref_oid: raw.head_ref_oid,
        base_ref_name: raw.base_ref_name,
        mergeable: raw.mergeable,
        review_decision: raw.review_decision,
        review_requests: raw
            .review_requests
            .nodes
            .into_iter()
            .filter_map(|node| {
                let reviewer = node.requested_reviewer?;
                match (reviewer.login, reviewer.slug) {
                    (Some(login), _) => Some(ReviewRequest::Login { login }),
                    (None, Some(team)) => Some(ReviewRequest::Team { team }),
                    (None, None) => None,
                }
            })
            .collect(),
        checks: required_checks(contexts),
        stack: raw.stack_entry,
        updated_at: raw.updated_at,
        additions: raw.additions,
        deletions: raw.deletions,
        unresolved_review_threads: raw
            .review_threads
            .nodes
            .iter()
            .filter(|thread| !thread.is_resolved)
            .count() as u64,
    }
}

/// The required checks among `contexts`, one per name (the rerun GitHub created last), ordered
/// by name. A commit status is mapped onto a check run's fields: `PENDING` and `EXPECTED`
/// become the status with no conclusion; any other state is the conclusion of a `COMPLETED`
/// status.
fn required_checks(contexts: Vec<Context>) -> Vec<Check> {
    let mut latest: BTreeMap<String, (u64, Check)> = BTreeMap::new();
    for context in contexts {
        let (rank, check) = match context {
            Context::CheckRun {
                name,
                status,
                conclusion,
                database_id,
                is_required: true,
            } => (
                database_id.unwrap_or(0),
                Check {
                    name,
                    status,
                    conclusion,
                },
            ),
            Context::StatusContext {
                context,
                state,
                is_required: true,
            } => {
                let pending = matches!(state.as_str(), "PENDING" | "EXPECTED");
                let check = Check {
                    name: context,
                    status: Some(if pending {
                        state.clone()
                    } else {
                        "COMPLETED".to_owned()
                    }),
                    conclusion: (!pending).then_some(state),
                };
                (0, check)
            }
            _ => continue,
        };
        match latest.get(&check.name) {
            Some((kept, _)) if *kept > rank => {}
            _ => {
                latest.insert(check.name.clone(), (rank, check));
            }
        }
    }
    latest.into_values().map(|(_, check)| check).collect()
}

/// Runs GraphQL queries through `gh api graphql`, adding up their rate-limit cost.
struct Gh {
    timeout: Duration,
    cost: Mutex<u64>,
}

impl Gh {
    /// Runs `query { <selections> rateLimit { cost } }` and returns its `data`. A non-zero
    /// exit, GraphQL errors, or running past the timeout is an error.
    fn query(&self, selections: &str) -> Result<Value> {
        let query = format!("query {{ {selections}rateLimit {{ cost }} }}");
        // coreutils `timeout` kills `gh` after the deadline and exits 124.
        let out = Command::new("timeout")
            .arg(self.timeout.as_secs().to_string())
            .args(["gh", "api", "graphql", "-f"])
            .arg(format!("query={query}"))
            .stdin(Stdio::null())
            .output()
            .context("run gh")?;
        if out.status.code() == Some(124) {
            bail!("gh api graphql timed out after {:?}", self.timeout);
        }
        if !out.status.success() {
            // gh's first stderr line names the failure; the rest can be a whole HTML page.
            bail!(
                "gh api graphql failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or_default()
            );
        }
        let mut response: Value =
            serde_json::from_slice(&out.stdout).context("gh api graphql printed malformed JSON")?;
        if let Some(errors) = response.get("errors") {
            bail!("GitHub returned errors: {errors}");
        }
        let data = response
            .get_mut("data")
            .map(Value::take)
            .ok_or_else(|| anyhow!("gh api graphql returned no data"))?;
        *self.cost.lock().unwrap() += data["rateLimit"]["cost"].as_u64().unwrap_or(0);
        Ok(data)
    }
}

/// The value at `path` in `data`, deserialised.
fn field<T: DeserializeOwned>(data: &Value, path: &[&str]) -> Result<T> {
    let value = path.iter().fold(data, |value, key| &value[key]);
    T::deserialize(value)
        .with_context(|| format!("unexpected GitHub response at {}", path.join(".")))
}

/// `s` as a GraphQL string literal (JSON's escaping is valid GraphQL).
fn literal(s: &str) -> String {
    serde_json::to_string(s).expect("a string serialises")
}

fn after_arg(after: Option<&str>) -> String {
    after
        .map(|cursor| format!(", after: {}", literal(cursor)))
        .unwrap_or_default()
}
