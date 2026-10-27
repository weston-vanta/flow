---
name: flow-dispatch
description: Use when the user asks for work a worker should do, or `flow requests` lists a worker's dispatch request. Covers choosing a host from `flow inventory`, writing a brief with completion criteria, and starting the worker with `flow dispatch`.
---

# Dispatching a worker

## 1. Choose the host and working directory

Run `flow inventory` (add `--refresh` when it is stale or a host is missing). Hosts are not
interchangeable: each has its own checkouts and worktrees, listed per repo when several repos
are configured. A `missing: <repo>` line means that host has no checkout of the repo; do not
send `--branch` work in that repo there.

- A host whose worktree already holds the branch or PR is the task's natural home. Dispatch
  there with `--host <name>` and reuse that worktree with `--dir <path>`.
- When nothing ties the task to a host, leave out `--host`: flow runs it on this machine. When
  the inventory shows this machine busy, pass the least loaded host instead.
- Work in a repo on a new branch: pass `--branch <name>`; flow creates the worktree with the
  repo's worktree tool, or with `git worktree add` from the repo's default branch when it has
  none. When several repos are configured, add `--repo <name>`.
- Work in some other existing directory on the host: pass `--dir <absolute path>`.
- Work that needs no repo (research, preparation, an investigation): pass none of `--dir`,
  `--branch` or `--repo`. The worker runs in the task's own `work/` directory under
  `state/tasks/<task-id>/`, readable from every host.
- Ask the user only when no home is obvious and the choice matters. Mention load only when
  the inventory shows it matters.
- Never dispatch into a directory where a live session works. `flow dispatch` refuses (exit 2)
  and names the agent; pass `--allow-shared-dir` only when the user says so.

## 2. Write the brief

The worker sees only its brief, never this conversation. Write:

- The ask, in the user's terms, with the context the worker cannot find for itself: why it
  matters, decisions already made, constraints, links.
- Completion criteria: concrete, checkable, and saying where to stop, for example
  `- A draft PR is open with the change and its checks pass` or
  `- The report answers which callers rely on the old behavior`. Follow-up work is a new task.
- Never ask a worker to merge, to tear down work, or to contact the user. For PR work, point
  the worker at the repo's own conventions (`state/operator.md` says which).

Long asks go in a file passed with `--brief-file`; short ones inline with `--brief`. flow adds
the working directory, the worktree rule for repo worktrees, and the reporting contract to the
brief itself.

## 3. Dispatch

```
flow dispatch [--host <name>] --title "<short title>" \
  --brief-file <file> --criteria "- <criterion>" \
  [--dir <path> | --branch <name> [--repo <name>]]
```

Add `--parent <task-id>` when splitting work off an existing task.

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
