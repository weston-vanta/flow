---
name: standup
description: Summarise the Director's work: what waits for them, each Task, everything else. Run as `/flow:standup`.
disable-model-invocation: true
---

# Standup

You are the Manager. Give the Director a three-part summary of all their work, then answer their follow-ups. First read `manager-rules.md` in this plugin's root, two directories above this skill's base directory, and hold to it throughout.

## Principles

- **Read fresh, read only flow and the Task directories.** Build the summary from `flow task list`, `flow session list`, `flow pr list --all` and `flow branch list`, run now. Your own conversation adds context, never state.
- **Scannable.** One line per Session or Task, phrases not sentences, nothing the Director can't act on or wouldn't miss.
- **A PR in short** is its number, draft or open, CI (passing, failing, running) and review (approved, changes requested, requested, none), plus "N feedback" when it has Feedback and "behind" when it is behind, then its Branch's Jira issue in short. A Session's **Stacks** are its PRs grouped by stack number, each bottom first and written `#12 → #13`; a PR with no stack is a Stack of one. A **Feature branch** is a Branch, other than the default branch, that the bottom PR of one of the Session's Stacks is based on, when the Session owns it: it heads a PR the Session owns (its feature PR) or is its entry in `flow branch list`. Write it as the Branch name, then its feature PR in short once it exists, then `⊃` and the Stacks based on it, for example `feature/x #20 ⊃ #21 → #22, #23`. It stands in place of the Stacks it holds, and its feature PR is no Stack of its own. Its **Branches** are its entries in `flow branch list` that are no PR's head and no Feature branch, each with its Jira issue in short.
- **A Jira issue in short** comes from the Branch's `jira`: `KEY-1 <state>` once fetched, the state judged from `category` and `status` (`done` → done; `indeterminate` with a status matching /review/i → in review; other `indeterminate` → in progress; anything else → to do), so `KEY-1 in review`. It is `KEY-1 not found` when `found` is false, the bare `KEY-1` while `found` is null, and nothing when `jira` is null. Write no summary and no note on the Jira data's age. A Jira issue is information only: it never makes anything wait for the Director.

## Steps

1. **Waiting for you.** Every Session whose status is waiting for you, with its Activity in a phrase, its CDE and its `task` slug if it has one. Then every PR waiting on the Director, which needs at least one changed file and no open PR outside its own Stack based on its head: open and mergeable (to merge), or a draft with CI passing whose owner is idle or waiting for you (to review or mark ready). Write "nothing" when the part is empty.
2. **Tasks.** One line per Task in `flow task list`, in its order. Look up each of its `sessions` in `flow session list`; its current Session is the one with the latest `origin.at`, the nearest proxy for joined last. Each line: slug · the current Session's status · CDE · latest Activity in a phrase · the Stacks in short and Branches of all its Sessions. Leave its Resources off the line.
3. **Everything else.** Each Session from `flow session list` whose `task` is null, in a few words (title, status, CDE, Stacks), then the Director's open PRs with `owner` null from `flow pr list --all`, in short with their title.
4. **Follow-ups.** When the Director asks about a Task, run `flow task get <slug>`. List its Resources, each as its `title`, or its URL shortened (scheme, `www.`, query and a trailing `/edit` dropped) when `title` is null. Read the files in its `dir` (`brief.md`, `spec.md`, `plan.md`, `answer.md`, `notes.md`) and, for an IC on your own CDE, its transcript at the `transcriptPath` from `flow session get`. For an IC on another CDE, say its transcript is out of reach and point the Director to its pane.
