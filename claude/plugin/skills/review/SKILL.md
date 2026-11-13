---
name: review
description: Review passes over a Task's PRs and Stacks. Use when an IC picks the Review passes for a plan or dispatches a Reviewer, and when a Reviewer runs a pass.
---

# Review

A Review pass is a scope (one PR, or one Stack) plus the Review dimensions checked over it, run by one Reviewer Subagent independently of the Builder. Each dimension is a file in `dimensions/` beside this one, holding its question and its evidence bar: what a finding on it must show. The files name no tool or harness, so any model can run them.

## Principles

- **Evidence, not taste.** A finding that doesn't meet its dimension's evidence bar is dropped.
- **Independent.** The Reviewer sees the spec, the code and the repo, never the Builder's account of its work.
- **Findings live in files.** They go under the Task's `reviews/`, never into the IC's context: the IC learns only the path and the count.

## Picking passes (IC)

At plan time, list the passes in `plan.md` and show them at plan approval. The defaults:

| Scope | Dimensions |
|---|---|
| each PR | intent, correctness, standards |
| each PR | tests |
| each Stack, once all its PRs are open | reviewability, simplicity, architecture |

A light Task gets one pass with every dimension instead.

To run a pass, dispatch a Reviewer with: the scope (worktree and base, or the Stack's PRs), the absolute paths of its dimension files and of `spec.md`, the findings file to write, `reviews/<pr-or-stack>-<dimensions>-<round>.md`, and an instruction to follow "Running a pass" in this skill.

## Running a pass (Reviewer)

1. Read the repo's guidance (`CLAUDE.md`, glossary, ADRs), the spec, and each dimension file you were given.
2. Review the scope along each dimension. Hold every finding to its dimension's evidence bar.
3. Triage: drop findings that miss the bar, merge duplicates, and mark each finding **blocking** (the PR shouldn't open, or the Stack shouldn't stand, as it is) or **non-blocking**.
4. Write the findings file: for each finding, its dimension, blocking or not, its location (file and line, PR or commit), the evidence, and the fix you suggest. Blocking findings come first.
5. Reply with only the file's path and the count, such as `reviews/03-writer-intent-correctness-standards-1.md: 4 findings, 1 blocking`.
