# A Session owns the PRs whose worktrees it created

A Session owns a PR because it created the worktree for the PR's branch, reported by `flow wt create` (which wraps `wt create`) as the reporter of that Branch (repo and branch name). The PR Writer fetches all of the user's own PRs without knowing about Sessions, and a PR's owner is the reporter of the Branch matching its repo and head branch; a later report replaces an earlier one, so the most recent reporter wins. We chose this over detecting links from what agents do, because detection was unreliable and would have needed guessing logic. The obvious source, Claude Code's own `pr-link` transcript records, uses an internal format. It records at most one PR per command, so 4 of 18 stack PRs had no record. It ignores `gh stack`, and it would also link `gh pr checkout` and comments on other people's PRs.

## Considered Options

- **Claude Code `pr-link` records**: no setup, but they miss stack PRs and over-link, as described above.
- **A flow hook parsing `git push` / `gh pr create` / `gh stack` commands**: complete, but it guesses from shell text and must decide who pushed, which is wrong when a dispatched worker in its own pane pushes the owner's PR.
- **An explicit `flow claim` the owner's skill runs, with takeover**: works before any PR exists, but it adds a claim lifecycle and takeover rules. Worktree creation already happens at the same moment for the same reason, one worktree per PR.

## Consequences

- A Session that never runs `flow wt create` (an ad-hoc conversation, or plain `git worktree add`) shows no PRs. The fix is to use `flow wt create`, not to add detection.
- `wt` stays unaware of flow; agents' instructions name `flow wt create` instead of `wt create`.
- Ownership moves only when another Session creates a worktree for the same branch; there is no takeover command.
