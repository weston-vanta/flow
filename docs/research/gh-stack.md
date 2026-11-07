# How `gh stack` and GitHub native stacks model PR stacks

Researched 2026-11-07 for the flow v2 Stack concept (ticket `.scratch/flow-v2/issues/13-gh-stack.md`).
Builds on `docs/research/github-pr-data.md` (branch `research/github-pr-data`).
Every call against GitHub was read-only.

## Summary

- **`gh stack` doesn't have its own model. It is a client of GitHub's native stack object.**
  It keeps local tracking in `.git/gh-stack`, an ordered list of branches with a trunk. Remote
  state goes through the Stacks REST API (`/repos/{o}/{r}/stacks…`) [S1][S2]. A stack made in
  the web UI and one made with `gh stack` are the same object.
- **Identity:** a native stack is identified by its repo-scoped `number`, which is shown in the
  UI and used in every API path. It also has a global `id`/`node_id` [D1][D2]. The number comes
  from the same sequence as issue and PR numbers (stack 220659 sits next to PRs 220657/220658;
  stack 220726 next to PR 220725), and the CLI says "stack and PR numbers never overlap" [C1].
  **The number is not stable:** reordering or restructuring a stack unstacks it and creates a
  new one with a new number (observed twice, below).
- **Order:** `position` is 1-based, with `1` at the bottom next to the stack's base. The REST
  `pull_requests` array is ordered bottom to top [D1][D2]. Order is set explicitly. PR number
  order doesn't predict it (stack 210590 had PR 210542 at position 3, between 209238 and 209252).
- **Membership is explicit, but it must agree with base/head.** Creating or extending a stack
  requires each PR's base ref to equal the head ref of the PR below it. PRs join only at the
  top (`POST …/stacks/{n}/add`) [D2]. GitHub doesn't stack PRs automatically. It shows a
  *recommendation banner* when open PRs already chain [D4].
- **Leaving:** "unstack" removes the open, draft and closed PRs. Merged and queued PRs stay
  forever, so a stack can shrink to one merged PR (297 of obsidian's 2,731 stacks are a single
  merged PR) [D2][D4]. A closed mid-stack PR stays a member and blocks every PR above it [D3].
- **On merge:** merging PR *k* merges every unmerged PR below it, either atomically or as one
  merge-queue group. GitHub then retargets the lowest unmerged PR to the stack base and runs a
  cascading rebase on the server [D3][G1][G2]. Since GA, deleting a stack's base branch
  retargets the stack instead of closing its bottom PR [G2].
- **Readable through `gh`:** GraphQL `PullRequest.stack` / `stackEntry` (read-only, no stack
  mutations) [D1][I1]. REST `stack` object on every pull resource, plus the `/stacks` list,
  get and filter endpoints [D2]. Timeline events `AddedToStackEvent` / `RemovedFromStackEvent`
  with `stackNumber` [I1][G2]. Webhook `pull_request.stacked` [D5]. There is no
  `Repository.stacks` GraphQL connection. Listing stacks needs REST.
- **Recommendation for flow:** derive Stack from base/head chains over the Session's PRs, and
  use the native `stack` fields to enrich it. Don't treat the native object as the source of
  truth. Details in "What flow should adopt".

## The native model

### Data shapes

GraphQL (confirmed against the live schema by introspection [I1], matches [D1]):

```graphql
PullRequest.stack      : PullRequestStack       # null if not stacked
PullRequest.stackEntry : PullRequestStackEntry  # null if not stacked
PullRequestStack       { id number size baseRefName entries(first/after/last/before) }
PullRequestStackEntry  { id position pullRequest stack }
AddedToStackEvent / RemovedFromStackEvent { actor createdAt pullRequest stackId stackNumber }
```

`PullRequestTimelineItemsItemType` includes `ADDED_TO_STACK_EVENT` and `REMOVED_FROM_STACK_EVENT`.
The mutation type has no stack mutations, and `Repository` has no stack fields [I1].

