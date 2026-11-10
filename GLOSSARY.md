# flow

flow shows the user's agent sessions across all their cloud development environments, with the pull requests each session produced, to the user in a local web app and to agents through the same data.

## Language

### Machines

**CDE**:
One Ona cloud development environment, as listed by `gitpod environment list`. Every CDE flow covers mounts the shared `/efs-home`.
_Avoid_: host, machine, environment, box

**Flow CDE**:
The one CDE, named `flow`, where every flow component runs. It is recycled by hand; anything that must survive a recycle lives on `/efs-home`.
_Avoid_: supervisor host, main CDE

### Work

**Session**:
One Claude Code conversation running in a herdr pane on some CDE, identified by its Claude session id. Its record outlives the pane. Its status is one of running, waiting for you, idle (alive, not working) or done (the conversation ended: exited, cleared, or its pane closed). `/clear` ends one Session and starts another in the same pane; resuming a conversation continues the same Session. A done Session stays listed while it owns an open PR, and for a day after it has none.
_Avoid_: agent, worker, task, conversation

**Activity**:
What a Session was last asked and, once it finished that turn, what it last said. Shown as one line per Session: the request while it is running, the reply otherwise. It is what the Session itself said, not a summary of it.
_Avoid_: summary, recap, status

**PR**:
One GitHub pull request, identified by its repository and number. Only PRs a Session owns are shown.
_Avoid_: pull, change, diff

**Owner**:
The Session that owns a Stack: responsible for seeing it through, even while other agents write, review or fix its PRs. A Session owns a PR by creating the worktree for its branch; when several Sessions have, the most recent one owns it. Each PR has at most one owner, and a Session owns at most one Stack. Owning is a relationship between a Session and its PRs, not a kind of Session; any Session can own a Stack.
_Avoid_: supervisor, lead, stack agent

**Branch**:
A git branch in a repository, identified by the repository and the branch name. It records which Session most recently created a worktree for it; that Session owns the PRs whose head is this branch.
_Avoid_: ref, worktree

**Stack**:
The PRs a Session owns, in the order of their GitHub stack (bottom first). A Session has at most one Stack; a single PR is a Stack of one. Not a thing of its own, only an ordering.
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
