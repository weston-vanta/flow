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
- A `~/.claude` shared across hosts. Without it, flow cannot summarize observed sessions
  running on other hosts, and the supervisor's conversation cannot be resumed with
  `--resume` after the supervisor moves to another host.
- The Ona CLI logged in as you on every host that may run the supervisor:
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

Then, from a shell pane inside herdr on the machine that should run the supervisor, start it:

```sh
flow up
```

The supervisor runs in that pane: `flow up` replaces itself with Claude Code there. Exit it as
you would any Claude Code session; the next `flow up` resumes its conversation. Removing
`claude_session` from `state/supervisor/lock.json` makes the next `flow up` start the
supervisor fresh.

`flow` uses `$FLOW_STATE` as the state directory when set, otherwise `state/` at the root of
its clone.

Workers run in the herdr session `flow up` ran in, which the supervisor's lock records; with no
lock, flow uses herdr's `default` session. To keep flow apart from the sessions you start by
hand, run `flow up` from a pane in a named herdr session. On each host, workers go in a herdr
workspace labelled `flow`, which the first dispatch there creates, one tab per task labelled
with its title and id, as `<title> · <id>`. A herdr server flow starts where none runs carries
the environment of flow's ssh login to that host, or of the flow command on its own host,
without flow's variables or a Claude Code session's (`CLAUDECODE`, `CLAUDE_*`), and the panes
you open there later inherit it. To have flow also see and watch the agents of other herdr
sessions, list them in `state/config.toml`; `flow init` does not write it:

```toml
herdr_sessions = ["default"]
```

flow does not manage how you view the hosts. The herdr client on your laptop is the view: save
each host as a machine there by hand, `herdr machine add <ona alias> --label <name>`, adding
`--remote-session <session>` when flow runs in a named herdr session, and its sidebar lists
every host's `flow` workspace with a tab per task.

Moving from the `flow` session: earlier versions ran the supervisor and workers in a herdr
session named `flow`, and their records leave that session out, which flow no longer accepts.
Exit the supervisor, run `scripts/migrate-2026-10-29.sh <state-dir>` once to write the session
in (`--dry-run` shows the changes first), then run `flow up` from a pane in the herdr session
you want. Open tasks stay in the session they started in, where
flow still watches and steers them; add `"flow"` to `herdr_sessions` to keep them in
`flow sessions` until they close. Once nothing runs in the old session, stop its server by hand
with `herdr --session flow server stop`.

## CLI

Commands exit 3 when the state directory is missing and 2 when they refuse a request as
given. Commands that take `--json` print raw JSON instead of the compact view.
`flow <command> --help` has the details.

Operator:

- `flow init [--force]`: write `state/config.toml` from what this machine can tell, then refresh the inventory; `--force` replaces an existing file.
- `flow up`: from a shell pane inside herdr, run the supervisor in that pane; anywhere else it exits 1. It asks only this host's herdr: when the lock records the supervisor on this host and herdr reports an agent there with the lock's Claude Code session, it prints `the supervisor is running in pane <id> (workspace <label>)` and exits 0. Otherwise it records this host, this pane and its herdr session in the lock, keeping the Claude Code session to resume, and replaces itself with Claude Code in this pane, resuming that session while Claude Code keeps a non-empty transcript of it and starting a fresh one otherwise. When the lock names another host that is not gone, it first prints `the previous supervisor was recorded on <host> since <time> ago; if it is still running there, exit it`. Starting one first writes `state/supervisor/watch-state.json` with every closed task marked read when that file does not exist, so the first `flow watch` replays only open tasks, and otherwise forgets the observed sessions it records, so the first `flow watch` announces none of the sessions the supervisor's start procedure asks about.
- `flow status [--all] [--json]`: open tasks, their holds, pending dispatch requests from workers (`req-id  from-task  host  title`), and observed sessions; `--all` includes done and orphaned tasks.
- `flow inventory [--refresh] [--json]`: worktrees, agents, and PRs per host; `--refresh` first discovers hosts from Ona (or the static list), probes each running one, and lists open PRs. `--json` gives each host its ssh alias, whether it is the host flow runs on (`local`), its Ona phase, when it was last seen, and whether it is reachable or gone.
- `flow run <host> -- <argv>...`: run a command on a host, passing its output and exit code through.
- `flow sessions [--summaries] [--json]`: herdr agents across hosts, with their tasks, as the supervisor, or as observed sessions. `--summaries` adds each session's latest summary, and `--json` includes summaries, pane ids and each agent's `owner`.
- `flow follow <host> <session>`: wake the supervisor when a session started by hand goes idle or done, not only when it turns blocked. `<session>` is the session's title as `flow sessions` shows it, or its pane id when several share the title, or `<herdr session>/<pane id>` when several herdr sessions hold that pane id; `flow sessions` then marks it `followed`.
- `flow unfollow <host> <session>`: stop waking the supervisor for a followed session except when it turns blocked; also accepts the pane id, or `<herdr session>/<pane id>`, of a followed session that is gone.

