# Agentic setups: one feature taxonomy, mapped onto flow

Researched 2026-11-10. This note merges two surveys into one list of features, grouped by
category. For each feature it gives how common it is, the best code-backed example, how it maps
onto flow, and whether it fits the user's constraints. It is input for choosing a steel thread.
It does not choose one.

## Sources

- **[V]** Vanta survey: `docs/research/vanta-agentic-setups.md` on branch
  [`research/vanta-agentic-setups`](https://github.com/weston-vanta/flow/blob/research/vanta-agentic-setups/docs/research/vanta-agentic-setups.md).
  Setup ids `S1`–`S13` (code-backed) and `D1`–`D9` (description only) are that note's.
- **[I]** Industry survey: `docs/research/industry-agentic-setups.md` on branch
  [`research/industry-agentic-setups`](https://github.com/weston-vanta/flow/blob/research/industry-agentic-setups/docs/research/industry-agentic-setups.md).
- **[L]** Launch facts: `docs/research/launching-a-session.md` on branch
  [`research/launching-a-session`](https://github.com/weston-vanta/flow/blob/research/launching-a-session/docs/research/launching-a-session.md).
- **[flow]** flow v2 on local `main` (`9986d69`): `GLOSSARY.md`, `docs/adr/0001`–`0003`,
  `src/session.rs`, `src/branch.rs`, `src/github.rs`, and `flow guide`.
- Primary sources re-read for this note, where the two surveys were thin or disagreed:
  - [Claude Code cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging) **[cc-xsm]**
  - [Claude Code agent view](https://code.claude.com/docs/en/agent-view) **[cc-av]**
  - [snowplow-dispatch `worker-protocol.md`](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/worker-protocol.md) (S1) **[S1-wp]**
  - mattpocock [`software-factory`](https://github.com/mattpocock/skills/blob/main/skills/in-progress/software-factory/SKILL.md) **[mp-sf]** and [`chief-of-staff`](https://github.com/mattpocock/skills/blob/main/skills/in-progress/chief-of-staff/SKILL.md) **[mp-cos]**

## How to read the tables

**Count** is `Vanta / Industry`: how many setups in each survey have the feature. Vanta counts
both code-backed and description-only setups. Industry counts named projects; "many" means the
survey says "nearly all" or lists more than eight. A count measures how often people built the
feature, not whether it worked.

**Example** is the best code-backed instance: a Vanta setup with code if there is one, then an
industry project with code, then a description.

**flow** says how the feature maps onto what flow has today, in these terms:

- **exists**: flow already has it.
- **skill**: lives in the IC's or Manager's skills and instructions; flow needs nothing.
- **Facet**: a new Facet on an existing Session, PR or Branch record, with its Writer named.
- **Writer**: a new Writer process.
- **action**: a path from the web app back to a Session. flow has none today: `flowd` serves
  reads only (ADR 0003).
- **launch**: needs flow (or a skill) to start a Session; see [L].
- **outside**: belongs to another tool (herdr, Claude Code, GitHub, `wt`).

**Fit** checks the user's three constraints: personal (one Director, not a team or org), the
Session as the only primitive, and Managers never woken by IC activity.

- **fits**: works as is.
- **adapt**: fits once reshaped; the cell says how.
- **conflicts**: breaks a constraint; the cell says which.
- **out**: outside the scope of a personal factory, or of flow.

### What flow has today

From [flow]:

| Record | Facets (Writer) |
| --- | --- |
| Session | `origin` (CDE id, cwd, herdr pane and socket), `status` (running, waiting for you, idle, done), `context` (title), `activity` (last request and reply). Writer: `flow hook`, run by each Session's Claude Code hooks (ADR 0001). |
| Branch | `reporter`: the Session that last ran `flow wt create` for it, which makes that Session the Owner of the Branch's PRs (ADR 0002). Writer: `flow wt create`. |
| PR | `github`: state, `isDraft`, head and base refs, `mergeable`, `reviewDecision`, size, `stackEntry` position, and the check rollup. Writer: the PR poller in `flowd`. |

Readers: the web app, through `flowd`'s read API, and agents, through the `flow` CLI. The Store
holds current state only, not history. "Director", "Manager", "IC" and "Task" are not yet in
`GLOSSARY.md`.

---

## 1. Defining work

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Grill the human before coding**, in batches, with a recommendation for each question | 4 / 6 | S1: the worker asks "your grilling questions in one turn, with your recommendation for each", as a typed JSON block [S1-wp] | skill (the user's kit already has `grill-with-docs`) | fits, if the IC grills the Director directly in its own pane. S1 relays grilling through its orchestrator, which the constraints rule out. |
| **Self-contained questions**, answerable cold without opening the ticket, box or chat | 1 / 2 | S1: "the human must be able to answer it without opening the ticket, this box, or earlier chat" [S1-wp]; mattpocock `loop-me` "brief" | skill | fits |
| **Written spec as an artifact**, then tickets from it | 2 / 7 | mattpocock `to-spec` → `to-tickets` (code); S12 and D2 describe the same chain | skill (the repo's `.scratch/<feature>/` tracker) | fits |
| **Task DAG** with dependencies, files, and command-checkable "done when" | 3 / 5 | S3 Set a Course ([SKILL.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/SKILL.md), PR); Beads `bd ready` | skill. flow has no Task record. | adapt: the Manager defines Tasks in tickets. If the dashboard should show a Task, flow needs a Task Facet on the IC's Session (see §10). |
| **Design contract** (locked decisions, open questions, rejected approaches) or glossary plus ADRs | 2 / 1 | S5 [`north-star`](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/north-star/SKILL.md); mattpocock `grill-with-docs` | exists as a practice in this repo (`GLOSSARY.md`, `docs/adr/`) | fits |
| **Repo-owned workflow policy** (routing a ticket to a workstream playbook, workflow templates) | 2 / 4 | S1 snowplow routing ([SKILL.md](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/SKILL.md)); Symphony `WORKFLOW.md` | skill | fits |
| **Bounded implementation window**: at most N unmerged dependent PRs before human feedback | 1 / 0 | S2 [`build`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/build/SKILL.md): "up to three unmerged dependent PRs" | skill. The Stack and the `github` Facet already show the count. | fits |
| **Outside-in layer ordering** of a Stack | 1 / 0 | S2 `build` | skill | fits |
| **Prove the gap** before building | 1 / 0 | S6 [`prove-the-gap`](https://github.com/YanxiChen-gh/Dotfiles/blob/main/shared-skills/prove-the-gap/SKILL.md) | skill | fits |
| **Bite-sized tasks** with exact paths and verification | 1 / 2 | superpowers `writing-plans` (2 to 5 minutes each) | skill | fits |
| **Tickets from a tracker** (local board, Jira, Linear, GitHub) | 2 / 6 | S11 Paperclip ticket store ([obsidian #141670](https://github.com/VantaInc/obsidian/pull/141670), closed PR); Symphony | skill today. A Writer if the dashboard should show tickets. | fits |
| **Goal ancestry**: each task carries its "why" | 0 / 1 | Paperclip (goal → project → task) | skill (a pointer to the spec in the brief) | fits |

## 2. Dispatch and delegation

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **The coordinator never writes code**; it only defines, delegates, tracks | 5 / 7 | S3 Captain: "You never write code"; S1, S2, S4 say the same in code | skill. "Manager" is a role a Session plays, as "Owner" is (glossary). | fits |
| **Workers are separate interactive Sessions** in visible panes | 4 / 5 | S4 [`rex-ona-workers`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/SKILL.md) (one Rex tab per worker, each SSH'd into its own CDE); Gas Town polecats; herdr | launch. flow already records each one as a Session. | fits. This is the user's IC. |
| **Workers are subagents** of the coordinator | 5 / 4 | S3 worker subagent per task; S12 `supervise`; superpowers SDD; mattpocock `implement-spec` | outside (Claude Code) | conflicts for ICs (an IC is a Session, not a subagent). Fits inside an IC, for reviewers and scouts. |
| **Launch a worker Session programmatically** | 5 / many | S1 (Stratus CLI), S4 (Rex plus ssh), S6 [`new-agent-tab.sh`](https://github.com/YanxiChen-gh/Dotfiles/blob/main/herdr/new-agent-tab.sh) (herdr); Gas Town `gt sling` | launch. flow v2 has no launch command; v1's `flow dispatch` failed live on timing and the trust dialog [L §7]. | fits. Facts in §9. |
| **Self-contained brief template** (task, decisions quoted, authorizations quoted with date, hard limits, report contract) | 3 / 1 | S4 [`worker-brief.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/references/worker-brief.md); S5 `delegate`: "The child cannot see this conversation" | skill. Delivered as the first prompt at launch [L]. | fits |
| **Per-task model and effort choice** | 2 / 4 | S1 `board prefs` and `resolve-model.ts` | launch (`--model`, `--effort` [L §4]) | fits |
| **Concurrency cap** on workers | 2 / 3 | S3: "at most 3 at once and queue the rest"; Gas Town `max_polecats` | skill | fits |
| **Confirm the batch once** in a table before launching | 1 / 0 | S1 dispatch | skill | fits |
| **Saved role library** (reviewer, simplifier, debugger) loaded by path | 2 / 2 | S5 [`delegate`](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/delegate/SKILL.md) | skill | fits |
| **Restart a lost worker** with the brief plus "decisions so far" | 1 / 2 | S1 dispatch | launch (`claude --resume`, or a new Session with the brief) | fits |
| **Manager triages worker questions**: answers factual ones, escalates product and scope ones | 1 / 2 | S1 `board auto … --basis factual\|procedural\|decided-earlier`; Agent Deck Conductor | none | conflicts: the Manager must be woken by each IC question. The alternative is that the IC asks the Director, and the dashboard shows the question (§6, §7). |
| **Event triggers wake a supervisor** (worker event, GitHub event, email) | 2 / 3 | S1 `stratus agent wait --consumer`; S4 [`watch-worker.sh`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/scripts/watch-worker.sh); Agent Deck watchers | none | conflicts (Manager wake-ups) |
| **Poll-and-reconcile daemon**, retry with backoff | 2 / 3 | S1 board reconcile against `stratus env list`; Symphony SPEC | Writer (`flowd` already polls GitHub) | adapt: a reconciler may write facts and flag Sessions for the Director, but must not prompt the Manager. Auto-retry is an accelerator (§11). |
| **Pull-based claiming** with locks or leases | 2 / 4 | S8 leases; Beads `--claim` | none. Ownership is "most recent `flow wt create` wins" (ADR 0002). | out: with one Manager, push assignment is enough. |
| **Schedules and recurring runs** | 1 / 6 | D3 cron on Ona (description); Orca automations, CC routines | outside (cron, CC routines). A scheduled run in a herdr pane shows up as a Session. | fits, later |
| **Bulk campaign mode** (one PR per item over a long list) | 1 / 2 | S11 (`claude -p --max-budget-usd` per ticket) | skill | out for now |

## 3. Isolation and compute

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Worktree per worker** | 5 / many | S12 `wt`; treehouse (S6); CC agent view, automatically [cc-av] | exists: `flow wt create`, which also records the Owner (ADR 0002) | fits. This is the ownership mechanism. |
| **Cheap worktrees** (overlayfs, hardlinked `node_modules`, warm pool) | 3 / 0 | S12 `wt` overlay layers; S6 [`treehouse/config.toml`](https://github.com/YanxiChen-gh/Dotfiles/blob/main/treehouse/config.toml) | outside (`wt`). A repo needs a `wt` base first; neither `/workspaces/flow.wt` nor `/workspaces/obsidian.wt` exists on the Flow CDE [L §6]. | fits |
| **Setup hooks for new worktrees** (env, deps, ports) | 1 / 4 | workmux `post_create`; S2 paired code plus KB worktrees | outside (`wt`) | fits |
| **One CDE per task**, named after it | 4 / 5 | S1, S4 (code); container-use, Agent of Empires | exists for visibility: `origin.cdeId` per Session. Launching onto another CDE needs ssh aliases, which only a CDE with the user's `gitpod login` has [L §3]. | adapt: the user runs 3–4 long-lived CDEs (S12), not one per task. |
| **Ledger of which environment ran which task** | 2 / 0 | S4 append-only `ledger.jsonl` | exists in effect: Session `origin` plus Branch `reporter` | fits |
| **Shared home over EFS** for skills, memory and docs | 3 / 0 | D1, D6, S12 (descriptions) | exists: the Store and the `flow` binary live on `/efs-home`. The Primary CDE's `/efs-home` is a different filesystem [L §3]. | fits |
| **Escape check**: the worker did not edit the main checkout | 1 / 0 | S3 run-voyage ("If a worker escaped its worktree, restore… and restart") | skill | fits |
| **Safe cleanup** that refuses to lose unpushed work, or asks first | 1 / 2 | S1 close-out with confirmation; CC `claude rm` keeps a dirty worktree | outside (`wt remove`), skill | fits |
| **Serialized environment creation** under a lock | 1 / 0 | S1 `board lock -- stratus env create` | none | out unless flow creates CDEs |
| **No isolation; partition files instead** | 0 / 2 | CC agent teams; CCPM | none | conflicts with ADR 0002 (ownership comes from creating a worktree) |

## 4. Quality loops (review and bar-raising inside the IC)

All of these run inside the IC, as the user wants. flow's part is to make their state visible.

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Deterministic backpressure** (tests, types, lint must pass) | 2 / 6 | S3 "done when" commands; Ralph; Archon `until_bash` | skill | fits |
| **Adversarial self-review** before the PR, gating QA | 2 / 1 | S1: "switch to a skeptic's stance, try to break your own change… QA starts only after it" [S1-wp]; addyosmani `doubt-driven-development` | skill. The review state could be a field of a self-report Facet (§7). | fits |
| **Fresh-context reviewer**, read-only, given the worktree path | 3 / 2 | S3 [`evaluator.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/evaluator.md); S6 [second-opinion](https://github.com/VantaInc/obsidian/pull/152793) | skill (a subagent inside the IC) | fits |
| **Parallel multi-lens review** (standards, spec, security, frontend) | 1 / 4 | S3 (Opus, Codex, frontend reviewers); mattpocock `code-review` (this repo's `code-review` skill) | skill | fits |
| **Cross-model-family review** (Codex or Gemini reviewing Claude) | 5 / 1 | S3 `codex exec -s read-only`; S5 [`thabto.py`](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/thabto/scripts/thabto.py) | skill. Needs the other CLI and its auth on each CDE. | fits |
| **Weighing findings**: agreement as signal, or severity and confidence gates | 2 / 2 | S3 ("agreement across model families is the strongest signal"); S5 verdict (`supported`/`rejected`/`unresolved`); Compound P0–P3 | skill | fits. The sources disagree (§12). |
| **Same worker fixes**, triaging each finding as fix or decline with a reason | 1 / 1 | S3 [`fix-round.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/fix-round.md) | skill | fits |
| **Simplify pass** before review | 1 / 3 | S13 (code-simplifier after each iteration); Compound `ce-simplify-code` | skill (this repo's `simplify`) | fits |
| **Reviewers get different context, not none** (section map, line ranges, a `tools:` list) | 2 / 0 | D4, D5 (descriptions) | skill | fits |
| **Evidence for "done"** (command, exit code, output; proof-of-work bundle) | 4 / 3 | S6 [`full-verification-workflow`](https://github.com/YanxiChen-gh/Dotfiles/blob/main/shared-skills/full-verification-workflow/SKILL.md); S8 policy-gated evidence; Symphony | skill (in the PR body). Facet if the dashboard should link it. | fits |
| **Verify against the running system** (browser, real service), not only CI | 1 / 5 | Anthropic harness (Puppeteer); D4: "All four of the broken bot PRs passed CI" | skill | fits |
| **Hook-enforced finish gates** (Stop or TaskCompleted hook blocks until checks pass) | 1 / 3 | CC `TaskCompleted` (exit 2 blocks); `ralph-loop` Stop hook; S1 cockpit sends the worker back | skill (a hook in the IC's settings). flow's own hooks only observe. | fits |
| **Bounded retry loops**, with explicit iteration or time caps | 2 / 3 | S5 [`long-session`](https://github.com/wihli/dotfiles/blob/main/skills/.local/share/skills/long-session/SKILL.md); S13 ralph-loop; Archon `max_iterations` | skill | fits |
| **Reviewer trained on team standards** or review history | 2 / 2 | S2 retro reads every review thread; Compound `REVIEW.md`; mattpocock `CODING_STANDARDS.md` | skill | fits |
| **Human review gate per task**, or an `autonomous` mode reviewed as a whole Stack | 1 / 2 | S3 (diff copied into the main checkout for IDE review); mattpocock `loop-me` ("push right") | exists: the Director reviews each Stack's PRs on GitHub, found through the dashboard | fits |
| **Human review coverage audit**: every landed change traces to a human-reviewed revision | 1 / 0 | S4 [`review-coverage.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/agent-trunk-feature-development/references/review-coverage.md) | skill | fits, for trunk workflows |

## 5. PR handling

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Draft PRs only; never mark ready, request reviewers or merge** | 4 / 1 | S1: "Open draft PRs only… Never mark a PR ready, request reviewers, or merge" [S1-wp]; S3; S4 | skill. The `github` Facet already has `isDraft`. | fits |
| **Stacked PRs** as a DAG from task dependencies | 2 / 2 | S3 dashboard (Base column `stacked on #N`); gh-stack | exists: a Session's Stack, ordered by `stackEntry`. [I] found no Session manager that shows a Stack per Session; CC agent view shows a count such as `3 PRs` [cc-av]. | fits. flow's distinctive feature. |
| **PR state polling** (merged, closed, CI, review) | 1 / 3 | S1 snowplow `--github-every` 5 minutes; CC agent view | exists: `flowd`'s poller (`state`, `reviewDecision`, `mergeable`, checks) | fits |
| **Babysit to green** (opt-in per item, or always) | 4 / 4 | S1 opt-in: "Only a message starting 'Babysit PR' turns this on" [S1-wp]; Compound `ce-babysit-pr` | skill (in the IC) | fits. The IC is the one kept busy, not the Manager. |
| **Route CI failures and review comments to the owning agent** | 0 / 4 | Orchestrator.inc ("back to the worker that owns the branch"); Orca | Unique to flow: it already knows every PR's Owner (ADR 0002). Delivering needs an action, or the IC polling `flow pr get`. | adapt: deliver to the IC (which owns the PR), never to the Manager. |
| **Conflict watch** across in-flight PRs | 1 / 1 | D7 Devin coordinator (description); Orchestrator.inc | exists in part: `mergeable` in the `github` Facet | fits |
| **Trunk or aggregate PR** with sync bots | 2 / 0 | S4 [`agent-trunk-feature-development`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/agent-trunk-feature-development/SKILL.md); D3 | skill. Child PRs get Owners the same way. | fits, optional |
| **PR description discipline** (short, for humans) | 2 / 0 | S7 [`crafting-prs`](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/crafting-prs/SKILL.md) with evals | skill (this repo's `pr`) | fits |
| **Re-plan commit history** onto a fresh branch | 1 / 0 | S7 [`replanning-branches`](https://github.com/zabrador/dotfiles/blob/master/ai/plugin/skills/replanning-branches/SKILL.md) | skill. A new branch needs `flow wt create` to keep its Owner. | fits |
| **Inline diff comments sent back to the agent** | 0 / 3 | vibe-kanban, Orca, Conductor | action | adapt (GitHub review comments plus the routing above) |
| **Merge queue**, or an integration branch with a merger agent | 0 / 2 | Gas Town Refinery; mattpocock `implement-spec` | none | out (the Director merges) |

## 6. Visibility (the Director's single pane of glass)

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **One view across machines** | 1 / 3 | flow itself (S12's author); herdr multi-machine; CC Remote Control and Projects | exists: flow's core | fits |
| **PR and CI state on each Session row** | 2 / 3 | S1 cockpit; CC agent view `#N` badge coloured by checks and review [cc-av] | exists: Stack with the `github` Facet | fits |
| **Grouped by what needs the Director** ("Waiting on you", Running, Done) | 3 / 6 | S1 [`cockpit.js`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/cockpit/cockpit.js); CC agent view puts "Ready for review" (open PR needing review or with failing checks) and "Needs input" above Working [cc-av] | exists in part: `status` has "waiting for you" from permission prompts and notifications. "Ready for review" can be derived from the `github` Facet. A question the IC asks in plain text is not detected. | fits. Grouping is a Reader change; the gap is the self-report (§7). |
| **Two lines per item** ("what this is", "where it stands") for a Director returning cold | 1 / 1 | S1 cockpit; CC agent view's Haiku-generated summary | exists in part: `context.title`, `activity`. The glossary rules out generated summaries ("It is what the Session itself said"). | adapt: a self-reported line from the IC, not a generated summary |
| **Interactive page that sends answers back** to the agent | 3 / 6 | S1 cockpit through Lavish; vibe-kanban and Orca diff notes | action. `flowd` serves reads only (ADR 0003). The Session's `origin` already holds pane, herdr socket and CDE, which a `herdr agent prompt` (over ssh for another CDE) would need [L §2–3]. | fits. This is the main missing component. |
| **Terminal multiplexer as the fleet view** | 5 / 4 | S6 herdr; S4 Rex | outside (herdr). flow sits above it. | fits |
| **Health actions on stale items** (nudge, resend, restart, stuck detection) | 1 / 3 | S1 `health:nudge`, `health:restart`; Gas Town Witness | Writer: the `liveness` Writer that ADR 0001 names as the fix for crashed or Esc'd Sessions, plus an action | adapt: flag stale ICs for the Director; never wake the Manager |
| **Desktop or phone notifications** | 1 / 4 | S5 [`claude-notify-watch`](https://github.com/wihli/dotfiles/blob/main/claude/.local/bin/claude-notify-watch); cmux attention rings | Reader (a notifier on status changes) | fits |
| **Status CLI or JSON of all sessions** | 2 / 3 | S8 `corp status`; `claude agents --json` | exists: `flow session list`, `flow pr list` | fits |
| **Named, colour-coded instances** | 2 / 0 | S9 [herd](https://github.com/VantaInc/herd/blob/main/README.md); S11 (naming the box after its task) | launch (`--name` becomes `context.title` [L §4]) | fits |
| **Ship log or event stream** per task | 2 / 3 | S3 [`dashboard.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/dashboard.md); disler hooks dashboard | none. The Store holds current state only. | adapt: needs a history store, which the Store's design excludes |
| **Status-coloured DAG diagram** | 1 / 0 | S3 (Excalidraw) | Reader | fits, later |
| **Cost and usage** tracking, budgets | 1 / 3 | S11 `--max-budget-usd`; Paperclip budgets | Facet (cost per Session, from the hook or transcript) | fits, later |
| **Event-sourced, replayable audit log** | 1 / 1 | S8 `corp verify` (its author calls the design a failure) | none | out (S8's lesson, §11) |
| **Mobile and remote steering** | 0 / 6 | Orca, Orchestrator.inc, CC Remote Control | outside | out |

## 7. Communication between agents (and with the Director)

The user's constraint, "Managers are not woken by IC activity", decides most rows here. Claude
Code's own messaging does wake a Manager: "When the receiving session is idle, Claude Code starts
a new turn with the message", and a `notify_when_idle` notice likewise starts a new turn in an
idle asking session [cc-xsm]. Neither survey said this.

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Machine-parsed status line** at the end of each worker turn | 1 / 1 | S1: `DISPATCH: <phase> \| <waiting on> \| <draft PR url or none> \| review: <done\|pending\|skipped>`, with fixed phase and waiting-on vocabularies [S1-wp]; Anthropic's JSON feature list | Facet: a self-report Facet on the Session, written by the IC through a `flow` command. ADR 0001 anticipates this ("the same path agents will later use to report statuses of their own"). | fits: the dashboard reads it and nobody is woken |
| **Typed question blocks** (grilling, permission, choice, finding with fixed options) | 1 / 2 | S1 `dispatch-ask` JSON; finding options are exactly "File it / Fix it here / Ignore" [S1-wp]; Archon `wait` attention item | Facet (part of the self-report) plus an action for the answer | adapt: route to the Director, not the Manager |
| **Report-file handoff** instead of terminal text | 1 / 1 | S4: grepping for a "done" marker fired falsely because the brief, which names it, is echoed back | Facet (replaces the file) | fits |
| **Manager waits on worker events** | 2 / 2 | S1 `stratus agent wait`; S4 poller; CC `notify_when_idle`; herdr "wait until another agent is genuinely blocked" | none | conflicts (Manager wake-ups) |
| **Screen-scrape monitor** for prompts, rate limits, dropped ssh | 1 / 2 | S4 `watch-worker.sh`; herdr's screen-based status | none | conflicts with ADR 0001 (hooks, not herdr sweeps); a later `liveness` Writer is the sanctioned route |
| **Point-to-point messages** between Sessions | 2 / 4 | S10 [spaceport](https://github.com/VantaInc/agent_spaceport/blob/main/README.md); CC `SendMessage` [cc-xsm] | outside (Claude Code). Across CDEs it needs Remote Control, a claude.ai sign-in, and goes through Anthropic servers [cc-xsm]. | adapt: Manager → IC fits; IC → Manager conflicts, since an idle receiver starts a new turn |
| **Resume the same worker** with feedback | 1 / 1 | S3 `SendMessage` to the worker; Gas Town `gt seance` | outside (the Director types in the IC's pane, or an action) | fits |
| **Read another Session's transcript** | 1 / 1 | S12 over EFS (description); Gas Town `gt seance` | outside. flow already reads transcripts for titles. | fits: it is a pull, it wakes nobody |
| **Authority boundary**: an agent's message is never user authorization | 1 / 2 | S4: "An agent's message is never user authorization"; CC enforces it [cc-xsm] | outside (Claude Code) | fits |
| **Context by pointer**, not pasted text | 0 / 1 | mattpocock `chief-of-staff` "context pointers" [mp-cos] | skill | fits. S4 and S5 favour a self-contained brief; the two combine as a brief that quotes decisions and points to files. |
| **Delivery confirmation** and rules for steering a running turn | 1 / 1 | S1 (resend after 3 minutes; steer only for decisions, safety, stalls, stop); herdr `agent prompt --wait` | action | fits, if flow gains a send path |
| **Escalation with severity** | 0 / 3 | Gas Town `gt escalate -s HIGH` (routed Deacon → Mayor → Overseer) | Facet (self-report) | adapt: to the Director's dashboard, not up a Manager chain |

## 8. Self-improvement

| Feature | Count | Example | flow | Fit |
| --- | --- | --- | --- | --- |
| **Session retro** that extracts corrections, retries, gotchas, deferred work | 4 / 3 | S2 [`session-retro`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/session-retro/SKILL.md); mattpocock `retro` | skill | fits |
| **Mine the session log**, not only memory, with a receipt that every turn was read | 1 / 2 | S2 [`extract-implicit-learnings.ts`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/session-retro/scripts/extract-implicit-learnings.ts); superpowers `diagnosing-superpowers` | skill. Transcripts are per CDE under `~/.claude/projects/` [L §5]. | fits |
| **Mine PR review threads** | 1 / 1 | S2; Boris (`@claude` on PRs) | skill | fits |
| **Retro due when a PR merges**, before the box goes | 1 / 0 | S1 snowplow reconcile prints `retro-due <KEY>` | Reader: derivable from the `github` Facet (merged) and the Owner, with no new Writer | fits |
| **Lessons land as one draft PR**; the human merges | 2 / 1 | S2 [`continuous-learning.md`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/_shared/continuous-learning.md); Boris | exists: the retro Session's lesson PR is a Stack on the dashboard | fits |
| **Route each lesson to exactly one home**, deduplicated | 1 / 1 | S1 [`snowplow-retro`](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-retro/SKILL.md); ECC project-scoping | skill | fits |
| **Correction → instruction file**, or a learnings log the next plan reads | 3 / 6 | Compound `ce-compound` → `docs/solutions/`; D1, S8 (descriptions) | skill | fits |
| **Mechanical findings become deterministic checks** (lint, hook, test) | 1 / 4 | mattpocock `retro` ("a mechanical one … gets a deterministic check, full stop"); CC `hookify` | skill | fits |
| **Skill evals** (does it trigger, does it help) and an optimizer loop | 4 / 3 | S7 [`run-evals.sh`](https://github.com/zabrador/dotfiles/blob/master/ai/evals/claude/run-evals.sh); S1 [`skill-optimizer`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/skill-optimizer/SKILL.md); anthropics `skill-creator` | skill | fits |
| **Retro sampling**: about one in five delegated Sessions, worst first | 0 / 1 | mattpocock `chief-of-staff` [mp-cos] | skill. A Manager could pick from `flow session list`. | fits: the Manager picks when the Director asks, not on IC events |
| **Monitoring agent files improvement reports** | 1 / 2 | S8 micromanager ("my only good employee"); OpenAI doc-gardening (search excerpt) | skill (a scheduled Session; its PRs show as a Stack) | fits |
| **Per-agent or per-model scorecard** from labelled outcomes | 1 / 2 | S5 `thabto_stats.py` | Facet or Writer (outcomes per Session) | fits, later |
| **Hook-observed "instincts"** clustered into skills | 0 / 1 | ECC continuous-learning-v2 | skill | fits, later |
| **One instruction source compiled per harness** | 2 / 1 | S6 [`agent-rules/build.py`](https://github.com/YanxiChen-gh/Dotfiles/blob/main/agent-rules/build.py) (`--check`) | outside | out (flow is Claude Code-only by ADR 0001) |

## 9. Launching a Session (facts from [L])

These bear on every dispatch row above. flow v2 has no launch command.

- **Making the pane**: `herdr workspace create`, `tab create` or `pane split`, each with
  `--cwd`, `--env` and `--no-focus`, return the new pane id. A herdr server keeps the
  environment of whoever started it, and every later pane inherits it; v1 lost every hook to a
  leaked `FLOW_STATE` this way.
- **Starting Claude**: `herdr agent start <name> --kind claude --pane <id> -- <args>` quotes
  the arguments and returns only when Claude is ready. It fails fast with `agent_not_ready` on a
  startup dialog, and rejects any argument with a newline. `herdr pane run` gives a full shell
  (`claude < brief.md` worked in a trial) but returns before anything runs. v1 failed 11 of 11
  live starts by waiting too early.
- **The brief**: a one-line prompt argument (21 KB worked), stdin redirection (worked, but is
  undocumented for interactive mode), a short "read this file" prompt (v1's choice; Activity then
  shows that line, not the brief), or `--append-system-prompt-file`.
- **Knowing the IC**: `claude --session-id <uuid>` fixes the id before launch, so a launcher can
  write facts about the IC (its Task, its Manager) before the IC's first hook runs.
- **Proof the IC took ownership**: only a Branch whose `reporter` is the IC's id shows that it
  ran `flow wt create`.
- **Main failure**: the workspace trust dialog. Trust is keyed on the git repository root, and
  for a worktree on the main checkout's root, so an IC started in a trusted checkout gets none.
- **Another CDE**: ssh needs `gitpod environment ssh-config` aliases, which only a CDE with the
  user's `gitpod login` has. `ONA_ID` is unset in ssh shells, so a herdr server started over
  ssh would give its Sessions no `cdeId` (inferred, not tried). Nothing remote was tried.

## 10. What a factory would need that flow lacks

Collected from the "flow" column. These are options, not a plan.

| Gap | Rows that need it | Shape it would take |
| --- | --- | --- |
| **A self-report from the IC** (phase, waiting on, question, review state, evidence) | §6 grouping and two-line context; §7 status line, typed questions, escalation; §4 review state | A new Session Facet, written by the IC through a new `flow` command. One Writer, the Session itself, as ADR 0001 anticipates. |
| **A Director → Session path** (answer a question, nudge, "babysit PR", resume) | §6 interactive page, health actions; §5 routing feedback to the Owner; §7 delivery | An action in `flowd` and the web app, which today are read-only (ADR 0003). The target pane is known from `origin`; another CDE needs ssh [L]. |
| **Launching an IC** | §2 launch, restart, model choice | A `flow` command, or a Manager skill wrapping herdr. Facts in §9. |
| **Linking an IC to its Task and Manager** | §1 Task DAG; §6 grouping by Task | A Facet on the IC's Session written at launch, keyed by a `--session-id` chosen up front. Keeps the Session as the only primitive. |
| **Liveness** (crashed, Esc'd, stalled ICs) | §6 health actions | The `liveness` Writer on its own Facet, which ADR 0001 already names. |
| **History** (ship log, event stream) | §6 ship log | Outside the Store as designed; the only row that would change a core decision. |

Everything else in the tables is either already in flow or lives in skills.

## 11. Anti-patterns and lessons

**The Manager/IC split is the convergent shape.** S1–S4 and firstmate (D7) arrived at it
separately: one session the human talks to, which never writes code, and workers that each own
one task. Gas Town is the industry's fullest version [V §3; I §3]. Most Managers in both surveys
are woken by their workers and relay questions (S1, S4, Agent Deck, Gas Town); the user's
constraint departs from that. The closest published match for "the human re-enters through the
board, not through a Manager" is mattpocock's factory, where "a run never waits for a human: it
ends, and the human's later change to the ticket tracker or git host triggers a new run" [mp-sf].

**Human review is the bottleneck.** Willison can land "one significant change at a time"; Ihli
and Whelan say the same at Vanta [V §3; I §3]. What helps: decision-ready questions answerable
cold (S1, `loop-me`), grouping by what needs the human (S1, CC agent view), small PRs (about 300
lines in James Wu's list, about 100 in addyosmani's), and Stacks.

**Brakes before accelerators.** "Every trigger is an accelerator. Accelerators without brakes
make a slop cannon." The brakes are automated checks, automated review and human review. Add
triggers one at a time, throttled, and "cap every loop of runs at a few tries before a human"
[mp-sf]. Runaway loops cost money (Paperclip), and CC advises 3 to 5 teammates [I §3].

**Too much governance kills throughput.** Nameless Corp (S8), event-sourced and policy-gated on
every transition, "vibe coded the DMV"; its follow-up spec cut 9+ worker types to 3 and gated
review by risk. Kevin Royer (D1) dropped his harness for native `claude agents` because newer
models "need much less guidance". mattpocock says process-owning frameworks "take away your
control and make bugs in the process hard to resolve" [V §3; I §3].

**Self-improvement accretes rules.** S2's `build` (573 lines) and `session-retro` (512 lines)
grew mostly through retro edits into long lists of narrow rules [V §3]. OpenAI cut a large
`AGENTS.md` that "crowded out the task" to about 100 lines of pointers (search excerpt), and
mattpocock uses `CLAUDE.md` "only for navigation pointers" [I §3]. Counterweights found in code:
one home per lesson with deduplication (S1), a human merge on every lesson PR (S1, S2), mechanical
lessons turned into deterministic checks (mattpocock `retro`), and evals with a baseline, the only
level that measures whether a change helped (S7, `skill-creator`) [V §3; I §3].

**Models are lenient on their own work, and reviewers need different context, not none.** Claude
"tends to be less critical of code that it wrote" (Will Hall). Reviewers given the whole design
re-read it "hundreds of times"; give them the section map and line ranges. "An empty subagent
isn't empty": about 51k tokens of listings against about 7k with a `tools:` list. Rules kept in
session memory never reached a fleet launched with memory off (Harel Oshri) [V §3].

**Unverified "done".** Agents declare victory early, mark features done without end-to-end
tests (Anthropic), and pass CI while broken in the running service (D4). The lead "can stop
early too", and task status lags (CC agent teams) [V §3; I §3]. Fix: evidence requirements and
command-checkable "done when" (§4).

**Work state belongs outside the agent.** Gas Town, Symphony, Anthropic and mattpocock keep the
session disposable and the ledger durable; JSON resists stray edits better than Markdown
(Anthropic) [I §3]. flow's Store, GitHub and `.scratch/` tickets already play this part.

**Fleet mechanics that failed in practice**, each now a rule in someone's code [V §3; L §7]:

- Concurrent environment creates returned the wrong environment (S1: serialize under a lock).
- Grepping a terminal for a "done" marker fired on the echoed brief (S4: write a report file).
- v1 `flow dispatch` waited for the agent before herdr saw it (11 of 11 starts failed), sat on
  `$HOME` trust dialogs after a bad path, and lost every hook to a leaked server environment.
- New CDEs need `claude auth login` by hand unless auth is passed through or home is on EFS.
- An agent with an Ona token can delete an environment; one person lost work to their own tool
  (S1 confirms each delete).
- Many worktrees on one box run out of disk (overlayfs, hardlinks, or more boxes).
- Remote `herdr agent wait` calls with 1-hour timeouts piled up over multiplexed ssh; killing the
  local ssh does not end the remote command (v1 F10).

**Other failure modes reported:**

- Parallel agents editing the same file overwrite each other (CC agent teams, which do not
  isolate in worktrees) [I §3].
- Ralph-style loops duplicate work through nondeterminism; "don't assume not implemented" [I §3].
- Naive PR watch loops wait for CI before reading comments, declare readiness before review
  lands, or reimplement feedback handling (`ce-babysit-pr`) [I §3]. Babysitting costs shared CI:
  rebasing a 10-PR stack starts 10 runs, and CI runs rose 40–50% (Vanta DX), which is why S1 makes
  it opt-in [V §3].
- Trust leaks: a peer's "approval" treated as consent. Claude Code refuses it [cc-xsm].
- Unattended runs left too long waste effort (CC agent teams) [I §3].
- LLMs "will happily grill you for hours on minutiae"; pick a design scope that is neither too
  small nor past the context window [V §3].
- Interactive sessions in visible tabs felt "more intuitive" than an agent-team lead, and
  one-shotting got "95% there" (Cong Qiaoben) [V §3].

## 12. Where the surveys disagreed or were thin

- **Does messaging an idle Manager wake it?** Neither survey said. The primary source says yes:
  an idle receiver "starts a new turn with the message", and so does a `notify_when_idle` notice.
  Own-machine messages go over a per-session socket; other machines need Remote Control and a
  claude.ai sign-in [cc-xsm]. So Claude Code messaging cannot carry IC → Manager traffic under
  the user's constraint.
- **Reviewer agreement.** S3 calls agreement across model families "the strongest signal"; S5
  says agreement "counts as zero evidence"; Compound says it "does not make a minor issue
  important". These are two uses, not a contradiction: agreement can rank findings for attention,
  but only checking the claim settles it [V §2.4; I §3].
- **Stacks per Session.** [I] found no session manager that shows a Stack per Session, while S3's
  dashboard shows a stacked-PR DAG. S3's view is per task inside one Captain's ship log, not per
  Session. CC agent view shows a PR count per session, coloured by the PR most in need of
  attention [cc-av]. The finding stands: a per-Session Stack is flow's own.
- **Self-contained brief vs pointers.** S4 and S5 insist the brief carry everything; mattpocock
  says communicate "primarily through context pointers" and "don't duplicate information already
  available" [mp-cos]. The same S4 brief quotes decisions and points to files, so both hold.
- **Worker status vocabulary.** [V] summarizes S1's status line; the primary source fixes its
  values (`aligning`, `grilling`, `implementing`, `reviewing`, `pr-review`, `qa`, `done`,
  `deferred`, `blocked`; waiting on `human`, `ci`, `nothing` or a reason) and requires a question
  in the same turn as any `blocked` [S1-wp]. It is the most complete schema for a self-report
  Facet found in either survey.
- **Description-only Vanta setups** (D1–D9) and the OpenAI and Gas Town blog posts (HTTP 403,
  cited via search excerpts or repo docs in [I]) were not re-checked; their rows rest on those
  descriptions.
