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

- `done: <id>`: `flow task show <id>` and `flow task brief <id>`; the `report` event in
  `flow task events <id> --last 5 --json` gives the report's path; read it. Check it against the
  brief's completion criteria. Met: tell the user the outcome with links. Not met: steer the
  worker to finish (`flow steer <id> "<what is missing>"`) and say so.
- `needs-decision: <id>` or `blocked: <id>`: `flow task show <id>` shows the worker's question
  as the hold. Follow the `flow-escalate` skill.
- `agent-blocked: <id>`: the worker's harness waits at a prompt (a permission or dialog), not
  a question the worker asked through flow. Check `flow task events <id>`; if you cannot tell
  what it waits on, `flow task hold <id> --on user --what "..."` and tell the user which pane
  (`flow task show <id>`) to attach to.
- `agent-done: <id>`: the worker's session stopped without reporting through flow. Check
  `flow task events <id>`, then steer it to continue or to report its status.
- `agent-gone: <id>`: the worker's pane or session disappeared. Check `flow sessions`. If the
  work cannot continue, `flow task orphan <id> --reason "<last known: branch pushed or not>"`
  and ask the user whether to redispatch. Never tear down what is left.
- `host-unreachable: <host>`: keep working from state. Tell the user plainly that `<host>` cannot
  be reached (an expired login, or a stopped or recycled host). A recycled host's tasks are
  orphaned with what was last known; ask before redispatching.
- `quiet: <id> <N>s`: no events and no worktree writes for N seconds. Check
  `flow task events <id> --last 5` and `flow sessions`, then steer the worker with a short
  status question.
- `request: <id>`: a dispatch request; follow the `flow-dispatch` skill.
- `adopted-<status>: <host>/<pane>`: a session the user asked you to follow changed state; tell
  them. `flow unfollow <host> <pane>` once they no longer need it.
- `heartbeat`: nothing happened. Glance at `flow status`; report only real changes.

## 3. Acknowledge

`flow wake ack --through N` with the N from the drain's `WAKE_ACK_REQUIRED` line, never
higher: wakes queued after the drain must stay pending.

## 4. End the turn

Tell the user what matters, then end the turn. Do not sleep, poll, or wait in the turn;
stopping re-arms `flow watch`. If `flow guard` blocks the stop, do what its reason says
(usually: drain and acknowledge the wakes it names), then end the turn again.
