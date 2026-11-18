---
name: plan
description: The IC's plan - turn an approved spec.md into plan.md, then one builder brief per PR. Run from flow:align.
---

# Plan

Cut the approved `spec.md` into PRs the Director can review, and give each PR a brief a Builder can start from cold.

## Principles

- **Each plan item is one PR**: one concern, small enough to review in one sitting. The repo's guidance on PR size and stacking wins.
- **A dependency is a base.** A PR that needs another's code stacks on it. Everything else bases on the default branch, or on its Feature branch, so it can start at once and Stacks build in parallel.
- **Feature branches are the Director's call.** A Task has zero, one or several, settled with the Director in Align; a small Task needs none. Each is based on the default branch and independent of the others: where one part needs another's code, use one Feature branch, or start the second once the first lands.
- **`plan.md` is for humans.** It says what each PR achieves, in words. It never tracks progress: the dashboard shows the PRs.

## Process

1. **Write `plan.md`** in the Task directory, before approval. It holds:
   - an ASCII picture of the PRs by base, numbered in build order. Each Feature branch comes off the default branch with its Stacks under it and its feature PR last, for example

     ```
     main ─┬─ 1 store-schema ── 2 writer ── 3 cli-get
           ├─ 4 web-view
           ├─ feature/billing ─┬─ 5 invoice-model ── 6 invoice-api
           │                   ├─ 7 billing-page
           │                   └─ feature PR
           └─ 8 docs
     ```
   - one line per PR saying what it achieves (in words, not by requirement number), ending with its Jira key when it has one, for example `2 writer: … (ABC-12)`. A Feature branch's line carries a key only when one is paired with the whole feature, such as an epic;
   - the Review passes, picked with `flow:review`.

   It lists no Resources: `flow task get` is their one list.
2. **Write the builder briefs** once the plan is approved: one `prs/<nn>-<name>.md` per PR, numbered as in the picture. A brief has no fixed format, only a test: could a fresh agent with this brief and the repo finish the PR without asking? Point at `spec.md`, the repo's guidance and the code by path rather than pasting them, and say what the PR leaves to its siblings. A brief carries no Jira keys: the IC records them, and the Builder needs none. A brief for a PR based on a Feature branch gives the one line of feature context its body opens with.
3. **Revise a brief** only when a reshape or a Revise round changes its PR, and `plan.md` whenever the plan changes, a key the Director names included.
