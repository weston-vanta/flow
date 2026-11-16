# A Session joins a Task only by running `flow task join`

A Task is the set of Sessions that joined it, identified by its slug, and a Session joins only by running `flow task join <slug>`, which `flow:ic` does first. Nothing carries a Task from one Session to the next on its own, because `/clear` is the Director's signal that they are done with the Task: the new Session in that pane is on no Task until the Director rejoins it explicitly, for example with `/flow:ic <brief>`.

## Considered Options

- **An environment variable set by `flow session start`**: it survives `/clear` (same process), so it would keep a Session on a Task the Director just ended.
- **The herdr pane**: same problem, and pane ids are short and reused.
- **A Task record written by `flow session start`**: it adds a second Writer of Task membership and a creation step; joining through `flow:ic` covers the launch already.

## Consequences

- A Session's Branches and PRs count toward its Task through ownership (ADR-0002); a Session that joins another Task takes them along.
- A Session the Director starts by hand is on no Task unless it runs `flow task join`.
