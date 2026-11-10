# Launching a Session from another Session

Research question: what mechanisms let one Session (a "Manager") start another Session (an
"IC") on some CDE, in its own herdr pane, with a brief as its first prompt, so that the IC then
runs `flow wt create` itself and owns its PRs (ADR 0002)? Facts and options only, no design
decision. Done 2026-11-10 on the Flow CDE with herdr 0.9.5 (private protocol 22) and Claude Code
2.1.322.

Sources, cited inline as:

- **[herdr-help]** the installed binary: `herdr --help`, `herdr workspace|tab|pane|agent|session`
  (no arguments prints each group's usage).
- **[herdr-skill]** `herdr --skill`, the agent skill file the binary prints.
- **[herdr-docs]** herdr's CLI reference at the v0.9.5 tag,
  https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/cli-reference.mdx
- **[herdr-src:`path`]** herdr source at tag v0.9.5, `https://github.com/herdrdev/herdr/blob/v0.9.5/<path>`.
- **[herdr-schema]** `herdr api schema --json`, the socket protocol's JSON Schema.
- **[cc-help]** `claude --help` (2.1.322).
- **[cc-cli]** https://code.claude.com/docs/en/cli-reference
- **[cc-perm]** https://code.claude.com/docs/en/permissions, section "Project allow rules and
  workspace trust".
- **[cc-sec]** https://code.claude.com/docs/en/security, section "Additional safeguards".
- **[cc-modes]** https://code.claude.com/docs/en/permission-modes
- **[ona-cli]** https://ona.com/docs/ona/integrations/cli
- **[trial]** a local probe run for this note, in a throwaway herdr session `flowresearch`
  (server started, then stopped and deleted), with `FLOW_STORE` pointed at a scratch directory so
  the `flow hook` writes stayed out of the real Store. Transcripts it created were deleted.
- **[v1]** flow v1 on `origin/main` (`a83a5bb`): `src/herdr.rs`, `src/harness/claude.rs`,
  `src/harness/handshake.rs`, `src/cmd/dispatch.rs`, commit messages, and the live run record
  `docs/verification/2026-10-e2e.md` (**[e2e]**; findings cited "F1", re-run findings "R-1").
- **[wt]** `/home/vscode/dotfiles/bin/wt` (the `wt` script; line numbers given), `wt guide`.
- **[flow]** this repo: `src/hook.rs`, `src/session.rs`, `src/services.rs`.
- **[prior:x]** earlier research branches: `research/herdr-visibility`
  (`docs/research/herdr-visibility.md`), `research/v1-lessons`, `research/claude-code-hooks`.

## Summary

- **Making the pane.** `herdr workspace create`, `herdr tab create` and `herdr pane split` each
  take `--cwd`, repeatable `--env KEY=VALUE` and `--no-focus`, and return the new root pane's id
  in JSON [herdr-help; herdr-skill]. They need a running herdr server. If a launcher starts one,
  every pane inherits the launcher's environment [e2e F6; trial].
- **Starting claude in it.** There are two ways. `herdr pane run <pane> "<command line>"` types
  the line and Enter into the shell and returns at once; success means only that herdr queued the
  text [herdr-docs]. `herdr agent start <name> --kind claude --pane <pane> -- <claude args>` quotes
  each argument for the shell, types `claude …`, and returns only when herdr sees Claude ready for
  input. If Claude is blocked at startup, for example on the trust dialog, it returns
  `agent_not_ready` at once [herdr-docs; herdr-src:`src/app/agents.rs`; trial]. `agent start`
  rejects any argument containing a control character, so a multi-line brief cannot be an argument
  [herdr-src; trial].
- **The brief.** `claude "<prompt>"` starts an interactive session with that first prompt
  [cc-cli]. In the trial, a 21 KB one-line argument worked. Redirecting a multi-line file into
  stdin of an interactive `claude` (`claude < brief.md`) also worked: the file became the first
  prompt, and the session stayed interactive and accepted typed prompts afterwards. Stdin
  redirection is not documented for interactive mode [trial]. Other routes: a short prompt that
  names a file to read (what v1 did), or `--append-system-prompt-file` [cc-cli; v1].
- **The id can be chosen up front.** `claude --session-id <uuid>` sets the conversation id [cc-cli].
  In the trial, that exact id came back from herdr's `agent start` result (`agent_session.value`),
  from `flow session get <id>` and in the transcript file name. A reused id fails with
  `Error: Session ID … is already in use.`, and a malformed one with
  `Error: Invalid session ID. Must be a valid UUID.` (both exit 1) [trial].
- **Confirming the start.** `agent start` succeeding means Claude was idle and ready [herdr-docs].
  The flow Store gets the Session from the IC's own `SessionStart` hook (status, cwd, pane, herdr
  socket, CDE id) within about a second, then its Activity on the first `UserPromptSubmit`/`Stop`
  [flow `src/hook.rs`; trial]. Neither proves the IC has run `flow wt create`. Only a Branch
  record naming the IC as reporter shows that.