REST [D2]:

- On every pull resource: `stack: {id, number, size, position, base: {ref, sha}}` or `null`.
  Verified: `GET /repos/VantaInc/obsidian/pulls/220657` returned
  `{number:220659, position:1, size:2, base:{ref:"weston-vanta/ra-787-sequential-score-config-reads"}}`.
- `GET /repos/{o}/{r}/stacks[?pull_request=N]` and `GET …/stacks/{number}` return
  `{id, number, node_id, base.ref, open, created_at, pull_requests:[{number,state,draft,merged_at,head{ref,sha}}]}`,
  bottom to top. `open` is false once every PR is merged or closed.
- Writes, which flow must not use: `POST …/stacks` (2–100 PRs, chain enforced),
  `POST …/stacks/{n}/add` (append at the top), `POST …/stacks/{n}/unstack`.
- Without the feature enabled the endpoints return 404. `weston-vanta/flow` returned `[]`, so
  the feature is on there and the repo has no stacks.

`stack.baseRefName` / `stack.base.ref` is the stack's trunk. A PR's own `baseRefName` is its
direct parent, and the two are equal only for the lowest unmerged PR [D1][D5].

### Lifecycle

| Event | What happens | Source |
|---|---|---|
| Create | Web: the second PR's "Create stack" checkbox. CLI: `gh stack submit` / `link` / `sync`. API: `POST /stacks`. A PR always exists before it joins, so `opened` webhooks never carry `stack`; `stacked` fires on join. | [D4][D5][C1] |
| Join | Only at the top: web "Add to stack", the recommendation banner, `gh stack link <stack#> …`, `POST …/add`. `link` never removes PRs. | [D2][D4][C1] |
| Reorder / insert / drop | Not supported in place. The web UI requires unstack and re-create. `gh stack modify` restructures locally, then `submit` "recreate[s] the stack on GitHub". | [D4][C1] |
| Unstack | Removes open, draft and closed PRs. Merged and queued PRs remain. Returns 204 when the stack dissolves. | [D2][D4] |
| Close a mid PR | Stays in the stack and blocks PRs above it from merging. | [D3] |
| Merge PR *k* | PR *k* and every unmerged PR below it land, either atomically or through the merge queue. Partial merges retarget the next PR to trunk and cascade-rebase the rest. | [D3][G1] |
| Base branch deleted | Since GA, GitHub retargets the stack, which supports a stack branching off another stack. `gh stack sync/rebase --accept-base-change` follows it locally. | [G2][C1] |
| All merged | The stack is complete and can't be extended. `gh stack submit` starts a new stack for new branches. | [D3] |

The GA changelog changed two merge details since the gh-stack docs were written. A stack now
lands through the merge queue as a single merge group, and the merge-commit method makes one
commit per PR [G2]. The FAQ in the v0.3.0 docs still describes "best effort" grouping and one
merge commit per group [D3]. Trust the changelog. Neither detail matters to flow.

### `gh stack` locally

The `.git/gh-stack` JSON schema [S3] has: `stacks[] {id, number, trunk{branch, head},
branches[] {branch, base (parent SHA at last sync), pullRequest{number, id, url, merged,
mergedHead}}}`. This state is per clone and invisible to GitHub. Remote reconciliation calls
`ListStacks`, `FindStackForPR` (`?pull_request=N`), `GetStack`, `CreateStack`, `AddToStack`
and `Unstack` [S2]. `gh stack link` targets users of jj, Sapling, git-town and similar tools: it
"does not rely on gh-stack local tracking state" [C1]. `gh stack view --json` reads local
tracking only, so it is no use for a remote dashboard. docs.github.com also lists a
"`gh-stack` skill" for agents [G1], so worker sessions may create native stacks on their own.

## Real-repo evidence (VantaInc/obsidian, read 2026-11-07)

