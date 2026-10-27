# flow

`flow` is a command-line tool that turns one Claude Code session into a supervisor for
worker sessions running across several cloud development environments. It dispatches
tasks, watches their progress, and reports back, keeping all durable state in a plain-file
state directory.

## Install

flow runs from a clone at `/workspaces/flow` on each host. Its `state/` is a symlink to a
state directory on storage every host shares, so tasks, briefs, reports and config are the
same wherever you run it.

You provide:

- A Rust toolchain on every host.
- herdr on every host, with its Claude Code integration: `herdr integration install claude`.
- Claude Code installed and logged in on every host.
- A `~/.claude` shared across hosts. Without it, flow cannot summarize adopted sessions
  running on other hosts, and the supervisor's conversation cannot be resumed with
  `--resume` after the supervisor moves to another host.
- The Ona CLI logged in on the supervisor's machine: `gitpod login`, then
  `gitpod environment ssh-config` so every environment has an ssh alias.
- Optionally, `gh auth login` on the hosts where flow should list your open PRs.

Then, on each host:

```sh
git clone https://github.com/weston-vanta/flow.git /workspaces/flow && bash /workspaces/flow/scripts/install.sh
export PATH="/workspaces/flow/target/release:$PATH"
```

Put the `export` in your shell profile too, so `flow` stays on `PATH` in new shells.

`scripts/install.sh` pulls the clone, creates the state directory, links `state/` to it,
builds `target/release/flow`, and tells you when `target/release` is not on your `PATH`. Each
step prints `done` or `already`. It is safe to run again, so a machine-start script can call
it on every start. `FLOW_DIR` (default `/workspaces/flow`) and `FLOW_STATE_DIR` (default
`/efs-home/flow/state`) change where the clone and the state directory are.

Once, on the machine that runs the supervisor:

```sh
flow init
```

`flow init` writes `state/config.toml` from what this machine can tell: the repository (where
Ona checked out this environment, else the only git repository under `/workspaces`), its
default branch, the `wt` worktree tool when it is on `PATH`, and hosts discovered from Ona.
It prints each choice and how it was found, refreshes the inventory, and lists what you still
provide. Edit `state/config.toml` to add repositories or override host names; `flow init
--force` writes it afresh.

Then start the supervisor and attach to it:

```sh
flow up
```

`flow` uses `$FLOW_STATE` as the state directory when set, otherwise `state/` at the root of
its clone.

## CLI

Commands exit 3 when the state directory is missing and 2 when they refuse a request as
given. Commands that take `--json` print raw JSON instead of the compact view.
`flow <command> --help` has the details.

Operator:

- `flow init [--force]`: write `state/config.toml` from what this machine can tell, then refresh the inventory; `--force` replaces an existing file.
- `flow up`: attach to the live supervisor session, or start one on this host and attach.
- `flow attach`: attach to the supervisor session.
- `flow status [--json]`: open tasks, their holds, and adopted sessions.
- `flow inventory [--refresh] [--json]`: worktrees, agents, and PRs per host; `--refresh` first discovers hosts from Ona (or the static list), probes each running one, and lists open PRs.
- `flow host list [--json]`: known hosts from the cache, gone ones last, marking the one this runs on.
- `flow host run <name> -- <argv>...`: run a command on a host, passing its output and exit code through.
- `flow sessions [--json]`: herdr agents across hosts, with their tasks or as adopted sessions.
- `flow follow <host> <pane-id>`: wake the supervisor when an adopted session changes state.
- `flow unfollow <host> <pane-id>`: stop waking the supervisor for an adopted session.
- `flow wake drain`: print the queued wakes not yet acknowledged.
- `flow wake ack --through <N>`: acknowledge every queued wake up to and including N.

Supervisor:

- `flow dispatch --title <title> --criteria <text> (--brief <text> | --brief-file <file>) [--host <name>] [--dir <path> | --branch <name> [--repo <name>]] [--parent <id>] [--allow-shared-dir]`: start a worker session for a new task.
- `flow steer <id> <text>`: send a message to a task's worker and log it.
- `flow task show <id> [--json]`: a task's fields and the head of its brief.
- `flow task brief <id>`: a task's brief in full.
- `flow task events <id> [--last <N>] [--json]`: a task's event log.
- `flow task list [--all] [--json]`: open tasks; `--all` includes done and orphaned ones.
- `flow task decide <id> --by user|supervisor --answer <text>`: answer the task's hold and lift it.
- `flow task hold <id> --on user|supervisor --what <text>`: hold the task until someone answers.
- `flow task orphan <id> --reason <text>`: mark the task orphaned: its work will not finish.
- `flow task close <id> [--force]`: close the task's herdr workspace, keeping its record, working directory and branch; `--force` closes it while the task is still working or pending.
- `flow requests [--json]`: dispatch requests not yet consumed.
- `flow requests consume <req-id>`: mark a request dispatched so it stops listing.

Worker:

- `flow task status <id> working|blocked|needs-decision [--message <text>]`: report the task's state; `--message` is required for `blocked` and `needs-decision`.
- `flow task done <id> --report <file>`: finish the task with a report.
- `flow task request-dispatch <id> --title <title> --brief <file> [--host <name>]`: ask the supervisor to dispatch sibling work.

Hooks, run by Claude Code rather than by hand:

- `flow guard`: Stop hook that blocks ending a turn while work is unwatched.
- `flow watch [--heartbeat <secs>] [--once]`: watch tasks and exit 2 with a reason when the supervisor should wake.
- `flow hook claude --task <id>`: record a worker's Claude Code hook event read as JSON from stdin.
- `flow hook supervisor`: record the supervisor's Claude Code session from its SessionStart hook on stdin.

## Docs

- Design: [docs/superpowers/specs/2026-10-26-flow-design.md](docs/superpowers/specs/2026-10-26-flow-design.md)
- Implementation plan: [docs/superpowers/plans/2026-10-26-flow-implementation-plan.md](docs/superpowers/plans/2026-10-26-flow-implementation-plan.md)
