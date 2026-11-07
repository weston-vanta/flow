# Claude Code hooks as a Session status source

Research for `.scratch/flow-v2/issues/14-claude-code-hooks.md`, done 2026-11-07 against Claude
Code 2.1.319 on the Flow CDE. The user's `~/.claude/settings.json`, dotfiles and herdr setup were
read, not changed. Experiments ran a throwaway `claude` with its own `CLAUDE_CONFIG_DIR` and a
`--settings` file in a scratch directory.

Sources, cited inline as:

- **[hooks]** https://code.claude.com/docs/en/hooks (fetched as `hooks.md`, 2026-11-07).
- **[guide]** https://code.claude.com/docs/en/hooks-guide.
- **[env]** https://code.claude.com/docs/en/env-vars.
- **[statusline]** https://code.claude.com/docs/en/statusline.
- **[herdr-hook]** herdr's installed Claude integration, `~/.claude/hooks/herdr-agent-state.sh`
  (`HERDR_INTEGRATION_VERSION=11`), and `herdr integration status`.
- **[local]** files on the Flow CDE: `~/.claude` symlinks, `~/dotfiles`, `/etc/claude-code`.
- **[exp]** experiments, described in [How the experiments ran](#how-the-experiments-ran).
- **[t01]** / **[t03]**: the answers to tickets 01 (herdr visibility) and 03 (transcripts).

## Summary

- Hooks report most status changes the moment they happen, and some that herdr cannot see:
  a permission prompt as soon as it opens, the subagents and background tasks still running at
  a turn's end, an API error ending a turn. They also carry the identity flow needs: session id,
  cwd and transcript path in the input; `HERDR_PANE_ID`, `ONA_ID` (the Ona environment id),
  hostname and `CLAUDE_PID` in the environment.
- Hooks cannot be the only source. No hook fires when the user presses `Esc` (no `Stop`, and no
  `idle_prompt` afterwards), when claude is SIGKILLed or crashes, or (by inference) when a CDE
  stops. A record written by hooks says "running" forever in those cases. Something must check
  liveness (`CLAUDE_PID` on that CDE, or a herdr sweep) and catch interrupts (the transcript
  writes `[Request interrupted by user]`).
- A hook that only appends to a file costs about 3 ms locally and 9 ms on `/efs-home`. One that
  starts `jq` or `python3` costs 15–30 ms. Hooks are synchronous by default, so hooking
  `PreToolUse`/`PostToolUse` adds that cost twice per tool call. `async: true` removes the wait.
- Installing: the only per-user place for hooks is `~/.claude/settings.json`, which on each CDE
  resolves into that CDE's own `~/dotfiles` checkout. Hook entries must be committed to dotfiles
  (or ship as a plugin that dotfiles' `enabledPlugins` turns on). Managed settings are
  Ona-mounted and org-owned. Primary's `/efs-home` is a separate filesystem [t01], so hooks there
  cannot write into a store the other CDEs share.

## Event coverage per Session status

Statuses are the map's: running, waiting for you, idle, done.

| Status | Hook signal | Timing | Misses |
| :- | :- | :- | :- |
| running | `UserPromptSubmit` | immediate [exp] | also fires on turns Claude starts itself: `/loop` and cron firings, a background subagent reporting back, cross-session messages [hooks] |
| running (progress) | `PreToolUse` / `PostToolUse` (`agent_id` set inside subagents) | immediate | cost per tool call; see [Latency](#latency) |
| waiting for you | `PermissionRequest` | immediate (PreToolUse +42 ms) [exp] | none for permission dialogs. Cleared by `PostToolUse` after approval [exp] |
| waiting for you | `Notification` `permission_prompt` | 6.5 s after the prompt [exp]; docs: ~6 s, each keystroke defers it [hooks] | redundant with `PermissionRequest` |
| waiting for you | `Notification` `elicitation_dialog`, `elicitation_url_dialog` | ~6 s, deferred by typing [hooks] | not tested (no MCP elicitation available) |
| waiting for you | `PreToolUse` with `tool_name: AskUserQuestion` [hooks] | immediate | not tested |
| waiting for you | none | — | a question asked in prose at the end of a turn looks like any `Stop`; `last_assistant_message` holds the text. Same gap as herdr [t01] |
| idle | `Stop` | immediate [exp] | **not on `Esc` interrupt** [hooks][exp]. Input has `last_assistant_message`, `background_tasks`, `session_crons` (empty when nothing is in flight) [hooks][exp] |
| idle | `StopFailure` (`error`: `rate_limit`, `authentication_failed`, …) | immediate | fires instead of `Stop` on API errors [hooks]; observed with a logged-out CLI [exp] |
| idle | `Notification` `idle_prompt` | exactly 60.0 s after `Stop` [exp] | **not after an `Esc` interrupt** (none within 75 s) [exp]; not while a background agent runs [hooks] |
| done | `SessionEnd` `reason: prompt_input_exit` | `/exit`, Ctrl+C twice, Ctrl+D twice [exp] | |
| done | `SessionEnd` `reason: other` | pane or terminal killed (tmux `kill-server`, i.e. SIGHUP) [exp]; also old session when `--fork-session` was launched [exp] | |
| done | none | — | **SIGKILL, crash, OOM** (no `SessionEnd`) [exp]; **CDE stopped** (not tested; a stopped VM runs nothing) |
| done (logout) | `SessionEnd` `reason: logout` | [hooks] | not tested |

`SessionEnd` hooks share a 1.5 s budget by default; a per-hook `timeout` or
`CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` raises it up to 60 s [hooks].

## Session identity across lifecycle events

All tested with the scratch setup [exp]; `source`/`reason` values match [hooks].

| What the user does | Events | Session id |
| :- | :- | :- |
| `claude` | `SessionStart source=startup` | new |
| `claude --resume <id>` | `SessionStart source=resume` | **same id** as before |
| `/resume <id>` inside a session | `SessionEnd reason=resume` (old id), then `SessionStart source=resume` (target id) | switches to target |
| `--resume <id> --fork-session` | `SessionStart source=fork` | new |
| `/clear` | `SessionEnd reason=clear` (old id), then `SessionStart source=clear` | **new id**, same pane |
| `/compact` or auto-compaction | `PreCompact`, `SessionStart source=compact`, `PostCompact`; no `SessionEnd` | same |
| subagent (Agent tool) | `SubagentStart` / `SubagentStop` with `agent_id`, `agent_type`; tool events inside carry `agent_id` | parent's |

Compaction also fired a `SubagentStop` with no matching `SubagentStart` (its internal summarizer)
[exp]; the docs say internal agents such as prompt suggestions and `/btw` do this too [hooks]. A
background subagent's report back to the parent fires the parent's `UserPromptSubmit` [hooks][exp].
So a store keyed by session id must treat `/clear` and `/resume` as "this pane now holds a
different session", and must ignore `SubagentStop` events whose `agent_id` it never saw start.

## Facts captured at start

| Fact | Where a hook gets it | Caveats |
| :- | :- | :- |
| session id | input `session_id`; env `CLAUDE_CODE_SESSION_ID` (matches input, updated on `/clear`) [env][exp] | |
| transcript | input `transcript_path` | |
| cwd | input `cwd`; `CwdChanged` gives `old_cwd`/`new_cwd` when a `cd` in the main conversation changes it [hooks] | |
| branch | not in any hook input. Run `git -C "$cwd" branch --show-current` | changes mid-session with no hook; re-read on `Stop` or `PostToolUse` for Bash. The status line JSON has `workspace.repo.*`, `worktree.branch` and `pr.number`/`pr.url` [statusline] |
| title | `SessionStart` `session_title`, only for a custom title (`--name`, `/rename`) [hooks] | the AI-generated title is not in any hook input; the status line's `session_name` has it [statusline] |
| CDE | env `ONA_ID` (= the Ona environment id; Flow is `01a1b357-…` in `gitpod environment list`), `IS_ON_ONA=true`, `HOSTNAME` (`ip-10-1-110-90`), `/etc/machine-id` [exp] | there is no `GITPOD_ENVIRONMENT_ID` in this environment [exp] |
| herdr pane | env `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`, `HERDR_SOCKET_PATH` [exp] | inherited, so wrong for nested sessions (below); absent outside herdr |
| process | env `CLAUDE_PID` (the claude process) [env][exp] | lets a sweeper on the same CDE check `kill -0` |
| model, effort | `SessionStart` `model` (optional), `effort.level` on tool and Stop events [hooks] | |

### Nested sessions inherit the pane

A `claude` started from inside another Claude session (a worker running `claude -p`, or `tmux`
started from a Bash tool inside a herdr pane) inherits the outer `HERDR_PANE_ID`. Its hooks
report the outer pane [exp]: every scratch session in these experiments reported `w1:p1`, the
pane of the session that launched them. Claude sets `CLAUDE_CODE_CHILD_SESSION=1`,
`CLAUDECODE=1`, `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PID` in every subprocess it spawns,
including hooks [env], so the variable in the hook's own env tells nothing. What does: the
claude process's own environment. The top-level claude's `/proc/<pid>/environ` had none of
these; a nested one's had `CLAUDE_CODE_CHILD_SESSION=1` and the outer session's
`CLAUDE_CODE_SESSION_ID` [exp]. A hook can read `/proc/$CLAUDE_PID/environ` and skip pane
attribution when it finds them. Since v2.1.178 Claude ignores an inherited marker under tmux
[env], so the `/proc` check needs the same exception if flow ever runs Claude in tmux.

herdr's hook does not make this check [herdr-hook]. After the first scratch `claude -p` (which
loaded the user's settings and so ran herdr's hook with the outer pane id), `herdr api snapshot`
still showed the outer session id on `w1:p1`. Whether herdr ignored the report or overwrote
and later corrected it was not checked.

## Installing on every CDE

Hook locations are user settings, project settings, local project settings, managed policy, an
enabled plugin's `hooks/hooks.json`, and skill or subagent frontmatter [hooks]. Entries merge
across levels; the same handler defined in two files runs once, but a plugin's copy stays
separate [hooks]. Edits to settings files are normally picked up by a running session's file
watcher [guide].

On these CDEs [local]:

- `~/.claude` → `/efs-home/.claude` (shared by Flow, Oncall, Secondary; Primary has its own
  `/efs-home` [t01]). Inside it, `settings.json` → `/home/vscode/dotfiles/agents/claude/settings.json`,
  a path local to each CDE. `~/dotfiles/bootstrap.sh` creates that link from a clone of
  `weston-vanta/dotfiles`. So the hook *registration* is per CDE and reaches each CDE through
  dotfiles, while a hook *script* placed under `~/.claude/hooks/` is shared at once by the
  three CDEs on one `/efs-home`.
- `/etc/claude-code/managed-settings.json` and `managed-settings.d/*.json` are symlinks into
  `/usr/local/gitpod/secrets/mnt/…`: org-provided (Vanta auto-mode rules, announcements). They
  set no `allowManagedHooksOnly`, so user hooks run, but flow cannot install hooks there.
- Plugins: install state lives in `~/.claude/plugins/` (shared on `/efs-home`), but turning a
  plugin on is `enabledPlugins` in `settings.json`, so a plugin still needs one dotfiles line.
  It does give a versioned bundle (`hooks/hooks.json` plus scripts).
- `claude --settings <file>` adds hooks for one launch [exp]; it covers sessions flow starts
  but not the ones the user starts by hand.
- herdr's integration writes its hook into user settings and its script to
  `~/.claude/hooks/herdr-agent-state.sh`, which says "managed by herdr; reinstalling or updating
  the integration overwrites this file. add custom hooks beside this file instead of editing
  it" [herdr-hook]. The script handles only `SessionStart` (not in subagents) and sends
  `pane.report_agent_session` (pane id, session id, transcript path, `source`) to the herdr
  socket with a 0.5 s timeout. All other herdr Claude state is screen-detected [t01].
- This CDE's dotfiles has an uncommitted change adding a second herdr `SessionStart` entry
  (`bash '/home/vscode/.claude/hooks/herdr-agent-state.sh' session`, matcher
  `^(startup|resume|clear|compact|fork)$`) beside the committed one (`bash
  "$HOME/.claude/hooks/herdr-agent-state.sh" session`, matcher `*`). The command strings differ,
  so by the dedupe rule above both run on every SessionStart. Not confirmed by observation.

The practical route is: commit flow's hook entries to `agents/claude/settings.json` in dotfiles,
keep the script on `/efs-home/.claude/hooks/` (or in a plugin), and pull dotfiles on each CDE.

## Latency

Hooks block Claude until they finish unless `async: true` [hooks]. Default timeouts: 600 s for
command hooks, 30 s on `UserPromptSubmit`, 1.5 s shared budget on `SessionEnd` [hooks].
Interactive `SessionStart` hooks at launch, `--resume` and `/clear` run in the background; only
the first response waits for them [hooks]. Async hooks are killed when a `claude -p` run tears
down [hooks].

Hook process cost, measured by running each command 30 times through `bash -c` with a hook
payload on stdin (Flow CDE) [exp]:

| Hook command | Median | p90 |
| :- | -: | -: |
| `true` | 1.5 ms | 1.6 ms |
| `cat >> local-file` | 2.7 ms | 2.8 ms |
| `cat >> /efs-home/…` | 8.7 ms | 9.2 ms |
| bash + `jq` (the experiment logger) | 15.3 ms | 15.4 ms |
| `python3` parsing the JSON | 18.0 ms | 18.6 ms |
| herdr's hook (exits after parsing) | 28.9 ms | 30.1 ms |

End to end, `claude -p` with one Bash tool call against the fake API (6 hook firings: start,
prompt, pre/post tool, stop, end; 8 runs each) took 575 ms minimum with no hooks, 585 ms with a
`cat` append, 660 ms with the jq logger and 678 ms with python. Medians were noise-dominated
(0.59–1.0 s) [exp].

So a turn costs (per-event cost) × (2 + 2 × tool calls) when tool events are hooked: a 30-tool
turn with a 15 ms hook adds about 0.9 s. Hooking only lifecycle events (`SessionStart`,
`UserPromptSubmit`, `PermissionRequest`, `Notification`, `Stop`, `StopFailure`, `SessionEnd`)
costs a few hooks per turn, and `async: true` hides even that.

On storage: `/efs-home` is NFS-mounted EFS. `open(2)` warns that `O_APPEND` "may lead to
corrupted files on NFS filesystems if more than one process appends data to a file at once"
(https://man7.org/linux/man-pages/man2/open.2.html). A shared append-only log written by hooks on
several CDEs is unsafe; one file per session (written then renamed) is not.

## Compared with herdr's screen-detected states

| Situation | herdr [t01] | Hooks |
| :- | :- | :- |
| turn running | `working` | `UserPromptSubmit` |
| permission prompt | `blocked`, only for prompt shapes herdr knows | `PermissionRequest`, immediate, for every prompt |
| prose question | idle/done (cannot tell) | `Stop` (cannot tell) |
| turn finished | `done` until viewed, then `idle` | `Stop` / `StopFailure`; no "viewed" notion |
| `Esc` interrupt | `idle` from the screen | **nothing** |
| background subagents still running | not shown | `Stop.background_tasks` |
| claude exits cleanly | agent disappears from pane | `SessionEnd` with reason |
| pane closed | `pane.closed`, all metadata lost | `SessionEnd reason=other` |
| claude killed / crashed | agent disappears | **nothing** |
| CDE stopped | host unreachable | **nothing** (inferred) |
| `/clear` | new session id via herdr's hook | `SessionEnd` + `SessionStart`, new id |
| cost | ~170 ms sweep, polled over ssh | 3–30 ms per hook, pushed, no ssh |

## How the experiments ran

- A logger hook (`bash` + `jq`) appended each event's stdin JSON, selected env vars, hostname,
  `/etc/machine-id` and a timestamp to a JSONL file. It was registered for 14 events in a JSON
  file passed with `--settings`.
- The nested `claude` had no credentials (the outer claude has `CLAUDE_CODE_OAUTH_TOKEN` in its
  own environment but does not pass it to its subprocesses), so `claude -p` first produced `StopFailure` with a login error.
  For the rest, `ANTHROPIC_BASE_URL` pointed at a ~150-line local fake Messages API that streams
  canned replies: plain text, 40 s of slow text (to interrupt with `Esc`), a `Bash` tool_use (to
  raise a permission prompt), an `Agent` tool_use, or a compaction summary.
  `CLAUDE_CONFIG_DIR` pointed at a scratch directory, so the user's config, history and
  settings were untouched.
- Interactive sessions ran in a private tmux server (`tmux -L hookexp`) inside herdr pane
  `w1:p1`, driven with `send-keys`. tmux had focus events off, so Claude could not tell
  whether the terminal had focus.

## Not verified

- Behaviour against the real API and real model (all turn-level tests used the fake API; the
  hook machinery is the same binary).
- CDE stop, OOM kill and host reboot (only SIGKILL and SIGHUP were tested).
- What fires when the user answers a permission prompt with "No", or interrupts during one.
- `elicitation_*` notifications, `AskUserQuestion`, `logout`, `StopFailure` for rate limits.
- Whether `idle_prompt`/`permission_prompt` are suppressed while the herdr pane has focus. The
  docs say these fire "only when you appear to be away from the terminal" and describe that by
  keystrokes [hooks]; tmux focus events were off here.
- Whether the duplicated herdr `SessionStart` entry runs twice, and what herdr does with a
  nested session's report for the outer pane.
- Hooks on Primary, Oncall or Secondary (all tests ran on Flow).
