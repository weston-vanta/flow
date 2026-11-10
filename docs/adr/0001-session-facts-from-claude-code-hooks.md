# Session facts come from Claude Code hooks, not from herdr

flow learns that a Session exists, where it runs and what state it is in only from a `flow` hook for Claude Code's lifecycle events (`SessionStart`, `UserPromptSubmit`, `PermissionRequest`, `Notification`, `PostToolUse`, `Stop`/`StopFailure`, `SessionEnd`), registered in each CDE's dotfiles Claude settings as one command, `flow hook`, run from the binary shared on `/efs-home`; there is no hook script and no installer, and `flow hook` reads the event name from its stdin. We considered sweeping `herdr api snapshot` on every CDE over ssh from the Flow CDE, which sees more (herdr detects state from the screen, so it notices an Esc interrupt, a crash and a stopped CDE), but it needs a long-running process, ssh connection management, a list of CDEs, and logic to tell an unreachable CDE from vanished Sessions: the kind of machinery that sank flow v1. Hooks need none of that, report instantly through a documented interface, and are the same path agents will later use to report statuses of their own.

## Consequences

- Known gaps, accepted for the MVP: after Esc a Session shows running until its next event; a crashed or killed Session, or one on a stopped CDE, never becomes done; a `claude` started inside another session inherits the outer pane id.
- The fix, when needed, is an additional `liveness` Writer on its own Facet (for example a per-CDE subscriber to the local herdr socket), not a change to the hooks.
- `PostToolUse` fires on every tool call; it runs asynchronously to keep it off the turn's latency.
