# flow: a supervisor for agent sessions across CDEs

Date: 2026-10-26. Status: approved design, pending implementation plan.

## Intent

The user works across several Ona cloud development environments (CDEs), each with its
own checkout of a large monorepo and its own git worktrees. Today that means hopping
between hosts and agent sessions, and no single session knows what is going on everywhere.

`flow` turns one agent session, started from whichever CDE is convenient, into the single
point of contact for all of that work. That session is the **supervisor**. It dispatches
tasks to **worker** sessions on the right CDE, follows them to completion, answers their
questions when it can and escalates when it should, reports outcomes, and also sees and
reports on sessions the user started by hand. It is harness-agnostic in principle, with a
short list of supported harnesses in practice.

The name `flow` is reused from an earlier, stalled project with similar product goals.
Nothing from that project's design is carried over.

## Goals

- One supervisor session, started anywhere, with a complete and current picture of every
  agent session across all CDEs: what it is doing, whether it is blocked, what it needs.
- Dispatch a task to a worker on a chosen CDE without the user opening that CDE.
- Follow workers without polling or spending tokens while nothing happens; the supervisor
  is woken when something is actionable.
- Keep working while the user is disconnected: the supervisor stays alive and keeps
  answering workers between the user's visits.
- Observe sessions the user started by hand so they appear in the same picture.
- Survive the supervisor session ending, the supervisor CDE being recycled, and any worker
  CDE being recycled. Durable truth lives on EFS, not in any session or host.
- Direct access when needed: the user can attach to any worker's terminal.
- Public code, private content: the repo can be public without leaking anything about the
  work it supervises.

## Non-goals (v1)

- Connected services (chat, documents, calendar, issue tracker). Designed around, not built.
- Codex workers. The harness adapter boundary exists so Codex can be added; only Claude
  Code is implemented.
- Sessions started outside herdr. The user will start every session in herdr, so observing
  them is herdr-only.
- Automatic credential refresh for the CDE control plane. The supervisor reports an
  expiring or expired login; renewing it is manual for now.
- Running workers anywhere other than a CDE reachable over Ona ssh.
- Merging PRs. Workers may open PRs; merging stays human.
- Replacing `wt`, `herdr`, or the monorepo's own skills. `flow` composes them.

## Anti-goals

