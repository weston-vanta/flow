# Lessons from flow v1 (before the v2 reset)

Research for `.scratch/flow-v2/issues/04-v1-lessons.md`. Sources: flow v1 at `a83a5bb` (main,
2026-11-04): source under `src/`, tests under `tests/`,
`docs/superpowers/specs/2026-10-26-flow-design.md` (the "spec"),
`docs/verification/2026-10-e2e.md` (the "e2e record": a live run against real CDEs, then a
re-run after fixes), and the 144 commits in `git log`. Commits are cited by short hash. The e2e
record's numbered findings are cited as "e2e F7", its re-run findings as "e2e R-1".

Everything below is what v1 observed with herdr 0.9.5 (private protocol 22), Claude Code
2.1.309 to 2.1.312 and the `gitpod` CLI of late October 2026. Newer versions may behave
differently. Tickets 01 (herdr visibility) and 03 (transcripts) should re-check anything v2
depends on.

## 1. herdr

### Keep

- **Identify a session by the Claude Code session id herdr reports, never by its pane id.**
  `agent list` and `agent wait` return `agent_session: {kind, value}` for each agent.
  `kind: "id"` gives the id directly. `kind: "path"` gives the transcript path, whose file stem
  is the id (`src/herdr.rs` `AgentSessionInfo::id`). v1 settled on this after a day of churn
  (`8da0321`, `2a95a8e`, module doc of `src/owner.rs`). The field can be null: herdr reports
  it only after its Claude integration hook has run (spec, "Verified constraints").
- **Key panes by (host, herdr session, pane id).** Pane ids are unique only within one herdr
  session. Alpha `w1:p1` appeared twice in one listing, once in session `flow` and once in
  `default` (e2e F8; the doc on `Agent.session` in `src/herdr.rs`).
- **herdr reuses workspace, tab and pane ids after a server restart.** It drops workspace
  metadata on restart but keeps labels (`83f66da`, `2b2f6fd`, `c3d88af`, `src/owner.rs`). A
  stored pane id therefore says nothing about who is in that pane later. v2's Session record
  outlives its pane, so it must be keyed by the Claude session id, with the pane treated as a
  current location that can change.
- **Which statuses actually appear.** herdr defines `idle`, `working`, `blocked`, `done` and
  `unknown`. At the end of every Claude turn it reported `idle`, and `done` was never seen
  (e2e Step 7 and R6). After `/exit` the agent simply disappears: calls answer
  `agent_not_running` or `agent_not_found`, never `done` (`acfb266`). v2's "done (pane gone)"
  status therefore has to come from the agent dropping out of listings, not from herdr's
  `done`.
- **Read a status this build does not know as `unknown`** instead of failing the whole listing
  (`fd6fdcc`; `AgentStatus::Unknown`).
- **Parse herdr errors from the JSON on stderr**, shaped `{"error":{"code","message"},"id"}`.
  The codes v1 handled are `timeout`, `agent_not_found`, `agent_not_running`,
  `workspace_not_found`, `tab_not_found`, `server_not_running` and `invalid_key` (constants in
  `src/herdr.rs`). Check the protocol with `herdr status server`, which prints `status:` and
  `private_protocol:` as text lines, and refuse anything below the protocol whose output you
  parse (`MIN_PROTOCOL = 22`).
- **`agent list` carries no tab or workspace labels.** Getting them takes a separate
  `tab list` or `workspace list` call (`ddba146`, `owner::label_where_needed`). v1 used
  `terminal_title_stripped` as the title, falling back to `terminal_title`. `cwd` and `agent`
  can be null (`AgentInfo` doc).
- **Listing must never start a herdr server.** Starting one is a side effect on the user's
  machine. The watcher and the observers call `Herdr::agents_if_running`, which returns `None`
  when no server runs for the session (`e03e7d7`, "never start herdr from the watcher").
