# flow

flow shows every Claude Code Session on every CDE, each with the Stack of PRs it owns: to the
user in a local web app served by `flowd`, and to agents as JSON through the `flow` CLI. It
launches a Session when asked, but never supervises one (see
[ADR-0004](docs/adr/0004-flow-launches-sessions-but-never-supervises-them.md)).

This is flow v2, under construction. Terms are defined in [GLOSSARY.md](GLOSSARY.md) and the
hard-to-reverse choices in [docs/adr/](docs/adr/).

## Install

On the Flow CDE, `scripts/install` builds the frontend and both release binaries and installs
`flow` and `flowd` into `/efs-home/flow/bin` (or the directory given as its argument),
replacing each by rename. A running `flowd` keeps its old binary until restarted. It also
installs the Claude Code plugin marketplace in `claude/` to `/efs-home/flow/claude` (beside the
bin directory): marketplace `flow`, plugin `flow`, whose hooks run `flow hook` on every Session
event. Each CDE's dotfiles enable `flow@flow` through `extraKnownMarketplaces` (a `directory`
source at `/efs-home/flow/claude`) and `enabledPlugins`, put the bin directory on PATH, and
point agents at `flow guide`, which prints [the agent note](docs/agent-note.md).

## Usage

You (the Director) talk to one Claude Code Session, which delegates each piece of work (a
Task) to a new Session of its own, an IC. The IC asks you what it needs to know, and its
helpers write the code and open draft PRs. You watch everything on the dashboard and answer
each Session in its own herdr pane. You need a running herdr server on every CDE you use, and a
`gitpod` login wherever you delegate from.

1. **Delegate.** In any Session in the repo's main checkout, run `/flow:go <ask>`. The ask can
   be a Jira link, a PRD link, a question or a sentence. The Session becomes your Manager. It
   asks at most one round of questions, then shows a short brief, a Task slug and where to run
   the IC: a CDE, or here. Once you confirm, it creates the Task and starts the IC with
   `flow session start` in a new herdr tab, without switching you to it, or tells you to run
   `/clear`, then `/flow:ic <slug>`.
2. **Align.** Go to the IC's tab. It questions you until you approve a `spec.md`, then shows
   a plan of the PRs, which you approve separately. The Task's files live in
   `/efs-home/flow/tasks/<slug>/`.
3. **Let it build.** The IC never writes code. Its Subagents (helpers inside the same Session)
   build each PR test-first in its own worktree, and others review it. It opens each PR as a
   draft once it passes review, and comes back to you only when it needs a decision. The first
   line of its reply says what it needs, and the dashboard shows that line.
4. **Review.** Review the drafts on GitHub as usual, then tell the IC in its pane. It addresses
   your comments and posts nothing on GitHub. It never marks a PR ready and never merges one.
   Once the PRs are merged or closed, tell it you're done, and it removes its worktrees.
5. **Check in.** Run `/flow:standup` in the Manager for what waits for you, where each Task
   stands, and everything else. The Manager reads state only when you ask; nothing wakes it.

The dashboard is the web app `flowd` serves (port 7171 by default). It lists every Session on
every CDE with its status and its PRs, grouped into Stacks, plus any branches it hasn't opened a
PR for yet. Agents read the same data as JSON through `flow session list`, `flow pr list` and
`flow branch list`.