- A supervisor that silently decides things that change what the user asked for.
- Hard-coded rules about kinds of tasks and their lifecycles (for example "a PR task is
  done after the first CI run"). What "done" means is stated per task in its brief.
- Depending on undocumented harness internals (inbox file formats, session registry
  filtering, launcher overrides). Documented hooks, CLI flags, and read-only consumption
  of on-disk state are acceptable; writing into harness-owned state is not.
- State that only exists in a process. If a fact matters after a restart, it is a file.
- Chatty cross-host polling over ssh as the steady state.
- Anything in the public repo that names the user's employer, projects, tickets, PRs,
  hosts, or environment ids.

## Verified constraints

Checked on 2026-10-23 and 2026-10-26 from one of the user's CDEs.

- `/efs-home` is one EFS filesystem shared by all of the user's running CDEs. A file
  written on one host is readable on the others. Cross-host visibility latency was not
  measured; it is assumed to be seconds at worst.
- `~/.claude` is a symlink to `/efs-home/.claude` on every CDE (created by the user's
  dotfiles `post-start-install.sh`). So Claude Code's session registry
  (`~/.claude/sessions/*.json`), transcripts
  (`~/.claude/projects/<cwd-slug>/<session-id>.jsonl`), and hooks directory are shared.
  Transcripts include periodic `away_summary` system entries that summarize what a session
  is doing. `~/.config/gh` is also on EFS, so GitHub auth survives a recycle.
- The registry filters peers to the local `pidDomain` (`linux:<machine-id>:<pid-namespace>`)
  and peer messaging uses Unix sockets in `/tmp`. Native cross-session messaging therefore
  does not work across hosts. Agent-team inboxes are files on EFS but are undocumented
  internals keyed to the lead session; they are not used.
- The `gitpod` CLI inside a CDE is environment-scoped until the user logs in. After
  `gitpod login` as the user, `gitpod environment list` shows all environments, and
  `gitpod environment ssh-config` installs an ssh alias `<env-id>.ona.environment` with a
  ProxyCommand. Native `ssh` to another CDE takes about 0.5 s with connection
  multiplexing; the `gitpod environment ssh` wrapper takes 4 to 5 s per call. The login
  token lives in `~/.ona/configuration.yaml` on the local host (lost on recycle), expires
  in 8 h, and carries a refresh token whose unattended behavior is unverified.
- `zsh -lc` is a login shell but not an interactive one: it reads `.zshenv` and
  `.zprofile`, not `.zshrc`, so `~/.local/bin` must be exported from `.zshenv` (the
  dotfiles do this). flow runs remote commands under `zsh -lc` for that reason.
- `herdr` 0.9.5 (protocol 22) is installed in `~/.local/bin` (local disk, lost on recycle)
  and running on the probe CDE. It reports native agent state (`idle`, `working`,
  `blocked`, `done`, `unknown`) per pane from terminal content, supports
  `agent wait --until <state>`, `agent prompt <text> --wait`, `agent read`,
  `agent start --kind claude|codex ...`, saved ssh machines (`herdr machine add
  <ssh-target>`, which prepares the remote server), and `herdr --machine <label> <api
  command>`. Its Claude integration hook lives at `~/.claude/hooks/herdr-agent-state.sh`
  (on EFS) and reports session identity to herdr, but the user's shared
  `~/.claude/settings.json` (a symlink into the dotfiles repo) registers it under a macOS
  path, so on Linux it never runs today.
- The user's shared Claude settings set `permissions.defaultMode: auto`; a managed
  settings file adds organization deny rules. Workers inherit both.
- Rust 1.96 and Go are in the CDE image. All CDEs are x86_64 with glibc 2.39.
- CDEs are recycled every few weeks by policy. Anything under `/workspaces` or `/home`
  that is not on EFS is lost then.
- Firstmate's wake-up design (Claude Code Stop hook with `asyncRewake: true`, a
  token-free watcher that exits 2 with a reason to wake the session, and a blocking
  turn-end guard) is the reference for the wake-up mechanism. Firstmate measured two
  gotchas: an exit 2 after the hook's timeout is not delivered, and async Stop hooks are
  not deduplicated across firings.

## Decisions

Each item records what was decided and, where the user did not decide it, marks it as the
designer's call.

### Shape

1. **Supervisor and workers.** One supervisor session is the user's point of contact.
   Workers do project work. Workers never address the user; their output reaches the user
   through the supervisor unless the user attaches to a worker directly. Intervention in a
   worker's terminal by the user is authoritative and the supervisor reconciles it.
2. **The supervisor runs detached and persistent.** It lives in a herdr pane on some CDE,
   so it keeps running and keeps answering workers while the user is disconnected. It and
   its workers run in the herdr session `flow up` ran in, beside the sessions the user starts
   by hand, so one herdr window shows them all. The user runs `flow up` from a shell pane in
   that herdr session on the CDE that should host the supervisor; it starts the supervisor or
   focuses the running one, and the user works with it in its herdr pane rather than owning
   the process in a terminal. The supervisor's lock names the host, herdr session and pane.
   At most one supervisor runs per state directory: `flow up` refuses while the lock records a
   running one anywhere else, until the user stops it with `flow down`.
3. **Session is the unit; tasks are its durable shadow.** A task record on EFS mirrors one
   worker session: brief, target CDE, worktree, harness, status events, decisions, report,
   and a `parent` pointer. Tasks are hierarchical through `parent`; a worker's in-session
   subagents are invisible to `flow` and need no record. The supervisor rolls the tree up.
