# Claude Code transcripts in the shared `~/.claude`

Research for flow v2, ticket `03-claude-code-transcripts`. Observed 2026-11-07 on Claude Code
2.1.317 to 2.1.319, against `/efs-home/.claude` (every CDE's `~/.claude` symlinks to it).
Transcripts are the user's private data: this note records structure only, with field names,
counts and sizes, never conversation content.

Sources are marked **[doc]** (official Claude Code docs, URL given) or **[obs]** (observed on
disk, read-only). Anything marked [obs] alone is undocumented internal behaviour: the docs say
"The entry format is internal to Claude Code and changes between versions, so scripts that
parse these files directly can break on any release" ([sessions][sessions]).

## 1. What `~/.claude` holds per conversation

A conversation is a **session**, keyed by a UUID session id.

| Path under `~/.claude/` | Per session? | What it is | Source |
| - | - | - | - |
| `projects/<project>/<session-id>.jsonl` | yes | The transcript: messages, tool calls, tool results, plus request records and metadata entries | [doc][claude-dir], [sessions] |
| `projects/<project>/<session-id>/subagents/agent-<id>.jsonl` + `.meta.json` | yes | Subagent transcripts, same format; the meta file holds `agentType`, `description`, `toolUseId`, `worktreePath`/`worktreeBranch` for worktree-isolated agents | [doc][claude-dir]; meta fields [obs] |
| `projects/<project>/<session-id>/tool-results/` | yes | Large tool outputs spilled out of the transcript | [doc][claude-dir] |
| `projects/<project>/memory/` | per project | Auto memory, not session data | [doc][claude-dir] |
| `sessions/<pid>.json` | yes, **live only** | One small file per running session, "used to detect concurrent sessions and crashes"; removed when the session exits | [doc][claude-dir]; fields [obs] |
| `session-env/<session-id>/` | yes | "Per-session environment metadata" (empty directories in every case observed) | [doc][claude-dir], [obs] |
| `file-history/<session-id>/` | yes | Pre-edit file snapshots for checkpoint restore | [doc][claude-dir] |
| `tasks/session-<id8>/` | yes | Task lists from the task tools | [doc][claude-dir], [obs] |
| `history.jsonl` | no (shared) | One line per typed prompt: `display`, `timestamp`, `project` (cwd), `sessionId` | [doc][claude-dir]; fields [obs] |

`<project>` is the working directory with every non-alphanumeric character replaced by `-`
(truncated to 200 chars plus a hash when longer) ([sessions]). Retention: transcripts and the
per-session directories are deleted after `cleanupPeriodDays`, **default 30 days**
([claude-dir]). Transcripts are plaintext and can contain anything a tool printed ([claude-dir]).

### `sessions/<pid>.json` (the live registry) [obs]

Observed fields: `pid`, `sessionId`, `cwd`, `startedAt`, `procStart`, `version`, `kind`
(`interactive`), `entrypoint` (`cli`), `name` + `nameSource` (`derived` or `user`), `status`
(`busy`, `idle`, `waiting`, `shell` seen) + `statusUpdatedAt`, `updatedAt`,
`messagingSocketPath`, and **`pidDomain`**, e.g. `linux:<machine-id>:pid:[<pid-ns-inode>]`.
The `<machine-id>` part equals `/etc/machine-id` of the CDE running the process (verified on
this CDE). This is the only place in `~/.claude` that ties a session to a machine.

`claude agents --json` reads this registry but filters to the current machine: run on this CDE
it listed only its own session, while `sessions/` held four more from another CDE [obs]. Its
documented fields are `cwd`, `kind`, `startedAt`, `pid`, `status` (`busy`/`waiting`/`idle`),
`waitingFor` (`permission prompt`, `input needed`, ...), `sessionId`, `name` ([agent-view]).

Caveat [obs, inferred]: the file is named by pid only, so two CDEs whose Claude processes share
a pid would write the same file in the shared directory. Unlikely, and harmless for a reader
that also checks `pidDomain` and `sessionId`, but it means `sessions/` is not a reliable
source on its own.

## 2. Mapping a conversation to CDE, pane, cwd, repo, branch

| Want | Where it comes from |
| - | - |
| session id | Transcript filename; `sessionId` on every record [obs]; hook input `session_id` [doc][hooks] |
| cwd | `cwd` on every message/attachment/system record, so the **latest** record gives the current cwd (one session showed 12 distinct cwds as it moved between worktrees) [obs]. Hook input `cwd` [doc][hooks]. SDK `SDKSessionInfo.cwd` [doc][sdk-ts] |
| branch | `gitBranch` on the same records; latest record wins (up to 10 branches in one session) [obs]. SDK `SDKSessionInfo.gitBranch` = "Git branch at the end of the session" [doc][sdk-ts] |
| repo | **Not recorded.** Derive from cwd (`git remote get-url origin` on that CDE), or from `prRepository` in `pr-link` records once a PR exists [obs] |
| CDE | **Not in the transcript** (no hostname or machine id in any record; the `environment` attachment holds only `platform`, `osVersion`, `shell`, `workingDirectory`, `isGitRepo`, `isWorktree`) [obs]. While live: `pidDomain` in `sessions/<pid>.json` gives the machine-id; map machine-id to CDE by reading `/etc/machine-id` once per CDE. After exit: nothing, unless flow recorded it while the session was alive. Sessions on two CDEs with the same cwd land in the same `projects/<project>/` directory |
| herdr pane | Not in `~/.claude`. `herdr agent list` returns, per pane, `pane_id`, `cwd`, `agent_status`, `terminal_title` and `agent_session.value` = the Claude session id [obs, herdr]; `herdr --machine <label> ...` reaches other CDEs (ticket 01). The Claude process also carries `HERDR_PANE_ID` and `CLAUDE_CODE_SESSION_ID` in its environment [obs] |

So the join key is the **session id**: herdr (per CDE) gives pane + session id; the transcript
gives cwd, branch, titles, PRs. CDE attribution must be captured while the pane is alive
(herdr per CDE, or `pidDomain`), because nothing durable records it.

An alternative that records CDE durably: a `SessionStart` hook (input includes `session_id`,
`transcript_path`, `cwd`, `source` = `startup`/`resume`/`clear`/`compact`/`fork`) [doc][hooks]
could write `hostname`/`HERDR_PANE_ID` somewhere flow reads. `~/.claude/settings.json` is a
symlink into dotfiles, so one hook would apply on every CDE [obs].

## 3. Record types and the signals in them

Counts across the 25 top-level transcripts present (70.9 MB total) [obs]:

| `type` | Share of bytes | Meaning / useful fields |
| - | - | - |
| `user` | 48.7% | User turns and tool results. `message.content` is a string for typed prompts, a list with `tool_result` blocks otherwise; `toolUseResult` carries structured tool output. `promptSource` (`typed`, `system`, `queued`, `sdk`), `origin.kind` (`human`, `task-notification`, `peer`), `turnOrigin` |
| `assistant` | 15.4% | Model output: `message.content[]` of `text`, `thinking`, `tool_use` (`name`, `input`, `id`) |
| `attachment` | 13.6% | Context injected into turns (`environment`, `queued_command`, `skill_listing`, ...) |
| `api-request`, `api-request-blob`, `api-request-shape` | 19.4% | Records of requests sent to the model (system prompt, tools, params) |
| `system` | 0.7% | `subtype`: `turn_duration` (end of a turn: `durationMs`, `messageCount`), `stop_hook_summary`, `away_summary`, `compact_boundary`, `local_command`, ... |
| `custom-title` / `agent-name` | small | Session name set with `-n`/`/rename` (`customTitle`, `agentName`) |
| `ai-title` | small | Generated title (`aiTitle`, 26-41 chars observed) |
| `last-prompt` | small | `lastPrompt` (the most recent user prompt text) + `leafUuid` |
| `pr-link` | small | `prNumber`, `prUrl`, `prRepository`, `timestamp` |
| `cost-state` | small | `totalCostUSD`, `totalLinesAdded/Removed`, `modelUsage` |
| `mode`, `permission-mode`, `queue-operation`, `file-history-*`, `tools-used`, `frame-link`, `atis-latch` | small | UI/runtime state |

Message-like records share `uuid`, `parentUuid`, `timestamp`, `sessionId`, `cwd`, `gitBranch`,
`version`, `entrypoint`, `isSidechain`.

### Goal / one-line summary

In order of preference:

1. **`customTitle`** if the user named the session. The docs' SDK `customTitle` is "the
   session's custom title when one is set ... Otherwise the AI-generated session title"
   [doc][sdk-ts]. On this setup flow v1 sets names to task ids (`2tfjx2zk`), so names are not
   always descriptive [obs].
2. **`aiTitle`**: "a short summary of your first prompt, written by a background request to
   the small/fast model"; replaced by a plan-based title when a plan is accepted; not generated
   for `claude -p` runs [doc][sessions]. Present in only 5 of 25 transcripts: sessions that
   were named did not get one [obs]. herdr's `terminal_title` showed the same kind of title for
   the live session [obs].
3. **First typed prompt** (`user` record with string content and `promptSource: "typed"`), or
   SDK `firstPrompt` [doc][sdk-ts]. Raw text: needs truncation or LLM summarising.

### Current activity

- **`system` / `away_summary`**: the session recap, "a one-line session recap" generated after
  3+ minutes away with at least three turns, capped at 400 characters (observed 211-295 chars),
  on by default (`awaySummaryEnabled`) [doc via [settings-ref] / interactive-mode, search
  summary]; present in 17 of 25 transcripts, often the last record [obs]. The best free
  "what is it doing / where is it" line, but stale while the session is busy.
- **`last-prompt.lastPrompt`**: what the user last asked [obs].
- **Latest `assistant` record**: last `text` block, or last `tool_use` name + short input
  (e.g. Bash `description`) for "running X" [obs].
- **Turn state**: a `system/turn_duration` record after the last `assistant` record means the
  turn ended [obs]. Authoritative live status is better taken from `sessions/<pid>.json`
  `status` / `claude agents --json` `status`+`waitingFor` [doc][agent-view], or herdr's
  `agent_status`.

### PRs created or pushed to

Claude Code writes **`pr-link`** records itself. The documented rules ([agent-view] "Pull
request status", [common-workflows]):

- linked when Claude creates a PR with `gh pr create` / `glab mr create`;
- linked when Claude edits, comments on, closes or readies a PR with `gh` and the command's
  output names the PR (`gh pr merge` usually does not);
- linked when Claude runs `gh pr checkout` or **pushes to a branch**: Claude Code looks the
  branch up with `gh pr view`, retrying after up to five later `git`/`gh`/`glab`/`curl`
  commands, so a PR opened after the push still links.

Observed: 19 distinct (session, PR) links in 11 transcripts. Every `gh pr create` whose output
named a PR produced a link within one record, and pushes to branches with open PRs produced
links with no PR URL in the tool output (one session linked a PR it never created, only pushed
to). Two PRs created in a single stacked-creation command were never linked, so `pr-link` can
miss PRs [obs]. `pr-link` records repeat (up to 54 times for one PR) and are interleaved with
later metadata, but not every linked PR is re-appended, so a reader must scan the whole file
once and keep the set [obs].

Fallback when `pr-link` misses: scan `assistant` `tool_use` blocks with `name: "Bash"` whose
`input.command` contains `gh pr create` (or `git push`) and regex
`https://github.com/<owner>/<repo>/pull/<n>` out of the matching `tool_result` in the next
`user` record (`tool_use_id` joins them) [obs].

## 4. Size and incremental reading

- 25 top-level transcripts: median 0.97 MB, p90 5.2 MB, max 24.2 MB; median 291 lines, max
  4,527. 88 `.jsonl` files including subagents; `projects/` totals 299 MB [obs].
- Largest single line 1.3 MB (a big tool result); typical max ~115 KB [obs].
- Transcripts are JSONL and appended to; a 900 KB prefix of a live transcript was unchanged
  while it grew [obs]. Exceptions are documented: a transcript can be set aside as
  `<session>.orphaned-*.jsonl` or `<session>.jsonl.superseded-*` instead of being overwritten
  [doc][claude-dir] (none present here). `/branch`/`--fork-session` create a new session id and
  file; two terminals resuming the same session interleave into one file [doc][sessions].
- The transcript "is written asynchronously and may lag the in-memory conversation" [doc][hooks].
- Cost: a full `json.loads` of the 24 MB transcript took 0.12 s (warm cache) [obs]. An
  incremental reader can keep `(path, byte offset, inode)` and parse only new complete lines;
  on shrink or inode change, rescan. Titles and `last-prompt` are re-appended about every 10-30
  records, always within the last 26 KB of every file observed, so a tail read is enough for
  name/title/last prompt; `pr-link` needs the full first scan (above).
- Scope for a scanner: `projects/*/*.jsonl` (skip `subagents/` unless wanted), filter by
  mtime; the SDK's `listSessions()` does the equivalent enumeration [doc][sdk-ts].

## 5. Official interfaces for session metadata

- **Transcript format: not documented, explicitly unstable** ([sessions]). Docs point scripts
  to `/export`, `claude -p --resume <id> --output-format json`, hooks / status line
  `transcript_path`, or the Agent SDK instead.
- **Hooks** common input: `session_id`, `transcript_path`, `cwd`, `permission_mode`,
  `hook_event_name`, `prompt_id`, `scratchpad_dir`, `effort`, `agent_id`/`agent_type`;
  events include `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `Stop`, `CwdChanged`,
  `PostToolUse` [doc][hooks].
- **Status line** commands get JSON on stdin including `session_id`, `session_name`, `transcript_path` and `cwd`
  [doc][statusline].
- **Agent SDK** (TypeScript and Python): `listSessions({dir, limit, includeWorktrees})`,
  `getSessionInfo(id)`, `getSessionMessages(id, {limit, offset, includeSystemMessages})`.
  `SDKSessionInfo` = `sessionId`, `summary` ("custom title, most recent prompt, auto-generated
  summary, or first prompt"), `lastModified`, `fileSize`, `customTitle`, `firstPrompt`,
  `gitBranch`, `cwd`, `tag`, `createdAt` [doc][sdk-ts]. These read `~/.claude/projects` (or
  `CLAUDE_CONFIG_DIR`) and would shield flow from format changes for title, cwd, branch; they
  do not expose PR links or machine. The docs describe session files as "local to the machine
  that created them" [doc][sdk-sessions]; here EFS makes them all visible from the Flow CDE.
- **`claude agents --json`**: live sessions on the current machine with status [doc][agent-view].
- **`claude --from-pr <n>`** and the `/resume` picker search by PR URL use the same PR links
  [doc][sessions].

## Not verified

- Whether `pr-link` records are written when the `gh` lookup fails (no `gh` auth on a CDE).
- Why two PRs created by one command were not linked (the docs' rules do not explain it).
- Whether a CDE's Claude Code clears another CDE's `sessions/` files as "crash leftovers" on
  launch (shared directory; `claude agents` filtering by `pidDomain` suggests it is machine-aware).
- Meaning of `status: "shell"` in `sessions/<pid>.json` (seen once; not in the documented
  `busy`/`waiting`/`idle` set).
- Concurrent appends to the shared `history.jsonl` from several CDEs over EFS.
- The `away_summary` documentation was read through search-result summaries of the settings
  and interactive-mode pages, not the pages themselves.

[sessions]: https://code.claude.com/docs/en/sessions
[claude-dir]: https://code.claude.com/docs/en/claude-directory
[hooks]: https://code.claude.com/docs/en/hooks
[agent-view]: https://code.claude.com/docs/en/agent-view
[common-workflows]: https://code.claude.com/docs/en/common-workflows
[sdk-ts]: https://code.claude.com/docs/en/agent-sdk/typescript
[sdk-sessions]: https://code.claude.com/docs/en/agent-sdk/sessions
[settings-ref]: https://code.claude.com/docs/en/settings-reference
[statusline]: https://code.claude.com/docs/en/statusline
