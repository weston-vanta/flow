# Task aaaaaaab: Add retry to the example client

You are a worker session run by a supervisor. This brief is your whole assignment.

## Working directory

`/workspaces/example-repo.wt/retry`

Work only inside this directory; never touch the main checkout or other worktrees.

## What is asked

Add retries with backoff to the example client.

Keep the public API.

## Completion criteria

- Tests cover the retry path
- A draft PR is open

## Reporting

Report through these exact commands, run from your shell.

- At the start, and on each phase change:
  `/opt/flow/bin/flow task status aaaaaaab working --message "<what you are doing>"`
- When you need a decision you cannot make from this brief or the code:
  `/opt/flow/bin/flow task status aaaaaaab needs-decision --message "<question>"`
  Then stop and wait for an answer to appear in your terminal.
- When something outside your control blocks you:
  `/opt/flow/bin/flow task status aaaaaaab blocked --message "<what blocks you>"`
- When work outside this task's scope should run as a sibling task, write its brief to a file,
  then: `/opt/flow/bin/flow task request-dispatch aaaaaaab --title "<title>" --brief <file> [--host <name>]`
- When every completion criterion is met, write a short Markdown report to a file, then:
  `/opt/flow/bin/flow task done aaaaaaab --report <path>`

You never address the user directly: everything goes to the supervisor through these commands.