4. **"Done" is defined per task, in the brief.** The brief states the completion criteria
   (for example "open a draft PR and stop", or "open a PR and keep it mergeable until it
   merges", or "write a report"). The worker reports `done` against those criteria. `flow`
   has one generic status vocabulary and no task-kind state machines; a task that watches
   a PR is just a task whose brief says so. Follow-up work is a new task, usually
   dispatched to the same natural home.
5. **Nested dispatch goes through the supervisor.** (Designer's call.) A worker that needs
   a sibling or child task writes a dispatch request into its task directory; the
   supervisor executes it. Placement logic and the ssh login live in one place and the
   supervisor always knows the whole tree.
6. **CDEs are not interchangeable.** Each has its own checkout at a different commit and
   its own worktrees, so most tasks have a natural home (the CDE whose worktree already
   holds the branch or PR). The supervisor keeps an inventory per CDE (worktrees, branches,
   open PRs, running sessions, load) and picks a home from it, asking the user only when
   no home is obvious and the choice matters.
7. **Placement never collides with live work.** The supervisor refuses to dispatch into a
   worktree that has a live session unless the user says so, surfaces CDE load in the
   inventory when it matters, and imposes no cap on workers per CDE.
8. **Observing hand sessions is herdr-only and in scope for v1.** Every session the user
   starts by hand runs in a herdr pane. The supervisor lists herdr agents on every known
   CDE, in every herdr session flow lists, matches them to task records where one exists and
   to the supervisor's own pane, and shows the rest as observed sessions with their herdr
   state and a summary drawn from the registry and transcript. herdr reuses pane and
   workspace ids after a server restart, so flow tells the supervisor and each worker by the
   Claude Code session herdr reports for its agent, and acts on a workspace only once herdr
   shows it is flow's: a task's by a label ending in the task id with the task's worker or no
   agent in its pane. An agent herdr reports no session for is the worker of the task whose
   recorded target is its pane while the pane's workspace label names the task (any label for a
   task adopted from a session that reported none); under another label any listing retires the
   task's target as reused, logging `target.retired`, which wakes the supervisor as
   `agent-gone` while the task is open. In the supervisor's pane it is the supervisor under the
   `flow-supervisor` label flow wrote, and observed under any other.
   An observed session wakes the supervisor only when it turns `blocked`; a followed one, an observed session on the follow list, also on `idle` and
   `done`. Steering an observed session is the same herdr operation as steering a worker,
   used only when the user asks. When the user asks for a hand session to become a worker,
   the supervisor adopts it with `flow task adopt`: an adopted session is a task in every
   way flow can manage, with a brief, a record whose herdr target names the pane in the
   user's herdr session, and the same watching as a dispatched worker. It keeps running
   where it is, so it gets no hooks: it reports through its own `flow task` calls and
   herdr's state. In place of the Stop hook, `flow watch` logs a `turn.end` event when herdr
   sees it go from `working` to `idle`, so the quiet check counts from its last turn's end
   as it does for a dispatched worker. A turn that ends while no watch run is up is not
   logged, so such a task may be reported quiet once early. Closing an adopted task leaves
   its workspace open, since the user owns it.
   The ladder, then: observed (seen, wakes on `blocked`), followed (also wakes on `idle` and
   `done`), adopted (a worker).

### Durability and layout

9. **Code per CDE, state on EFS.** The repo is cloned to `/workspaces/flow` on each CDE
   (local disk: fast git and fast `cargo build`). Durable state lives at `/efs-home/flow/`
   and is reached from the clone through a symlink `/workspaces/flow/state ->
   /efs-home/flow/state`. A flow-owned, idempotent `scripts/install.sh` pulls the clone,
   creates the state directory and symlink, builds the binary, and links it into
   `~/.local/bin`; the user's own machine-start script clones the repo once, calls that
   script, installs herdr, and keeps its herdr hook path platform-aware. `flow init` writes
   the operator config from what the local machine can tell it. (Layout is the user's; the
   symlink direction and the split between the install script and the user's script are the
   designer's reading.)
10. **The supervisor always runs from `/workspaces/flow`.** The harness keys transcripts by
    cwd, and transcripts are on EFS, so `claude --resume` from `/workspaces/flow` on any
    CDE resumes the same supervisor conversation after a recycle. Session start reads the
    state directory first and trusts it over the conversation.
11. **State is plain files.** One directory per task under `state/tasks/<task-id>/` with
    small JSON documents and an append-only `events.jsonl`. Writes are atomic (write
    temp, rename) and serialized with a mkdir-style lock directory, both of which are safe
    on NFS. No database in v1. (Designer's call.)
12. **Public repo, private content.** `weston-vanta/flow` on GitHub stays public; its
    `main` is reset with a fresh history. The repo holds code, generic docs, skills, and
    the supervisor's instructions. Everything about the actual work lives in `state/` on
    EFS and is never committed: briefs, reports, decisions, inventory, logs, and
    operator configuration (`state/config.toml`: CDE names and ssh aliases, the monorepo
    path and conventions, the worktree tool). This separation is structural, not
    enforced by tooling: nothing about the work is inside the clone, so it cannot be
    committed by accident. Committed docs and commit messages use generic terms, as this
    document does. A denylist scan can be added later if a slip actually happens.

### Transport and runtime

13. **EFS files for durable communication; ssh for control.** Status, requests, decisions,
    and reports are files. Starting a worker and running a herdr API command on another CDE
    go over ssh using the Ona aliases. Nothing in the steady
    state polls over ssh.
14. **herdr is the only runtime.** Workers, observed sessions, and the supervisor itself run
    in herdr panes. `flow` uses herdr for: starting a harness in a pane, native
    idle/working/blocked/done state, waiting on state transitions, reading terminal output,
    steering with confirmation, session identity (once the hook path is fixed), and
    focusing a pane. A herdr server is started on demand on each CDE, without flow's environment
    variables or an invoking Claude Code session's, since the panes a user opens there later
    inherit its environment. The adapter pins a minimum herdr protocol and fails loudly on
    mismatch. tmux is not a fallback.
15. **Every host that may run the supervisor needs the user's Ona login**
    (`gitpod login`, then `gitpod environment ssh-config`). There is no dedicated check or
    repair command. When an ssh or `gitpod` call fails because the login is missing or
    expired, the supervisor keeps working from EFS state and says plainly that it cannot
    reach other CDEs until the user logs in again.

### Harness

16. **Claude Code only in v1, behind an adapter.** The adapter owns: how to launch the
    harness in a herdr pane with a brief, which hooks it installs for status reporting
    (through project-level or `--settings` files, never the user's shared settings), how
    to steer it, how to find its transcript and session id, and how to ask it to resume.
    Codex is the next adapter; nothing outside the adapter may assume Claude Code.
17. **Workers inherit the user's permissive permission mode.** A worker that still hits a
    prompt reads as `blocked` in herdr and is routed to the supervisor like any other
    decision.
18. **Workers report through hooks and files, not by messaging the supervisor.** The
    adapter installs hooks that append events to the task's `events.jsonl` (session start,
    turn end, stop), and the brief tells the worker how to write a structured status line
    (`working`, `blocked`, `needs-decision`, `done`) and its final report.

### Wake-up and supervision

19. **Firstmate's Stop-hook watcher pattern.** The supervisor's project-level Claude Code
    settings register a Stop hook with `asyncRewake: true` and a 25-hour timeout that runs
    `flow watch`. `flow watch` first lists every live CDE's herdr agents, without starting a
    herdr server and with a time limit per CDE, to find the sessions the user started by
    hand. It then blocks on the fleet: herdr `agent wait` per worker and per hand session
    (remote through ssh, one long-lived connection per CDE), file events on `state/`, and a
    heartbeat while tasks are open or sessions are followed. A run watching only hand
    sessions lists the CDEs again hourly and ends quietly after a day. It absorbs benign
    events in-process and exits 2 with a one-line reason when something is actionable. The
    harness delivers that as hook feedback and the supervisor takes a turn. A blocking Stop
    hook, `flow guard`, refuses to let the session end while work is under way and no
    watcher is live. `flow watch` is single-flight across concurrent hook firings and
    re-arms itself before the hook timeout.
20. **Actionable means:** a worker reached `done` or `blocked` or wrote `needs-decision`; a
    worker went quiet beyond a threshold without evidence it is working (herdr `working`,
    recent writes in its worktree); a worker's session or pane disappeared; a CDE became
    unreachable; a dispatch request appeared; a session the user started by hand turned
    `blocked`, or one the user follows went idle or done; the heartbeat fired. The heartbeat
    fires only while tasks are open or sessions are followed. An adopted session wakes the
    supervisor as its task, never as a session started by hand.

### Decision authority

21. **The supervisor is the user's delegate, not proxy.** It answers a worker's question
    itself when the answer follows from the brief, the codebase, or the understanding it
    has built with the user. It escalates when the answer could change what the user asked
    for, when more than one resolution is reasonable and they lead to different work, or
    when the question shows the brief has drifted from what was agreed. Escalations are
    batched and specific: what was asked, what the supervisor would choose and why, what it
    needs. While a question is open the worker is held, and the hold is recorded in the
    task so a recycled supervisor knows who it is waiting on. Every decision the supervisor
    makes alone is logged in the task record.

### Work product

22. **Worktrees via `wt`.** A dispatched task that needs a fresh branch gets a worktree on
    the target CDE through the user's `wt` tool; a task with a natural home reuses the
    existing worktree. The task record stores the worktree path.
23. **Deliverables follow the brief and the monorepo's conventions.** When a brief asks
    for a PR, the worker uses the monorepo's own skills for branches, templates, and
    labels. Investigation briefs deliver a report file in the task directory. The
    supervisor reports plain outcomes with links.

### Implementation

24. **CLI in Rust,** one binary `flow`, built with `cargo build --release` in the clone on
    each CDE (or copied from EFS; same arch and libc everywhere). Startup time matters
    because hooks run on every turn end. Dependencies should favor plain processes (`ssh`,
    `herdr`) over libraries that reimplement them.
25. **The supervisor's instructions live in the repo** as `CLAUDE.md` plus skills under
    `.claude/skills/`, kept short and pointing at `flow --help` for mechanics rather than
    duplicating them. Operator-specific instruction (monorepo conventions, preferred CDE
    names) is loaded from `state/` at session start, not committed.

## Guidelines

- **Agents read and write state through the CLI, never the files.** The on-disk format is
  the CLI's concern (JSON documents, JSONL event logs, TOML configuration; Markdown for
  briefs and reports because agents author and read those as prose). Token efficiency is
  the CLI's job: `flow status`, `flow task show`, and friends render compact views with
  only the fields the question needs, and offer a way to expand.

## Task record

```
state/
  config.toml        operator configuration (CDEs, monorepo, worktree tool)
  tasks/<task-id>/
    task.json        id, parent, title, host, cwd, repo, harness, created, owner, origin
    status.json      current state, last event, hold (who/what/since), herdr target
    events.jsonl     append-only: {ts, source, kind, data}
    brief.md         what the worker was told, including its completion criteria
    decisions.jsonl  questions, who answered (supervisor|user), rationale
    report.md        final report (when done)
    requests/        dispatch requests written by the worker, consumed by the supervisor
  hosts/<env-id>.json inventory: name, ssh alias, worktrees, branches, PRs, sessions, load,
                     last seen
  supervisor/        lock (host, herdr target, session id), watcher ledger, wake queue
```

Task ids are short, time-ordered, and never reused.

## Flows

**Start or focus.** `flow up` runs from a shell pane inside herdr and reads
`state/supervisor/`. With no lock, a stopped one, or one whose CDE the inventory marks gone,
it starts a supervisor here. With a lock on this CDE in the herdr session it runs in, one
listing looks for the supervisor: the agent herdr reports the lock's Claude Code session for,
or with no session reported, the agent in the lock's pane under the `flow-supervisor` label.
Found, it is focused; not found, one is started here. A lock anywhere else is refused without
asking any host. Starting removes any watch lock the previous supervisor's watcher left,
writes `watch-state.json` with every closed task marked read when there is none yet, opens a
`flow-supervisor` workspace in this herdr session running the harness from `/workspaces/flow`
with `--resume` of the recorded supervisor session when one exists and a first prompt to run
the session start procedure (whose turn end arms `flow watch`), takes the lock with this CDE
and herdr session, records the session herdr reports for the new supervisor, and focuses its
pane.

**Stop.** `flow down` finds the supervisor on the lock's CDE, by its Claude Code session or
by the `flow-supervisor` label on the lock's pane, closes its pane, marks its lock stopped,
keeping the Claude Code session to resume, removes the watch lock, which ends the watcher at
its next cycle, and leaves every worker running. When it finds no supervisor, it closes
nothing and says so. The next `flow up` does not look for the stopped supervisor: it starts
one in its own herdr session on its own CDE resuming that session.

**Dispatch.** User asks for work. Supervisor picks a CDE from inventory (or asks), writes
the task directory and brief with completion criteria, ensures a worktree, ensures a herdr
server on the CDE, starts the worker in a herdr pane with the brief, records the herdr
target, and ends its turn. `flow watch` arms.

**Wake.** A worker writes `done`. `flow watch` exits 2 with `done: <task-id>`. The
supervisor's turn starts with that line, reads the report, checks it against the brief's
criteria, tells the user (or queues the news if the user is away), and ends the turn.
`flow watch` re-arms.

**Escalate.** A worker writes `needs-decision`. Watch wakes the supervisor. It reads the
question; if it can answer, it steers the worker through herdr and logs the decision; if
not, it records a hold naming the user, asks the user, and on reply steers the worker and
clears the hold.

**Observe.** On session start and on each wake, the supervisor lists herdr agents on every
known CDE, matches them to tasks where possible, and lists the rest as observed sessions
with their latest summary and state. When the user asks for one of them to become a worker,
the supervisor adopts it: flow records the task with a brief, drops the session from the
follow list, and prompts it in place to read the brief and report through flow.

**Recycle.** A worker CDE is recycled: watch sees the CDE unreachable and its sessions
gone; the supervisor marks affected tasks `orphaned` with what was last known (branch
pushed or not) and asks the user whether to redispatch. The supervisor CDE is recycled:
once the inventory marks that CDE gone, the user runs `flow up` from a herdr pane on another
CDE; it starts one that resumes the recorded session, reads `state/`, re-arms the watcher,
and reports what changed while it was away. Before then, `flow down` marks it stopped once Ona
lists that CDE stopped; while Ona lists it running and ssh cannot reach it, the supervisor may
still run there, so `flow down` keeps the lock and names the way out.

## CLI

Commands exit 3 when the state directory is missing and 2 when they refuse a request as
given. Commands that take `--json` print raw JSON instead of the compact view.
`flow <command> --help` has the details.

Operator:

- `flow init [--force]`: write `state/config.toml` from what this machine can tell, then refresh the inventory; `--force` replaces an existing file.
- `flow up`: from a shell pane inside herdr, keep one supervisor for the state directory and focus its pane; anywhere else it exits 1. With no supervisor running, or one recorded on a host the inventory marks gone, it starts one in this herdr session on this host, resuming the recorded Claude Code session, and records both in the lock. With one recorded on this host in this herdr session, it focuses it when one listing finds it, by the Claude Code session herdr reports for it or, with none reported, as the agent in the lock's pane under the `flow-supervisor` label; finding none, it starts one. With one recorded on another host or in another herdr session, it asks no host and exits 1: run `flow down` first. It ends by printing `supervisor is in workspace flow-supervisor`. Starting one first writes `state/supervisor/watch-state.json` with every closed task marked read when that file does not exist, so the first `flow watch` replays only open tasks.
- `flow down`: stop the supervisor, leaving every worker and task running. It closes the supervisor's pane on the lock's host, found by its Claude Code session in any listed herdr session or as the agent in the lock's pane under the `flow-supervisor` label; finding none, it closes nothing and says so. It marks the lock stopped keeping that session for the next `flow up` to resume, removes the watch lock, which ends the watcher at its next cycle, and says how many open tasks keep running. It fails, changing nothing, when the supervisor's host cannot be reached, since the supervisor may still run there, and names the two ways out: restore ssh access to that host, or stop its environment in Ona and run `flow inventory --refresh`, which marks the host gone, so `flow down` clears the lock without ssh.
- `flow status [--all] [--json]`: open tasks, their holds, pending dispatch requests from workers (`req-id  from-task  host  title`), and observed sessions; `--all` includes done and orphaned tasks.
- `flow inventory [--refresh] [--json]`: worktrees, agents, and PRs per host; `--refresh` first discovers hosts from Ona (or the static list), probes each running one, and lists open PRs. `--json` gives each host its ssh alias, whether it is the host flow runs on (`local`), its Ona phase, when it was last seen, and whether it is reachable or gone.
- `flow inventory --sync [--yes] [--refresh] [--json]`: bring herdr's saved SSH machines on this machine in step with the hosts, in place of showing them. Every other host that is not gone gets a machine labelled with its name, for its ssh alias and the herdr session flow runs in; a gone host's machine is removed when it still has that alias, and a host's machines for another alias or session are removed, before its new one is added when it has none current, since herdr names each machine once; when that add fails, flow says the host is left without a machine. Machines whose label is not a host's name are never touched. A host that did not answer the last refresh, or that Ona reports stopped, is not added, since herdr must reach it to add it: it is listed on stderr, and its machine stays as it is. With nothing to change it says so, noting any host it did not add. It prints the plan and asks before applying it; `--yes` applies it without asking, and with no terminal and no `--yes` it exits 1 having changed nothing. `--refresh` refreshes first, and `--json` prints the plan as JSON, applied only with `--yes`. A failing step stops the rest and says how many were applied.
- `flow run <host> -- <argv>...`: run a command on a host, passing its output and exit code through.
- `flow sessions [--summaries] [--json]`: herdr agents across hosts, with their tasks, as the supervisor, or as observed sessions. `--summaries` adds each session's latest summary, and `--json` includes summaries, pane ids and each agent's `owner`.
- `flow follow <host> <session>`: wake the supervisor when a session started by hand goes idle or done, not only when it turns blocked. `<session>` is the session's title as `flow sessions` shows it, or its pane id when several share the title, or `<herdr session>/<pane id>` when several herdr sessions hold that pane id; `flow sessions` then marks it `followed`.
- `flow unfollow <host> <session>`: stop waking the supervisor for a followed session except when it turns blocked; also accepts the pane id, or `<herdr session>/<pane id>`, of a followed session that is gone.

Supervisor:

- `flow dispatch (--title <title> (--brief <text> | --brief-file <file>) | --request <req-id> [--title <title>]) --criteria <text> [--host <name>] [--dir <path> | --branch <name> [--repo <name>]] [--parent <id>] [--allow-shared-dir]`: start a worker session for a new task. `--request` dispatches a pending request from `flow status`: it supplies the brief, and the title, host and parent unless given, and is consumed once the worker starts.
- `flow task adopt <host> <session> --title <title> --criteria <text> [--brief <text> | --brief-file <file>] [--parent <id>]`: take over a session started by hand as a task. `<session>` is resolved as `flow follow` resolves it; a task's or the supervisor's pane, or a session that is not Claude Code or has no working directory herdr knows, is refused. The task gets a brief, the session leaves the follow list and is prompted to read the brief; when that prompt cannot be delivered, or anything else fails once the task exists, the task is held `blocked` on the supervisor, naming the failure, and the command exits 1. The brief, worded for a session with a conversation and a user at its terminal, runs its `flow` commands with `FLOW_STATE` set.
- `flow steer <id> <text>`: send a message to a task's worker and log it; a `needs-decision` or `blocked` task reads `working` again. It sends nothing when the task's pane holds no agent of the task's, since herdr reuses pane ids, and refuses with "the worker's pane is gone" once flow has retired the task's herdr target.
- `flow task show <id> [--json]`: a task's fields and the head of its brief, naming the brief's file when the head leaves lines out; `--json` carries the full brief.
- `flow task events <id> [--last <N>] [--json]`: a task's event log.
- `flow task decide <id> --by user|supervisor --answer <text>`: answer the task's hold and lift it.
- `flow task hold <id> --on user|supervisor --what <text>`: hold the task until someone answers.
- `flow task orphan <id> --reason <text>`: mark the task orphaned: its work will not finish.
- `flow task close <id> [--force]`: close the task's herdr workspace, keeping its record, working directory and branch; `--force` closes it while the task is still open (working, pending, blocked or needs-decision). The workspace is closed only while herdr lists it under the task's label, which ends in the task id, and the task's pane holds the task's worker or no agent; under another label flow retires the task's target and closes nothing, and a pane holding another agent refuses. An adopted task's workspace is left open to its owner.

Worker:

- `flow task status <id> working|blocked|needs-decision [--message <text>]`: report the task's state; `--message` is required for `blocked` and `needs-decision`.
- `flow task done <id> --report <file>`: finish the task with a report.
- `flow task request-dispatch <id> --title <title> --brief <file> [--host <name>]`: ask the supervisor to dispatch sibling work.

Hooks and plumbing, run by Claude Code and the supervisor rather than by hand, and left out of `flow --help`:

- `flow guard`: Stop hook that blocks ending a turn while work is unwatched.
- `flow watch [--heartbeat <secs>] [--once]`: watch tasks and exit 2 with a reason when the supervisor should wake.
- `flow wake drain`: print the queued wakes not yet acknowledged.
- `flow wake ack --through <N>`: acknowledge every queued wake up to and including N.
- `flow hook claude --task <id>`: record a worker's Claude Code hook event read as JSON from stdin.
- `flow hook supervisor`: record the supervisor's Claude Code session from its SessionStart hook on stdin.

## Left to the builder

- Exact JSON schemas and the event vocabulary.
- How `flow watch` multiplexes remote herdr waits (one ssh master connection per CDE with
  `herdr --machine`, or a small per-CDE agent that writes to EFS) and how it recovers from
  a dropped connection without reporting a dead worker.
- The quiet-worker threshold and what counts as evidence of progress.
- The brief template given to workers, including how they state completion criteria and
  write status lines and reports.
- Inventory refresh strategy (on demand over ssh vs. a per-CDE hook that publishes to EFS).
- The user's machine-start script that clones the repo, calls `scripts/install.sh` (which links the binary into `~/.local/bin`), installs herdr, and keeps the herdr hook path platform-aware.
- Test strategy: the state layer and watcher classification should be testable without
  ssh or herdr; adapters get integration tests against a real second CDE.

## Amendments (2026-10-27)

Agreed with the user after Task 13 landed; they refine decisions 9, 12, 22 and the layout
above and are binding for Tasks 14–19.

- **Hosts, not CDEs.** Code, CLI, instructions and the README say `host`. This document
  keeps "CDE" where it explains Ona. Config, task records and wake reasons use `host`.
- **Several repos.** Config holds `[[repos]]` (name, path, default branch, optional
  worktree tool and worktrees directory) instead of one monorepo. `--branch` takes
  `--repo` when more than one is configured. Without a worktree tool, plain
  `git worktree add` into a sibling `<repo>-worktrees/` directory is used.
- **Working directory, not worktree.** A task has a `cwd` on its host. A repo worktree is
  one way to get one (`--branch`); an existing directory is another (`--dir`); with
  neither, the worker runs in `state/tasks/<id>/work/` on the shared volume, so
  non-repo work (research, preparation, investigations) has a durable home readable from
  every machine. The brief's checkout warning appears only for worktrees. The
  shared-directory refusal applies to every mode.
- **Host is optional.** `flow dispatch` defaults to the machine it runs on.
- **Hosts are discovered, not listed.** Ona is the source of truth: `flow inventory
  --refresh` lists environments through the Ona CLI, derives name, ssh alias and repo
  location, adds new hosts, and marks missing ones gone. Config keeps only overrides
  (name slugs, exclusions) and a static list for machines not managed by Ona. When the Ona
  token has expired, refresh says so in one line and uses the cached set.
- **`flow init` and `scripts/install.sh`** are flow's (install.sh links the binary into
  `~/.local/bin`); the user's machine-start script shrinks to cloning once, calling the
  install script, installing herdr, and fixing the herdr hook path. The README lists what
  the user still provides.
- **CLI reference.** The README and this document carry the list of subcommands (Task 17).
