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
- The Ona CLI logged in as you on every host that may run or attach to the supervisor:
  `gitpod login`, then `gitpod environment ssh-config` so every environment has an ssh alias.
- Optionally, `gh auth login` on the hosts where flow should list your open PRs.

Then, on each host:

```sh
git clone https://github.com/weston-vanta/flow.git /workspaces/flow && bash /workspaces/flow/scripts/install.sh
```

install.sh links the binary into `~/.local/bin` (`FLOW_BIN_DIR`), which most shells already
have on `PATH`; it tells you if yours does not.

`scripts/install.sh` pulls the clone, creates the state directory, links `state/` to it,
builds `target/release/flow`, and links that binary into `FLOW_BIN_DIR`. Each step prints
`done` or `already`. It is safe to run again, so a machine-start script can call it on every
start. `FLOW_DIR` (default `/workspaces/flow`), `FLOW_STATE_DIR` (default
`/efs-home/flow/state`) and `FLOW_BIN_DIR` (default `~/.local/bin`) change where the clone,
the state directory and the binary link are.

Once, on the machine that runs the supervisor:

```sh
flow init
```

`flow init` writes `state/config.toml` from what this machine can tell: the repository (where
Ona checked out this environment, else the only git repository under `/workspaces`), its
default branch, the `wt` worktree tool when it is on `PATH`, and hosts discovered from Ona.
It prints each choice and how it was found, refreshes the inventory with a line per host
(`flow inventory` shows it in full), and lists what you still provide. Edit
`state/config.toml` to add repositories or override host names; `flow init --force` writes it
afresh.

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
- `flow up`: attach to the live supervisor session, or start one on this host and attach. When stdout is not a terminal, it prints the command to attach with instead of attaching.
- `flow status [--all] [--json]`: open tasks, their holds, pending dispatch requests from workers (`req-id  from-task  host  title`), and adopted sessions; `--all` includes done and orphaned tasks.
- `flow inventory [--refresh] [--json]`: worktrees, agents, and PRs per host; `--refresh` first discovers hosts from Ona (or the static list), probes each running one, and lists open PRs. `--json` gives each host its ssh alias, whether it is the host flow runs on (`local`), its Ona phase, when it was last seen, and whether it is reachable or gone.
- `flow run <host> -- <argv>...`: run a command on a host, passing its output and exit code through.
- `flow sessions [--summaries] [--json]`: herdr agents across hosts, with their tasks or as adopted sessions; `--summaries` adds each session's latest summary, and `--json` includes summaries and pane ids.
- `flow follow <host> <session>`: wake the supervisor when a session started by hand goes idle or done, not only when it turns blocked. `<session>` is the session's title as `flow sessions` shows it, or its pane id when several share the title; `flow sessions` then marks it `followed`.
- `flow unfollow <host> <session>`: stop waking the supervisor for a followed session except when it turns blocked; also accepts the pane id of a followed session that is gone.

Supervisor:

- `flow dispatch (--title <title> (--brief <text> | --brief-file <file>) | --request <req-id> [--title <title>]) --criteria <text> [--host <name>] [--dir <path> | --branch <name> [--repo <name>]] [--parent <id>] [--allow-shared-dir]`: start a worker session for a new task. `--request` dispatches a pending request from `flow status`: it supplies the brief, and the title, host and parent unless given, and is consumed once the worker starts.
- `flow steer <id> <text>`: send a message to a task's worker and log it; a `needs-decision` or `blocked` task reads `working` again.
- `flow task show <id> [--json]`: a task's fields and the head of its brief, naming the brief's file when the head leaves lines out; `--json` carries the full brief.
- `flow task events <id> [--last <N>] [--json]`: a task's event log.
- `flow task decide <id> --by user|supervisor --answer <text>`: answer the task's hold and lift it.
- `flow task hold <id> --on user|supervisor --what <text>`: hold the task until someone answers.
- `flow task orphan <id> --reason <text>`: mark the task orphaned: its work will not finish.
- `flow task close <id> [--force]`: close the task's herdr workspace, keeping its record, working directory and branch; `--force` closes it while the task is still open (working, pending, blocked or needs-decision).

Worker:

- `flow task status <id> working|blocked|needs-decision [--message <text>]`: report the task's state; `--message` is required for `blocked` and `needs-decision`.
- `flow task done <id> --report <file>`: finish the task with a report.
- `flow task request-dispatch <id> --title <title> --brief <file> [--host <name>]`: ask the supervisor to dispatch sibling work.

Hooks and plumbing, run by Claude Code and the supervisor rather than by hand, and left out of `flow --help`:

- `flow guard`: Stop hook that blocks ending a turn while work is unwatched.
- `flow watch [--heartbeat <secs>] [--once]`: watch tasks and exit 2 with a reason when the supervisor should wake. Every session started by hand, outside flow's herdr session, wakes the supervisor when it turns `blocked`; `flow follow` adds `idle` and `done` for that session. While only such unfollowed sessions are watched there is no hourly heartbeat: the watch lists the hosts again every hour, so a session started meanwhile is watched within the hour, and ends quietly after a day.
- `flow wake drain`: print the queued wakes not yet acknowledged.
- `flow wake ack --through <N>`: acknowledge every queued wake up to and including N.
- `flow hook claude --task <id>`: record a worker's Claude Code hook event read as JSON from stdin.
- `flow hook supervisor`: record the supervisor's Claude Code session from its SessionStart hook on stdin.

## Docs

- Design: [docs/superpowers/specs/2026-10-26-flow-design.md](docs/superpowers/specs/2026-10-26-flow-design.md)
- Implementation plan: [docs/superpowers/plans/2026-10-26-flow-implementation-plan.md](docs/superpowers/plans/2026-10-26-flow-implementation-plan.md)