Sessions started by hand sit on a ladder:

- **observed**: a herdr agent flow can see but does not manage: any agent in a listed herdr session whose pane is neither a task's nor the supervisor's. It wakes the supervisor when it turns `blocked`, and once when a listing first finds it (`observed-new`). The supervisor asks you whether to adopt each observed session at its start and when one appears; adopting interrupts the session mid-turn with a brief, so sessions you use interactively are usually better left observed.
- **followed**: an observed session on the follow list. It also wakes the supervisor on `idle` and `done`.
- **adopted**: a session started by hand that the supervisor took over as a task with `flow task adopt`. It is a worker in every way flow can manage, and `flow sessions` shows its task id.

herdr reuses pane ids after its server restarts, so a pane alone does not say whose agent it holds. flow tells the supervisor and each task's worker by the Claude Code session herdr reports for its agent: the agent reporting the session the supervisor's lock records is the supervisor, wherever it runs, and one reporting the session a task recorded is that task's worker until `flow task close`. An agent herdr reports no session for is the worker of the task whose recorded herdr target is its pane while the pane's tab label still names the task, as dispatch labels it, or, for a task adopted from a session herdr reported no session for, under any label. Under a label naming no such task herdr reused the pane: flow retires the task's herdr target, logging a `target.retired` event with the reason, and the agent is observed. The listings `flow sessions`, `flow follow`, `flow task adopt` and the watcher make can each retire a target this way, and the watcher wakes the supervisor with `agent-gone: <id>` when an open task's target is retired. Any other such agent in the lock's pane, the one `flow up` ran in, is the supervisor. The supervisor's SessionStart hook records its Claude Code session in the lock, so telling it by its session needs herdr's Claude Code integration. Any other agent is observed.

Supervisor:

- `flow dispatch (--title <title> (--brief <text> | --brief-file <file>) | --request <req-id> [--title <title>]) --criteria <text> [--host <name>] [--dir <path> | --branch <name> [--repo <name>]] [--parent <id>] [--allow-shared-dir]`: start a worker session for a new task, in a tab labelled `<title> · <id>` in the host's `flow` workspace; with no such workspace it creates one and takes its first tab. `--request` dispatches a pending request from `flow status`: it supplies the brief, and the title, host and parent unless given, and is consumed once the worker starts.
- `flow task adopt <host> <session> --title <title> --criteria <text> [--brief <text> | --brief-file <file>] [--parent <id>]`: take over a session started by hand as a task. `<session>` is resolved as `flow follow` resolves it. The session must be an observed one, not a task's or the supervisor's, and Claude Code with a working directory herdr knows. flow writes the task and its brief, drops the session from the follow list, and prompts it to read the brief, printing `adopted <id> on <host> (<pane>)`. Without `--brief`, the brief asks it to continue the work already under way. When the prompt cannot be delivered, for example because the session is blocked, the task is held `blocked` on the supervisor and the command exits 1; `flow steer` can deliver it later. Any other failure once the task exists also holds it `blocked` on the supervisor, naming the failure, and exits 1. The brief's commands set `FLOW_STATE`, since the session was not started by flow.
- `flow steer <id> <text>`: send a message to a task's worker and log it; a `needs-decision` or `blocked` task reads `working` again. It sends nothing when the task's pane holds no agent of the task's, since herdr reuses pane ids, and refuses with "the worker's pane is gone" once flow has retired the task's herdr target.
- `flow task show <id> [--json]`: a task's fields and the head of its brief, naming the brief's file when the head leaves lines out; `--json` carries the full brief.
- `flow task events <id> [--last <N>] [--json]`: a task's event log.
- `flow task decide <id> --by user|supervisor --answer <text>`: answer the task's hold and lift it.
- `flow task hold <id> --on user|supervisor --what <text>`: hold the task until someone answers.
- `flow task orphan <id> --reason <text>`: mark the task orphaned: its work will not finish.
- `flow task close <id> [--force]`: close the task's herdr tab, keeping its record, working directory and branch; `--force` closes it while the task is still open (working, pending, blocked or needs-decision). herdr closes the `flow` workspace with its last tab. herdr reuses tab ids, so it closes the tab only while herdr lists it under the task's label, which ends in the task id, and the task's pane holds the task's worker or no agent; under another label herdr reused the tab, so flow retires the task's target and closes nothing; a pane holding another agent refuses, closing nothing. An adopted task's tab belongs to whoever started the session, so it stays open: flow only stops treating the pane as the task's and prints `closed <id> (pane left to its owner)`.

An adopted task keeps running in the session that was started by hand, so flow cannot add hooks to it. It reports through its own `flow task` calls and herdr's view of it. In place of the Stop hook, the watcher logs a `turn.end` event on the task whenever herdr sees the session go from `working` to `idle`, so a session that stops without reporting wakes the supervisor as `quiet` once the quiet threshold passes, as a dispatched worker would. A turn that ends while no watcher runs is not logged, so that wake may come early. It does not wake on `idle` at once, since the user may still be typing in that session. flow knows the session by the Claude Code session id it had when adopted; once that id changes, through `/clear` or a resume under a new id, flow sees an observed session in the task's pane and no longer watches or steers it as the task.

Worker:

- `flow task status <id> working|blocked|needs-decision [--message <text>]`: report the task's state; `--message` is required for `blocked` and `needs-decision`.
- `flow task done <id> --report <file>`: finish the task with a report.
- `flow task request-dispatch <id> --title <title> --brief <file> [--host <name>]`: ask the supervisor to dispatch sibling work.

Hooks and plumbing, run by Claude Code and the supervisor rather than by hand, and left out of `flow --help`:

- `flow guard`: Stop hook that blocks ending a turn while work is unwatched.
- `flow watch [--heartbeat <secs>] [--once]`: watch tasks and exit 2 with a reason when the supervisor should wake. Every observed session wakes the supervisor when it turns `blocked`, except one adopted as a task, which is watched as that task's worker; `flow follow` adds `idle` and `done` for that session. A listing that finds an observed session in a pane neither its run's previous listing nor the saved one held wakes the supervisor once with `observed-new: <host> <pane> "<title>"`. While only such unfollowed sessions are watched there is no hourly heartbeat: the watch lists the hosts again every hour, so a session started meanwhile is watched within the hour, and ends quietly after a day.
- `flow wake drain`: print the queued wakes not yet acknowledged.
- `flow wake ack --through <N>`: acknowledge every queued wake up to and including N.
- `flow hook claude --task <id>`: record a worker's Claude Code hook event read as JSON from stdin.
- `flow hook supervisor`: record the supervisor's Claude Code session in its lock from its SessionStart hook on stdin.

## Docs

- Design: [docs/superpowers/specs/2026-10-26-flow-design.md](docs/superpowers/specs/2026-10-26-flow-design.md)
- Implementation plan: [docs/superpowers/plans/2026-10-26-flow-implementation-plan.md](docs/superpowers/plans/2026-10-26-flow-implementation-plan.md)
