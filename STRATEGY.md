# flow Strategy

## Vision

<!-- vision: author-approved 2026-11-24 -->

`flow` exists so that one Director can hand loose asks to coding agents and get back PRs worth reviewing, across many Sessions and CDEs, without steering each conversation or keeping in their head who is doing what.
It serves the Director and the agents working for them.
It has two halves: the `flow@flow` skills, a process that turns an ask into a spec, a plan and reviewed draft PRs; and the Store, a picture of every Session, Task and PR that the dashboard shows the Director and the CLI shows agents.
It owns exactly one thing: the path from the Director's ask to PRs they can judge, and the record of where each piece of work stands on it.

### The Director aligns and reviews; agents do the rest

The Director's time goes on two things only: agreeing what a Task delivers, and reviewing what comes back.
A Manager turns an ask into a Task and a short brief in about a minute, makes no design decisions, and never codes.
An IC owns a Task from brief to deliverable: it grills the Director with numbered questions and a recommended answer each, writes the spec and plan, and dispatches Builders and Reviewers, but never touches code itself.
That boundary holds even for a one-line fix: the IC resumes the Builder that built the PR rather than editing the code.
Agents find facts themselves and ask only what only the Director can decide: requirements, scope, data model, architecture and accepted tradeoffs.
Each decision is made once, with the IC, and written into `spec.md`; a later comment that changes a requirement goes back to the Director as a spec change.
An IC carries its PRs to mergeable on its own: it fixes failing CI, resolves merge conflicts and addresses agent reviewers' findings, and it addresses a human's review comments only with the Director's approval.
Once the Director approves a PR, on GitHub or in the IC's pane, the IC may merge it into its Feature branch; merging into the default branch is always the Director's.
Everything an agent posts on GitHub appears under the Director's name, so an agent may reply on a thread an agent reviewer started, but a reply to a human is a draft the Director approves first.
Agents create Jira issues only when the Director asks, and otherwise only keep their own Branches' issues current.

### Quality comes from the process, not from pushing

A PR is good because of four things: an agreed target in `spec.md`, independent review, evidence, and small, legible PRs.
Builders work test-first, so evidence exists from the start, and claims in a PR body are only those that actually ran against the final diff.
Reviewers never see the Builder's account of its work, and a finding that does not meet its Review dimension's evidence bar is dropped.
Each PR is one concern, small enough to review in one sitting, and explains itself to a staff-level reviewer who was not in the grilling, through the facts that force its shape.
Context is spent deliberately: findings and detail live in files, and each role reports in a line.

### Determinism in the tools, judgement in the skills

flow is its binaries and its skills built together: the binaries make the steps that must not vary deterministic, and the skills guide the agents through everything that needs judgement.
The skills separate procedure from guidance.
The procedure is flow's: the roles, the phases, the approvals, the files each role writes and the handoffs between them.
How to align, write a spec, plan, build or review is guidance; flow ships its own opinions, and a Director can plug in their favourite skills in their place.
Each repo's `CLAUDE.md`, glossary, ADRs and templates decide what good looks like there and win over flow's guidance; where the repo is silent, flow's bar applies, test-first included, even if that means adding a repo's first tests.
Skills and Review dimensions name no harness or tool they do not need, so any capable agent can run them.

### flow improves itself

flow keeps a record of how past work went, so the Director and agents can find where the factory is slow.
Where a repo's guidance was missing, conflicting or wrong, the IC writes a line in `notes.md`.
Recurring gaps become draft PRs to that repo's guidance, reviewed by the Director like any other PR, because PRs are how a repo changes.

### Observe, launch on request, never supervise

The binaries take over only the steps agents keep getting wrong in skill text, such as launching a Session, which failed 11 of 11 times from skill text alone.
flow starts a Session only when the Director asks; launching work is always the Director's act.
flow never wakes, steers or nudges a Session on anyone's behalf, and no Session reports to another; a Session may wait on its own work through flow's reads.
flow v1 put a supervisor at its centre and became too complex to use; a change that brings any part of it back revisits ADR-0004 first.

### One Store of reported facts, one picture

Every fact comes from an interface meant to carry it, such as agent hooks, `flow wt create`, `flow task join`, and the GitHub and Jira APIs; flow does not infer facts from screens or shell text.
When a fact is missing, the fix is to make its source report it, not to add detection.
Each Facet has exactly one Writer, and the dashboard and the CLI read the same facts through the same functions.
The dashboard answers "who needs me now" first, a lamp lights only when the Director has something to act on, and done work leaves the list on its own.
flow sends no notifications and does not actively monitor Sessions; the Director's attention is already on the dashboard, and they can ask about any Session.
The dashboard takes the Director to a Session, such as by focusing its herdr pane; the conversation stays in the agent's own harness.
Worktrees and Branches are the agents' implementation detail; the Director launches from the repo's main checkout and never manages them.
flow is held to an MVP bar: core behaviour correct, half the code wherever half will do, and no defence against a failure that costs less than the defence.

### Scope

flow is not a supervisor, an orchestrator, or a message bus between Sessions.
It is not an agent harness: the Director talks to a Session in that Session's own pane.
It is not a GitHub or Jira client: its binaries write to neither, and its agents write only branches, draft PRs, Feature-branch merges and their own Jira issues.
It is not a team tool: it works for one Director, whose CDEs alone share `/efs-home`.
Its skills are not a standalone product: they lean on the binaries for the steps that must not vary.
It does not start CDEs or herdr servers.
Scheduled work that looks back, such as retros, reviews or roadmap building, is in scope; scheduled work that launches Tasks is not.

A change aligns when it spends less of the Director's time per reviewable PR, raises the bar every PR meets through spec, independent review or evidence, makes the work visible sooner or more truthfully, helps flow learn from past work, lets the Director swap in their own guidance skills, or replaces fragile skill text with one tested step taken on request.
A change should be resisted when it pulls the Director into implementation detail, lets the agent that built work also judge it, puts the IC's hands on code, overrides a repo's own standards, hard-codes guidance into procedure, has flow act on a Session unasked, launches work without the Director, infers a fact a source could report, merges into the default branch or answers a human without the Director, or adds machinery that costs more than the failure it prevents.
