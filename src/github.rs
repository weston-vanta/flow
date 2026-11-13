//! The PR Writer: polls GitHub through `gh api graphql` and writes each PR's `github` facet.
//! All of flow's knowledge of GitHub's GraphQL schema is in this module.
//!
//! A cycle has two steps. Discovery searches for every PR the user authored, open or closed
//! in the last [`RECENT_DAYS`] days. Detail fetches every open PR, plus each closed or merged
//! PR whose `updatedAt` differs from the stored one, in per-repo batches of at most
//! [`BATCH`] PRs, [`PARALLEL`] batches at a time. A batch that fails writes nothing.
//! Detail compares each PR's head with its repository's default branch, whose name is looked
//! up once per repository and kept in [`DefaultBranches`].

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

use crate::pr::{self, Check, Github, PrState, Review, ReviewRequest, StackPosition, Thread};
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

/// Each repository's default branch name by `owner/name`, kept across cycles for the daemon's
/// life.
#[derive(Default)]
pub struct DefaultBranches(Mutex<HashMap<String, String>>);

impl DefaultBranches {
    /// `repo`'s default branch, looked up through `gh` the first time it is asked for. The lock
    /// is held over the lookup so concurrent batches of one repository look it up once.
    fn get(&self, gh: &Gh, owner: &str, name: &str, repo: &str) -> Result<String> {
        let mut known = self.0.lock().unwrap();
        if let Some(branch) = known.get(repo) {
            return Ok(branch.clone());
        }
        let data = gh.query(&format!(
            "repo: repository(owner: {}, name: {}) {{ defaultBranchRef {{ name }} }} ",
            literal(owner),
            literal(name)
        ))?;
        let branch: String = field(&data, &["repo", "defaultBranchRef", "name"])?;
        known.insert(repo.to_owned(), branch.clone());
        Ok(branch)
    }
}

