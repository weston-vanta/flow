# GitHub PR data for the flow v2 dashboard

Research for `.scratch/flow-v2/issues/02-github-pr-data.md`, done 2026-11-07.
Every query below was run read-only with `gh api graphql` against the user's PRs in
`VantaInc/obsidian`, `VantaInc/fprm-knowledge-base` and `weston-vanta/flow`.

## Summary

- **Everything comes from GraphQL in a single aliased query**: one `repository(...)` alias
  per repo and one `pullRequest(number:N)` alias per PR, all in one document. For 22 PRs across 3
  repos this cost **5 points** (of 5,000/hour) and 9,680 nodes.
- **Two fields can't be read directly.** "Reviewers still required" has no API field;
  `reviewDecision` only says *whether* more review is needed. "Behind" isn't reported in
  `obsidian`, because its ruleset does not require an up-to-date branch, so it has to be
  computed with `compare`.
- **Don't use the CI rollup state alone.** `statusCheckRollup.state` reports `FAILURE` when a
  non-required check was merely `CANCELLED`. Use `isRequired` checks instead (see CI below).
- **Stacks**: GitHub has a native stack object (`PullRequest.stack` / `stackEntry`; stacked
  PRs went GA 2026-10-06), but membership is explicit, so it doesn't cover every chain.
  To get every chain, link PRs by matching each PR's `baseRefName` to another PR's
  `headRefName`.
- **The real constraint is latency, not points.** The 22-PR query that fetched check
  contexts took 8–9 s. GitHub's limit is 10 s, and one run failed with a truncated response.
  So split the refresh into batches and only fetch check contexts when the head commit has
  changed.

## Field by field

| Dashboard field | Source (GraphQL `PullRequest` unless noted) | Notes |
|---|---|---|
| number, title, url | `number`, `title`, `url` | direct |
| draft | `isDraft` | direct. `mergeStateStatus: DRAFT` is deprecated in favour of `isDraft` [1] |
| additions / deletions | `additions`, `deletions` | direct |
| merged | `merged` / `state == MERGED` | direct. On merged PRs `mergeStateStatus` is `UNKNOWN`, so check `merged` first (verified) |
| conflict | `mergeable == CONFLICTING` | direct [1]. `UNKNOWN` means GitHub is still computing it in a background job, so retry on the next refresh [2]. PR 220408 stayed `UNKNOWN` across several queries |
| behind | `mergeStateStatus == BEHIND` only if the branch rule requires an up-to-date branch [1][3]; otherwise `baseRef { compare(headRef: "<headRefName>") { behindBy aheadBy status } }` | **derived**. obsidian's ruleset has `strict_required_status_checks_policy: false` plus a merge queue, so `BEHIND` never appears there. `compare` worked in the aliased query (e.g. 220598 `behindBy: 82`). For a stacked PR it measures how far behind the parent branch it is |
| clean | none of the above | `mergeStateStatus` is mostly `BLOCKED` (by reviews or checks), so it doesn't work as a "clean" signal. Get blocking reasons from the CI and reviewer fields |
| CI passing / failing / running | `statusCheckRollup { contexts(first:100) { nodes { ... on CheckRun { name status conclusion isRequired(pullRequestNumber: N) } ... on StatusContext { context state isRequired(pullRequestNumber: N) } } pageInfo { hasNextPage endCursor } } }` | **derived** (see CI below) |
| unresolved review comments | `reviewThreads(first:100) { nodes { isResolved } }` and count where `!isResolved` | direct [1]. Covers diff review threads only; general PR conversation comments can't be resolved. 220657/220658 each had 2 unresolved |
| reviewers still required | `reviewDecision`, `reviewRequests { asCodeOwner requestedReviewer }`, `latestOpinionatedReviews(writersOnly:true) { state author onBehalfOf { slug } }` | **not available; approximate** (see Reviewers below) |
| stack | `baseRefName`/`headRefName`, plus optionally `stack { number size }` and `stackEntry { position }` | see Stacks below |

[1] GraphQL reference, Pull requests: https://docs.github.com/en/graphql/reference/pulls
(`MergeStateStatus`: BEHIND "The head ref is out of date", BLOCKED, CLEAN, DIRTY, DRAFT
(deprecated), HAS_HOOKS, UNKNOWN, UNSTABLE; `MergeableState`: CONFLICTING, MERGEABLE, UNKNOWN
"still being calculated"; `PullRequestReviewDecision`: APPROVED, CHANGES_REQUESTED,
REVIEW_REQUIRED.)
[2] REST "Get a pull request": "If the value is null, then GitHub has started a background job
to compute the mergeability … resubmit the request."
https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request
[3] About protected branches, "Require branches to be up to date before merging":
https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches

### CI

- `statusCheckRollup.state` (on `PullRequest` or the last commit) combines *every* check
  run and status. For 220408 and 220598 it reported `FAILURE` only because a non-required
  "AI Review Nudge" run was `CANCELLED`, while all three required checks had passed (confirmed
  with `gh pr checks --required`). The 218719–218721 stack showed `FAILURE` the same way, from
  dozens of cancelled superseded runs.
- `isRequired(pullRequestNumber: N)` on each context accounts for branch protection *and*
  rulesets. The PR number has to be written into the query as a literal, which an aliased
  query can do. The required checks for obsidian `main` (from
  `GET /repos/VantaInc/obsidian/rules/branches/main`) are `pr-checks-passed`, the
  security-risk-assessment label check, and `vanta/buildkite-pr-builds-and-tests/pr`.
- obsidian PRs have 138–232 contexts, and the required ones were often past the first 100, so
  **paging is necessary**. Each extra page cost 1 point. Several runs share a name (reruns), so
  deduplicate by name and keep the latest. `gh pr checks --required` does this for you, but
  costs one call per PR.
- Suggested mapping:
  - any required check `FAILURE`/`ERROR`/`TIMED_OUT`/`STARTUP_FAILURE`/`ACTION_REQUIRED` → failing
  - else any required check not `COMPLETED` (or status `PENDING`/`EXPECTED`) → running
  - else → passing
  - a repo with no required checks → fall back to `rollup.state`, or to
    `contexts.checkRunCountsByState`, ignoring `CANCELLED`/`SKIPPED`
- A cheaper aggregate is `contexts(first:1) { checkRunCountsByState { state count }
  statusContextCountsByState { state count } }`. It returns counts with no paging (3.4 s for 22
  PRs versus 8–9 s with nodes), but it can't tell required checks from the rest.

### Reviewers still required (not directly available)

GitHub exposes:

- `reviewDecision`, which says whether more review is needed, not who must give it. It was
  `null` on drafts.
- pending `reviewRequests`, each with `asCodeOwner`.
- each writer's latest opinionated review, with `onBehalfOf` naming the teams it satisfied.

Requests can stay pending after they're satisfied. On 218720, `grc-risk` was still requested
even though `wrporter` had approved on behalf of `grc-risk`. The best approximation is:
**pending requests with `asCodeOwner: true`, minus teams in `onBehalfOf` of an APPROVED review,
minus users who approved.** It gave sensible lists (220598 → `grc-risk`, `approval-bypassers`)
but failed on 218721: `reviewDecision: REVIEW_REQUIRED` with nothing left on the list.

Rules the API doesn't break down are the likely cause. obsidian's ruleset has
`require_last_push_approval`, `require_extra_approval_for_unattributed_changes`, and
code-owner review [4]. Approval-style status checks such as "Frontdoor Group Owner Approval"
could also block. And on a native stack, merge requirements come from the bottom PR's base [5].
The UI should therefore show "review required" from `reviewDecision`, plus the approximate
names as a hint. An exact list would mean parsing CODEOWNERS against the changed files
ourselves. Branch-protection details: `baseRef.branchProtectionRule` and `refUpdateRule` were
`null` in obsidian, which uses rulesets. The REST `GET /repos/{o}/{r}/rules/branches/{branch}`
returns the active rules with read access [4].

[4] REST "Get rules for a branch": "Returns all active rules that apply to the specified branch."
https://docs.github.com/en/rest/repos/rules#get-rules-for-a-branch
[5] About stacked pull requests: https://docs.github.com/en/pull-requests/get-started/about-stacked-prs

### Stacks

- **Inferred chain (works for every PR):** PR B sits on PR A when `B.baseRefName ==
  A.headRefName` in the same repo. This found the user's
  220598 → 220657 → 220658 chain and the 218718 → 218719 → 218720 → 218721 chain. Since only
  PRs linked to a Session are shown, match against the set being refreshed. If a parent PR isn't
  in that set, you can look it up with `repository.pullRequests(headRefName: X, first: 1)`.