- **The main failure is a trust dialog.** It appears in any directory that has not been trusted.
  Trust is keyed on a git repository's root, and for a worktree on the main checkout's root
  [cc-perm]. So an IC started in a trusted main checkout (or one of its worktrees) gets no
  dialog. One started in `$HOME`, or in a directory herdr could not `cd` into (herdr then falls
  back to `$HOME`), always does [cc-sec; e2e F3]. Every v1 dispatch failed live on this and on
  timing (11 of 11 starts in the first run) [e2e F1].

## 1. herdr: making a pane

### Commands

| Command | Creates | Returns (JSON) |
| --- | --- | --- |
| `herdr workspace create [--cwd PATH] [--label TEXT] [--env K=V]… [--focus\|--no-focus]` | a workspace with one tab and one pane | `.result.workspace`, `.result.tab`, `.result.root_pane` |
| `herdr tab create [--workspace ID] [--cwd PATH] [--label TEXT] [--env K=V]… [--focus\|--no-focus]` | a tab with one pane | `.result.tab`, `.result.root_pane` |
| `herdr pane split [<pane>\|--pane ID\|--current] --direction right\|down [--ratio F] [--cwd PATH] [--env K=V]… [--focus\|--no-focus]` | a pane beside another | `.result.pane` |

[herdr-help; herdr-skill "Creation responses expose the IDs to use next"]

