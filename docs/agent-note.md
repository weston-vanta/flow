## flow

flow records every Claude Code Session on every CDE and the PRs each one owns. `flow guide`
prints this note.

- Create worktrees with `flow wt create <branch> [wt args]`, never plain `wt create`: it runs
  `wt create` and records this Session as the owner of the branch's PRs.
- Four reads, each printing JSON: `flow session list`, `flow session get <id>`,
  `flow pr list` (owned PRs only) and `flow pr get <owner/repo#number>`. A PR's `owner` is its
  owning Session's id, and `$CLAUDE_CODE_SESSION_ID` is yours. Your Stack:
  `flow pr list | jq '.[] | select(.owner == env.CLAUDE_CODE_SESSION_ID)'`