- **Native stacks (GA 2026-10-06 [6]):** `stack { number size baseRefName entries { position
  pullRequest { number } } }` and `stackEntry { position }` are read-only GraphQL fields [5].
  Membership is explicit (created with `gh stack` or `POST /repos/{o}/{r}/stacks` [7]), not
  inferred. In practice, native stack 220659 held only 220657 and 220658, with
  `baseRefName` set to 220598's branch rather than `main`, so 220598 was outside it. Native
  data can supplement the inferred chain but can't replace it.

[6] https://github.blog/changelog/2026-10-06-stacked-pull-requests-generally-available/
[7] https://docs.github.com/en/enterprise-cloud@latest/rest/pulls/stacks

## Cost of one refresh

GitHub's rules [8]:

- 5,000 points/hour per user, shared by every client using the user's token, including
  agents on other CDEs.
- Cost: sum the requests each connection would need at its `first`/`last` size, ÷ 100,
  minimum 1.
- At most 500,000 nodes per query; secondary limits of 2,000 points/minute and 100 concurrent
  requests.
- Requests that run past 10 s are terminated, and still deduct points.

Measured:

| Query | PRs | cost | nodes | time |
|---|---|---|---|---|
| aliased, all fields, contexts(first:100) with `isRequired` | 22 (3 repos) | 5 | 9,680 | 9.2 s, 8.1 s, 9.2 s; one earlier run returned truncated JSON at ~11 s |
| aliased, all fields, contexts as counts only | 22 | 5 | 7,502 | 3.4 s |
| aliased, all fields | 11 | 3 | 4,840 | — |
| `search(type:ISSUE, first:50, "is:pr is:open author:…")`, same fields | 7 found | 8 | 11,100 | — |
| one extra contexts page | 1 | 1 | — | — |

Most of the cost comes from `reviewThreads(first:100)` and `contexts(first:100)` multiplied
across PRs, not from the PR count. `search` charges for its `first:50` even when it finds fewer
PRs, so aliases on known PR numbers are cheaper. That fits "only PRs linked to a Session".

A refresh of ~20 PRs costs about 5 points, plus ~1–2 points per PR whose contexts need paging.
In total that's ≤ ~15 points even with every PR's checks re-fetched. At one refresh per minute
that's ≤ 900 points/hour (< 20% of the budget).

To stay under the 10 s timeout:

- split the query into one batch per repo, or batches of ≤ 8 PRs, and run them in parallel;
- fetch `contexts` nodes only for PRs whose `headRefOid` changed or whose required checks
  were still running, and cache the CI verdict per head commit.

GraphQL has no conditional requests (ETags), so every poll is charged.

[8] https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api

## Example query shape (per PR alias)

```graphql
query {
  rateLimit { cost remaining }
  r0: repository(owner: "VantaInc", name: "obsidian") {
    p220598: pullRequest(number: 220598) {
      number title url isDraft additions deletions state merged
      baseRefName headRefName headRefOid mergeable mergeStateStatus reviewDecision
      stack { number size } stackEntry { position }
      baseRef { compare(headRef: "weston-vanta/ra-787-sequential-score-config-reads") { behindBy } }
      reviewRequests(first: 20) { nodes { asCodeOwner requestedReviewer { ... on User { login } ... on Team { slug } } } }
      latestOpinionatedReviews(first: 20, writersOnly: true) { nodes { state author { login } onBehalfOf(first: 10) { nodes { slug } } } }
      reviewThreads(first: 100) { nodes { isResolved } }
      statusCheckRollup { state contexts(first: 100) {
        pageInfo { hasNextPage endCursor }
        nodes { ... on CheckRun { name status conclusion isRequired(pullRequestNumber: 220598) }
                ... on StatusContext { context state isRequired(pullRequestNumber: 220598) } } } }
    }
  }
}
```

`compare(headRef:)` and `isRequired(pullRequestNumber:)` need literal values, so the client
generates this query text. It needs each PR's `headRefName`, which it already has from the
previous refresh, or gets from a first pass without `compare`.

## Not verified

- How `mergeStateStatus` orders DIRTY vs BLOCKED when a PR both conflicts and is blocked. The
  design uses `mergeable` for conflicts, so this ordering doesn't matter.
- Why 218721 is `REVIEW_REQUIRED` with no outstanding code-owner request. The cause is inferred,
  not confirmed.
- Exactly how much time `isRequired` adds. The latency difference was dominated by fetching
  context nodes at all.
- Whether unresolved threads ever exceed 100 on one PR. That would need paging, the same as
  contexts.