- Creation leaves focus unchanged by default. Without `--cwd`, the new terminal follows
  `terminal.new_cwd`, which by default follows the source pane or workspace. "Each `--env
  KEY=VALUE` adds or replaces that variable in the new root shell." herdr's own `HERDR_*`
  variables win over the caller's [herdr-docs].
- In the trial, `workspace create --cwd <dir> --label … --env FLOW_STORE=… --no-focus` returned
  `root_pane.pane_id: "w1:p1"` and `tab_id: "w1:t1"`. `tab create --workspace w1` returned
  `w1:p2` [trial].
- The pane's `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` are what `flow hook` records as the Session's
  `pane` and `herdrSocket` [flow `src/hook.rs`]. The trial Session's origin had
  `pane: "w1:p2"` and `herdrSocket: …/sessions/flowresearch/herdr.sock` [trial].
- IDs are scoped to one herdr server (session) [herdr-skill]. After a server restart, herdr reuses
  workspace, tab and pane ids [prior:v1-lessons §1].
- herdr also has `herdr worktree create` (a git worktree plus a workspace for it)
  [herdr-help]. It is not `flow wt create`, so it would record no Branch owner (ADR 0002).

### The server

- Every command above talks to a running server over its socket. Under `--machine`, herdr
  "never installs, starts, or restarts a server" [herdr-docs "Saved SSH machines"]. v1 started
  missing servers itself with `setsid -f sh -c '<scrub env>; exec herdr --session <s> server'`,
  every stream on `/dev/null` [v1 `src/herdr.rs` `start_detached_server`].
- A server keeps the environment of whoever started it, and every pane opened in it later
  inherits that environment. A stale `FLOW_STATE` in a server broke every hook in every pane in
  v1's live run [e2e F6]. The trial confirmed it: a server started from inside this Claude Code
  session, with only a few variables unset, passed `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_EFFORT`,
  `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_SESSION_ATTENDED` and others into the new pane's
  shell (read from `/proc/<shell pid>/environ`) [trial].
- A herdr session list per CDE is available: `herdr session list --json` [herdr-help;
  prior:herdr-visibility "Shape"].

## 2. herdr: starting `claude` in the pane

### `pane run`

`herdr pane run <pane_id> <command>`: "honors live bracketed-paste mode and submits text plus
Enter atomically"; "success acknowledges admission, not command execution" [herdr-docs;
herdr-skill]. In the CLI, `pane run` joins its remaining arguments with spaces, unquoted, and sends
them as `pane.send_input {text, keys: ["Enter"]}`. There is no `pane.run` socket method
[herdr-src:`src/cli/pane.rs` `pane_run`; herdr-schema]. So the caller must quote the command line
for the pane's shell itself. v1 quoted each word [v1 `src/herdr.rs` `pane_run`]. Anything the
shell can do is available, for example `claude … < brief.md` or `env K=V claude …`.

After `pane run`, herdr does not report the agent right away. In v1's live run Claude took
"about 1 to 20 s to appear" (the second CDE's shell started slowly). A `herdr agent wait` issued at
once failed with `agent_not_found`, and once exited 1 with no output at all [e2e F1, R-1].

### `agent start`

`herdr agent start <name> --kind claude --pane ID [--timeout MS] [-- <agent-args...>]`
[herdr-help]. From the docs and source:

- The target must be "an existing available shell pane": the shell owns the foreground, with no
  command, editor or agent running. Otherwise it fails with `agent_pane_busy`. A name must
  match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents (`agent_name_taken`)
  [herdr-docs; herdr-src:`src/app/agents.rs` `start_agent`].
- herdr builds `claude <args…>`, quotes each argument for the pane's shell (POSIX single quotes,
  or PowerShell quoting), and submits it through the same input path as `pane run`
  [herdr-src:`src/app/agents.rs`, `src/platform/mod.rs` `interactive_unix_shell_command`,
  `src/platform/linux.rs` `shell_quote`]. The result echoes the `argv` it ran [trial].
- Any argument containing a control character (a newline, for example) is rejected before
  anything is typed: `{"code":"invalid_agent_argument","message":"agent arguments cannot be
  encoded safely for the target shell"}` [herdr-src; trial]. Shell redirection is not possible
  either, since every argument is quoted.
- "A successful start returns only after the expected agent owns the same terminal and is ready
  for interactive input. If detection reports `blocked` during startup, the command returns
  `agent_not_ready` immediately." Default timeout is 30 s; it must be above 3 s and at most 300 s
  [herdr-docs]. A fixed 3 s settle delay applies (`AGENT_START_SETTLE_DELAY`)
  [herdr-src:`src/app/agents.rs`].
- On macOS only, `pane run` and `agent start` reject a command longer than the terminal's
  canonical line-input limit [herdr-docs; herdr-skill].

Trial results [trial]:

| Case | Result | Time |
| --- | --- | --- |
| Untrusted scratch dir, `-- --session-id <uuid> --model haiku "Reply with only the word ok."` | `agent_not_ready` ("blocked during startup"); pane showed the trust dialog with the cursor on `No, exit`; `agent get` had no `agent_session` and `launch_pending: true`; nothing written to the Store, no transcript | 3.4 s |
| Same, then `agent send-keys t1 esc` | Claude exited (declined); pane back at the shell prompt | |
| Trusted worktree of `/workspaces/flow`, same args plus `--name trial-t2` | Success: `agent_status: idle`, `interactive_ready: true`, `agent_session.value` = the chosen uuid. The first prompt had already been answered (`● ok`). The Store had the Session (origin at +0.4 s, Activity `{prompt, reply: "ok"}` at +1.6 s) | 3.5 s |
| Trusted dir, one 21,024-character single-line prompt argument | Success; answered | 7.6 s |
| Argument with an embedded newline | `invalid_agent_argument`, nothing typed | instant |

The trial does not show whether `agent start` returns before or after the first turn when that
turn is long. The docs only promise "ready for interactive input" [herdr-docs]. Its success
result already carried `agent_session`, which herdr's Claude integration reports from
`SessionStart` [prior:herdr-visibility "The Claude Code integration"]. That was observed, not
documented.

v1 first tried `agent start` with `claude --version` and hit a timeout, because that command
exits without ever becoming interactive. It then switched to `pane run` plus its own handshake
(commit `acfb266` and the one before it, whose message records the failed smoke).

### Sending input later

`herdr agent prompt <target> <text> [--wait]` submits text plus Enter to a running agent and
refuses (`agent_blocked`) when the agent sits at an approval or question dialog. With `--wait` it
returns `agent_prompt_stalled` if no `working`/`blocked` activity follows within 5 s
[herdr-docs; herdr-skill]. `herdr agent send-keys <target> <key>…` sends logical keys
(`enter`, `down`, `esc`, `ctrl+c`) [herdr-docs]. These are the only ways to answer a dialog, or
to deliver a brief after launch instead of in argv.

## 3. Another CDE

- **Plain ssh.** `ssh <env-id>.ona.environment 'herdr …'` works from the Flow CDE once
  `gitpod environment ssh-config` has written the aliases. herdr is on the non-interactive ssh
  `PATH` on every CDE [prior:herdr-visibility "Plain ssh"]. ssh joins its remote arguments into
  one string run by the remote shell, so a command line must be quoted twice. This is v1's
  `remote_command` [prior:v1-lessons §3]. For `pane run`, that becomes three levels (ssh string,
  herdr argument, pane shell). ssh's own failures exit 255, and `ConnectTimeout=10` makes an
  unreachable host fail within seconds [prior:v1-lessons §3].
- **`herdr --machine <profile>`.** This forwards `workspace`, `tab`, `pane` and `agent` commands
  (all but `attach` and local `explain --file`) "through the JSON API over non-interactive SSH;
  API payloads are not interpolated into the SSH shell command". It "never installs, starts, or
  restarts a server"; failures do not fall back to Local; "Requests are not automatically retried
  after connection failure" [herdr-docs "Saved SSH machines"]. "A connection failure does not
  prove a mutation was not applied: inspect remote state before retrying" [herdr-skill]. Each
  profile targets one remote herdr session [prior:herdr-visibility "`herdr --machine`"]. No
  profiles exist on this CDE; not tried.
- **Where the launcher can run.** This CDE has no `*.ona.environment` entries in `~/.ssh/config`
  [trial: `grep -c`]. In v1, only the machine with the user's `gitpod login` could list and reach
  other CDEs. Another CDE's `gitpod` is scoped to that environment and has no ssh aliases
  [prior:v1-lessons §2; e2e R5]. So a Manager on a CDE other than the Flow CDE cannot reach
  other CDEs without that setup.
- **The brief file across CDEs.** Flow, Oncall and Secondary share `/efs-home`; Primary's
  `/efs-home` is a different filesystem [prior:herdr-visibility "Side finding"]. A brief written
  under `/efs-home` is therefore not readable on every CDE. It can also be sent over ssh stdin
  (`ssh host 'cat > <path>' < brief`).
- **`ONA_ID`.** It is unset in ssh shells [prior:v1-lessons §2]. `flow hook` records the
  Session's CDE from `ONA_ID` [flow `src/hook.rs`]. *Inference:* a herdr server started over ssh
  would pass that missing variable to every pane, and the IC's Session would have no `cdeId`.
  Panes of a server started from a normal terminal have it.
- **A stopped CDE.** Ona lists stopped environments with a non-running phase. Start one with
  `ona environment start <id-or-name>` [ona-cli]; the binary is named `gitpod` on these CDEs
  [prior:v1-lessons §2]. Until it runs, ssh fails (exit 255 within `ConnectTimeout`). Even after
  it starts, a herdr server may not be running there, and `--machine` will not start one
  [herdr-docs].

## 4. Claude Code: the launch command

### Starting interactively with a first prompt

- `claude "query"`: "Start interactive session with initial prompt" [cc-cli]. The prompt is the
  positional `[prompt]` argument [cc-help].
- With `-p`, Claude prints and exits. "The workspace trust dialog is skipped when Claude is run in
  non-interactive mode (via -p, or when stdout is not a TTY…)" [cc-help]. An IC that should
  stay in its pane as a conversation therefore cannot use `-p`.
- Stdin into an interactive session (not documented in [cc-cli], which shows piping only with
  `-p`). In the trial, `echo '…' | claude --model haiku` and
  `claude --model haiku < brief.md` (a three-line file with quotes, an apostrophe and `$HOME`)
  each showed the whole input as the first prompt, verbatim. Claude answered it and stayed
  interactive. A later `herdr agent prompt` was answered too [trial].

### Flags relevant to an IC

| Flag | What it does | Source |
| --- | --- | --- |
| `--session-id <uuid>` | "Use a specific session ID for the conversation (must be a valid UUID)" | [cc-cli; cc-help] |
| `--name`, `-n <name>` | Display name, shown in `/resume` and the terminal title; resumable by name. In the trial it became herdr's `terminal_title_stripped` and flow's `context.title` | [cc-cli; trial] |
| `--model <model>` | Model alias or full name | [cc-help] |
| `--effort <level>` | low … max | [cc-help] |
| `--permission-mode <mode>` | `acceptEdits`, `auto`, `bypassPermissions`, `manual` (= `default`), `dontAsk`, `plan`; overrides `defaultMode` from settings | [cc-cli; cc-help] |
| `--dangerously-skip-permissions` | Same as `--permission-mode bypassPermissions` | [cc-cli] |
| `--append-system-prompt <text>` / `--append-system-prompt-file <path>` | Append to the default system prompt; the file form exists "for text too long" for argv; both can be combined (v2.1.283+), file content first. All system prompt flags "work in both interactive and non-interactive modes" | [cc-cli "System prompt flags"] |
| `--system-prompt[-file]` | Replace the default prompt entirely (drops tool guidance and safety instructions) | [cc-cli] |
| `--settings <file-or-json>` | Extra settings that override the same keys from settings files for this session; a regular file ≤ 2 MiB | [cc-cli] |
| `--add-dir <dirs…>` | Extra directories tools may access | [cc-help] |
| `-w`, `--worktree [name]` | Claude's own worktree at `<repo>/.claude/worktrees/<name>`; not `flow wt create`, so no Branch owner | [cc-cli] |
| `--bg` | Starts a background agent managed by Claude's own supervisor, not in a herdr pane; checks workspace trust first and in a script exits with `Workspace not trusted` | [cc-cli; cc-perm] |

The working directory is the shell's cwd. There is no `--cwd` flag on `claude` itself
[cc-help]; `claude agents --cwd` only filters a listing [cc-cli]. The pane's `--cwd` decides it.

### Dialogs that can block the start

- **Workspace trust.** "Claude Code shows the trust dialog in interactive sessions only." In a
  repository, trust is keyed on the git repository root and covers the whole repository; "In a
  worktree, it uses the main checkout's root". Outside a repository, it is keyed on the start
  directory and covers its subdirectories. In the home directory, it is held "for the current
  session only" and is never written to disk, so the dialog appears on every launch [cc-perm;
  cc-sec]. Trust can be set by hand: `projects["<path>"].hasTrustDialogAccepted = true` in
  `~/.claude.json` [cc-perm]. On this CDE `/workspaces/flow`, `/workspaces/obsidian`,
  `/home/vscode/dotfiles`, `/workspaces/fprm-knowledge-base` and three old v1 task dirs are
  trusted [trial: read of `~/.claude.json`]. In the trial, a git worktree of `/workspaces/flow`
  got no dialog, which matches the main-checkout rule [trial]. The dialog's cursor starts on
  `No, exit`, and Esc declines and exits [trial]. v1 saw the same in 2.1.309 [v1 `acfb266`].
  `--dangerously-skip-permissions` is not documented to skip it.
