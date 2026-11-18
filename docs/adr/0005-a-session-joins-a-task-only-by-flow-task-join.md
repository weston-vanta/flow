# A Session joins a Task only by running `flow task join`

A Task is the set of Sessions that joined it, identified by its slug, and a Session joins only by running `flow task join <slug>`, which `flow:ic` does first. Nothing carries a Task from one Session to the next on its own, because `/clear` is the Director's signal that they are done with the Task: the new Session in that pane is on no Task until the Director rejoins it explicitly, with `/flow:ic <slug>`.

## Considered Options

- **An environment variable set by `flow session start`**: it survives `/clear` (same process), so it would keep a Session on a Task the Director just ended.
- **The herdr pane**: same problem, and pane ids are short and reused.
- **A Task record written by `flow session start`**: it adds a second Writer of Task membership; joining through `flow:ic` covers the launch already.

## Consequences

- A Session's Branches and PRs count toward its Task through ownership (ADR-0002); a Session that joins another Task takes them along.
- A Session the Director starts by hand is on no Task unless it runs `flow task join`.

## Amendment: a Task begins at `flow task create`

A Task now begins when `flow task create <slug>` creates its Task directory (`$FLOW_HOME/tasks/<slug>/`), so flow is the one place that knows where that directory lives and whether a slug is taken. `/go` runs it on the Director's go; `flow task join` fails for a slug with no Task directory. The directory is what makes a Task exist; the Store holds a `task` record only once the Task has a Resource. Creating a Task joins no Session, so membership still has one Writer, `flow task join`, and a Task is still listed only while one of its Sessions is. Every IC is on a Task: `/flow:ic` with no slug points the Director at `/flow:go` rather than creating one. See [What the skills say about Jira issues](../../.scratch/jira/issues/06-skills-and-jira.md).
