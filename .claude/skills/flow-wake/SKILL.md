---
name: flow-wake
description: Use when a turn starts with `Stop hook feedback` from `flow watch` or `flow guard`, or when `flow wake drain` lists wakes at session start. Covers draining the wake queue, handling each reason, acknowledging, and ending the turn.
---

# Handling wakes

## 1. Drain

`flow wake drain` prints one `seq  age  reason` row per unhandled wake, then
`WAKE_ACK_REQUIRED: flow wake ack --through N`. The hook feedback that woke you names only
one reason; handle every row.

## 2. Handle each reason

- `done: <id>`: `flow task show <id>`, and the full brief from the file it names (or `flow task
  show <id> --json`); the `report` event in `flow task events <id> --last 5 --json` gives the
  report's path; read it. Check it against the brief's completion criteria. To look at the
  worker's files on another host, use `flow run <host> -- <argv>`, never plain `ssh`. Met: tell
  the user the outcome with links, then retire the worker's pane with `flow task close <id>`;
  its worktree and branch stay. When it refuses because herdr lists that workspace under another
  label, or another agent runs in the task's pane, the workspace is no longer the task's: leave
  it, and tell the user. Not met: steer the worker to finish (`flow steer <id> "<what is missing>"`) and say so.
- `needs-decision: <id>` or `blocked: <id>`: `flow task show <id>` shows the worker's question
  as the hold. Follow the `flow-escalate` skill.
- `agent-blocked: <id>`: the worker's harness waits at a prompt (a permission or dialog), not
  a question the worker asked through flow. Check `flow task events <id>`; if you cannot tell
  what it waits on, `flow task hold <id> --on user --what "..."` and tell the user which pane
  to attach to: `flow task show <id>` names it as `<herdr session>/<pane>`. Always name the
  herdr session, since the same pane id can exist in several.
- `agent-done: <id>`: the worker's session stopped while its task still reads `working` or
  `pending`, without reporting through flow. Check `flow task events <id>`, then steer it to
  continue or to report its status. A worker that stops after reporting `needs-decision` or
  `blocked` is waiting on its hold and does not wake you.
- `agent-gone: <id>`: the worker's pane or session disappeared, or herdr reused its pane and flow
  retired the task's herdr target (`target.retired` in its events). Check `flow sessions`. If the
  work cannot continue, `flow task orphan <id> --reason "<last known: branch pushed or not>"`
  and ask the user whether to redispatch. Never tear down what is left. For an adopted task
  (`origin` is `adopted` in `flow task show`), its owner closed the pane: orphan it with what
  was last known and `flow task close` it, which makes no herdr call; there is nothing to
  redispatch unless the user asks.
- `host-unreachable: <host>`: keep working from state. Tell the user plainly that `<host>` cannot
  be reached (an expired login, or a stopped or recycled host). After `flow inventory
  --refresh`, `flow inventory` shows a recycled host as `gone`; its tasks are orphaned with what
  was last known; ask before redispatching.
- `quiet: <id> <N>s`: no events and no writes in the task's working directory for N seconds.
  Check `flow task events <id> --last 5` and `flow sessions`, then steer the worker with a
  short status question.
- `request: <id>`: a dispatch request; follow the `flow-dispatch` skill.
- `observed-new: <host> <pane> "<title>"`: a session the user started by hand appeared. Tell
  the user which host and session it is and ask whether to adopt it, stating once that adopting
  delivers a brief with flow's reporting protocol to that session as a prompt, which interrupts
  an agent mid-turn and redirects it to the task you define, and that a session they are using
  interactively is usually better left observed. Adopt it only on a yes (`flow-dispatch`
  skill); otherwise leave it observed.
- `observed-blocked: <host> <pane> "<title>"`: a session the user started by hand is stuck at a
  harness prompt (a permission or dialog). Tell the user which host and session it is and that
  they need to attend to it. Do not steer a hand session unless they ask you to.
- `observed-<status>: <host> <pane> "<title>"`, for `idle`, `done` or `unknown` (its pane went
  away): a session the user asked you to follow changed state; tell them what it is doing, from
  `flow sessions --summaries`. A summary whose `matched_by` is `cwd` in `flow sessions --json`
  is a guess by directory; say so. `flow unfollow <host> <title>` once they no longer need it.
- A wake queued before flow called these sessions observed may read `adopted-<status>: …`;
  handle it as `observed-<status>: …`.
- A session taken over with `flow task adopt` is a task: its wakes are the task reasons above,
  never `observed-`. It has no hooks; the watcher logs a `turn.end` event whenever herdr sees
  it go from working to idle. A `quiet:` wake for it means it ended a turn, or reported, that
  long ago without reporting since and without writing in its working directory: it may have
  finished, or stopped to ask the user something in its own terminal. A turn that ended while
  no watcher ran is not logged, so the wake may come early; look before acting on it.
- `heartbeat`: nothing happened. Glance at `flow status`; report only real changes.
- `watch-failed: <error>`: the watcher hit an error. Each distinct error wakes you once;
  the watcher started at your next turn end keeps retrying it in the background and resumes
  watching when it clears. Tell the user what it says; it likely needs fixing outside the
  conversation. The same error waking you again a heartbeat (an hour) later means it is
  still failing.

## 3. Acknowledge

`flow wake ack --through N` with the N from the drain's `WAKE_ACK_REQUIRED` line, never
higher: wakes queued after the drain must stay pending.

## 4. End the turn

Tell the user what matters, then end the turn. Do not sleep, poll, or wait in the turn;
stopping re-arms `flow watch`. If `flow guard` blocks the stop, do what its reason says
(usually: drain and acknowledge the wakes it names), then end the turn again.
