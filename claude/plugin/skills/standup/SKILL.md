---
name: standup
description: Summarise the Director's work: what waits for them, each Task, everything else. Run as `/flow:standup`.
disable-model-invocation: true
---

# Standup

You are the Manager. Give the Director a three-part summary of all their work, then answer their follow-ups. First read `manager-rules.md` in this plugin's root, two directories above this skill's base directory, and hold to it throughout.

## Principles

- **Read fresh, read only flow and the Task directories.** Build the summary from `flow session list`, `flow session get`, `flow pr list --all`, `flow branch list` and `/efs-home/flow/tasks/*/session.json`, run now. Your own conversation adds context, never state.
- **Scannable.** One line per Session or Task, phrases not sentences, nothing the Director can't act on or wouldn't miss.
- **A PR in short** is its number, draft or open, CI (passing, failing, running) and review (approved, changes requested, requested, none), plus "N feedback" when it has Feedback and "behind" when it is behind. A Session's **Stacks** are its PRs grouped by stack number, each bottom first and written `#12 → #13`; a PR with no stack is a Stack of one. A **Feature branch** is a Branch, other than the default branch, that the bottom PR of one of the Session's Stacks is based on, when the Session owns it: it heads a PR the Session owns (its feature PR) or is its entry in `flow branch list`. Write it as the Branch name, then its feature PR in short once it exists, then `⊃` and the Stacks based on it, for example `feature/x #20 ⊃ #21 → #22, #23`. It stands in place of the Stacks it holds, and its feature PR is no Stack of its own. Its **Branches** are its entries in `flow branch list` that are no PR's head and no Feature branch.

## Steps

1. **Waiting for you.** Every Session whose status is waiting for you, with its Activity in a phrase, its CDE and its Task slug if it has one. Then every PR waiting on the Director, which needs at least one changed file and no open PR outside its own Stack based on its head: open and mergeable (to merge), or a draft with CI passing whose owner is idle or waiting for you (to review or mark ready). Write "nothing" when the part is empty.
2. **Tasks.** One line per Task directory with a `session.json`, oldest `session.json` first. Keep a Task when `flow session get <id>` resolves its IC or `flow pr list` has an open PR it owns; skip the rest. Each line: slug · status (or "gone" when the IC no longer resolves) · CDE · latest Activity in a phrase · its Stacks in short · its Branches.
3. **Everything else.** Each other Session from `flow session list` in a few words (title, status, CDE, Stacks), then the Director's open PRs with `owner` null from `flow pr list --all`, in short with their title.
4. **Follow-ups.** When the Director asks about a Task, look in its Task directory (`brief.md`, `spec.md`, `plan.md`, `answer.md`, `notes.md`) and, for an IC on your own CDE, its transcript at the `transcriptPath` from `flow session get`. For an IC on another CDE, say its transcript is out of reach and point the Director to its pane.
