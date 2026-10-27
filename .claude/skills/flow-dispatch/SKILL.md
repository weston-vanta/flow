---
name: flow-dispatch
description: Use when the user asks for work a worker should do, or `flow requests` lists a worker's dispatch request. Covers choosing a host from `flow inventory`, writing a brief with completion criteria, and starting the worker with `flow dispatch`.
---

# Dispatching a worker

## 1. Choose the host and worktree

Run `flow inventory` (add `--refresh` when it is stale or a host is missing). Hosts are not
interchangeable: each has its own checkout and worktrees.

- A host whose worktree already holds the branch or PR is the task's natural home. Reuse that
  worktree with `--worktree <path>`.
- Otherwise pick a host and pass `--branch <name>`; flow creates the worktree with the
  configured worktree tool.
- Ask the user only when no home is obvious and the choice matters. Mention load only when
  the inventory shows it matters.
- Never dispatch into a worktree where a live session works. `flow dispatch` refuses (exit 2)
  and names the agent; pass `--allow-shared-worktree` only when the user says so.

## 2. Write the brief

The worker sees only its brief, never this conversation. Write:

- The ask, in the user's terms, with the context the worker cannot find for itself: why it
  matters, decisions already made, constraints, links.
- Completion criteria: concrete, checkable, and saying where to stop, for example
  `- A draft PR is open with the change and its checks pass` or
  `- The report answers which callers rely on the old behavior`. Follow-up work is a new task.
- Never ask a worker to merge, to tear down work, or to contact the user. For PR work, point
  the worker at the monorepo's own conventions (`state/operator.md` says which).

Long asks go in a file passed with `--brief-file`; short ones inline with `--brief`. flow adds
the worktree rule and the reporting contract to the brief itself.

## 3. Dispatch

```
flow dispatch --host <name> --title "<short title>" \
  --brief-file <file> --criteria "- <criterion>" \
  --branch <name>
```

Use `--worktree <path>` instead of `--branch` to reuse a worktree, and add
`--parent <task-id>` when splitting work off an existing task.

It prints `dispatched <task-id> on <host> (<pane>)`. Then:

- A warning that the worker is blocked on something other than the trust dialog: check
  `flow task events <task-id>` (`worker.blocked_at_start` holds the pane text) and steer it or
  ask the user.
- A failure after the task was created leaves it `blocked` with a supervisor hold naming the
  failure. Fix the cause and dispatch again, then `flow task orphan <old-id> --reason "..."`.

Tell the user in one line what went where, and end the turn; `flow watch` arms on the stop.

## Worker requests

`flow requests` lists `req-id  from-task  host  title`; `flow requests --json` adds the
`brief_path`. Dispatch it as above with `--brief-file <brief_path> --parent <from-task>`,
using the requested host unless the inventory shows a better home, then
`flow requests consume <req-id>`. A request outside what the user asked for is a question
for the user (`flow-escalate`), not an automatic dispatch.
