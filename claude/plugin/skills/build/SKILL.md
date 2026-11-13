---
name: build
description: Build one PR of a Task as a Builder, test-first in its worktree. Use when an IC dispatches a Builder or opens a PR, and when a Builder starts a PR or a Revise round.
---

# Build

A Builder is a Subagent that writes the code for one PR in a worktree the IC created. The IC dispatches it, runs the Reviews and opens the PR; the Builder never sees a Reviewer.

## Principles

- **Test-first.** Each behaviour starts as a failing test through the repo's existing test seams, so the evidence for the change exists from the start.
- **The repo's guidance is the standard.** Its `CLAUDE.md`, glossary, ADRs and templates win over any default here.
- **Explain with `flow:explain`.** Commits, the PR title and the PR body follow it, and whoever changes the diff rewrites the body.
- **Note the gaps.** When you must make a choice the repo's guidance should have made (it was missing, two pieces conflicted, or following it led somewhere worse), and another Task in this repo would hit it too, add one line to the Task's `notes.md`: the guidance's location (or none), what happened, and what was done instead.
- **Report in a line or two.** The IC's context is for the Task; the detail lives in commits and files.

## Dispatching a Builder (IC)

Give it the worktree path and base, the paths of `spec.md` and its brief (`prs/<nn>-<name>.md`), the PR file to write (`prs/<nn>-<name>.pr.md`), and an instruction to follow "Building (Builder)" in this skill. For findings to address, add their source and `flow:revise`.

Keep the Builder that built a PR for its per-PR Review rounds, by continuing it rather than starting a new one. Per-Stack work (a reshape, or a follow-up PR with its own brief) and each Revise round get a fresh Builder.

## Building (Builder)

1. Read the brief, `spec.md` and the repo's guidance. Then build the PR test-first in the worktree, and nothing the brief leaves to its siblings.
2. Run the repo's checks (build, tests, lint) until they pass, and commit.
3. Write the PR file: the title on the first line, then the body.
4. Propose labels from the repo's rules, including any security label the diff calls for.
5. Reply with the checks you ran and their result, the labels, and any of the repo's PR gates the PR would fail (such as size, missing tests or a weak description).

## Opening the PR (IC)

The PR opens once its per-PR passes leave no blocking finding, or after the second round. Push the branch and open it as a draft at once, following the repo's rules for opening PRs: its template headings verbatim, its labels, the Builder's proposed security label, and any body markers. If one of the repo's gates would stop the PR, open the draft anyway, report the condition to the Director, and add a `notes.md` line. When a later round changes the PR, push and replace the body with the rewritten PR file.