- **bypassPermissions warning.** "The first time you start an interactive session with this mode
  enabled, Claude Code shows a warning dialog". Accepting sets `skipDangerousModePermissionPrompt:
  true` in `~/.claude/settings.json`; declining exits [cc-modes]. That key is absent from this
  CDE's `~/.claude/settings.json` [trial: grep].
- **Auto mode.** The default mode here: the trial sessions showed "auto mode on" [trial].
  Auto mode has no confirmation dialog, only a one-time notice [cc-modes].
- **Permission prompts during work.** In Manual mode most edits and commands ask; even
  `bypassPermissions` asks for explicit ask rules, `AskUserQuestion`, and critical-path `rm`
  [cc-modes]. Such a prompt is a `PermissionRequest`, which flow records as waiting
  [flow `src/hook.rs`].

## 5. Learning the IC's Session id and confirming it started

| Method | When it is known | Notes |
| --- | --- | --- |
| Choose it: `claude --session-id $(uuidgen)` | before launch | Collisions fail loudly (`already in use`, exit 1). Whether the in-use check spans other project directories was not settled: the cross-directory probe failed on login first [trial] |
| `herdr agent start` result / `herdr agent get <pane>` → `agent_session.value` | once herdr's Claude integration hook has run at `SessionStart` | Absent while blocked on the trust dialog (`SessionStart` had not run) [trial]. The field can be null if the integration is missing [prior:v1-lessons §1] |
| flow Store: `flow session list`, matching `origin.pane` + `origin.herdrSocket` + `origin.cdeId` | after the IC's `SessionStart` hook | Written by the IC itself [flow `src/hook.rs`]. Pane ids repeat across herdr sessions and after server restarts, so all three are needed [prior:v1-lessons §1] |
| Transcript path `~/.claude/projects/<cwd slug>/<id>.jsonl` | after the first write | The file name is the id [trial; prior:herdr-visibility] |

What each signal proves:

- `agent start` success: Claude runs and is idle or ready, past any trust dialog [herdr-docs].
- Session in the Store with status idle: `SessionStart` ran [flow `src/hook.rs`].
- Activity `prompt` equal to the brief: the brief reached Claude as a turn (`UserPromptSubmit`)
  [flow `src/hook.rs`; trial].
- A Branch whose reporter is the IC's id: the IC ran `flow wt create` successfully
  [flow `src/services.rs` `wt_create`; ADR 0002]. Nothing earlier proves this.

## 6. Failure modes

| Failure | What is seen | Source |
| --- | --- | --- |
| Pane created, `claude` typed, but no agent yet | `agent wait`/`get` → `agent_not_found` for 1–20 s; sometimes exit 1 with no output | [e2e F1, R-1] |
| Trust dialog | herdr `blocked`; `agent start` → `agent_not_ready`; no `SessionStart`, so no Store record and no `agent_session` | [trial; herdr-docs] |
| Other startup dialog (bypass warning, unknown prompt) | `blocked` only if herdr recognises the prompt shape; otherwise `idle`/`unknown` | [cc-modes; prior:herdr-visibility "How Claude Code status is decided"] |
| Pane shell not at its prompt (slow rc file, previous command) | `agent start` → `agent_pane_busy`; `pane run` text is queued into whatever is in the foreground | [herdr-src:`src/app/agents.rs`; herdr-docs] |
| `--cwd` path does not exist | herdr opened the pane in `$HOME` (v1: a bad worktree path); Claude then shows the trust dialog for `$HOME` on every launch | [e2e F3, Step 3; cc-sec] |
| Brief with newlines as an `agent start` argument | `invalid_agent_argument` | [herdr-src; trial] |
| Brief as a `pane run` command line | Caller must shell-quote it (and again for ssh). Long text is typed into the shell and stays in scrollback (the 21 KB trial line took about 4 s longer to start than a short one) | [herdr-src:`src/cli/pane.rs`; trial] |
| Argument size | Linux limits one argv string to 128 KiB (`MAX_ARG_STRLEN`, kernel constant; not tested here). 21 KB worked [trial] | |
| `--session-id` reused or malformed | exit 1 with the messages above; the pane returns to the shell | [trial] |
| Stale or leaked environment in the herdr server | Hooks misreport or fail in every pane; `CLAUDE_*` leak confirmed | [e2e F6; trial] |
| Stopped CDE or no ssh alias | ssh exit 255; `--machine` fails without retry; mutation state unknown after a connection failure | [prior:v1-lessons §3; herdr-docs; herdr-skill] |
| No herdr server on the target | `server_not_running`; `--machine` will not start one | [prior:v1-lessons §1; herdr-docs] |
| IC not inside a git repo | `wt: error: not inside a git repository: <dir>`, exit 1; flow records nothing | [trial; wt l.421; flow `src/services.rs`] |
| Repo has no wt base | `no base at <repo>.wt/.base; run wt base-init first`. On this CDE neither `/workspaces/flow.wt` nor `/workspaces/obsidian.wt` exists | [wt l.683–685, 959–961; trial: `ls`] |
| Worktree name already exists | `worktree '<name>' already exists` | [wt l.660–661] |
| No passwordless sudo / non-Linux / state dir on NFS | `passwordless sudo is required for overlay mounts`; or a filesystem error naming `wt.stateDir` | [wt l.451–462, 672–676] |
| `origin` not a GitHub URL | `wt create` succeeds but flow prints an `{error, help}` saying the Session was not recorded as owner | [flow `src/services.rs`, `src/bin/flow.rs` `wt_create`] |
| Leftover processes after the IC finishes | A repo tool's process held the worktree busy and blocked `wt remove` | [e2e R-3] |

## 7. What v1 did, and why it failed live

v1's `flow dispatch` [v1 `src/cmd/dispatch.rs`, `src/harness/claude.rs`,
`src/harness/handshake.rs`]:

1. ran `wt create` itself (the dispatcher, not the worker, created the worktree), and took the
   last output line as the path;
2. `herdr tab create --cwd <path>` in the `flow` workspace of a `flow` herdr session, starting
   that server if needed;
3. wrote a per-task `--settings` file whose hooks called `flow hook claude --task <id>`;
4. typed `env FLOW_STATE=<dir> claude --settings <file> --name <task id> "Read <task
   dir>/brief.md and follow it exactly."` with `pane run`;
