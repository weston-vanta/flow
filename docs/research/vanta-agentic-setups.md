# Personal agentic engineering setups at Vanta

Research date: 2026-11-10. Sources: Vanta Slack (read only, via `slack-vanta`) and the code each
lead pointed to (VantaInc repos, personal GitHub dotfiles, open PRs). The question: which
*personal* setups do Vanta engineers run, and what features do they have, to inform growing flow
into a personal "software factory" (Director, Manager sessions, IC sessions, quality loops,
self-improvement).

Evidence key: **code** means I read the source; **code (PR)** means the code is in an open or
closed PR rather than on a default branch; **description** means only a Slack message (or a doc I
could not open) describes it. Slack links are permalinks into `vantahq.slack.com`; most need Vanta
SSO. VantaInc repos are private or internal.

Scope: personal setups. CI-run or org-wide factories are listed briefly at the end as out of scope.

---

## 1. Catalog of setups

### Code-backed

| # | Setup | Owner | Code | Slack | What it does |
|---|---|---|---|---|---|
| S1 | **dispatch + cockpit** (access-mgmt KB) and **snowplow-dispatch** (Snowplow KB) | Sam Storino (dispatch), David Zhu (KB, build, session-retro) | [dispatch/SKILL.md](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/SKILL.md), [board.ts](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/scripts/board.ts), [cockpit.js](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/cockpit/cockpit.js); [snowplow-dispatch](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/SKILL.md), [worker-protocol.md](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/worker-protocol.md) | [intro](https://vantahq.slack.com/archives/C09P1RBN615/p1792177260878319), [snowplow](https://vantahq.slack.com/archives/C0C8FQ51KS7/p1793740629461269?thread_ts=1793382361.578199), [cockpit reply](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1794245821310399?thread_ts=1794243146.124169), [show & tell](https://vantahq.slack.com/archives/C06T4MLSGCF/p1794344278443289?thread_ts=1794271235.769079) | A local orchestrator takes N Jira tickets, creates one Ona box per ticket through the Stratus CLI, sends each worker a brief, monitors them via `stratus agent wait`, relays grilling questions, and renders an HTML "cockpit" board (optionally live via Lavish) where the human answers questions. Workers end every turn with a machine-parsed `DISPATCH:` status line. |
| S2 | **build / session-retro / start-work-session** (access-mgmt KB) | David Zhu | [build/SKILL.md](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/build/SKILL.md), [session-retro/SKILL.md](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/session-retro/SKILL.md), [extract-implicit-learnings.ts](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/session-retro/scripts/extract-implicit-learnings.ts), [continuous-learning.md](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/_shared/continuous-learning.md), [start-work-session](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/start-work-session/SKILL.md) | [retro tip](https://vantahq.slack.com/archives/C08TGG8MABC/p1784750678151799?thread_ts=1784748193.479739), [local plans / remote builds](https://vantahq.slack.com/archives/C0B0DGDDT46/p1788366856637799?thread_ts=1788366217.485759), [KB as SDLC](https://vantahq.slack.com/archives/C0BTTUYVCLV/p1794338229354439?thread_ts=1794330551.005929) | A coordinator that "never performs hands-on implementation" delivers a feature as outside-in layer PRs. A retro skill mines the session log for failure-then-retry patterns, reads all PR review threads, and publishes verified lessons as a draft PR to the KB. A launcher creates paired code+KB worktrees, with storage-pressure handling and session hibernation. |
| S3 | **sail** (Set a Course + Captain) | Mark Whelan | [PR #9](https://github.com/VantaInc/fprm-knowledge-base/pull/9): [SKILL.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/SKILL.md), [run-voyage.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/run-voyage.md), [evaluator.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/evaluator.md), [fix-round.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/fix-round.md), [dashboard.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/dashboard.md) — code (PR) | [demo post](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793989490158459) | Plans requirements into a task DAG, then a "Captain" session that never writes code runs each unblocked task: worker subagent in a worktree, then parallel adversarial reviewers (Opus, Codex, frontend), then a fix round in the same worker, then human review (or `autonomous` mode), then draft PR and babysit to green. Status lives in `ship-log.md` plus a colored Excalidraw DAG. "About 25 PRs in this week." |
| S4 | **rex-ona-workers** + **agent-trunk-feature-development** | Jeremiah Pratt (rex-ona-workers; generalized James Wu's trunk workflow) | [rex-ona-workers/SKILL.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/SKILL.md), [watch-worker.sh](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/scripts/watch-worker.sh), [worker-brief.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/references/worker-brief.md), [agent-trunk-feature-development](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/agent-trunk-feature-development/SKILL.md), [review-coverage.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/agent-trunk-feature-development/references/review-coverage.md) | [Rex post](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1794243146124169), [trunk generalization](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793909284605199?thread_ts=1793826357.153539) | A coordinator opens one Rex terminal tab per worker, each SSH'd into its own Ona env running `claude --permission-mode auto` on a brief. An append-only ledger records which env ran which task. Workers signal completion by writing a report file; a poller watches for the file and for permission prompts. Pairs with a trunk-based procedure set (aggregate PR, child PRs, review-coverage audit). |
| S5 | **THABTO** (Two Heads Are Better Than One) + tool-neutral dotfiles | Eric Ihli | [thabto/SKILL.md](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/thabto/SKILL.md), [README](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/thabto/README.md), [thabto.py](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/thabto/scripts/thabto.py), [delegate](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/delegate/SKILL.md), [north-star](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/north-star/SKILL.md), [AGENTS.md](https://github.com/wihli/dotfiles/blob/main/agents/.config/AGENTS.md) | [show & tell links](https://vantahq.slack.com/archives/C06T4MLSGCF/p1794344278443289?thread_ts=1794271235.769079), [bottleneck comment](https://vantahq.slack.com/archives/C0CF5EZE42C/p1793999551644509?thread_ts=1793995210.571089) | Read-only multi-model debate: each participant (Claude Code, Codex, OpenCode, Pi) attempts independently, reviews another in a shuffled ring, revises, then the coordinator writes a synthesis and a `verdict.json` ruling on each claim. Later "ground truth" labels feed a per-model scorecard. Also a saved subagent role library and one AGENTS.md compiled for every harness. |
| S6 | **wezterm + herdr + treehouse + agent-rules compiler** | Yanxi Chen | [Dotfiles](https://github.com/YanxiChen-gh/Dotfiles): [agent-rules/build.py](https://github.com/YanxiChen-gh/Dotfiles/blob/main/agent-rules/build.py), [herdr/new-agent-tab.sh](https://github.com/YanxiChen-gh/Dotfiles/blob/main/herdr/new-agent-tab.sh), [treehouse/config.toml](https://github.com/YanxiChen-gh/Dotfiles/blob/main/treehouse/config.toml), [full-verification-workflow](https://github.com/YanxiChen-gh/Dotfiles/blob/main/shared-skills/full-verification-workflow/SKILL.md); [second-opinion plugin (obsidian #152793)](https://github.com/VantaInc/obsidian/pull/152793) | [setup](https://vantahq.slack.com/archives/C09KFLXT5PC/p1783636679170379?thread_ts=1783631372.970229), [second-opinion](https://vantahq.slack.com/archives/C08TGG8MABC/p1779388612516889) | Terminal-native, vendor-neutral fleet: herdr shows each agent's status in tabs; a key binding takes a pooled treehouse worktree (node_modules hardlinked), opens a herdr workspace with an agent and optional nvim split, and returns the worktree to the pool when closed. One rules source compiles to Claude, Codex, OpenCode, and Cursor formats with a drift check. Independent-grader verification skill. |
| S7 | **Personal git/PR plugin with skill evals** | Robert Zablit | [ai/plugin/skills](https://github.com/zabrador/dotfiles/tree/master/ai/plugin/skills) ([crafting-prs](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/crafting-prs/SKILL.md), [maintaining-prs](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/maintaining-prs/SKILL.md), [replanning-branches](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/replanning-branches/SKILL.md)), [ai/evals/claude/run-evals.sh](https://github.com/zabrador/dotfiles/blob/master/ai/evals/claude/run-evals.sh) | [crafting-prs thread](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1793729060216229) | A personal Claude plugin of PR-craft skills (atomic commits, re-planning a branch into clean commits, PR descriptions, babysitting PRs), each with `trigger-evals.json` (does it fire?) and `evals.json` (does following it produce good results?) plus runner scripts. |
| S8 | **Nameless Corp** (governed orchestrator) | Ty Kowalewski | [nameless-corp](https://github.com/VantaInc/nameless-corp) ([README](https://github.com/VantaInc/nameless-corp/blob/main/README.md), [simplify-worker-model spec](https://github.com/VantaInc/nameless-corp/blob/main/docs/superpowers/specs/2026-03-27-simplify-worker-model.md), [micromanager reports](https://github.com/VantaInc/nameless-corp/blob/main/docs/micromanager/README.md)) | [post-mortem](https://vantahq.slack.com/archives/CBQ5RMYPJ/p1774645319860999), [personal AGENTS.md](https://vantahq.slack.com/archives/C08TGG8MABC/p1784751784250079?thread_ts=1784748193.479739) | A Go, event-sourced orchestrator: workers claim tasks with leases, every transition is policy-gated with required evidence and separation of duties, and a "micromanager" agent files bug and improvement reports. The author calls it a failure ("I accidentally vibe coded the DMV"). He later kept a tool-neutral personal harness at `~/.ty-dev-harness` (AGENTS.md pasted in Slack) with a local ticket board, ExecPlans, runbooks, and a "feedback promotion loop". |
| S9 | **herd** (Ona CLI) | Sam Storino | [herd README](https://github.com/VantaInc/herd/blob/main/README.md) | [QOL post](https://vantahq.slack.com/archives/C02UPC49CKX/p1783023288229979?thread_ts=1776184234.149209) | A Mac-side CLI to create, SSH into, and delete Ona envs by name; it gives each instance a stable color in the IDE and cmux, pastes local images into a remote Claude session, and lets Claude call back to open the IDE. |
| S10 | **spaceport** (agent message bus) | Wes Hedrick | [agent_spaceport](https://github.com/VantaInc/agent_spaceport) ([README](https://github.com/VantaInc/agent_spaceport/blob/main/README.md), [skill](https://github.com/VantaInc/agent_spaceport/blob/main/skills/spaceport/SKILL.md)) | none found | A single Rust binary that keeps one mailbox per agent name; agents `send`, `recv --wait`, reply-to, and broadcast through shell commands. A Claude coordinator can spawn Sonnet, Opus, and Codex participants and compare their answers. |
| S11 | **Migration orchestrator** | Cong Qiaoben | [obsidian #141670](https://github.com/VantaInc/obsidian/pull/141670) — code (PR, closed) | [loom post](https://vantahq.slack.com/archives/C09P1RBN615/p1776884826440559) | A script that turns a large migration backlog (946 tests) into tickets in a local Paperclip tracker, then runs a worker loop per ticket (`claim → generating → pr-open`) that makes a worktree, runs `claude -p` with a `--max-budget-usd` ceiling, and opens a draft PR. Later versions launched one interactive Claude session per Ona instance, named after its task. |
| S12 | **wt** (overlayfs worktrees) + `supervise` | Weston Fribley | [dotfiles/bin/wt](https://github.com/weston-vanta/dotfiles/tree/main/bin) (supervise: description) | [wt](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793990465690639?thread_ts=1793989490.158459), [supervise](https://vantahq.slack.com/archives/C08K1G2STRT/p1785953186657169?thread_ts=1785888423.504159), [3–4 long-lived boxes](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1794331892900329?thread_ts=1794243146124169) | Worktrees built as overlayfs layers on a warm base, so a new one needs no dependency install and little disk. A `grill-with-docs → to-spec → to-tickets → supervise` chain dispatches tickets to worktree subagents. (This is the flow author's own setup; listed for completeness.) |
| S13 | **Ralph loop CLI** | Kevin Royer | [royerk/ralph-loop](https://github.com/royerk/ralph-loop) | [wip note](https://vantahq.slack.com/archives/C08TGG8MABC/p1769203426496409?thread_ts=1769197688.020569) | Runs Claude Code repeatedly on one prompt file, without auto-compact, with a stop condition and an optional code-simplifier pass after each iteration. |

### Description only

| # | Setup | Owner | Slack | What is described |
|---|---|---|---|---|
| D1 | **auto-ship** | Kevin Royer | [auto-ship](https://vantahq.slack.com/archives/C09KFLXT5PC/p1773681901048629?thread_ts=1773668287.150359), [spec→PRs suite](https://vantahq.slack.com/archives/C09EU91H534/p1773678975331819?thread_ts=1773668481.140739), [later: `claude agents`](https://vantahq.slack.com/archives/C09KFLXT5PC/p1783631952813869?thread_ts=1783631372.970229) | Prompt → brainstorm doc → plan as `.md` plus `.task.json` with dependencies (becoming stacked PRs) → `auto-execute` in worktrees, draft PR, babysit → `auto-ship-status`. Reflection skills run at the end of each workflow and update a memory file. Kept in `~/` so it is shared across Ona boxes. He later dropped it for native `claude agents`, because newer models "need much less guidance". |
| D2 | **Two-phase spec + autonomous executor with dashboard** | Chris Ng | [thread](https://vantahq.slack.com/archives/C09EU91H534/p1773940728671699?thread_ts=1773668481.140739) | An interactive spec workflow (research, questions, tasks), then an autonomous run: parallel subagents, static analysis and lint, review, "adds failure patterns back to local best practices", and a dashboard that tracks tasks and babysits PRs. |
| D3 | **Trunk-based daily workflow with cron bots** | James Wu | [writeup](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793826357153539), [reviewer bar](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793892669644589?thread_ts=1793826357.153539), [cron on Ona](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793827089727149?thread_ts=1793826357.153539) | A daily empty "trunk" PR that dev PRs merge into (CI non-blocking on children). Claude cron jobs on his Ona box fix trunk CI every 30 minutes and merge main into the trunk and stacks every 3 hours. A PR skill trained on a year of the team's review comments. "About one epic per day" with one engineer. Code generalized in S4. |
| D4 | **Elephant-goldfish 16-session fleet** | Harel Oshri | [fleet lessons](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1794315640742329?thread_ts=1793631355696759), [security triage](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793894038585929) | One long-context "elephant" for design, fresh "goldfish" reviewers, then 16 agent sessions for implementation: "three plan phases across six repos… caught and fixed 30 defects on its own." Also one read-only subagent per security ticket with a fixed verdict schema (prompt published in the thread). |
| D5 | **Aligner agent reviews implementers' code** | Hamilton Greene; Trevor Rex | [Hamilton](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1793633102109169?thread_ts=1793631355696759), [Durable Context](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1793722919382989?thread_ts=1793631355696759), [Trevor](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1793645899356709?thread_ts=1793631355696759), [workflow doc](https://vantahq.slack.com/archives/C08TGG8MABC/p1789665389243549) | Hamilton: a fresh reviewer at each step (align, plan, review), given "Durable Context" plus change context and an adversarial focus, ideally from a second model family. Trevor: agent A owns requirements, agents B/C implement, and A reviews in side chats, which catches missed requirements. |
| D6 | **Rex everywhere + cross-model review** | Will Hall | [Rex over ssh](https://vantahq.slack.com/archives/CBQ5RMYPJ/p1793720672025289?thread_ts=1792528301.850419), [Gemini review](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793895005947739?thread_ts=1793826357.153539), [EFS + tmux](https://vantahq.slack.com/archives/C09KFLXT5PC/p1778517547736849) | A Rex server on each Ona box, tunnelled over SSH and registered as a host; reviews stacks and trunks with Gemini in Cursor because "Claude… tends to be less critical of code that it wrote"; a shared EFS `~/docs` across boxes. |
| D7 | **firstmate-based harnesses** | Ryan Wong; Sam Chiou | [Ryan](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793989677559059?thread_ts=1793989490.158459), [Sam](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793990064750359?thread_ts=1793989490.158459), [Devin PM bot](https://vantahq.slack.com/archives/C0CATH4F47N/p1794258570995269?thread_ts=1794257968.991459) | Built on the OSS [firstmate](https://github.com/kunchenguid/firstmate) "agent distro": you talk to one first mate, which spawns crewmates in tmux or herdr tabs on [treehouse](https://github.com/kunchenguid/treehouse) worktrees, supervises them with a zero-token bash watcher, and escalates only real decisions. Sam Chiou runs a similar shape in Devin: a coordinator session posts milestone status, tracks conflicting PRs, and asks the PM which gaps to dispatch. |
| D8 | **Demo/walkthrough skill chain** | Wesley Porter | [thread](https://vantahq.slack.com/archives/C08K1G2STRT/p1794251064620719) | Composable skills that record demo videos and generate feature walkthrough artifacts after a PR ships. "They are evolving nearly daily." Private dotfiles. |
| D9 | **Others in passing** | David Bernstein; Zheng Tao; Leo Bach; Travis Watson; Daniel Sauerbrun; Kevin Royer | [1 agent per Ona](https://vantahq.slack.com/archives/C08TGG8MABC/p1788285031653039?thread_ts=1788284947.280569), [orchestrator-claudes + worktree subagents](https://vantahq.slack.com/archives/C09P1RBN615/p1777287080896109?thread_ts=1776884826.440559), [local orchestrator for remote envs](https://vantahq.slack.com/archives/C09JB9RSKFW/p1793650325422659?thread_ts=1793647543.006809), ["roast the code"](https://vantahq.slack.com/archives/C08TGG8MABC/p1779389627552549?thread_ts=1779388612.516889), [herdr cross-session skill](https://vantahq.slack.com/archives/C06T4MLSGCF/p1794349651282119?thread_ts=1794271235.769079), ["devil's advocate teammate"](https://vantahq.slack.com/archives/C09EU91H534/p1773864973339159?thread_ts=1773668481.140739) | One-line patterns, covered in the feature lists below. |

---

## 2. Features by category

Each feature lists the setups that have it and the evidence type (C = code, D = description).

### 2.1 Defining work

| Feature | Setups |
|---|---|
| **Grill the human before coding**, batched, each question carrying the agent's recommendation | S1 C ([brief](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/SKILL.md): "Ask build-ticket's grill-me batch in one turn, with your recommendation for each"), S2 C, S12 D, D2 D |
| **Questions must be self-contained** so the human can answer without opening the ticket or box | S1 C ("make each question self-contained so they can answer it without opening the ticket, the box, or earlier chat") |
| **Requirements → task DAG** with dependencies, files, and command-checkable "done when" | S3 C ([Set a Course](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/SKILL.md)), D1 D (`.task.json` with dependencies), S8 C (TOML workflow templates) |
| **Spec → tickets chain** (grill-with-docs, to-spec, to-tickets) | S12 D, D2 D |
| **Design contract file** with locked decisions, open questions, rejected approaches, and a dated decision log | S5 C ([north-star](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/north-star/SKILL.md)), D5 D (Durable Context) |
| **Bounded implementation window**: how many dependent PRs may proceed before human feedback ("up to three unmerged dependent PRs as a starting limit") | S2 C ([build](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/build/SKILL.md)) |
| **Outside-in layer ordering** of a stack (outermost caller first, stubs for unfinished inner layers) | S2 C |
| **Routing a ticket to a workstream skill** from its parent epic, with playbook and gotchas pasted into the brief | S1 C ([snowplow Routing](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/SKILL.md)) |
| **Prove the gap / frame the problem** before building | S6 C ([prove-the-gap](https://github.com/YanxiChen-gh/Dotfiles/blob/main/shared-skills/prove-the-gap/SKILL.md)) |
| **Local ticket board** for work that must survive chat or move between agents | S8 D (ty-dev-harness AGENTS.md: "Use the local ticket board when work should survive chat, be handed between agents"), S11 C (Paperclip ticket store) |

### 2.2 Dispatch and delegation

| Feature | Setups |
|---|---|
| **Coordinator never writes code**; it only delegates, tracks, presents | S1 C ("does not plan, implement, review code"), S2 C ("The coordinator never performs hands-on implementation"), S3 C ("You never write code"), S4 C ("You don't do the workers' code work yourself") |
| **Self-contained brief template** (task, state, decisions quoted, authorizations quoted with date, hard limits, report contract) | S4 C ([worker-brief.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/references/worker-brief.md)), S1 C (brief block), S5 C (delegate: "The child cannot see this conversation, so anything you leave out is simply gone") |
| **Confirm the batch once** in a table (ticket, title, box, model) before launching | S1 C |
| **Per-batch model and effort choice**, saved as a preference and overridable | S1 C (`board prefs`, `resolve-model.ts`), S5 C (default pair, escalate to frontier only for hard tasks) |
| **Saved subagent role library** (reviewer, simplifier, debugger) loaded by path, not pasted | S5 C ([delegate](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/delegate/SKILL.md)), D4 D (agent definitions with `tools:` lists) |
| **Concurrency cap** on workers | S3 C ("at most 3 at once and queue the rest"), S2 C (implementation window) |
| **Triage worker questions**: answer factual/procedural ones yourself and log them; send product, scope, contract, and permission questions to the human | S1 C (`board auto … --basis factual\|procedural\|decided-earlier`) |
| **Restart a lost worker** in a fresh thread with the brief plus "Decisions so far" | S1 C |
| **Bulk campaign mode** (one PR per task across a long list, bounded attempts and spend) | S11 C (PR), out-of-scope campaign orchestrator (see §4) |

### 2.3 Isolation and compute

| Feature | Setups |
|---|---|
| **One Ona box per ticket**, named after the ticket | S1 C, S4 C, S11 D ("it was hard until i named the ona instance the task name"), D9 D |
| **Worktree per worker on one box** | S3 C, S6 C, S12 C, D1 D, D7 D |
| **Cheap worktrees**: overlayfs layers, hardlinked node_modules, or a warm pool | S12 C (`wt` overlayfs), S6 C (treehouse pool, hardlinks), S2 C (storage-pressure and hibernation scripts) |
| **Ledger of which env ran which task**, so reuse is safe | S4 C (append-only `ledger.jsonl`: `assigned`/`completed`/`abandoned`), S1 C (board reconcile against `stratus env list`) |
| **Never reuse a box for new work** / only reuse after `completed` | S1 C, S4 C |
| **Serialized env creation under a lock** (concurrent creates returned the wrong env) | S1 C (`board lock -- stratus env create …`) |
| **Escape check**: verify a worker did not edit the main checkout | S3 C ("If a worker escaped its worktree, restore… and restart the task") |
| **Fleet on 3–4 long-lived boxes** rather than one per task | S12 D |
| **Shared home over EFS** so skills, memory, and docs follow you across boxes | D1 D, D6 D, S12 D |

### 2.4 Quality loops (review, bar-raising)

| Feature | Setups |
|---|---|
| **Adversarial self-review before the PR** ("switch to a skeptic's stance, try to break your own change") gating QA | S1 C (cockpit sends the worker back if review isn't done before `qa`), S6 C (independent grading pass) |
| **Separate reviewer agents**, read-only, given the worktree path rather than the diff | S3 C ([evaluator.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/evaluator.md)), S6 C ([second-opinion](https://github.com/VantaInc/obsidian/pull/152793)), D5 D |
| **Cross-model-family review** (Codex or Gemini reviewing Claude) | S3 C (`codex exec -s read-only`), S5 C, S10 C, D5 D, D6 D |
| **Cross-reviewer agreement as signal**: flag findings that several reviewers raised; per-reviewer block/suggest/nit scores | S3 C ("agreement across model families is the strongest signal") |
| **Agreement is not evidence**: rule each claim from sources, record `supported`/`rejected`/`unresolved` with a `settle_by` check | S5 C (THABTO verdict) |
| **Same worker fixes**, triages each finding as fix or decline with a reason | S3 C ([fix-round.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/fix-round.md)) |
| **Human review gate per task** with the diff copied into the main checkout for IDE review, or an `autonomous` mode reviewed as a whole stack | S3 C |
| **Separate reviewer context, not empty context**: give reviewers the design section map and line ranges | D4 D, D5 D |
| **Devil's-advocate teammate / "roast the code"** | D9 D |
| **Human review coverage audit** before trunk merges: trace every landed change to a human-reviewed revision | S4 C ([review-coverage.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/agent-trunk-feature-development/references/review-coverage.md)) |
| **Evidence requirements**: claims of "done" need artifacts (command, exit code, output) | S8 C (policy-gated evidence), S6 C ([evidence template](https://github.com/YanxiChen-gh/Dotfiles/blob/main/shared-skills/full-verification-workflow/SKILL.md)), S2 C, D4 D (fixed verdict/evidence schema) |
| **Verify against how the service actually runs**, not just CI | D4 D ("All four of the broken bot PRs passed CI") |
| **Team review-history-trained PR skill** | D3 D ("trained on the past year of reviews"), S2 C (retro reads every review thread) |

### 2.5 PR handling

| Feature | Setups |
|---|---|
| **Draft PRs only; never mark ready, request reviewers, or merge** | S1 C, S3 C ("Neither mode lets the Captain merge"), S4 C (hard limits), S8 D (ty: "Never post… in GitHub… as the user") |
| **Babysit to green is opt-in**, toggled per item, to save CI | S1 C ("Don't babysit the draft PR… Only a message starting 'Babysit PR' turns this on"), S3 C (always babysits) |
| **PR state polling** on a slow cadence: merged → Done, closed → needs you, CI shown | S1 C (snowplow `--github-every` 5 minutes) |
| **Stacked PR DAG** from task dependencies, with Base column (`stacked on #N`) | S3 C (dashboard), D1 D |
| **Trunk / aggregate PR** with auto-sync bots | S4 C, D3 D |
| **PR description discipline** (short, for humans, no "how many tests passed") | S7 C (crafting-prs with evals), D3 D ("two lines long… not Claudeish"), [thread](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1793729060216229) |
| **Commit-history re-planning** onto a fresh branch off the merge-base | S7 C ([replanning-branches](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/replanning-branches/SKILL.md)) |
| **Conflict watch across in-flight PRs** | D7 D (the Devin coordinator flags open PRs that touch nearby files, [Slack](https://vantahq.slack.com/archives/C0CATH4F47N/p1794258392743649?thread_ts=1794257968.991459)) |

### 2.6 Visibility and dashboards

| Feature | Setups |
|---|---|
| **Board grouped by what needs the human** ("Waiting on you", Running, Done), one page per orchestrator window | S1 C ([cockpit.js](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/cockpit/cockpit.js)) |
| **Two-line context per item**: "What this is" and "Where it stands", for a human returning cold | S1 C |
| **Interactive HTML page that sends answers back to the agent** (Lavish) | S1 C, Ryan Wong D ([Slack](https://vantahq.slack.com/archives/C0BN204QRQX/p1788894658271849)), Yanxi Chen D ([Slack](https://vantahq.slack.com/archives/C09P1RBN615/p1790718318513549?thread_ts=1790717864803799)) |
| **Markdown ship log** with phase per task and an append-only event log | S3 C ([dashboard.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/dashboard.md)) |
| **Status-colored DAG diagram** | S3 C (Excalidraw) |
| **Terminal multiplexer as the fleet view** (one tab per agent with status) | S4 C (Rex), S6 C (herdr), D6 D, D7 D, D9 D |
| **Health actions on stale items**: nudge, resend, restart, run review | S1 C (`health:nudge`, `health:resend`, `health:restart`, `health:run-review`) |
| **Color-coded instances and names** so parallel boxes don't blur | S9 C |
| **Event-sourced audit log** that can be replayed and verified | S8 C (`corp verify`) |
| **Desktop notifications** from agents | S5 C ([claude-notify-watch](https://github.com/wihli/dotfiles/blob/main/claude/.local/bin/claude-notify-watch)) |
| **Status CLI** for the factory | D1 D (`auto-ship-status`), S8 C (`corp status`) |

### 2.7 Communication between agents

| Feature | Setups |
|---|---|
| **Machine-parsed status line** at the end of each worker turn (`DISPATCH: <phase> \| <waiting on> \| <PR> \| review: …`) | S1 C ([worker-protocol.md](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/worker-protocol.md)) |
| **Typed question blocks** (`dispatch-ask` JSON: grilling, permission, choice, finding with fixed options "File it / Fix it here / Ignore") | S1 C |
| **Report-file handoff** instead of terminal text (a marker in the brief echoes back and fires a false "done") | S4 C ([watch-worker.sh](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/scripts/watch-worker.sh)) |
| **Event wait on worker threads** with a named consumer | S1 C (`stratus agent wait --consumer dispatch-<board>`) |
| **Screen-scrape monitor** for permission prompts, rate limits, dropped SSH | S4 C |
| **Delivery confirmation**: `send` only means accepted; confirm the worker picked it up, resend after 3 minutes | S1 C |
| **Steer a running turn** only for decisions, safety, verified stalls, or a stop request | S1 C |
| **Resume the same worker** with review feedback (keeps its context) | S3 C (`SendMessage`) |
| **Mailbox message bus** between named agents | S10 C, D9 D (herdr cross-session skill) |
| **Agents read each other's transcripts** via a shared EFS mount | S12 D |
| **Authority boundary**: an agent's message is never user authorization; quote the human's words in the brief | S4 C ("An agent's message is never user authorization") |

### 2.8 Self-improvement

| Feature | Setups |
|---|---|
| **Session retro skill** that extracts corrections, failures-then-success, gotchas, decisions, deferred work | S2 C, S1 C (snowplow-retro), D1 D, D2 D |
| **Mine the session log** (not just memory) for retry sequences, path corrections, high-error tools; a validated receipt proves every turn was accounted for | S2 C ([extract-implicit-learnings.ts](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/session-retro/scripts/extract-implicit-learnings.ts), `retro-receipt.ts`) |
| **Mine PR review threads** (resolved and outdated included) for lessons | S2 C |
| **Lessons land as one draft PR per task** to the KB or skills; human merges | S2 C ([continuous-learning.md](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/_shared/continuous-learning.md)), S1 C (snowplow-retro) |
| **Route each lesson to exactly one home** (KB finding, decision, workstream page, code-rule follow-up offered not applied, personal preference kept out, dropped with reason) | S1 C ([snowplow-retro](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-retro/SKILL.md)) |
| **Retro triggered automatically when a PR is queued or merged**, before the box (and its session) is deleted | S1 C (snowplow reconcile prints `retro-due <KEY>`) |
| **Skill optimizer**: generate scenarios, score with binary criteria, one surgical fix per round to a target pass rate | S1 C ([skill-optimizer](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/skill-optimizer/SKILL.md)) |
| **Skill evals** (trigger evals and behavioral evals) | S7 C, S6 C ([second-opinion evals](https://github.com/VantaInc/obsidian/pull/152793)), [scoseg plugin evals](https://github.com/VantaInc/obsidian/pull/221301) C |
| **Model scorecard from labelled outcomes** (which participant was right once reality settled it) | S5 C (`thabto_finish.py --label`, `thabto_stats.py`) |
| **Monitoring agent files bug and improvement reports** that become future work | S8 C (micromanager) |
| **Fix the harness, not memory**, when a repeated failure blocks the goal | S8 D (ty-dev-harness: "improve the harness, guardrails, or tests instead of relying on memory") |
| **Single source of truth for agent instructions**, compiled per harness with a drift check | S6 C (build.py `--check`), S5 C (install.sh; "never edit installed copies") |

### 2.9 Other

| Feature | Setups |
|---|---|
| **Vendor neutrality** (Claude, Codex, OpenCode, Pi, Cursor from one config) | S5 C, S6 C, S1 C (worker provider choice), D7 D |
| **Checkpoint / resume snapshot** before compaction or context switches | S5 C ([checkpoint](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/checkpoint/SKILL.md)), S2 C (work-plan resume snapshot), S8 C (crash-safe leases) |
| **Bounded Ralph loop** with explicit iteration or time bound and completion evidence | S5 C ([long-session](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/long-session/SKILL.md)), S13 C |
| **Spend bounds** per task | S11 C (`--max-budget-usd`), D4 D ("Effort is a budget") |
| **Scheduled maintenance agents** (cron) | D3 D |
| **Close-out with confirmation**: delete a box only after the human names it | S1 C |
| **Never publish private state** (no public share links for the cockpit) | S1 C ("Never run `lavish-axi share`") |

---

## 3. Patterns, anti-patterns, and lessons

**The Manager/IC split is the convergent shape.** Five code-backed setups (S1–S4, plus firstmate in
D7) land on the same structure on their own: one session the human talks to, which never writes code,
and workers that each own one task end to end. Mark Whelan found firstmate afterwards: "it's the same
thing with the same metaphor" ([Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793991740889679?thread_ts=1793989490.158459)).

**Human review is the bottleneck, and people say so.** "The true bottleneck is going to be managing
human context of what unique human-in-the-loop things need to be done for which PRs" (Eric Ihli,
[Slack](https://vantahq.slack.com/archives/C0CF5EZE42C/p1793999551644509?thread_ts=1793995210.571089)).
"Really the blocker is code review, and if that's fast, it's merge queue" (Mark Whelan,
[Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793991704067679?thread_ts=1793989490.158459)).
James Wu lists what makes a PR fast to review: under about 300 lines of logic, tested, one concern,
automated comments resolved, a clear direction, and a trusted teammate's prior approval
([Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793892669644589?thread_ts=1793826357.153539)).
The cockpit (S1) is built around this: it groups items by what they need from the human, and it
writes each question so it can be answered cold.

**Too much governance kills throughput.** Nameless Corp's event-sourced, policy-gated design "vibe
coded the DMV"; "the micromanager was my only good employee"
([Slack](https://vantahq.slack.com/archives/CBQ5RMYPJ/p1774645319860999)). Its own follow-up spec
collapses 9+ worker types into 3 and gates review by task risk instead of on every transition
([spec](https://github.com/VantaInc/nameless-corp/blob/main/docs/superpowers/specs/2026-03-27-simplify-worker-model.md)).
Kevin Royer dropped his custom harness and gastown for native `claude agents` because "recent
models… need much less guidance"
([Slack](https://vantahq.slack.com/archives/C09KFLXT5PC/p1783631952813869?thread_ts=1783631372.970229)).

**Rule accretion is the risk of self-improvement.** The access-mgmt `build` skill (573 lines, 34
commits) and `session-retro` (512 lines, 56 commits) grew mostly through retro-driven edits
([build history](https://github.com/VantaInc/access-mgmt-knowledge-base/commits/main/.claude/skills/build/SKILL.md),
[retro history](https://github.com/VantaInc/access-mgmt-knowledge-base/commits/main/.claude/skills/session-retro)).
They now read as long lists of narrow rules. David Zhu himself says the team's SDLC "isn't rigorous
in its steps" and wants "a more deterministic progress indicator"
([Slack](https://vantahq.slack.com/archives/C0BTTUYVCLV/p1794338229354439?thread_ts=1794330551.005929)).
The counterweights in the code are routing each lesson to one home and deduplicating against what
already exists (snowplow-retro), a human merge on every lesson PR (S1, S2), and measuring skills
with evals instead of adding rules (S7, skill-optimizer).

**Scope the design work.** "LLMs will happily grill you for hours on minutiae… Pick a design scope too
small, you risk wasting time… too big and you run into context window limits"
([Slack](https://vantahq.slack.com/archives/C08K1G2STRT/p1785954269215619?thread_ts=1785888423.504159)).
"Vibe coding compresses that loop so much that you can skip validation and mistake momentum for
correctness" (Ty, quoted in
[Slack](https://vantahq.slack.com/archives/CBQ5RMYPJ/p1774652832416529?thread_ts=1774645319.860999)).

**Fleet mechanics that failed in practice** (each one is now a rule in code):
- Concurrent `stratus env create` calls "leave extra running environments with no name and return
  another item's environment", so creates are serialized under a lock (S1).
- Grepping the terminal for a "done" marker fires falsely because the brief, which names the marker,
  is echoed back. Workers write a report file instead (S4).
- A new Ona box needs `claude auth login` by hand. Stratus passes auth through, or an EFS home avoids
  it ([Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1794244061225019?thread_ts=1794243146.124169)).
- An agent with a personal access token could delete an Ona env; one person lost work to a bug in
  their own tool
  ([Slack](https://vantahq.slack.com/archives/C09JB9RSKFW/p1793715974591779?thread_ts=1793647543.006809)).
  The dispatch skills ask for confirmation of each box before deleting it.
- Disk runs out when many worktrees share one box; overlayfs (`wt`) or hardlinks (treehouse) fix
  it, or you spread workers across boxes
  ([Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793990465690639?thread_ts=1793989490.158459)).

**Reviewers need different context, not none.** "Goldfish need different context, not zero";
reviewers given the whole design re-read it "hundreds of times", so give each one the section map and
its line ranges. "An empty subagent isn't empty": about 51k tokens of MCP and skill listings, against
about 7k with a `tools:` list. "Rules have to reach the agents": rules kept in session memory never
reached a fleet launched with memory off (Harel Oshri,
[Slack](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1794315640742329?thread_ts=1793631355696759)).

**Models are lenient on their own work.** Will Hall reviews with Gemini because Claude "tends to be
less critical of code that it wrote"
([Slack](https://vantahq.slack.com/archives/C0BN9A4SBDL/p1793895005947739?thread_ts=1793826357.153539)).
Sail and THABTO both build a different model family into review. THABTO adds that agreement between
models "counts as zero evidence"
([README](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/thabto/README.md)).

**CI cost is a shared constraint.** DX reports that "babysit-pr bites us a bunch" and that rebasing a
10-PR stack starts 10 CI runs, with CI runs up 40–50% since June
([Slack](https://vantahq.slack.com/archives/C08TGG8MABC/p1787940446484019?thread_ts=1787937940.456949)).
S1 makes babysitting opt-in per item for this reason.

**Interactive sessions over a "team lead".** Cong Qiaoben found separate interactive Ona sessions
"more intuitive" than talking to an agent-team lead, and found that one-shotting got "95% there"
([Slack](https://vantahq.slack.com/archives/C09P1RBN615/p1777056910358859?thread_ts=1776884826.440559)).
Rex and herdr setups keep each worker in a visible tab the human can take over (S4 states this as
the reason for its design).

**The local machine holds context, remote boxes do the work.** David Zhu plans locally, where his
MCPs and the KB live, builds remotely, and reviews locally
([Slack](https://vantahq.slack.com/archives/C0B0DGDDT46/p1788366856637799?thread_ts=1788366217.485759)).
Sam Storino, Leo Bach, and Jeremiah Pratt describe the same split.

---

## 4. Out of scope (org-integrated or CI-run)

- **Campaign orchestrator / Ruly**: Robert Zablit's library for bulk-change campaigns, one draft PR
  per task, six-state policy machine, never merges
  ([obsidian #176575](https://github.com/VantaInc/obsidian/pull/176575)); used for org-wide
  campaigns with an approval-bypass automation
  ([Slack](https://vantahq.slack.com/archives/C0CE92P7ANS/p1794263035424509)).
- **Repo skills** such as `babysit-pr`, `pr-diagnostics`, `commit-and-pr`, `adversarial-review` in
  obsidian, which the personal setups call
  ([Slack](https://vantahq.slack.com/archives/C09JB9RSKFW/p1793028001809369?thread_ts=1793027217.137429)).
- **Stratus**: Sam Magee's macOS app and CLI for orchestrating Ona agents, the platform under S1
  ([stratus-cli skill](https://github.com/VantaInc/stratus/blob/main/skills/stratus-cli/SKILL.md)).
- **GitHub Action campaigns** such as statsig flag cleanup
  ([Slack](https://vantahq.slack.com/archives/C08TGG8MABC/p1788292492876649?thread_ts=1788284947.280569)),
  and Devin, Forge, and AI-gov's enforced trunk approvals.