**Adoption.** 2,731 native stacks; the oldest is from 2026-08-07, in public preview. 49 have a
non-`main` base. Across 3,124 open PRs: 835 are in a native stack (316 stacks). 1,105 are in an
inferred chain (their base is another open PR's head, or their head is another open PR's base).
**352 chained PRs (32%) belong to no native stack**, and 307 of those don't touch a native stack
at all. Native stacks are common but far from universal, even in a repo where people use them.

**The "omitted bottom PR" from the earlier research (stack 220659) is a branching stack, not a
bug.** 220598 (base `main`) was opened at 22:41 and never stacked. 220657 and 220658 were
created at 23:29 and stacked together at 23:30 as stack 220659, whose `baseRefName` is
220598's head branch. GitHub treats that branch as this stack's trunk, which the GA release
supports explicitly ("one stack branches off another") [G2]. Two more open stacks in obsidian
have the same shape (217226 on top of PR 216519, 219246 on top of 206872). The base/head chain
is 220598 → 220657 → 220658, but the native stack covers only the top two.

**Restructuring changes the number.** PR 219792's timeline: `AddedToStack 219798` →
`RemovedFromStack 219798` → `BaseRefChanged ra-787-1-… → main` → `AddedToStack 220585` (all on
2026-11-06). Stack 209240 was likewise dissolved and recreated as 210590 when 210542 and
210560 were inserted at positions 3–4.

**After a merge, base/head no longer describes the stack.** In stack 210590 (7 PRs, all
merged):

| pos | PR | merged | `baseRefName` now | `MergedEvent.mergeRefName` |
|---|---|---|---|---|
| 1 | 209231 | 10-26 22:22:53 | main | main |
| 2 | 209238 | 10-26 22:22:58 | main | main |
| 3 | 210542 | 10-28 02:59:39 | main | main |
| 4 | 210560 | 10-28 02:59:39 | …presenter-move | …presenter-move |
| 5 | 209252 | 10-28 02:59:39 | …resolver-rewire | …resolver-rewire |
| 6 | 209258 | 10-28 02:59:39 | …account-graphql | …account-graphql |
| 7 | 209264 | 10-28 23:27:48 | main | main |

PRs that were the lowest unmerged one when a merge ran were retargeted to `main`, and the
timeline shows no `BaseRefChangedEvent` for the server-side retarget. PRs in the same atomic
group report a merge into their parent branch. Inferring from base/head would split this
landed stack into {209231}, {209238}, {210542→210560→209252→209258} and {209264}. The native
stack keeps all seven in order.

**Closed members stay.** In stack 218722, 218617 (position 1) is merged, 218718 (position 2) is
closed without merging and still a member, and 218719–218721 are open. 218719's base is still
218718's branch, so both models show the closed PR in the chain.

**Cost.** Adding `stack{number size baseRefName entries(first:50){nodes{position pullRequest{number}}}}`
and `stackEntry{position}` to an aliased query over 12 PRs, timelines included, cost 1 point.

## Native model vs base/head inference

| | Native `stack` | Base/head inference |
|---|---|---|
| Coverage | Only stacks someone explicitly created (UI, `gh stack`, API, agent skill) | Every chain, whatever tool made it (jj, git-town, plain `gh pr create --base`, agents) |
| Branching stacks | Splits them: the stack under the base branch is separate (220598 / 220659) | One chain up to the trunk |
| Order | Explicit `position` | Walk the parent links. Ambiguous if two PRs share a base (a fan-out) |
| Identity | `number`, which changes on restructure | None. flow must define one |
| Merged history | Kept: members and order survive merges | Lost once GitHub retargets to `main` |
| Closed members | Kept until unstacked | Kept while the child still targets the closed PR's branch |
| Signals GitHub uses | Merge rules, CI triggers and merge box are all evaluated against `stack.baseRefName` [D3] | Not available |
| Read cost | A few fields in the same aliased query | Free: `baseRefName` / `headRefName` are already fetched |

## What flow should adopt

flow's Stack should be **derived**: an ordered chain of the Session's PRs built from
`baseRefName == headRefName` edges. The native object should be an **overlay**, not the source:

