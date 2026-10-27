---
name: flow-escalate
description: Use when a worker reports `needs-decision` or `blocked`, or a task holds on a question. Covers deciding whether to answer yourself or escalate to the user, recording the decision or hold with `flow task decide` and `flow task hold`, and steering the worker with `flow steer`.
---

# Answering or escalating a worker's question

## 1. Read the question

`flow task show <id>` shows the hold: who it waits on and the worker's question.
`flow task brief <id>` and `flow task events <id> --last 10` give the context.

## 2. Decide or escalate

Apply the decision authority rule in `CLAUDE.md`:

- **Answer it yourself** when the answer follows from the brief, the codebase, or what you
  and the user have agreed.
- **Escalate** when the answer could change what the user asked for, when more than one
  resolution is reasonable and they lead to different work, or when the question shows the
  brief has drifted from what was agreed.

## 3a. Answering yourself

1. `flow task decide <id> --by supervisor --answer "<the answer and why>"` logs the decision
   and lifts the hold.
2. `flow steer <id> "<the answer, as an instruction the worker can act on alone>"` delivers it.
   Deciding records the answer; only steering reaches the worker.

## 3b. Escalating

1. `flow task hold <id> --on user --what "<the question>"` records that the task waits on the
   user, so a recycled supervisor knows.
2. Ask the user, batched with any other open questions, each one specific: what was asked,
   what you would choose and why, what you need from them. Then end the turn; the worker
   stays held.
3. When the user answers: `flow task decide <id> --by user --answer "<their answer>"`, then
   `flow steer <id> "<the answer as an instruction>"`.

## Blocked workers

A worker `blocked` on something outside its control (access, a broken environment, a failing
dependency): fix it yourself if that is within the brief and steer the worker on; otherwise
hold on the user and say what is needed.

## Steering

- `flow steer <id> <text>` prompts the worker's herdr pane and logs a `steer` event. It
  returns once the worker starts on the message. A `needs-decision` or `blocked` task reads
  `working` again afterwards, and a hold on you is lifted; a hold on the user stays.
- herdr refuses to prompt a worker blocked at a harness prompt (such as a permission dialog);
  hold on the user and tell them which pane to attach to.
- Keep steer messages self-contained: the worker has its brief and its own context, not yours.
- Steer adopted sessions (those not started by `flow dispatch`) only when the user asks;
  `flow steer` targets tasks.