5. polled `herdr agent wait --until blocked|idle|working`. When the pane was `blocked`, it read
   the screen and, if it showed a trust dialog, typed `down`… `enter` to reach `Yes`.

What went wrong in the live run [e2e]:

- F1: every start (11 of 11) failed, because `agent wait` ran before herdr saw the agent. Tasks
  were left blocked and trust dialogs unanswered. The fix was to retry until a deadline. The
  re-run then passed 5 of 6, and one failed on an empty herdr error (R-1).
- F3: the `wt` status note was taken as part of the path, so herdr fell back to `$HOME` and the
  worker sat on a `$HOME` trust dialog.
- F6: a leaked `FLOW_STATE` in the herdr server disabled every hook.
- F10: remote `herdr agent wait` calls with 1 h timeouts piled up on worker hosts over
  multiplexed ssh. Killing the local ssh client does not end the remote command.
- Keys sent the moment the trust dialog renders are dropped; v1 waited for `blocked` first
  [v1 `acfb266`].

## 8. Options and trade-offs

**Launch primitive**

| Option | For | Against |
| --- | --- | --- |
| `herdr agent start … -- <args>` | herdr quotes the args; one call blocks until ready; fails fast with `agent_not_ready` on a dialog; returns `agent_session` | No control characters in args, so no multi-line brief in argv and no stdin redirect; needs the pane's shell idle at its prompt; 3 s minimum |
| `herdr pane run "<line>"` | Full shell: `< brief.md`, `env K=V`, anything | Caller quotes (twice over ssh); returns before anything runs, so the caller must poll and treat `agent_not_found` as "not yet" (v1's F1) |
| `herdr --machine` vs plain ssh for remote | `--machine` sends JSON with no shell quoting | One saved profile per remote herdr session; never starts a server; no retry |

**Delivering the brief**

| Option | For | Against |
| --- | --- | --- |
| Whole brief as the prompt argument | It is literally the first prompt; Activity shows it | Must be one line for `agent start`; quoting for `pane run`/ssh; echoed into the shell scrollback |
| `claude < brief.md` (stdin) | Multi-line, no quoting; observed to work and stay interactive | Undocumented for interactive mode; needs `pane run`; the file must be readable on that CDE |
| Short prompt "Read <path> and follow it" (v1) | One short line works with either primitive | The first prompt is an indirection, and flow's Activity shows that line, not the brief; an extra tool call; the file must be on that CDE's filesystem |
| `--append-system-prompt-file <path>` plus a short prompt | Standing instructions survive in the system prompt; documented for interactive mode | Not the first prompt; the file must be on that CDE; system-prompt snapshot rules apply on resume [cc-cli] |
| `herdr agent prompt` after start | Separates "started" from "briefed"; herdr handles bracketed paste | Two steps; refused while blocked; `agent_prompt_stalled` possible |

**Knowing the id**: choosing `--session-id` up front lets the Manager know the IC's id before
any hook runs. Reading `agent_session` from herdr, or matching the Store's origin by pane, avoids
generating ids but is unavailable until `SessionStart` runs.

**Avoiding the trust dialog**: start the IC in a trusted main checkout (worktrees inherit trust)
and let it run `flow wt create` there; mark the directory trusted in `~/.claude.json`
beforehand (writes Claude's own state); or answer the dialog with keys after herdr reports
`blocked` (v1's approach, screen-scraping).

## Not verified

- Whether `agent start` returns before or after a long first turn completes (the trial turns
  finished within about a second).
- Stdin into interactive `claude` on other versions; it is undocumented and could change.
- Whether `--session-id`'s in-use check covers transcripts in other project directories. The
  probe from a scratch directory failed with `Not logged in · Please run /login` before reaching
  it; why `-p` there was not logged in was not investigated.
- Anything remote: no ssh aliases or `--machine` profiles exist on this CDE, so remote launches
  were not tried.
- `wt create` in a repo with a base (none exists on this CDE), and git's refusal to check out a
  branch already checked out elsewhere inside wt's overlay.
- How herdr classifies the bypassPermissions warning dialog (`blocked` or not).
