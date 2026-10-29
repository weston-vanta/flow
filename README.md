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

The supervisor and its workers run in herdr's `default` session, the one a herdr window
shows, beside the sessions you start by hand, so one window shows them all. Each task's
workspace is labelled with its title and id, as `<title> · <id>`, and the supervisor's is
`flow-supervisor`. A herdr server flow starts where none runs carries the environment of
flow's ssh login to that host, or of the flow command on its own host, without flow's
variables or a Claude Code session's (`CLAUDECODE`, `CLAUDE_*`), and the panes you open there
later inherit it. To keep flow in a herdr session of
its own instead, set both keys in `state/config.toml`; `flow init` writes neither:

```toml
herdr_session = "flow"
herdr_sessions = ["flow", "default"]
```

`herdr_session` is where the supervisor and workers run. `herdr_sessions` lists the sessions
whose agents flow sees and watches, and always includes `herdr_session`.

To see every host in one herdr window, run `flow inventory --sync` on the machine you attach
from, after `flow inventory --refresh` or with it. It saves a herdr SSH machine per other host
on that machine, each showing the host's herdr session flow runs in, so the window's sidebar
lists every host's workspaces and agents next to the local ones. Run it again when hosts come
and go; it asks before changing anything.

Moving from the `flow` session: earlier versions ran the supervisor and workers in a herdr
session named `flow`. A supervisor running there is still found and attached to, and open
tasks stay in the session they started in, where flow still watches and steers them; add
`"flow"` to `herdr_sessions` to keep them in `flow sessions` until they close. To move the
supervisor, exit it, then run `flow down` and `flow up`, which starts a new one in the
configured session.

## CLI

Commands exit 3 when the state directory is missing and 2 when they refuse a request as
given. Commands that take `--json` print raw JSON instead of the compact view.
`flow <command> --help` has the details.

Operator:

- `flow init [--force]`: write `state/config.toml` from what this machine can tell, then refresh the inventory; `--force` replaces an existing file.
- `flow up`: attach to the live supervisor session, or start one on this host and attach. When stdout is not a terminal, it prints the command to attach with instead of attaching. When the supervisor's pane holds an agent in or below `/workspaces/flow`, in the supervisor's workspace, that flow cannot confirm is the supervisor, it starts nothing and exits 1, saying what herdr shows there, since a second supervisor would resume the same session. Only the user can tell which it is: if it is the supervisor, `flow down --force` closes that pane; if it is their own session, `flow down --forget` leaves it alone and clears the lock; then `flow up`.
- `flow down [--yes] [--force | --forget]`: stop the supervisor, leaving every worker and task running. It finds the supervisor in the pane its lock records, as `flow up` does, and closes that pane, or the whole workspace when the pane is its only one, saying which; when the pane holds no agent, it does so only while herdr lists the lock's workspace as `flow-supervisor`, and otherwise notes that the pane is already gone. When another session clearly runs in that pane, the supervisor is gone, and it leaves that pane to its owner and says so. When flow cannot tell the agent there from the supervisor, it refuses and changes nothing, and the user says which it is. `--force` takes it to be the supervisor: once herdr lists the lock's pane in the lock's workspace it closes that pane alone, never a workspace by id, and herdr closes the workspace with its last pane; otherwise it refuses. `--forget` takes it to be someone else's: it closes nothing and offers no server stop. The two exclude each other. It then marks the lock stopped, keeping the supervisor's Claude Code session, and clears the watch lock. It stops the watcher, and the herdr waits and ssh connections it started, only when it runs on this machine, has touched the lock in the last 10 minutes, and is a `flow watch` process; otherwise it removes the lock alone and says why. A watcher on another host ends at its next cycle. When the supervisor ran in a herdr session that is neither the configured one nor herdr's `default`, and that session has no agents and no workspaces left, it offers to stop that session's server; `--yes` stops it without asking, and with no terminal it is left running with the command to stop it. It ends by saying how many open tasks keep running. The next `flow up` starts the supervisor on the host it runs on and resumes that session, so the conversation carries on. A supervisor already stopped has nothing to stop. When the supervisor's host cannot be reached it fails and changes nothing. A flow from before the stopped mark, still installed on another host, reads a stopped lock as a supervisor that died, so its `flow up` starts one there resuming the same session.
- `flow status [--all] [--json]`: open tasks, their holds, pending dispatch requests from workers (`req-id  from-task  host  title`), and observed sessions; `--all` includes done and orphaned tasks.
- `flow inventory [--refresh] [--json]`: worktrees, agents, and PRs per host; `--refresh` first discovers hosts from Ona (or the static list), probes each running one, and lists open PRs. `--json` gives each host its ssh alias, whether it is the host flow runs on (`local`), its Ona phase, when it was last seen, and whether it is reachable or gone.
- `flow inventory --sync [--yes] [--refresh] [--json]`: bring herdr's saved SSH machines on this machine in step with the hosts, in place of showing them. Every other host that is not gone gets a machine labelled with its name, for its ssh alias and the herdr session flow runs in; a gone host's machine is removed when it still has that alias, and a host's machines for another alias or session are removed, before its new one is added when it has none current, since herdr names each machine once; when that add fails, flow says the host is left without a machine. Machines whose label is not a host's name are never touched. A host that did not answer the last refresh, or that Ona reports stopped, is not added, since herdr must reach it to add it: it is listed on stderr, and its machine stays as it is. With nothing to change it says so, noting any host it did not add. It prints the plan and asks before applying it; `--yes` applies it without asking, and with no terminal and no `--yes` it exits 1 having changed nothing. `--refresh` refreshes first, and `--json` prints the plan as JSON, applied only with `--yes`. A failing step stops the rest and says how many were applied.
- `flow run <host> -- <argv>...`: run a command on a host, passing its output and exit code through.
- `flow sessions [--summaries] [--json]`: herdr agents across hosts, with their tasks, as the supervisor, or as observed sessions. An agent that may be the supervisor, one flow cannot confirm, shows `supervisor?` in the TASK column and `"maybe_supervisor": true` in the JSON, and is neither the supervisor nor observed. `--summaries` adds each session's latest summary, and `--json` includes summaries and pane ids.
- `flow follow <host> <session>`: wake the supervisor when a session started by hand goes idle or done, not only when it turns blocked. `<session>` is the session's title as `flow sessions` shows it, or its pane id when several share the title, or `<herdr session>/<pane id>` when several herdr sessions hold that pane id; `flow sessions` then marks it `followed`.
- `flow unfollow <host> <session>`: stop waking the supervisor for a followed session except when it turns blocked; also accepts the pane id, or `<herdr session>/<pane id>`, of a followed session that is gone.

Sessions started by hand sit on a ladder:

- **observed**: a herdr agent flow can see but does not manage: any agent in a listed herdr session whose pane is neither a task's nor the supervisor's. It wakes the supervisor only when it turns `blocked`.
- **followed**: an observed session on the follow list. It also wakes the supervisor on `idle` and `done`.
- **adopted**: a session started by hand that the supervisor took over as a task with `flow task adopt`. It is a worker in every way flow can manage, and `flow sessions` shows its task id.

herdr reuses pane ids after its server restarts, so a pane alone does not say whose agent it holds. An agent in a task's pane is the task's while the task is open, unless herdr reports a Claude Code session for it other than the task's; once the task is done or orphaned, only while herdr reports the task's own session there. An agent in the supervisor's pane is the supervisor when herdr reports the supervisor's session for it, or, while flow has not recorded that session or herdr reports none, when it runs in `/workspaces/flow` itself in the workspace `flow up` opened for it, still labelled `flow-supervisor`. One in or below that cwd and in that workspace that flow cannot confirm, because herdr reports another session, the label changed or it runs in a subdirectory, may be the supervisor: `flow sessions` marks it `supervisor?`, the watch leaves it alone, and `flow up` and `flow down` refuse beside it until the user runs `flow down --force` or `flow down --forget`. Any other agent is observed.

Supervisor:

- `flow dispatch (--title <title> (--brief <text> | --brief-file <file>) | --request <req-id> [--title <title>]) --criteria <text> [--host <name>] [--dir <path> | --branch <name> [--repo <name>]] [--parent <id>] [--allow-shared-dir]`: start a worker session for a new task. `--request` dispatches a pending request from `flow status`: it supplies the brief, and the title, host and parent unless given, and is consumed once the worker starts.
- `flow task adopt <host> <session> --title <title> --criteria <text> [--brief <text> | --brief-file <file>] [--parent <id>]`: take over a session started by hand as a task. `<session>` is resolved as `flow follow` resolves it. The session must be an observed one, not a task's or the supervisor's, and Claude Code with a working directory herdr knows. flow writes the task and its brief, drops the session from the follow list, and prompts it to read the brief, printing `adopted <id> on <host> (<pane>)`. Without `--brief`, the brief asks it to continue the work already under way. When the prompt cannot be delivered, for example because the session is blocked, the task is held `blocked` on the supervisor and the command exits 1; `flow steer` can deliver it later. Any other failure once the task exists also holds it `blocked` on the supervisor, naming the failure, and exits 1. The brief's commands set `FLOW_STATE`, since the session was not started by flow.
- `flow steer <id> <text>`: send a message to a task's worker and log it; a `needs-decision` or `blocked` task reads `working` again.
- `flow task show <id> [--json]`: a task's fields and the head of its brief, naming the brief's file when the head leaves lines out; `--json` carries the full brief.
- `flow task events <id> [--last <N>] [--json]`: a task's event log.
- `flow task decide <id> --by user|supervisor --answer <text>`: answer the task's hold and lift it.
- `flow task hold <id> --on user|supervisor --what <text>`: hold the task until someone answers.
- `flow task orphan <id> --reason <text>`: mark the task orphaned: its work will not finish.
- `flow task close <id> [--force]`: close the task's herdr workspace, keeping its record, working directory and branch; `--force` closes it while the task is still open (working, pending, blocked or needs-decision). herdr reuses workspace ids, so it closes the workspace only while herdr lists it under the task's label, which ends in the task id, with the task's pane in it; otherwise it refuses and closes nothing, naming what herdr lists. An adopted task's workspace belongs to whoever started the session, so it stays open: flow only stops treating the pane as the task's and prints `closed <id> (pane left to its owner)`.

An adopted task keeps running in the session that was started by hand, so flow cannot add hooks to it. It reports through its own `flow task` calls and herdr's view of it. In place of the Stop hook, the watcher logs a `turn.end` event on the task whenever herdr sees the session go from `working` to `idle`, so a session that stops without reporting wakes the supervisor as `quiet` once the quiet threshold passes, as a dispatched worker would. It does not wake on `idle` at once, since the user may still be typing in that session.

Worker:

- `flow task status <id> working|blocked|needs-decision [--message <text>]`: report the task's state; `--message` is required for `blocked` and `needs-decision`.
- `flow task done <id> --report <file>`: finish the task with a report.
- `flow task request-dispatch <id> --title <title> --brief <file> [--host <name>]`: ask the supervisor to dispatch sibling work.

Hooks and plumbing, run by Claude Code and the supervisor rather than by hand, and left out of `flow --help`:

- `flow guard`: Stop hook that blocks ending a turn while work is unwatched.
- `flow watch [--heartbeat <secs>] [--once]`: watch tasks and exit 2 with a reason when the supervisor should wake. Every observed session wakes the supervisor when it turns `blocked`, except one adopted as a task, which is watched as that task's worker; `flow follow` adds `idle` and `done` for that session. While only such unfollowed sessions are watched there is no hourly heartbeat: the watch lists the hosts again every hour, so a session started meanwhile is watched within the hour, and ends quietly after a day.
- `flow wake drain`: print the queued wakes not yet acknowledged.
- `flow wake ack --through <N>`: acknowledge every queued wake up to and including N.
- `flow hook claude --task <id>`: record a worker's Claude Code hook event read as JSON from stdin.
- `flow hook supervisor`: record the supervisor's Claude Code session from its SessionStart hook on stdin.

## Docs

- Design: [docs/superpowers/specs/2026-10-26-flow-design.md](docs/superpowers/specs/2026-10-26-flow-design.md)
- Implementation plan: [docs/superpowers/plans/2026-10-26-flow-implementation-plan.md](docs/superpowers/plans/2026-10-26-flow-implementation-plan.md)