- **The herdr sessions to list come from config; v1 never discovered them.** It listed its own
  session plus a configured `herdr_sessions` list, default `["default"]`
  (`Config::listed_sessions`). It found no herdr command that lists sessions. A process works
  out its own pane's session from `HERDR_SOCKET_PATH`: `<config>/herdr.sock` is `default` and
  `<config>/sessions/<name>/herdr.sock` is `<name>` (`src/herdr.rs` `session_of_socket`). Not
  verified: whether herdr has since added a command to list sessions.

### Pitfalls

- **A herdr server inherits the environment of whoever started it, and every pane opened later
  inherits it in turn.** A stale `FLOW_STATE` in the server's environment silently broke every
  hook in every pane (e2e F6). v1 then removed its own variables and every `CLAUDE*` variable
  before starting a server (`83f66da`, `414e821`, `src/vars.rs`). v2 only observes, so it
  should not start servers at all.
- **Blocking remote `herdr agent wait` calls leak processes when ssh is multiplexed.** Killing
  the local ssh client does not end the remote command. Waits with 1 h timeouts piled up on
  worker hosts, some on panes closed long before (e2e F10, still unfixed in the re-run R6). v2
  should prefer short, bounded `agent list` calls over long-lived remote waits, or wrap remote
  waits so they exit when stdin closes.
- **A wait's output can include a stray stderr line.** v1 takes the first line of the merged
  output that parses (`fd6fdcc`). herdr also sometimes exits 1 with no output at all (e2e R-1;
  `NO_ANSWER`). Treat that as "no answer", not as a known error.
- **herdr sees an agent in a new pane only seconds after it starts**, 1 to 20 s on a slow CDE.
  `agent_not_found` in that gap meant "not yet", not "gone" (e2e F1; `2587b35`, `LAUNCH_GRACE`
  in `src/watch/panes.rs`). For v2 this only means a newly opened pane can show no agent for a
  short while.
- **herdr may report a cwd through a symlink** (`/workspaces/flow` instead of its physical
  path), so comparing cwds for equality is unreliable (`0afc4f3`, `b7237f1`).

## 2. Ona and the `gitpod` CLI

### Keep

- **Listing.** `gitpod environment list -o json` prints a JSON array. v1 reads `id`,
  `metadata.name` (free text), `status.phase` (such as `ENVIRONMENT_PHASE_RUNNING`) and
  `status.content.contentLocationInMachine` (`src/ona.rs`). It kills the listing after 30 s.