1. **Edges.** Link PR B on top of PR A when `B.baseRefName == A.headRefName` in the same repo,
   and also when A and B share a native `stack.number` with adjacent `position`s. The native
   edges keep merged stacks intact after GitHub retargets them to `main`.
2. **Order.** Use the native `position` where present. Elsewhere, walk the parent links from the
   PR whose base isn't another PR's head.
3. **Display.** When the chain (or part of it) is a native stack, show the stack number and
   link to `https://github.com/{o}/{r}/stack/{number}` (URL format from [S2]). A branching
   stack shows as one chain, so mark where the native stack starts.
4. **Identity.** Don't key flow state on `stack.number`, because restructuring changes it.
   Key on the Session, or on the chain's bottom PR, and treat the native number as an
   attribute that can change.
5. **Merge semantics.** When a native stack is present, the dashboard can say that merging PR
   *k* lands everything below it [D3]. Unstacked chains merge PR by PR as usual.

This works whether or not the user creates stacks with `gh stack`, and it costs nothing extra
beyond the fields `github-pr-data.md` already queries. An unstacked chain that GitHub would
offer to stack shows up as a chain without a stack number. flow could hint "not a GitHub
stack", but it must not create the stack itself, because that's a write.

## Not verified

- Fan-out (two open PRs with the same base) wasn't measured in obsidian. Native stacks forbid it.
  Inference needs a rule, for example one chain per top PR or a tree.
- Retargets after a partial merge left no `BaseRefChangedEvent` on the PRs above. I checked
  only `BASE_REF_CHANGED_EVENT` and didn't check whether another timeline item type records
  them.
- The behavior when a stack's base branch is deleted (auto-retarget) comes only from the
  changelog [G2]. I didn't observe it.
- I didn't run `gh stack` commands that read local state (`view`, `checkout`), because no local
  stack is tracked here and `checkout` writes to the clone.

## Sources

- [D1] gh-stack docs, GraphQL API reference (v0.3.0): https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/reference/graphql-api.md
- [D2] gh-stack docs, REST API reference: https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/reference/rest-api.md
- [D3] gh-stack docs, FAQ and "Working with Stacked PRs": https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/faq.md, https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/guides/stacked-prs.md
- [D4] gh-stack docs, "Stacked PRs in the GitHub UI": https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/guides/ui.md
- [D5] gh-stack docs, Webhooks reference: https://github.com/github/gh-stack/blob/v0.3.0/docs/src/content/docs/reference/webhooks.md
- [S1] `gh stack` v0.3.0 (`gh extension list`: `github/gh-stack v0.3.0`)
- [S2] gh-stack source, API client: https://github.com/github/gh-stack/blob/v0.3.0/internal/github/github.go
- [S3] gh-stack source, local state schema: https://github.com/github/gh-stack/blob/v0.3.0/internal/stack/schema.json
- [C1] `gh stack <cmd> --help` for init, add, checkout, modify, unstack, view, link, merge, push, rebase, submit and sync (v0.3.0, run 2026-11-07)
- [G1] GitHub Docs, "About stacked pull requests": https://docs.github.com/en/pull-requests/get-started/about-stacked-prs
- [G2] GitHub Changelog, "Stacked pull requests generally available" (2026-10-06): https://github.blog/changelog/2026-10-06-stacked-pull-requests-generally-available/
- [I1] GraphQL introspection of `PullRequestStack`, `PullRequestStackEntry`, `PullRequest`, `AddedToStackEvent`, `RemovedFromStackEvent`, `PullRequestTimelineItemsItemType` and the mutation type, via `gh api graphql` on 2026-11-07
- Real data: `GET /repos/VantaInc/obsidian/stacks` (all pages), `GET /repos/VantaInc/obsidian/pulls?state=open` (all pages), and GraphQL reads of PRs 220598, 220657, 220658, 218617–218721, 219792, 209231–210560 and 209252–209264, on 2026-11-07.
