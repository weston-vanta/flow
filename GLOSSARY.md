# flow

flow shows the Director's agent sessions across all their cloud development environments, with the pull requests each session produced, to the Director in a local web app and to agents through the same data. It launches a Session when asked, but never supervises one.

## Language

### Machines

**CDE**:
One Ona cloud development environment, as listed by `gitpod environment list`. Every CDE flow covers mounts the shared `/efs-home`.
_Avoid_: host, machine, environment, box

**Flow CDE**:
The one CDE, named `flow`, where every flow component runs. It is recycled by hand; anything that must survive a recycle lives on `/efs-home`.
_Avoid_: supervisor host, main CDE

### Factory

**Director**:
The one person flow works for. They define work with Managers, align with ICs and review their PRs.
_Avoid_: user, human, owner

**Manager**:
A Session the Director uses to delegate Tasks to ICs and to report on them. It never codes. A role, not a kind of Session: any Session that delegates a Task is a Manager.
_Avoid_: supervisor, coordinator, orchestrator, lead

**IC**:
A Session on a Task that owns it through to its deliverable: it aligns with the Director, plans, dispatches Builders and Reviewers, and owns the resulting PRs. It never touches code itself. A role, not a kind of Session.
_Avoid_: worker, agent, implementer

**Task**:
One piece of work the Director delegates through a Manager, from its brief to its deliverable: PRs, a spec, or an answer. Identified by its slug. A Task is the Sessions that joined it, usually one IC at a time; its Branches and PRs are the ones those Sessions own. A Session is on at most one Task, and only by joining it explicitly: `/clear` leaves the new Session on none. A Task is listed while any of its Sessions is. It can point at any number of Resources.
_Avoid_: job, ticket, work item, assignment

**Builder**:
A Subagent that implements one PR test-first in its worktree, and addresses the findings on it.
_Avoid_: implementer, coder, worker

**Reviewer**:
A Subagent that checks work against one Review pass, independently of whoever built it, and reports findings.
_Avoid_: critic, checker

**Review dimension**:
One question a review asks, such as correctness or simplicity, with the evidence a finding on it must show.
_Avoid_: criterion, lens, axis

**Review pass**:
A scope (one PR, or one Stack) plus the Review dimensions checked over it, run by one Reviewer.
_Avoid_: round, review

**Explainer**:
An interactive page that explains a change, an existing system or a plan through one diagram, for reviewers who need the design before the diffs. An IC builds one only when the Director asks.
_Avoid_: walkthrough, review guide, architecture doc

### Work

**Session**:
One Claude Code conversation running in a herdr pane on some CDE, identified by its Claude session id. Its record outlives the pane. Its status is one of running, waiting for you, idle (alive, not working) or done (the conversation ended: exited, cleared, or its pane closed). `/clear` ends one Session and starts another in the same pane; resuming a conversation continues the same Session. A done Session stays listed while it owns an open PR, and for a day after it has none; one that never created a worktree stays listed only five minutes.
_Avoid_: agent, worker, conversation

**Subagent**:
A helper a Session runs inside its own conversation. It is not a Session and has no Session id of its own; what it does is reported as its Session's.
_Avoid_: agent, worker, child session

**Activity**:
What a Session was last asked and, once it finished that turn, what it last said. Shown as one line per Session: the request while it is running, the reply otherwise. It is what the Session itself said, not a summary of it.
_Avoid_: summary, recap, status

**PR**:
One GitHub pull request, identified by its repository and number. Only PRs a Session owns are shown.
_Avoid_: pull, change, diff

**Jira issue**:
One issue in Jira, identified by its key (such as `ABC-123`). An epic is a Jira issue like any other. flow knows a Jira issue only as the one a Branch implements: each Branch has at most one. A Jira issue a Task points at (its Source, an epic) is just a Resource. flow never changes Jira issues in Jira.
_Avoid_: ticket, card, story

**Resource**:
Something outside flow that a Task points at, such as a Jira issue, a doc or an Explainer, identified by its URL.
_Avoid_: link, attachment, external record, reference

**Feedback**:
A PR's review threads that are unresolved and not outdated: what is still left to address. Resolved threads, threads on code that has since changed, top-level comments and review bodies are not feedback.
_Avoid_: comments, threads

**Behind**:
A PR is behind when the oldest commit on its repository's default branch that its head lacks landed there more than 24 hours ago. Only PRs based on the default branch can be behind; one based on another PR's branch never is. Measured by when that commit landed, not by how many commits are missing. A queued PR is never behind.
_Avoid_: stale, out of date

**Queued**:
A PR is queued while it is in its repository's GitHub merge queue. Its position is its place in that queue (1 merges next). A queued PR needs nothing from its owner.
_Avoid_: enqueued, merging, landing

**Owner**:
The Session that owns a Stack: responsible for seeing it through, even while Subagents or other Sessions write, review or fix its PRs. A Session owns a Branch by creating a worktree for it; when several Sessions have, the most recent one owns it. It owns the PRs whose head is that Branch. Each PR has at most one owner; a Session can own any number of Stacks. Owning is a relationship between a Session and its Branches and PRs, not a kind of Session; any Session can own a Stack.
_Avoid_: supervisor, lead, stack agent

**Branch**:
A git branch in a repository, identified by the repository and the branch name. Its owner is the Session that most recently created a worktree for it, and owns the PRs whose head is this branch. A Branch its owner has opened no PR for yet is shown under that Session.
_Avoid_: ref, worktree

**Stack**:
PRs a Session owns that build on one another, each based on the one below it, in the order of their GitHub stack (bottom first). A Session can own several Stacks, all in one repository; a single PR based on the default branch is a Stack of one. Not a thing of its own, only an ordering.
_Avoid_: chain, series

### Data

**Store**:
The one place flow keeps what it knows, on `/efs-home`. Everything the web app and CLI show is read from the Store, never from herdr, GitHub or transcripts directly. It holds current state only, not history.
_Avoid_: cache, database, state

**Facet**:
One named part of a Session, PR or Branch, such as a Session's status or a PR's GitHub data, written by exactly one Writer and stamped with when its fact was observed. A newer write replaces it; an older one is dropped.
_Avoid_: component, field group, section

**Writer**:
Anything that puts facts into the Store, always through flow, never by touching the Store directly: such as the hook in each Session or the poller that fetches PRs from GitHub. Each Facet has exactly one Writer.
_Avoid_: monitor, observer, producer

**Reader**:
Anything that shows the Store's facts, always through flow's one read path, never the Store directly: the web app (through flow's read API), or an agent (through the `flow` CLI). Both get the same facts.
_Avoid_: consumer, client
