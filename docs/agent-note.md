## flow

flow records every Claude Code Session on every CDE and the PRs each one owns. `flow guide`
prints this note.

- Create worktrees with `flow wt create <branch> [wt args]`, never plain `wt create`: it runs
  `wt create` and records this Session as the owner of the branch's PRs.
- Five reads, each printing JSON: `flow session list`, `flow session get <id>`,
  `flow pr list` (owned PRs only; `--all` adds your PRs no Session owns, with `owner` null),
  `flow pr get <owner/repo#number>` and `flow branch list` (every Branch a listed Session
  owns, with `at`, when its worktree was created; drop those that head a PR in
  `flow pr list`). A PR's `owner` is its owning Session's id, and `$CLAUDE_CODE_SESSION_ID`
  is yours. Your Stack:
  `flow pr list | jq '.[] | select(.owner == env.CLAUDE_CODE_SESSION_ID)'`
