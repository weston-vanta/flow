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
- Tasks: `flow task create <slug>` creates a Task and its Task directory,
  `$FLOW_HOME/tasks/<slug>/`, and prints `{slug, dir}`; it fails if the slug is taken and
  joins no Session. `flow task join <slug>` puts this Session on that Task, replacing any
  earlier one (a Session is on at most one Task, and `/clear` leaves the new Session on none).
  `flow task list` prints every Task a listed Session is on, in slug order, and
  `flow task get <slug>` prints one; each is `{slug, dir, sessions: [ids], resources}`.
  `flow session list` and `flow session get` carry `task`: the Session's Task slug, or null.
- Managers launch an IC with `flow session start --name <task-slug> --brief <path> <repo-dir>`.
  It opens a new, unfocused herdr tab in `<repo-dir>` (the repo's main checkout, which needs a
  wt base), in a workspace named after your Session, starts Claude there with a fresh Session
  id, sends `/flow:ic <brief path>`, and prints `{id, name, cde, pane}`. Keep the brief on
  `/efs-home`. Add `--cde <name>` to launch on another running CDE (it never starts one); the
  output is the same, with that CDE's name as `cde`. On failure it prints `{error, help}`, leaves the pane as it is and retries
  nothing: show the error to the Director rather than trying again.
