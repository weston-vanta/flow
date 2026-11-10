# Personal agentic engineering setups: what exists and what features they have

Researched 2026-11-10. Question: which *personal* setups for running many coding-agent sessions
(mostly around Claude Code) are popular, and what features do they contain? This feeds the plan to
grow flow into a personal "software factory": a human Director works 1:1 with Manager Sessions,
which delegate Tasks to IC Sessions, each with its own review and bar-raising workflow that
produces PR stacks, and the system improves itself over time.

Method: read each project's README, docs or skill files through `gh api` and the vendor's docs.
Star counts and last-push dates come from `gh api repos/<owner>/<repo>` on 2026-11-10. They
measure attention, not use. CI-integrated, org-wide SDLC factories are out of scope and only noted
in passing.

Two sources could not be fetched directly. OpenAI's "Harness engineering" post and Steve
Yegge's Gas Town blog post both returned HTTP 403. Gas Town claims are therefore cited to its repo
docs. OpenAI claims come from search-result excerpts of the post and are marked *(via search
excerpt)*.

---

## 1. Catalog

### 1a. First-party Claude Code surfaces

These are worth knowing first, because many third-party features are now built in.

| Name | What it does | Status |
|---|---|---|
| [Subagents](https://code.claude.com/docs/en/agents) | Delegated workers inside one session. Each has its own context and returns a summary. | GA |
| [Agent view (`claude agents`)](https://code.claude.com/docs/en/agent-view) | One terminal screen to dispatch and monitor background sessions. Groups them by state (Ready for review, Needs input, Working, Completed). Gives each dispatched session its own worktree under `.claude/worktrees/`, shows a `#N` PR badge colored by CI and review state, and offers `claude agents --json` for scripts. | Research preview |
| [Agent teams](https://code.claude.com/docs/en/agent-teams) | A lead session spawns teammates that share a task list (pending, in progress, completed, with dependencies and file-locked claiming) and a mailbox (`~/.claude/teams/{team}/inboxes/{agent}.json`). Has `TeammateIdle`, `TaskCreated` and `TaskCompleted` hooks for quality gates. Limits: no nested teams, the lead is fixed, no resume of in-process teammates. | Experimental, off by default |
| [Cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging) | `ListAgents` and `SendMessage` between your own sessions, over a per-session Unix socket locally or via Remote Control across machines. Can send one notice when another session next goes idle (`notify_when_idle`). Incoming messages never count as user consent, and loops are rate-limited. | GA (v2.1.224+) |
| [Projects](https://code.claude.com/docs/en/claude-projects) | One ongoing conversation at claude.ai/code that starts parallel cloud "threads". They share project instructions (up to 16k characters) and project memory (`MEMORY.md`). An Overview pane shows threads by state (ready for review, waiting on you). Thread PRs are watched with auto-fix. | Public beta |
| [Dynamic workflows, `/batch`](https://code.claude.com/docs/en/agents) | Script-held plans that run many subagents and cross-check them. `/batch` splits one change into 5 to 30 worktree-isolated subagents. | GA |
| [claude-plugins-official](https://github.com/anthropics/claude-plugins-official) (46.8k stars) | Anthropic's plugin directory. Includes `ralph-loop` (a Stop-hook loop), `code-review` (parallel agents with confidence scoring), `claude-md-management` (`/revise-claude-md` captures session learnings), `hookify` (turns observed unwanted behaviors into hooks) and `skill-creator`. | Active |

### 1b. Multi-session managers: terminal, TUI and desktop

| Name | Popularity | What it does |
|---|---|---|
| [claude-squad](https://github.com/smtg-ai/claude-squad) | 8.8k stars, pushed 2026-08 | A TUI over tmux. Each task gets its own git workspace. Runs in the background with optional auto-accept (`-y`). You review the diff before applying it, and can check changes out before pushing. Works with Claude, Codex, Gemini and Aider. |
| [Conductor](https://www.conductor.build/docs/) | Closed source (no public repo found) | A Mac app for running Claude Code, Codex, Cursor and OpenCode in parallel. Each task gets its own workspace, branch, terminal, diff and review path, then you open a PR, merge and archive. |
| [Crystal → Nimbalyst](https://github.com/stravu/crystal) | 3.1k stars, archived and redirected | Parallel Claude Code and Codex sessions in git worktrees. Now Nimbalyst, an "AI-native workspace". |
| [vibe-kanban](https://github.com/BloopAI/vibe-kanban) | 28.6k stars | A kanban of issues. Each workspace gets a branch, terminal and dev server. Inline diff comments go back to the agent, and PRs are created with AI descriptions. **Sunsetting**: the [shutdown post (2026-04-10)](https://www.vibekanban.com/blog/shutdown) says most users were free and its early features (multi-agent, diff comments, preview) are now "taken for granted". |
| [Orca](https://github.com/stablyai/orca) | 121k stars | A desktop, mobile and remote app. Has a worktree per agent, an "orchestrate" mode where one agent splits work and starts workers, line-level diff notes sent back to the agent, failing checks and PR comments routed back to the agent, a GitHub and Linear task inbox, scheduled automations, and a usage meter. |
| [Orchestrator.inc / agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator) | 14.4k stars | A project's main agent plans and starts a worker per piece in its own worktree. A board sorts sessions into lanes from agent, PR, CI and review state. CI failures, review comments and conflicts go "back to the worker that owns the branch". An optional reviewer agent reads every PR first. Has a mobile app. |
| [Emdash](https://github.com/generalaction/emdash) | 6.2k stars | A desktop ADE. Has a worktree per task, takes tickets from Linear, GitHub, Jira and others, shows diffs, PRs and CI checks, and supports remote machines over SSH. |
| [Sculptor](https://github.com/imbue-ai/sculptor) | 246 stars | A desktop app for parallel agents in isolated workspaces, shipped with workflow skills (for example `fix-bug`: reproduce, failing test, fix). |
| [cmux](https://github.com/manaflow-ai/cmux) | 28.8k stars | A Ghostty-based macOS terminal. Panes get attention rings when an agent needs you. A sidebar shows branch, PR status, ports and the latest notification. Has a socket API and a browser pane. |
| [herdr](https://github.com/herdrdev/herdr) | 50k stars | A terminal "runtime". Sessions survive detach and restart, one window spans several machines, and panes are marked working, blocked or idle. Agents drive herdr through a CLI and socket: they "spawn panes, prompt each other, and wait until another agent is genuinely blocked". |
| [Agent Deck](https://github.com/asheshgoplani/agent-deck) | 1.2k stars | A TUI over all sessions. Its **Conductor** mode is a supervising session that "answers routine questions, escalates the interesting ones to your phone, and never lets a `waiting` worker rot". Watchers (GitHub events, email) can wake it. |
| [Agent of Empires](https://github.com/agent-of-empires/agent-of-empires) | 4.1k stars | A tmux session manager with TUI, web, CLI and HTTP API. Status detection, worktrees, optional container sandboxing, diff review. |
| [workmux](https://github.com/raine/workmux) | 3.0k stars | Pairs a git worktree with a tmux window. Runs `post_create` setup hooks, has `workmux merge` for cleanup, a dashboard and sidebar, status in tmux window names, and a `/worktree` delegation skill. |
| [uzi](https://github.com/devflowinc/uzi) | 584 stars, stale since 2025-06 | A CLI with a worktree and tmux session per agent, dev-server port management, auto-confirm, then checkpoint and merge. |
| [container-use (Dagger)](https://github.com/dagger/container-use) | 4.1k stars | An MCP server that gives each agent a container plus a git branch, with full command history ("what agents actually did, not just what they claim"). You can drop into an agent's terminal. |
| [opcode](https://github.com/winfunc/opcode), [claudecodeui](https://github.com/siteboon/claudecodeui) | 22.4k / 14.6k stars | GUIs and web UIs for Claude Code sessions (remote and mobile access). |
| [claude-code-hooks-multi-agent-observability](https://github.com/disler/claude-code-hooks-multi-agent-observability) | 1.6k stars | A dashboard fed by hooks: hook script → HTTP → SQLite → WebSocket → Vue live view of all agents' events. |

### 1c. Orchestrators with roles, dispatch and persistent work state

| Name | Popularity | What it does |
|---|---|---|
| [Gas Town](https://github.com/gastownhall/gastown) (Steve Yegge) | 18.8k stars, very active | The closest public analogue to a Director/Manager/IC factory. Roles: **Mayor** (coordinator you talk to), **Crew** (your long-lived agents), **Polecats** (workers with persistent identity and ephemeral sessions), **Witness** (per-repo watchdog for stuck agents), **Deacon** (supervisor patrol), **Refinery** (Bors-style bisecting merge queue with verification gates). Work lives in **Beads** (a git/Dolt-backed issue graph), and each agent's queue is its **Hook**. Also has convoys (bundles of work), formulas/molecules (TOML workflow templates), `gt escalate` with severities, a scheduler that caps concurrent polecats, `gt seance` (query a predecessor session), and a `gt feed` TUI with a problems view (stalled, zombie, "GUPP violation"). |
| [Beads](https://github.com/gastownhall/beads) | 28.7k stars | A dependency-aware issue graph for agents: `bd ready`, `bd update --claim`, `bd close`. Hash IDs avoid merge collisions, and "compaction" summarizes old closed tasks. Has message issues and `bd remember` for durable facts. |
| [Tmux-Orchestrator](https://github.com/Jedward23/Tmux-Orchestrator) | 1.8k stars, stale since 2025-07 | A three-tier Orchestrator → Project Manager → Engineer hierarchy in tmux. Agents self-schedule check-ins (`schedule_with_note.sh 30 "..."`), commit every 30 minutes, and keep a `LEARNINGS.md`. |
| [Claude Code Agent Farm](https://github.com/Dicklesworthstone/claude_code_agent_farm) | 922 stars | 20 to 50 parallel agents for sweeps (bug fixing, best practices). Lock-based coordination, heartbeat dashboard, auto-restart, context auto-clear, HTML run reports. |
| [oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode) | 40.7k stars | A plugin. `/team` runs a staged pipeline `team-plan → team-prd → team-exec → team-verify → team-fix (loop)` on Claude Code agent teams. `omc team N:codex` spawns tmux workers on other CLIs, `/deep-interview` clarifies requirements, and `/autopilot` and `/ralph` are autonomous modes. |
| [Paperclip](https://github.com/paperclipai/paperclip) | 113k stars | "If OpenClaw is an employee, Paperclip is the company." Agents in an org chart with roles and a reporting line, atomic task checkout, budgets with auto-pause, approval gates, routines, a "Skill Studio" with evals and version history, and "performance reviews for agents". Business-oriented rather than code-only. |
| [Archon](https://github.com/coleam00/Archon) | 23.9k stars | A "factory builder". YAML workflow graphs with `loop … until_bash: "bun run test"`, `fresh_context`, `max_iterations`, and `wait` nodes that raise one "attention item" with a recommendation. Has queues, a per-run event log and transcript, and per-run cost. |
| [Symphony (OpenAI)](https://github.com/openai/symphony) | 28.0k stars | A language-agnostic [SPEC.md](https://github.com/openai/symphony/blob/main/SPEC.md) for a daemon that polls an issue tracker, creates a per-issue workspace, runs an agent, retries with backoff, reconciles runs against issue state, and loads policy from a repo-owned `WORKFLOW.md`. A run may end at a handoff state such as "Human Review". Team-oriented, but small enough to run alone. |
| [HumanLayer](https://github.com/humanlayer/humanlayer) | 12.9k stars | A daemon per host, plus desktop and web apps to steer sessions, plans and reviews. Ships Research-Plan-Implement skills. Moving toward teams. |
| [Ralph](https://ghuntley.com/ralph/) / [snarktank/ralph](https://github.com/snarktank/ralph) | 22.2k stars | `while :; do cat PROMPT.md \| claude-code; done`. Each iteration is a fresh context working on one plan item, with tests as backpressure. |
| [ruflo (claude-flow)](https://github.com/ruvnet/ruflo) | 81k stars | A "meta-harness" of swarms, memory and hooks via MCP. Its marketing claims (self-learning, federation) are broad. |
| [CCPM](https://github.com/automazeio/ccpm) | 8.4k stars | PRD → epic → GitHub issues → parallel agents. Several agents work *in the same worktree* on separate streams. |

### 1d. Skill packs and methodologies

| Name | Popularity | What it does |
|---|---|---|
| [obra/superpowers](https://github.com/obra/superpowers) (Jesse Vincent) | 316k stars | Mandatory, auto-triggering skills. **brainstorming** → **using-git-worktrees** → **writing-plans** (tasks of 2 to 5 minutes with exact paths and verification) → **subagent-driven-development** (a fresh subagent per task, two-stage review: spec compliance, then code quality) → **TDD** → **requesting-code-review** between tasks, where "critical issues block progress" → **finishing-a-development-branch**. Also has `diagnosing-superpowers`, which reads a transcript to explain what went wrong. |
| [mattpocock/skills](https://github.com/mattpocock/skills) | 321k stars | Small composable skills: `grill-with-docs` (shared glossary plus ADRs), `to-spec`, `to-tickets`, `implement-spec` (a ticket DAG and frontier, an implementer subagent per ticket in its own worktree, a merger subagent onto an integration branch, then `code-review`), and `code-review` (parallel Standards and Spec subagents plus a Fowler-smell baseline). Also `retro`, plus `chief-of-staff` and `software-factory` (in progress). |
| [Compound Engineering (Every)](https://github.com/EveryInc/compound-engineering-plugin) | 26.0k stars | A loop of brainstorm → plan → work → simplify → review → **compound** (write learnings to `docs/solutions/`, which the next plan reads). `ce-code-review` picks reviewer personas from the diff, gates findings by confidence and assigns P0 to P3. `ce-babysit-pr` watches comments, CI and base movement, and has `stack-ready` and `stack-land` postures for managed stacks. `/lfg` runs the whole pipeline. |
| [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) | 115k stars | 25 lifecycle skills (Define, Plan, Build, Verify, Review, Ship), including `doubt-driven-development` (adversarial fresh-context review of in-flight decisions) and `code-review-and-quality` (five axes, changes of about 100 lines). |
| [ECC](https://github.com/affaan-m/ECC) | 301k stars | A huge pack (71 agents, 314 skills) plus **continuous-learning-v2** "instincts" (see §2.8). |
| [wshobson/agents](https://github.com/wshobson/agents) | 42.4k stars | 202 agents, 185 skills and 105 commands written once and generated for 7 harnesses. |
| [Spec Kit (GitHub)](https://github.com/github/spec-kit) | 149k stars | `/speckit-constitution` → `specify` → `plan` → `tasks` → implement. |
| [BMAD Method](https://github.com/bmad-code-org/BMAD-METHOD), [GSD](https://github.com/open-gsd/gsd-core) | 55.2k / 11.4k stars | Process-owning spec-driven frameworks with persona agents. |
| [anthropics/skills `skill-creator`](https://github.com/anthropics/skills) | 192k stars | Write a skill, then run evals: with-skill and baseline subagents run in the same turn, outputs are graded and benchmarked, and the description is tuned so the skill triggers. |
| [claude-mem](https://github.com/thedotmack/claude-mem) | 116k stars | A second agent "takes notes while it works". The next session starts with a compact index of them. |
| [Hermes Agent (Nous)](https://github.com/NousResearch/hermes-agent) | 262k stars | A general agent (not coding-specific) with a "built-in learning loop": it "creates skills from experience, improves them during use", and searches its past conversations. |

### 1e. PR-stack tooling agents use

| Name | Popularity | What it does |
|---|---|---|
| [github/gh-stack](https://github.com/github/gh-stack) | 1.8k stars | GitHub's official stacked-PR `gh` extension. Creates branches, keeps them rebased, sets PR bases, has `gh stack view --json`, and ships an agent skill (`gh skill install github/gh-stack`). Compound Engineering's `ce-babysit-pr` drives `gh stack merge`. |
| [ghstack](https://github.com/ezyang/ghstack), [av (Aviator)](https://github.com/aviator-co/av) | 1.0k / 511 stars | Older stacked-diff CLIs. |

### 1f. Practitioner write-ups

- **Boris Cherny (created Claude Code)**: setup thread ([X](https://x.com/bcherny/status/2007179832300581177), [transcribed on GitHub](https://github.com/shanraisshan/claude-code-best-practice/blob/main/tips/claude-boris-13-tips-03-jan-26.md)). 5 local plus 5 to 10 web Claudes, numbered tabs with system notifications, most sessions started in Plan mode, a shared `CLAUDE.md` updated "anytime Claude does something incorrectly", `@claude` tagged on PRs to update `CLAUDE.md` ("his version of Compounding Engineering"), `/commit-push-pr`, `code-simplifier` and `verify-app` subagents, and a PostToolUse formatter. He calls verification loops the most important tip.
- **Simon Willison**: ["Embracing the parallel coding agent lifestyle"](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/). He parallelizes research, codebase explanations, small maintenance and *carefully specified* work. The limit is review: he can "only focus on reviewing and landing one significant change at a time." Uses "scout" agents whose code is thrown away.
- **Geoffrey Huntley**: [Ralph](https://ghuntley.com/ralph/). One item per loop, a deterministic context stack (specs plus `fix_plan.md`), subagents for expensive reads, a single builder for tests and builds, `AGENTS.md` updated by the loop ("the heart of the loop"), and tuning prompts "like a guitar". He is skeptical of Ralph on existing codebases.
- **Anthropic**: ["Effective harnesses for long-running agents"](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents). An initializer agent writes `init.sh`, a JSON feature list (all failing) and a progress file. Coding agents do one feature per session and flip only `passes`. Observed failures: premature victory, messy handoffs, features marked done without end-to-end tests.
- **OpenAI, Ryan Lopopolo**: ["Harness engineering"](https://openai.com/index/harness-engineering/) *(via search excerpt)*. Three engineers merged about 1,500 PRs with no hand-written code. `AGENTS.md` is a ~100-line "table of contents", not an encyclopedia. "Golden principles" are enforced by custom linters and structural tests. Recurring background "garbage collection" tasks open small refactor PRs, and a doc-gardening agent fixes stale docs.

---

## 2. Feature enumeration

Each feature lists the setups that have it. "CC" means first-party Claude Code.

### 2.1 Defining work (intake, specs, decomposition)

| Feature | Setups |
|---|---|
| Requirements interview before code ("grill", "brainstorm", "deep interview") | superpowers `brainstorming`; mattpocock `grill-with-docs`; Compound `ce-brainstorm`; oh-my-claudecode `/deep-interview`; addyosmani `interview-me`; Boris (Plan mode) |
| Written spec/PRD as an artifact | mattpocock `to-spec`; Spec Kit; BMAD; CCPM; Compound (one plan file that grows); Ralph (specs per topic); Anthropic harness (JSON feature list) |
| Decompose into a task DAG with dependencies and a ready frontier | mattpocock `to-tickets` + `implement-spec` ("frontier"); Beads (`bd ready`); CC agent teams (blocked tasks); Gas Town convoys; Symphony (`blocked_by` on issues) |
| Bite-sized tasks with exact files and verification steps | superpowers `writing-plans` (2 to 5 minutes each); addyosmani `planning-and-task-breakdown` |
| Shared domain glossary and ADRs as context | mattpocock `grill-with-docs` / `domain-modeling` |
| Repo-owned workflow policy file | Symphony `WORKFLOW.md`; Archon `.archon/workflows/*.yaml`; Gas Town formulas (TOML); oh-my-claudecode stage profiles |
| Tickets from an external tracker (Linear, GitHub, Jira) | Orca; Emdash; Symphony; vibe-kanban; CCPM (GitHub Issues); mattpocock (any tracker set up once) |
| Goal ancestry: tasks carry the "why" | Paperclip (goal → project → task) |

### 2.2 Dispatch and scheduling

| Feature | Setups |
|---|---|
| A Manager agent splits work and starts workers | Gas Town Mayor; Orca orchestrate; Orchestrator.inc main agent; CC agent teams lead; CC Projects; Tmux-Orchestrator PM; mattpocock `implement-spec` / `chief-of-staff` |
| Pull-based claiming with locks | CC agent teams (file locks); Beads `--claim`; Paperclip atomic checkout; Agent Farm locks |
| Push-based assignment to a named worker | Gas Town `gt sling <bead> <rig>`; CC agent teams (lead assigns) |
| Concurrency cap / rate-limit governor | Gas Town scheduler (`max_polecats`); Symphony bounded concurrency; Ralph (one builder at a time) |
| Poll-and-reconcile daemon (stop runs whose ticket changed) | Symphony; Gas Town Witness/Deacon patrols |
| Retry with backoff and bounded tries | Symphony; Archon `max_iterations`; mattpocock `software-factory` ("cap every loop of runs at a few tries before a human") |
| Schedules and recurring runs | Orca automations; Paperclip routines; Archon queues; CC cloud routines; Tmux-Orchestrator self-scheduling; mattpocock `chief-of-staff` (suggests schedules) |
| Event triggers (GitHub event, email) wake a supervisor | Agent Deck watchers; Gas Town nudges; mattpocock `software-factory` triggers |
| Per-task choice of agent and model | Gas Town `--agent`; Orchestrator.inc; oh-my-claudecode `N:codex`; CC agent teams (model per teammate) |

### 2.3 Isolation

| Feature | Setups |
|---|---|
| Git worktree per agent | Nearly all: claude-squad, Conductor, Crystal, vibe-kanban, Orca, Orchestrator.inc, Emdash, workmux, uzi, Gas Town hooks, CC agent view (automatic), superpowers `using-git-worktrees`, mattpocock `implement-spec` |
| Container or VM per agent | container-use; Agent of Empires (Docker, Podman, Apple); Gas Town Docker; Orca cloud VM; CC Projects cloud threads |
| Setup hooks for new worktrees (`.env`, deps, ports) | workmux `post_create`; uzi port management; Symphony workspace hooks; Anthropic `init.sh` |
| *No* isolation, partition files instead | CC agent teams ("Agent teams don't isolate teammates in worktrees"); CCPM (several agents in one worktree) |
| Safe cleanup that refuses to lose unpushed work | CC agent view (`claude rm` keeps a dirty worktree); workmux `merge` |

### 2.4 Quality loops (review and bar-raising)

| Feature | Setups |
|---|---|
| Deterministic backpressure: tests, types, lint must pass | Ralph; Archon `until_bash`; superpowers TDD; mattpocock `tdd`; OpenAI custom linters; Boris PostToolUse formatter |
| Review after each task by a fresh-context subagent | superpowers SDD (spec compliance then code quality); mattpocock `implement` → `code-review` |
| Parallel multi-lens review (standards / spec / security / perf) | mattpocock `code-review` (Standards and Spec axes); Compound `ce-code-review` (personas chosen from the diff); CC `code-review` plugin; CC agent-teams example (security, perf, tests) |
| Confidence or severity gating of findings | Compound (P0 to P3, confidence-gated, "agreement between reviewers does not make a minor issue important"); CC `code-review` plugin (confidence scoring) |
| Repo-owned standards that the reviewer enforces | Compound `CODING_STANDARDS.md` + `REVIEW.md`; mattpocock `CODING_STANDARDS.md` (read at review, not implementation) |
| Simplify pass before review | Compound `ce-simplify-code`; Boris `code-simplifier`; CC `code-simplifier` plugin |
| Adversarial or debate review | addyosmani `doubt-driven-development`; CC agent teams "competing hypotheses" |
| Hook-enforced gates on finishing | CC `TaskCompleted` / `TeammateIdle` (exit 2 blocks); `ralph-loop` Stop hook; Gas Town Refinery verification gates |
| End-to-end verification in a browser | Anthropic harness (Puppeteer); Boris (`verify-app`, Chrome extension); Orca and vibe-kanban built-in browser; Compound `ce-test-browser`; OpenAI (Chrome DevTools) |
| Proof-of-work bundles for the human | Symphony (CI status, review feedback, walkthrough videos); CCPM [`proof`](https://github.com/automazeio/proof) |
| Human checkpoint pushed late, as a decision-ready brief | mattpocock `loop-me` ("push right", "brief"); Archon `wait` attention item with a recommendation |

### 2.5 PR handling

| Feature | Setups |
|---|---|
| Open a PR from the workspace | Conductor, vibe-kanban, Orca, Emdash, Orchestrator.inc, CC agent view (draft PR, never pushes to main), Boris `/commit-push-pr` |
| Route CI failures and review comments back to the owning agent | Orchestrator.inc; Orca; CC Projects / cloud auto-fix; Compound `ce-babysit-pr` → `ce-resolve-pr-feedback` / `ce-debug` |
| Babysit a PR until merge-ready, with a budget and settle window | Compound `ce-babysit-pr` (default 8h of active watch) |
| Stacked PRs | gh-stack (with agent skill); Compound `ce-babysit-pr` `stack-ready` / `stack-land`. **No session manager in this survey models a PR stack per session**: they show one PR or a count ("3 PRs" in CC agent view). |
| Merge queue | Gas Town Refinery (Bors-style bisecting, failed MRs isolated and re-dispatched) |
| Integration branch with a merger agent | mattpocock `implement-spec` |
| Inline diff comments sent back to the agent | vibe-kanban; Orca; Conductor |

### 2.6 Visibility and dashboards

| Feature | Setups |
|---|---|
| Sessions grouped by attention state (needs input / ready for review / working / done) | CC agent view; CC Projects Overview; Orchestrator.inc lanes; Agent Deck; herdr (working, blocked, idle); Gas Town `gt feed --problems` |
| PR/CI status on each session row | CC agent view (`#N` colored); cmux sidebar; Orchestrator.inc |
| Notifications and attention rings | cmux; Boris (system notifications); Orca and Orchestrator.inc mobile push; Agent Deck → Telegram/Slack |
| Live one-line summaries per session | CC agent view (Haiku-generated summary) |
| Stuck or zombie detection and nudges | Gas Town Witness + problems view (`n` nudge, `h` handoff); Agent Farm heartbeats; Agent Deck Conductor |
| Event stream / observability backend | disler hooks dashboard; Gas Town OTEL data model; container-use command history; Archon per-run log |
| Mobile and remote steering | Orca; Orchestrator.inc; Paperclip; CC Remote Control / Projects; Agent of Empires web; claudecodeui |
| Cost and usage tracking, budgets | Paperclip (budgets, auto-pause); Archon (cost per run); Orca usage meter |
| Machine-readable JSON of all sessions | CC `claude agents --json`; gh-stack `--json`; Beads JSON output |

### 2.7 Communication between agents

| Feature | Setups |
|---|---|
| Point-to-point messages between sessions | CC cross-session messaging / agent-team mailboxes; Gas Town mail (`gt mail`, `gt nudge`); herdr ("prompt each other"); Beads message issues |
| Notify-when-idle subscription | CC `notify_when_idle`; herdr "wait until another agent is genuinely blocked" |
| Escalation to a supervisor or human, with severity | Gas Town `gt escalate -s HIGH` (routed via Deacon → Mayor → Overseer); Agent Deck Conductor → phone; Archon attention items |
| Context via pointers, not pasted text | mattpocock `implement-spec` / `chief-of-staff` ("context pointers"); CC Projects memory index |
| Query a predecessor session | Gas Town `gt seance --talk <id>` |
| Trust boundary: peer messages are not user consent | CC cross-session messaging and agent teams (explicitly enforced) |
| Plan-approval handshake from worker to lead | CC agent teams (plan mode teammate sends approval request) |

### 2.8 Self-improvement

| Feature | Setups |
|---|---|
| Correction → instruction file | Boris (shared `CLAUDE.md`, `@claude` on PRs); Ralph (loop updates `AGENTS.md`); CC `/revise-claude-md`; CC Projects ("tell Claude to remember the correction") |
| Learnings log the next plan reads | Compound `ce-compound` → `docs/solutions/` ("Run one teaches it. Run two remembers."); Tmux-Orchestrator `LEARNINGS.md`; Beads `bd remember` |
| Retro on sessions that proposes environment changes | mattpocock `retro` (navigation, automated checks, coding standards, tool economy, no-ops) and `chief-of-staff` (retro "about one in five" subagent sessions, worst ones first) |
| Turn mechanical review findings into deterministic checks | mattpocock `retro` ("a mechanical one … gets a deterministic check, full stop"); mattpocock `software-factory` (human review "catches … becomes new automated checks"); OpenAI golden principles + linters |
| Automatic observation via hooks → atomic "instincts" with confidence → clustered into skills | ECC continuous-learning-v2 (PreToolUse/PostToolUse capture, a Haiku observer, confidence 0.3 to 0.9, project-scoped then promoted to global when seen in 2+ projects, `/evolve` clusters instincts into skills) |
| Behaviour → hook | CC `hookify` (analyzes conversation for unwanted behaviors) |
| Skill testing / evals | superpowers `writing-skills` (TDD for skills: pressure scenarios, watch the agent fail without the skill first); anthropics `skill-creator` (with-skill vs baseline runs, benchmark, description tuning); Paperclip Skill Studio (saved test inputs, version history) |
| Transcript diagnosis of what went wrong | superpowers `diagnosing-superpowers`; mattpocock `retro` |
| Per-agent track record / performance | Gas Town (agent CVs, `bd stats --actor`; capability routing planned, not built); Paperclip ("performance reviews for agents") |
| Background entropy cleanup | OpenAI garbage-collection tasks + doc-gardening agent *(via search excerpt)*; mattpocock `improve-codebase-architecture` (suggested every few days) |
| Cross-session memory | claude-mem; CC auto memory / Projects memory; Hermes; Beads compaction |

### 2.9 Other

| Feature | Setups |
|---|---|
| Persistent sessions that survive terminal close, reboot or SSH drop | herdr; Agent of Empires; CC agent view supervisor (restarts crashed sessions, stops idle unattached ones after about an hour); Gas Town hooks |
| Handoff / context refresh | Gas Town `gt prime` and the feed's `h` handoff; mattpocock `handoff`; Compound `ce-handoff`; Anthropic progress file |
| Multi-harness support (Claude, Codex, Gemini, ...) | Almost every manager; wshobson generators; Gas Town presets |
| Agent identity and attribution in commits | Gas Town (`gastown/polecats/toast <owner@…>`) |
| Federation across installs | Gas Town Wasteland |
| Governance: approvals, scoped secrets, config rollback | Paperclip; Archon approval gates |

---

## 3. Patterns, anti-patterns and lessons

**Converged on (table stakes).** Worktree-per-agent, a state-grouped session list, diff review and
open-PR-from-workspace now appear in almost every manager, and Claude Code itself ships them
([agent view](https://code.claude.com/docs/en/agent-view)). vibe-kanban's
[shutdown post](https://www.vibekanban.com/blog/shutdown) says exactly this: those features are now
"taken for granted". Crystal has folded into Nimbalyst, and uzi and Tmux-Orchestrator have gone
quiet. Differentiation has moved to **planning and review**, which is also where vibe-kanban said
the value now lies.

**Review is the bottleneck, so build for review speed.** Willison: one significant change at a
time ([post](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)). mattpocock: present a
decision-ready "brief", never raw output, and "push right" the checkpoint
([`loop-me`](https://github.com/mattpocock/skills/blob/main/skills/in-progress/loop-me/SKILL.md)).
Archon raises one "attention item" with a recommendation
([README](https://github.com/coleam00/Archon)). OpenAI's garbage-collection PRs are "reviewed in
under a minute" *(via search excerpt)*. Small PRs (addyosmani: about 100 lines) and stacks are how
review stays cheap.

**Put the bar in the reviewer, not the implementer.** mattpocock `retro` argues the implementer has
the most context pressure, so coding standards belong to a fresh-context reviewer that reads a
diff ([retro](https://github.com/mattpocock/skills/blob/main/skills/engineering/retro/SKILL.md)).
superpowers and mattpocock both use fresh subagents with separate spec and quality axes.
Compound adds that reviewer agreement does not make a minor finding important
([ce-code-review](https://github.com/EveryInc/compound-engineering-plugin/blob/main/docs/guides/ce-code-review.md)).

**Brakes before accelerators.** "Every trigger is an accelerator. Accelerators without brakes make a
slop cannon." The brakes are automated checks, automated review and human review. Add triggers one
at a time, throttled, and cap retry loops before a human
([software-factory](https://github.com/mattpocock/skills/blob/main/skills/in-progress/software-factory/SKILL.md)).

**Work state lives outside the agent.** Gas Town (Beads plus Hooks, "survives crashes and
restarts"), Symphony (tracker plus filesystem recovery, no DB required), Anthropic (JSON feature
list plus progress file), and mattpocock `software-factory` ("a run's only memory of earlier runs
is the ticket tracker, the git host and the orchestrator") all keep the agent's session
disposable and the ledger durable. Anthropic found that JSON resists inappropriate edits better
than Markdown
([post](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)).

**Supervisors for liveness.** At 20+ agents, stalled and zombie workers dominate. Gas Town has a
three-tier Witness/Deacon/Dogs watchdog and a problems view. Agent Deck's Conductor "never lets a
`waiting` worker rot". CC agent view restarts crashed sessions. Claude Code's own docs warn the
team lead "can stop early too" and that "task status can lag"
([agent teams](https://code.claude.com/docs/en/agent-teams)).

**Self-improvement is mostly "correction → durable file", with escalating rigor:**
1. Human-curated `CLAUDE.md` updates (Boris).
2. A loop-written `AGENTS.md` (Ralph) or learnings docs (Compound).
3. A retro over transcripts that proposes changes, and turns mechanical issues into lint rules or
   hooks (mattpocock `retro`, `hookify`).
4. Hook-observed, confidence-scored instincts clustered into skills (ECC).
5. Skills treated as code with evals: a baseline without the skill, then with it (superpowers
   `writing-skills`, `skill-creator`).

Only the last level measures whether a change helped. ECC adds project-scoping to avoid
"cross-project contamination"
([continuous-learning-v2](https://github.com/affaan-m/ECC/blob/main/skills/continuous-learning-v2/SKILL.md)).

**AGENTS.md as a map, not an encyclopedia.** OpenAI's single big file "crowded out the task …
went stale", so they cut it to about 100 lines of pointers *(via search excerpt)*. mattpocock
recommends using `CLAUDE.md` "incredibly sparingly, usually only for navigation pointers"
([retro](https://github.com/mattpocock/skills/blob/main/skills/engineering/retro/SKILL.md)).

**Anti-patterns and failure modes reported by the sources:**
- *Process-owning frameworks take control away.* mattpocock: GSD, BMAD and Spec-Kit "take away
  your control and make bugs in the process hard to resolve"
  ([README](https://github.com/mattpocock/skills)).
- *Premature victory and unverified "done".* Anthropic's harness post; CC agent teams ("lead can
  stop early").
- *Duplicate implementations from nondeterminism*, Ralph's "Achilles' heel". Mitigation: "don't
  assume not implemented" ([Ralph](https://ghuntley.com/ralph/)).
- *Same-file edits by parallel agents.* CC agent teams don't isolate in worktrees: "Two teammates
  editing the same file leads to overwrites."
- *Too many agents.* CC recommends 3 to 5 teammates and says token cost scales linearly
  ([agent teams](https://code.claude.com/docs/en/agent-teams)). Paperclip lists "runaway loops waste
  hundreds of dollars" as a core problem.
- *Naive PR watch loops.* They wait for CI and then read comments (burning a cycle), declare
  readiness before review lands, or reimplement feedback resolution
  ([ce-babysit-pr](https://github.com/EveryInc/compound-engineering-plugin/blob/main/docs/guides/ce-babysit-pr.md)).
- *Trust leaks between agents.* Claude Code treats a peer's "approval" as untrusted
  ([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)).
- *Unattended runs too long.* "Letting a team run unattended for too long increases the risk of
  wasted effort" ([agent teams](https://code.claude.com/docs/en/agent-teams)).

**Gaps relevant to flow.** None of the managers surveyed shows *PR stacks per Session* as a first-class
view. They show one PR or a count, and stack tooling (gh-stack, `ce-babysit-pr stack-*`) lives in
skills and CLIs. Cross-environment visibility (many CDEs, one view) exists only in multi-machine
tools (herdr, Orca remote, CC Remote Control and Projects). A Director → Manager → IC hierarchy
with its own review workflow per IC is most fully built in Gas Town (Mayor → Polecats + Refinery
+ Witness). Claude Code's own primitives (Projects, agent teams, cross-session messaging, agent
view) now cover the plumbing, but cap nesting: "teammates cannot spawn their own teammates".