- **Running, stopped and gone are different host states.** A stopped host is still listed,
  with a phase other than running. A gone host is no longer listed. Unreachable over ssh is a
  fourth, separate fact. Mixing them up made v1 give wrong advice (`6bd1048`, "an unreachable
  host is not a stopped one"; `2403ff1`). Only running hosts are probed
  (`HostInventory::probed`).
- **Detect a token scoped to one environment.** Inside a CDE, `gitpod` is scoped to that
  environment until the user runs `gitpod login`, and the listing then shows only the local
  environment. When only the local environment is listed but other hosts are cached, v1 treats
  it as a login problem: it keeps the cached hosts and warns "run gitpod login" rather than
  marking every other host gone (`ONLY_LOCAL_VISIBLE` in `src/inventory/discovery.rs`). It also
  keeps the cache, with a warning, when the listing fails.
- **Host names stay stable.** A name comes from a slug of the Ona name and can be overridden in
  config. A host keeps its name across later listings even when a newer environment would get
  the same slug; collisions get `-2`, `-3` (`150ada5`, `assign_names`).
- **The local host is identified by `$ONA_ID`** (`src/host.rs`). The variable is unset in ssh
  shells (e2e Step 6), so a command run over ssh cannot learn its own environment id from it.
- **Forget a gone host once nothing refers to it** (`133f44e`). v2's version of this question
  is how long done Sessions on a gone CDE are kept.

### Pitfalls

- **The login exists only on the machine where `gitpod login` ran.** It is stored in
  `~/.ona/configuration.yaml` on local disk, which a recycle wipes, and expires after 8 h.
  Whether its refresh token keeps it alive unattended was never verified (spec, "Verified
  constraints"; still open when v1 ended). Another CDE's `gitpod` runs as that environment's
  own principal: it sees only itself and has no ssh aliases for other CDEs. That is why v1's
  supervisor could only run on the operator's machine (e2e F11). For v2, the Flow CDE needs the
  user's login and `ssh-config`, and the dashboard needs a visible "login expired" state.

## 3. ssh to CDEs

### Keep (all in `src/runner.rs` unless noted)

- **Use native ssh.** The alias is `<env-id>.ona.environment`, installed by
  `gitpod environment ssh-config` with a ProxyCommand. Native ssh with multiplexing takes about
  0.5 s per call; the `gitpod environment ssh` wrapper takes 4 to 5 s (spec).
- **Options.** `BatchMode=yes`; `ConnectTimeout=10`, so a stopped host fails within seconds
  instead of hanging on the TCP connect; `ControlMaster=auto`; `ControlPersist=600`;
  `ControlPath=<dir>/ssh-%C`.
- **Keep control sockets on local disk**, in `$XDG_RUNTIME_DIR/flow` or else
  `/tmp/flow-<uid>`, with mode 0700. Unix sockets cannot be bound on NFS or EFS, and their
  paths are capped at 108 bytes (`1a5d7da`). Create the directory before anything uses it
  (`c406463`).
- **Run remote commands under `zsh -lc`.** It reads `.zshenv` and `.zprofile` but not
  `.zshrc`, so `~/.local/bin`, where herdr lives, must be exported from `.zshenv` (`95fdd48`).
  ssh joins its remote arguments into one string, so the command line must be quoted twice
  (`remote_command`).
- **ssh's own failures exit 255.** Tell them apart from the remote command failing
  (`SSH_FAILURE_STATUS`, `herdr::is_unreachable`) so an unreachable host is reported as such.
  A host that answers again clears its stale error (`be64623`, `40fe098`).
- **Ask every host in parallel, with a time limit per host**, and do not ask a host again while
  its previous call is still running (`f5aabcf`, `6ed91a1`, `Settings::list_timeout` in the
  watcher).
- **Close stdin, and do not pipe a stderr nobody reads**, for long-lived children; otherwise
  they block (`1a5d7da`, `Runner::spawn` doc).

## 4. Observing sessions and matching them to panes

### Keep

- **Reading Claude Code's on-disk state is enough, and it works across CDEs**, because
  `~/.claude` is a symlink to `/efs-home/.claude` on every CDE (spec). Honour
  `$CLAUDE_CONFIG_DIR` before `$HOME/.claude`.
  - Registry: `~/.claude/sessions/*.json`, with `sessionId`, `cwd`, `name`, `status` (such as
    `busy` or `idle`), `updatedAt` (Unix milliseconds) and `pidDomain`
    (`linux:<machine-id>:<pid-namespace>`). Files vanish as sessions end, so tolerate NotFound
    in the middle of a read, and skip files that do not parse (`observe::registry_sessions`).
  - Transcript: `~/.claude/projects/<slug>/<session-id>.jsonl`, where the slug is the cwd with
    every character that is not an ASCII letter or digit replaced by `-` (`project_slug`).
- **The matching order that finally worked:** herdr's reported session id first. Fall back to
  the cwd only when no other agent without an id, on any host, shares that cwd; otherwise
  leave the agent unmatched (`2bde346`, `observe::match_agents` doc). Record how each match was
  made (`MatchedBy::Task | Id | Cwd`) so a guess can be shown as a guess.
- **Never write Claude Code's state.** The spec's anti-goals allow reading it but rule out
  writing into state the harness owns.

### Pitfalls

- **The cwd fallback is wrong more often than it looks.** Every CDE shares the registry and
  has the same main-checkout path. As a result, every hand session in `/workspaces/<repo>`, on
  every host, matched the same newest registry session and showed the same summary (e2e F7,
  unchanged in R6). When herdr gives no id, show "unknown session" rather than a guess.
- **Where a process sits does not tell you who it is.** Most of 2026-10-29 (32 commits, from
  `ddba146` to `c3d88af`) went into telling the supervisor apart from workers and hand
  sessions by pane, workspace label, cwd and lock. Id reuse or symlinks broke each of those
  rules, until everything was rebuilt on the reported session id (`8da0321`). v2 has no
  supervisor and no workers, so it should not bring back the idea of an owner at all. A Session
  is simply a Claude session id seen in some pane.
- **Two counts from two sources disagree.** `flow status` counted sessions from the cached
  inventory and `flow sessions` from a live listing, and the supervisor kept remarking on the
  difference (e2e F9, unchanged in the re-run). Each view should have one source of truth.
- **A new pane should be reported once.** v1's watcher kept the panes it saw last on disk, so
  it woke once for each new observed session (`observed-new`, doc of `src/watch/mod.rs`). It
  also seeded that record so the first run after start would not replay history (`983fe57`).
  The general lesson: compare against a snapshot kept on disk, and treat the first listing as
  the baseline.

## 5. Summaries

- **Claude Code writes its own summaries.** Transcripts contain periodic system lines with
  `subtype: "away_summary"` and the text in `content` (spec; `observe::latest_summary`). v1
  showed the newest one, reading at most the last 1 MiB of the transcript. This is cheap and
  needs no LLM call.
- **Window boundary bug.** Starting the read exactly at the window's first byte dropped a line
  that began there, treating it as a partial line. The fix reads one byte before the window
  and discards everything up to the first newline (`13fb159`).
- **v1 never established when an `away_summary` is written**; the spec only says "periodic".
  Not verified: how often one is written, whether it appears only after a session goes idle or
  "away", and whether a session that never goes away has any at all. This bears directly on the
  map's open question about summaries, and ticket 03 should check it.

## 6. PR listing

### What v1 did

- For each configured repo, on each host, inside that host's checkout, it ran
  `gh pr list --author @me --state open --json number,url,headRefName,state --limit 100`
  (`PR_LIST_ARGV` and `open_prs` in `src/inventory.rs`). It then matched PRs to the host's
  worktrees by head branch (`join_prs`). A PR showed up only if its branch was checked out in a
  worktree.
- When `gh` failed, v1 kept the cached PRs and stored the error in `pr_error`, shown as a
  "gh failed:" line, so stale data never passed for fresh (`fca5f6f`). herdr, worktree-tool and
  probe errors follow the same pattern (`herdr_error`, `tool_error` and `probe_error` on
  `HostInventory`).

### Pitfalls and what to do differently (partly inference)

- **v1 linked PRs to worktrees, never to sessions.** Matching by branch also misses PRs whose
  worktree is gone, PRs by other authors, closed or merged PRs, and anything past 100. v2's
  "PRs a Session produced" needs a different link (ticket 07).
- *Inference:* GitHub is central and `~/.config/gh` is on EFS (spec), so `gh` can run once
  from the Flow CDE instead of over ssh on every host.
- `gh auth` is per machine unless `~/.config/gh` is shared; v1's `flow init` listed it as an
  optional manual step.

## 7. Lessons from the code, tests and history overall

### Keep

- **Route every remote operation through one `Runner` seam**, which runs locally or over ssh
  depending on the host (`src/runner.rs`). This made a scripted `FakeRunner` possible:
  `$FLOW_FAKE_RUNNER` names a JSON script whose entries match calls by host and argv words, and
  calls are logged to `<script>.calls.jsonl`. `$FLOW_FAKE_ONA` does the same for the Ona
  listing. Integration tests drive the real binary against recorded herdr JSON in
  `tests/fixtures/`. A live smoke test behind an environment flag (`examples/herdr_smoke.rs`,
  `FLOW_HERDR_LIVE=1`) caught herdr facts the fakes could not (`a578549`, `acfb266`).
- **Cache data per host together with the error that made it stale, and show its age**
  ("seen 9s"). An unreachable host keeps its cached data, marked unreachable, instead of
  failing the command (`52a6207`).
- **Keep tests away from real state.** A worker that ran `cargo test` had `FLOW_STATE` set, so
  the tests wrote to the real shared state (`0d28077`). Tests now remove every environment
  variable the binary reads, and `tests/state_isolation.rs` enforces this. Fixture ages are
  relative to the current time (`be64623`).
- **Run a live end-to-end test early.** The first live run found problems the planned unit and
  integration tests had missed: 11 of 11 starts failing, wrong summaries, colliding pane ids
  and leaked processes (e2e record). Plan one early for v2.

### Pitfalls

- **Schema churn in persisted state.** v1 needed a migration script nine days in (`2ceceda`,
  `868334a`). Keep persisted formats minimal and versioned.
- **Development sessions in the repo triggered the product's hooks.** The Stop hooks fired for
  every Claude Code session started in the repo (`6dc5cf7`, `49c7f58`). The reset removes those
  hooks. Do not add repo-level Claude hooks that act on sessions flow does not own.

## 8. v1 design choices the user probably judged failures (inference)

The user said v1 "did not achieve my product goals"; the repo does not record why. What follows
is inferred from the history and from what the v2 map chose instead. Confirm it with the user
rather than treating it as fact.

1. **A chat session as the interface.** v1's only view of the fleet was a Claude Code
   supervisor reporting in conversation, woken by Stop hooks. The watch, guard and wake
   machinery is the largest part of the code: `src/watch/mod.rs` alone is 3,243 of about
   19,450 lines, and 53 commits touch `src/watch` or the guard and wake commands. It kept being
   reworked: a heartbeat added and then removed (`c653cca`), a re-arm to outlive the hook
   timeout (`49fc288`), races in the guard (e2e F4), wakes lost at resume (e2e R-2). v2 replaces
   all of it with a dashboard and a CLI, which suggests the conversational supervisor was the
   main miss.
2. **Dispatch and supervision.** Workers were launched into panes with `pane run`, the trust
   dialog was answered with typed keystrokes, and a handshake waited for the agent. Every start
   failed in the first live run (e2e F1), and starts still failed occasionally afterwards
   (R-1). v2 drops dispatch entirely.
3. **Ownership and identity logic.** About 42 commits touch `src/owner.rs`,
   `src/supervisor.rs` or `flow up`, mostly about working out whose agent sits in a pane. None
   of it serves observation.
4. **Host inventory at the centre.** Worktrees, `wt`'s busy and dirty state, load averages and
   per-host PRs were collected so the supervisor could choose a host to dispatch to. v2 centres
   on Sessions and PRs; worktrees and load are not part of its destination.
5. **Features added and removed within days.** `flow inventory --sync` for herdr's saved
   machines was added in `e19e4a8` and dropped in `592961a`. `flow attach`, `flow down`,
   `follow` and `--heartbeat` also came and went, and the CLI was folded several times
   (`c0faee5`, `90aed7c`, `0bd9e5e`). The command surface outgrew the core concepts, which runs
   against the user's v2 rule "simple above all".

## Not verified

- herdr behaviour after 0.9.5 (protocol 22). That includes whether a command to list sessions
  now exists, and whether `agent_session` is always reported once the Claude integration hook
  is installed on Linux. The spec noted the hook was registered under a macOS path and did not
  run on Linux; by the e2e run herdr was reporting session ids.
- When, and how often, Claude Code writes `away_summary` lines.
- Whether the Ona login refreshes itself unattended.
- The user's actual reasons for judging v1 a failure (section 8 is inference).