/// One poll cycle: discovery, then detail, with `gh` killed after `timeout`.
pub fn poll_prs(store: &Store, timeout: Duration, branches: &DefaultBranches) -> Cycle {
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
                                    write_batch(store, gh, branches, repo, numbers).with_context(
                                        || {
                                            format!(
                                                "batch {repo} {numbers:?} failed; nothing written"
                                            )
                                        },
                                    )
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

/// The PR fields fetched in detail, other than check contexts and the default-branch compare.
const PR_FIELDS: &str = "number url title state isDraft headRefName headRefOid baseRefName \
    mergeable reviewDecision updatedAt additions deletions changedFiles stackEntry { position stack { number } } \
    reviewRequests(first: 20) { nodes { asCodeOwner requestedReviewer { ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug } } } } \
    latestOpinionatedReviews(first: 20, writersOnly: true) { nodes { author { login } state onBehalfOf(first: 5) { nodes { slug } } } } \
    reviewThreads(first: 100) { nodes { isResolved isOutdated path line comments(first: 1) { nodes { author { login } bodyText url createdAt } } } }";

/// The selection comparing the PR's head with `default_branch`: how far that branch is ahead,
/// and its oldest commit the head lacks, with the PR that landed it. `associatedPullRequests`
/// is asked of that one commit only; across many commits GitHub answers HTTP 502.
fn compare(default_branch: &str) -> String {
    format!(
        "headRef {{ compare(headRef: {}) {{ aheadBy commits(first: 1) {{ nodes {{ committedDate \
         associatedPullRequests(first: 1) {{ nodes {{ mergedAt }} }} }} }} }} }}",
        literal(default_branch)
    )
}

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
    changed_files: u64,
    stack_entry: Option<StackEntry>,
    review_requests: Connection<RequestNode>,
    latest_opinionated_reviews: Connection<RawReview>,
    review_threads: Connection<RawThread>,
    head_ref: Option<HeadRef>,
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct StackEntry {
    position: u32,
    stack: Number,
}

#[derive(Deserialize)]
struct Number {
    number: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestNode {
    as_code_owner: bool,
    requested_reviewer: Option<Reviewer>,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

/// The login of `author`, or `ghost` (as GitHub shows it) for a deleted account.
fn login(author: Option<Author>) -> String {
    author.map_or_else(|| "ghost".to_owned(), |author| author.login)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReview {
    author: Option<Author>,
    state: String,
    on_behalf_of: Connection<Slug>,
}

#[derive(Deserialize)]
struct Slug {
    slug: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    line: Option<u64>,
    comments: Connection<Comment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    author: Option<Author>,
    body_text: String,
    url: String,
    created_at: String,
}

#[derive(Deserialize)]
struct HeadRef {
    compare: Option<Comparison>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comparison {
    ahead_by: u64,
    commits: Connection<Commit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    committed_date: String,
    associated_pull_requests: Connection<Merged>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Merged {
    merged_at: Option<String>,
}

#[derive(Deserialize)]
struct Reviewer {
    login: Option<String>,
    slug: Option<String>,
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
        details_url: Option<String>,
        is_required: bool,
    },
    StatusContext {
        context: String,
        state: String,
        target_url: Option<String>,
        is_required: bool,
    },
}

/// The check-context selection for PR `number`, from the page after `after`.
fn contexts(number: u64, after: Option<&str>) -> String {
    format!(
        "statusCheckRollup {{ contexts(first: 100{}) {{ pageInfo {{ hasNextPage endCursor }} nodes {{ __typename \
         ... on CheckRun {{ name status conclusion databaseId detailsUrl isRequired(pullRequestNumber: {number}) }} \
         ... on StatusContext {{ context state targetUrl isRequired(pullRequestNumber: {number}) }} }} }} }}",
        after_arg(after)
    )
}

/// Fetches PRs `numbers` of `repo` in one query, pages through their check contexts, and
/// writes them all, stamped with the time the fetch began. Returns how many were written.
fn write_batch(
    store: &Store,
    gh: &Gh,
    branches: &DefaultBranches,
    repo: &str,
    numbers: &[u64],
) -> Result<usize> {
    let at = Timestamp::now();
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow!("repo {repo:?} is not owner/name"))?;
    let default_branch = branches.get(gh, owner, name, repo)?;
    let compare = compare(&default_branch);
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
                "p{n}: pullRequest(number: {n}) {{ {PR_FIELDS} {compare} {} }} ",
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

    let githubs: Vec<Github> = prs
        .into_iter()
        .map(|raw| github(repo, &default_branch, raw))
        .collect();
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

fn github(repo: &str, default_branch: &str, raw: RawPr) -> Github {
    let contexts = raw
        .status_check_rollup
        .map(|rollup| rollup.contexts.nodes)
        .unwrap_or_default();
    // When the oldest commit the head lacks landed: its PR's merge, as a merge queue commits
    // hours before landing, else its commit date. Past 1,000 commits behind GitHub lists only
    // the newest 1,000, so this is then an upper bound.
    let behind_since = raw
        .head_ref
        .and_then(|head| head.compare)
        .filter(|_| raw.state == PrState::Open && raw.base_ref_name == default_branch)
        .filter(|compare| compare.ahead_by > 0)
        .and_then(|compare| compare.commits.nodes.into_iter().next())
        .map(|commit| {
            commit
                .associated_pull_requests
                .nodes
                .into_iter()
                .find_map(|pr| pr.merged_at)
                .unwrap_or(commit.committed_date)
        });
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
                let as_code_owner = node.as_code_owner;
                match (reviewer.login, reviewer.slug) {
                    (Some(login), _) => Some(ReviewRequest::Login {
                        login,
                        as_code_owner,
                    }),
                    (None, Some(team)) => Some(ReviewRequest::Team {
                        team,
                        as_code_owner,
                    }),
                    (None, None) => None,
                }
            })
            .collect(),
        reviews: raw
            .latest_opinionated_reviews
            .nodes
            .into_iter()
            .filter(|review| matches!(review.state.as_str(), "APPROVED" | "CHANGES_REQUESTED"))
            .map(|review| Review {
                login: login(review.author),
                state: review.state,
                teams: review
                    .on_behalf_of
                    .nodes
                    .into_iter()
                    .map(|team| team.slug)
                    .collect(),
            })
            .collect(),
        checks: required_checks(contexts),
        stack: raw.stack_entry.map(|entry| StackPosition {
            number: entry.stack.number,
            position: entry.position,
        }),
        updated_at: raw.updated_at,
        additions: raw.additions,
        deletions: raw.deletions,
        changed_files: raw.changed_files,
        threads: raw
            .review_threads
            .nodes
            .into_iter()
            .filter(|thread| !thread.is_resolved && !thread.is_outdated)
            .filter_map(|thread| {
                let comment = thread.comments.nodes.into_iter().next()?;
                Some(Thread {
                    login: login(comment.author),
                    path: thread.path,
                    line: thread.line,
                    body: comment.body_text,
                    url: comment.url,
                    created_at: comment.created_at,
                })
            })
            .collect(),
        behind_since,
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
                details_url,
                is_required: true,
            } => (
                database_id.unwrap_or(0),
                Check {
                    name,
                    status,
                    conclusion,
                    url: details_url,
                },
            ),
            Context::StatusContext {
                context,
                state,
                target_url,
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
                    url: target_url,
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
