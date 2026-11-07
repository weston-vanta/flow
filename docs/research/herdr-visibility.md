# herdr visibility across CDEs

Research for `.scratch/flow-v2/issues/01-herdr-visibility.md`, done 2026-11-07 against herdr
0.9.5 (protocol 22) on all four running CDEs (Flow, Secondary, Oncall, Primary). Every probe was
read-only: no herdr config, saved machine, server or pane was changed.

Sources, cited inline as:

- **[bin]** the installed binary: `herdr --help`, `herdr <group>`, `herdr --skill`,
  `herdr api schema --json` (the protocol's JSON Schema, 272 KB).
- **[docs:page]** herdr's docs at the v0.9.5 tag,
  `https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/<page>.mdx`
  (index: https://herdr.dev/llms.txt).
- **[probe]** commands run on the CDEs on 2026-11-07, with output quoted below.
- **[v1]** flow v1's `src/herdr.rs`, `src/runner.rs`, `src/observe.rs` in this repo.

## Summary

- One read, `herdr api snapshot` (socket method `session.snapshot`), returns every pane in a
  herdr session with its agent kind, agent status, Claude Code session id, cwd and terminal
  title. That is everything herdr knows that a Session needs.
- herdr's agent states are `idle`, `working`, `blocked`, `done`, `unknown`. For Claude Code they
  come from screen scraping; the Claude integration reports only the session id.
- Map: `working` → running; `blocked` → waiting for you; `idle` and `done` → idle (herdr's `done`
  is "finished a turn, not yet viewed", not "pane gone"); `unknown` with an agent present →
  idle, flagged unclassified; no agent → not a Session. Session done is the pane, or the claude
  process, disappearing, which herdr does not report with any detail.
- From the Flow CDE, plain ssh to `<env-id>.ona.environment` running `herdr --session <s> api
  snapshot` works on every CDE today with no setup on the remote side. A full sweep (3 remote
  CDEs, 2 running herdr sessions each, plus local) takes about **170 ms** with warm ssh control
  connections in parallel, about **1.1 s** cold in parallel, and about 4 s for the very first
  connection to a CDE.
- `herdr --machine` needs one saved profile per remote *session* and cannot stream events;
  `herdr --remote` is interactive TUI attach only. Neither beats plain ssh for this.
- When a pane closes, herdr emits `pane_closed` with only `pane_id` and `workspace_id`; the pane's
  agent kind, Claude session id, title, cwd, status and screen are gone from every read. An
  observer must have cached them before the close.
- Side finding: Primary's `/efs-home` is a different filesystem from the Flow CDE's. Primary's
  Claude Code transcripts are **not** readable from the Flow CDE (see the end).

## What herdr exposes

### Shape

A herdr *server* is one named session (`default`, or `herdr --session <name>`), with its own
socket. Each server holds workspaces (`w1`), tabs (`w1:t1`) and panes (`w1:p1`). IDs are opaque,
scoped to one server, and closed tab and pane IDs are not reused [bin `--skill`]. A CDE can run
several servers: every remote CDE here runs `default` and `flow` (the latter left by flow v1)
[probe: `herdr session list --json`]. A full sweep therefore has to enumerate sessions per CDE.

### Reads

| Read | CLI | Socket method | Returns |
| --- | --- | --- | --- |
| Whole session | `herdr api snapshot` | `session.snapshot` | workspaces, tabs, layouts, `panes[]`, `agents[]`, focus |
| Agents only | `herdr agent list` / `get <target>` | `agent.list` / `agent.get` | panes that host a recognised agent |
| Panes | `herdr pane list` / `get` | `pane.list` / `pane.get` | every pane, agent or not |
| Process | `herdr pane process-info --pane ID` | `pane.process_info` | foreground argv, pid, cwd, shell pid |
| Screen text | `herdr pane read` / `agent read` | `pane.read` / `agent.read` | visible or recent scrollback |
| Why a status | `herdr agent explain <target>` | `agent.explain` | manifest rule that matched, with evidence |

[bin `herdr api`, `herdr agent`, `herdr pane`; schema `request.oneOf`]

`PaneInfo` fields (schema `event.$defs.PaneInfo`): required `pane_id`, `terminal_id`,
`workspace_id`, `tab_id`, `focused`, `agent_status`, `revision`; optional `agent`,
`agent_session {source, agent, kind, value}`, `cwd`, `foreground_cwd`, `terminal_title`,
`terminal_title_stripped`, `title`, `display_agent`, `label`, `state_labels`, `tokens`, `scroll`,
`restore_error`. `agents[]` entries add `state_change_seq` and, once a turn has completed,
`completion_seq` [probe].

A real entry from Oncall [probe: `herdr agent list`, trimmed]:

```json
{"agent":"claude",
 "agent_session":{"agent":"claude","kind":"id","source":"herdr:claude",
                  "value":"fd0cca9f-d06a-43f5-946e-1f3837b79c63"},
 "agent_status":"idle","completion_seq":552,"state_change_seq":552,
 "cwd":"/workspaces/obsidian.wt/weston-vanta/ra-787-followup",
 "pane_id":"w1:pD","tab_id":"w1:tB","workspace_id":"w1","focused":true,
 "terminal_title":"✳ 2tfjx2zk","terminal_title_stripped":"2tfjx2zk"}
```

`agent_session.value` is the Claude Code session id (the same UUID as the transcript file name
`~/.claude/projects/<cwd-slug>/<id>.jsonl`; checked for this pane and for this research's own
pane, whose value equals its scratchpad's session id) [probe]. `terminal_title_stripped` is
Claude Code's conversation title. `cwd` is the pane's working directory.

### Events

`events.subscribe` keeps a socket connection open and pushes events [docs:socket-api "Event
subscriptions"]. Relevant subscription types [schema `request.$defs.Subscription`]:

- `pane.created`, `pane.updated`, `pane.closed`, `pane.exited`, `pane.agent_detected`: no filter
  needed.
- `pane.agent_status_changed`: **requires a `pane_id`**, so a subscriber must add one entry per
  pane and resubscribe as panes appear. If any referenced pane does not exist the whole request is
  rejected and the connection closed.

Payloads [schema `event.$defs.EventData`]: `pane_created` and `pane_updated` carry a full
`PaneInfo`; `pane_agent_status_changed` carries `pane_id`, `agent`, `agent_status`, `title`,
`state_labels`; `pane_agent_detected` carries `agent`, `final_status`, `released`;
`pane_closed` and `pane_exited` carry only `pane_id` and `workspace_id`.

History is not durable and does not replay; a slow subscriber gets `events_lost` and must
resubscribe and reconcile with `session.snapshot`. Snapshots and events share no sequence, so
events should be treated as invalidations that trigger a re-read [docs:socket-api].

I opened a subscription over ssh on Oncall (90 s) and locally (400 s). Both got
`subscription_started`. The Oncall one saw no events because no pane changed state in that
window. The local one received
`{"event":"pane.agent_status_changed","data":{"agent":"claude","agent_status":"idle","pane_id":"w1:p1","workspace_id":"w1"}}`
215 s in, when the parent pane finished its turn [probe]. So events are confirmed locally;
over ssh the stream is confirmed to connect but was not seen carrying an event.

### The Claude Code integration

Installed on every CDE as `~/.claude/hooks/herdr-agent-state.sh`, integration v11 [probe:
`herdr integration status`]. It runs only on `SessionStart` and calls
`pane.report_agent_session` with `agent_session_id` (the Claude session id) and
`agent_session_path` (the transcript path) [probe: script source]. It does **not** report state:
"Claude Code state comes from Herdr's screen manifest detection" [docs:integrations "Claude
Code"]. The session id is what lets herdr restart the pane with `claude --resume <id>` after a
server restart [docs:session-state "Native agent session restore"]. Subagents are skipped
(`agent_id` present → exit).

The transcript path is sent but not echoed back: `PaneInfo.agent_session` has only `source`,
`agent`, `kind`, `value` [schema]. The path follows from cwd and id.

### How Claude Code status is decided

herdr finds the agent process in the pane's foreground process group and matches rules in a
detection manifest against the live bottom of the screen and the OSC terminal title [docs:agents
"How status works"]. Manifests auto-update from herdr.dev [docs:agents "Detection manifests"].
`herdr agent explain` shows the rules; on this pane, for example, `osc_title_working` (title
starts with a braille or `◐◑◒◓` spinner) won at priority 1100, and blocked rules look for
"Do you want to proceed?", "esc to cancel" forms, MCP elicitation prompts and similar [probe].

Consequences: `blocked` is strict and only fires for prompt shapes herdr knows; an unknown prompt
falls back to `idle` (`default_known_agent_idle_fallback`) [docs:agents "Blocked state"].
Claude waiting on background subagents counts as `working` (rule
`background_agents_working`) [probe].

## Mapping onto Session statuses

herdr's `AgentStatus` enum is `idle | working | blocked | done | unknown` [schema]. Definitions:
`done` is "idle but not yet marked seen"; explicit `pane focus` / `agent focus` mark it seen,
reads do not, and each TUI client tracks its own Done badge [docs:cli-reference, bin
`--skill`]. `unknown` means an agent is present but unclassified, and also appears on plain
shell panes with no `agent` field [probe: Secondary `w1:p1`].

| herdr | Session status | Notes |
| --- | --- | --- |
| `working` | running | includes waiting for background subagents |
| `blocked` | waiting for you | permission or question UI recognised on screen |
| `done` | idle | turn finished, not yet viewed; could drive an "unread" marker |
| `idle` | idle | |
| `unknown` + `agent` set | idle, flagged unclassified | e.g. transcript viewer, model picker |
| no `agent` (plain shell) | not a Session | |
| pane absent from snapshot, or `agent` gone from a known pane | done | herdr gives no reason |

Two things the table does not cover:

- "Waiting for you" from herdr is only `blocked`. A Claude turn that ended with a question in
  prose is `idle`/`done` to herdr. Telling those apart needs the transcript.
- A claude process that exits while its pane stays open: the pane stays in the snapshot with no
  `agent`, and a `pane_agent_detected` event with `released: true` fires [schema]. That is a
  done Session in a live pane. An observer keyed on pane presence alone would call it idle.

## Reading other CDEs from the Flow CDE

### Plain ssh (works today)

`gitpod environment ssh-config` writes `Host <env-id>.ona.environment` entries whose
`ProxyCommand` is `gitpod environment ssh <env-id> --proxy-connect`, user
`gitpod_devcontainer` (which lands as `vscode`) [probe]. On this CDE it had not been run; I ran
it with `--global-config-path` and `--ona-config-dir` pointing at a scratch directory so
`~/.ssh/config` stayed untouched. It generated a fresh ed25519 key in that directory. ssh did
add the three CDEs' host keys to `~/.ssh/known_hosts`.

Every remote CDE has herdr 0.9.5 on the non-interactive ssh `PATH`
(`/home/linuxbrew/.linuxbrew/bin/herdr`), a running `default` server and a running `flow`
server, and no saved machines [probe]. The sweep, one ssh round trip per CDE:

```sh
ssh <env-id>.ona.environment 'for s in <running sessions from herdr session list --json>; do
  herdr --session "$s" api snapshot; done'
```

flow v1 did the same, wrapping each call in `zsh -lc` and multiplexing with
`ControlMaster=auto`, `ControlPersist=600`, `ConnectTimeout=10` [v1 `src/runner.rs`
`ssh_connection_options`, `remote_command`]. The `zsh -lc` wrapper was not needed here.

A remote `events.subscribe` stream also works over ssh by piping a small socket client to
`python3 -` on the remote host (python3 is present on every CDE) [probe].

### Sweep latency

Measured from the Flow CDE: `herdr session list` plus `api snapshot` of each running session
(2 per remote CDE, 6 remote snapshots) plus the local snapshot [probe:
`sweep.sh`, repeated runs]:

| ssh connection | order | per CDE | full sweep |
| --- | --- | --- | --- |
| first ever to a CDE | n/a | ~4.1 s | n/a |
| fresh connection (`ControlPath=none`) | sequential | 0.25–0.34 s (Primary 0.9–1.1 s) | 1.4–1.7 s |
| fresh connection | parallel | same | 1.07 s |
| warm control master | parallel | 61–72 ms (Primary ~160 ms) | **~170 ms** |
| warm control master | sequential | same | ~310 ms |

Local `herdr api snapshot` takes 4 ms. Primary is slower throughout because bare ssh to it
takes ~0.8 s (it is on a different network, `10.0.x` against `10.1.x` for the others) [probe].
The control socket path must stay under 108 bytes; a path under the session scratchpad failed
with `ControlPath too long` [probe].

### `herdr --machine`

Routes API commands (`agent`, `pane`, `api snapshot`, `status server`, …) to a saved SSH profile
"through the JSON API over non-interactive SSH", with no TUI needed [docs:cli-reference "Saved
SSH machines"]. Limits that matter here:

- A profile targets **one remote session** [docs:connecting-machines "Add a machine"]; covering
  `default` and `flow` on three CDEs means six profiles.
- The selector must be a saved profile, not a hostname, and `machine add` starts the remote
  server if it is not running and may offer to install or replace herdr [docs:cli-reference].
- `events.subscribe` is not among the forwarded commands; `agent wait` is.
- The prefix "targets one machine at a time; combined listings … are not included".
- Herdr manages its own shared ssh connection (`ControlPersist 600`) when
  `remote.manage_ssh_config=true` [docs:connecting-machines].

I did not add profiles, so its latency is not measured. Since it rides ssh too, it cannot be
faster than plain ssh, and it adds profile state to keep in sync with `gitpod environment list`.

### `herdr --remote`

Attaches the interactive TUI to a remote server over ssh [bin `--help`;
docs:persistence-remote]. Not usable for reading state from a program.

### What the TUI does with machines

A TUI with saved machines shows every connected machine's workspaces and agent states in one
sidebar and keeps cached state dimmed when a connection drops [docs:connecting-machines].
That combined view lives only in the TUI client; the CLI does not expose it.

## What is lost when a pane closes

- **Event:** `pane_closed {pane_id, workspace_id}` only [schema]. Unlike `workspace_closed`,
  which includes a final snapshot "when Herdr can still identify it" [docs:socket-api], a pane
  close carries no agent, session id, title, cwd or last status.
- **Reads:** the pane leaves `session.snapshot`, `pane.list`, `agent.list`. Its ID is never
  reused [bin `--skill`]. Any agent name is cleared.
- **Screen:** scrollback is gone. Pane screen history (`[experimental] pane_history`) is off by
  default and only replays after a server restart anyway [docs:session-state].
- **On disk:** `session.json` is rewritten without the pane. `session-snapshots/` keeps up to 48
  older layouts, with `agent_session` refs, at most one per 15 minutes, as private recovery files
  [docs:session-state; probe: `~/.config/herdr/session.json`]. Not a reliable record.
- **What survives:** the Claude Code transcript, keyed by the session id herdr reported. So the
  observer must record pane → Claude session id (and title, cwd) while the pane is alive; after
  the close, that record plus the transcript is all there is.

For contrast, a herdr server restart keeps the layout and resumes Claude panes with
`claude --resume <id>` (integration v6+), but ends every process [docs:session-state "What
survives"].

## Side finding: Primary's EFS is separate

The map's notes say `~/.claude` → `/efs-home/.claude` on every CDE "so every CDE's Claude Code
transcripts are readable from the Flow CDE". The symlink holds on every CDE, but [probe]:

- Primary's `/efs-home` reports 1.3T used; the Flow CDE's reports 2.0T.
- Primary's live session `9302d94e-…` has its transcript at
  `~/.claude/projects/-workspaces-obsidian/9302d94e-….jsonl` on Primary, and no such file exists
  on the Flow CDE. Oncall's sessions (`195ef9e3-…`, `fd0cca9f-…`) are present on the Flow CDE.
- `~/.claude/settings.json` is a per-CDE symlink into `~/dotfiles` and differs on each CDE.

So Flow, Oncall and Secondary share an EFS (Secondary sees this research session's own transcript, written on the Flow CDE); Primary, which
uses a different environment class, does not. Reading Primary's transcripts needs ssh too.

## Not verified

- An event actually arriving over an ssh-forwarded subscription (no state changes happened in
  the windows I watched).
- `herdr --machine` latency and behaviour (no profiles added, by instruction).
- How a `blocked` Claude pane looks in a snapshot (none was blocked during the probes); the
  mapping relies on the documented enum and manifest rules.
- Whether the TUI's focus marks a pane seen server-side. Oncall's focused pane, which had just
  finished a turn, reported `idle`, not `done`, which suggests it does.
