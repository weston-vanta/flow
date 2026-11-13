---
name: plan
description: The IC's plan - turn an approved spec.md into plan.md, then one builder brief per PR. Run from flow:align.
---

# Plan

Cut the approved `spec.md` into PRs the Director can review, and give each PR a brief a Builder can start from cold.

## Principles

- **Each plan item is one PR**: one concern, small enough to review in one sitting. The repo's guidance on PR size and stacking wins.
- **A dependency is a base.** A PR that needs another's code stacks on it. Everything else bases on the default branch, so it can start at once and Stacks build in parallel.
- **`plan.md` is for humans.** It says what each PR achieves, in words. It never tracks progress: the dashboard shows the PRs.

## Process

1. **Write `plan.md`** in the Task directory, before approval. It holds:
   - an ASCII picture of the PRs by base, numbered in build order, for example

     ```
     main ─┬─ 1 store-schema ── 2 writer ── 3 cli-get
           ├─ 4 web-view
           └─ 5 docs
     ```
   - one line per PR saying what it achieves (in words, not by requirement number);
   - the Review passes, picked with `flow:review`;
   - the external records, once the Director has asked for any.
2. **Write the builder briefs** once the plan is approved: one `prs/<nn>-<name>.md` per PR, numbered as in the picture. A brief has no fixed format, only a test: could a fresh agent with this brief and the repo finish the PR without asking? Point at `spec.md`, the repo's guidance and the code by path rather than pasting them, and say what the PR leaves to its siblings.
3. **Revise a brief** only when a reshape or a Revise round changes its PR, and `plan.md` whenever the plan changes.
