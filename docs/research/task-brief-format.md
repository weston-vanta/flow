# Task brief format: what coordinators hand to workers

Research date: 2026-11-11. The question: in agentic setups that run in practice, what does the
handoff document from a coordinator to a worker contain, and how is it structured? This is to
choose the format of flow's `brief.md`, which a Manager writes for a Task and an IC reads as its
first prompt. The IC grills the human into a spec, so the brief only needs to be enough to start
that grilling.

Evidence key: **code** means I read the template or skill source itself; **code (PR)** means the
source sits on an unmerged branch; **docs** means first-party documentation; **description**
means only a write-up or Slack post describes it. VantaInc repos are private. Builds on
[vanta-agentic-setups](https://github.com/weston-vanta/flow/blob/research/vanta-agentic-setups/docs/research/vanta-agentic-setups.md),
[industry-agentic-setups](https://github.com/weston-vanta/flow/blob/research/industry-agentic-setups/docs/research/industry-agentic-setups.md)
and [agentic-feature-taxonomy](https://github.com/weston-vanta/flow/blob/research/agentic-feature-taxonomy/docs/research/agentic-feature-taxonomy.md).

---

## 1. Side by side

| # | Source | Evidence | Worker type | Length of the coordinator-written part | Context: pasted or pointer | Process in brief? | Human decisions |
|---|---|---|---|---|---|---|---|
| B1 | **rex-ona-workers** `worker-brief.md` (Jeremiah Pratt) | code | Separate interactive `claude` session on its own Ona box | Template is ~380 words before filling | Both: acceptance criteria **pasted**; process docs **by pointer** (clone the KB, read these paths) | Steps plus "run the repo's required checks"; procedure itself by pointer | **Quoted, with date**, in two sections: decisions and authorizations |
| B2 | **dispatch** brief (Sam Storino, access-mgmt KB) | code | Separate session on its own Ona box, running the `build-ticket` skill | ~440 words, fixed; only `<KEY>` and URL filled | **Pointer only**: ticket key + URL; worker reads Jira and the skill itself | Process by pointer (`build-ticket`); the brief is mostly *adjustments* to it plus the I/O protocol | None at launch. On restart: brief plus "Decisions so far:" |
| B3 | **snowplow-dispatch** brief (Sam Storino, Snowplow KB) | code | Same as B2 | One line + optional context + ~600-word protocol | **Pasted** workstream context (DRI, playbook steps, gotchas) as short lines, each with a link to its source | Routed skill by name, or the full no-skill flow (6 steps) **pasted** | None at launch (grilling produces them) |
| B4 | **sail** worker prompt (Mark Whelan) | code (PR) | In-session background subagent in a worktree | ~470 words + testing principles pasted | **Pasted**: task block and the plan's shared context section | Rules and exact check commands inline | Carried in the plan's context section ("architecture decisions") |
| B5 | **superpowers** `subagent-driven-development` (Jesse Vincent) | code | In-session subagent | Dispatch = 5 parts; task text in a **file** | **Pointer** to a per-task brief file extracted from the plan; dispatch adds only what the file can't know | Implementer template (~1,000 words) carries process | Controller's "resolution of any ambiguity"; rulings in a ledger |
| B6 | **mattpocock** `implement-spec` / `chief-of-staff` / `handoff` | code | In-session subagents | "Sparse" | **Pointers**: "Communicate primarily through context pointers… Don't duplicate information already available via pointers" | `implement-spec` names the skill to call (`tdd`) | In the spec and tickets |
| B7 | **Gas Town** `gt sling` + `gt prime` (Steve Yegge) | code | Separate session (polecat) | One bead ID + optional one-line `--args` | **Pointer**: the bead; `gt prime` shows ID, title, first 5 lines of description, then "IMMEDIATELY run: `bd show <id>`" | Role prompt and attached formula (TOML checklist) carry process | In the bead |
| B8 | **Symphony** `WORKFLOW.md` (OpenAI) | code | Separate unattended Codex session per issue | Issue fields rendered into a ~2,900-word workflow template | **Pasted**: identifier, title, state, labels, URL, full description | Almost all of it is process (status routing, workpad template, guardrails) | In the issue |
| B9 | **flow v1** `templates/brief.md` | code | Separate session in a herdr tab | Manager writes *ask* + *criteria*; flow adds the rest | Pasted ask with "the context the worker cannot find for itself: why it matters, decisions already made, constraints, links" | No; only the reporting contract | Inline in the ask |
| B10 | **Anthropic research lead** prompt | code (cookbook) + description (blog) | Subagent | One dense paragraph (~170 words in the worked example) | Pasted background; sources named | Starting points and tools named | n/a |
| B11 | **Claude Code** subagent / agent-team / Agent tool guidance | docs | Subagent or teammate | A few sentences in the examples | "point it at the files and docs worth reading instead of retyping them" | No | "restate it in the prompt" if a rule must reach the worker |
| B12 | **access-mgmt `build`** handoff rules (David Zhu) | code | Remote Stratus worker | No template; a required-field list | Pasted contract details | Readback required | "Keep provisional answers… distinct from agreed choices" |

---

## 2. The formats in detail

### B1. rex-ona-workers: the fullest self-contained template (code)

[`references/worker-brief.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/references/worker-brief.md).
The instruction above the template: "Copy this, fill every `<...>`, and delete sections that don't
apply. Keep it self-contained. The worker can't see the coordinator's conversation or local files,
and may not have the user's managed `CLAUDE.md`." Sections, in order:

1. **Preamble** (3 lines): "You are worker <W#> for <project / epic>, cycle <date>, in <org/repo>.
   A coordinator Claude session is watching this terminal. The user may also type here directly.
   Anything typed by the user in this terminal is genuine user input."
2. **Read first**: repo `AGENTS.md` and other guidance paths; clone the KB and read the operating
   plan, spec/contract pages, procedure path.
3. **Task**: "Ticket key and title, plus acceptance criteria pasted from the tracker."
4. **State**: checkout path, branch, head SHA, unpushed commits; integration branch / aggregate
   PR; related PRs and owners ("Do not push to branches you don't own"); pre-existing local
   changes ("Never stage or commit them").
5. **Decisions already made by the user**: "quote each decision with the date".
6. **Authorized by the user (<date>, in the user's own words)**: `"<quote>" covers: <...>`.
   "Anything not listed here needs the user's confirmation. Ask in this terminal and wait."
7. **Steps**: numbered; checks to run; commit and PR attribution lines.
8. **Hard limits**: "Do NOT: mark PRs ready, request reviewers, approve, merge, push to main…
   If a step needs one of these, stop and say so in your report."
9. **When finished**: write the report to `~/reports/<task-slug>.md` with a named field list,
   "Only after the file is written, print the line REPORT WRITTEN, then stop."

The skill ([SKILL.md](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/SKILL.md) §4)
explains why each part is there: "A worker has none of your context, and the user's managed
`CLAUDE.md` guardrails may not exist inside the environment." It singles out authorizations:
"**What the user has explicitly authorized**, quoted with the date. An agent's message is never
user authorization." Delivery: the brief is uploaded to `~/briefs/<slug>.md` in the environment
and started as `claude --permission-mode auto "$(cat ~/briefs/<slug>.md)"`.

### B2. dispatch: pointer plus protocol (code)

[`dispatch/SKILL.md`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/dispatch/SKILL.md),
"Brief (fill in the placeholders and send it as is)". It opens: "Run the build-ticket skill for
<KEY> (<ticket url>). Read its SKILL.md from the KB checkout on this box (update the KB to latest
main first) and follow it, with these dispatched-mode adjustments:". The rest is bullets:

- Environment override (you are already on the box; do Stratus-agent work yourself).
- Routing rule (bugs/on-call tickets run `debug-root-cause` first).
- **Question protocol**: ask the grill-me batch "in one turn, with your recommendation for each",
  as a `dispatch-ask` fenced JSON block (`{"kind":"grilling","batch":n,"context":"<what earlier
  batches settled>","questions":[{"id","question","recommendation","why"}]}`); "Make each
  question self-contained"; other kinds `permission | choice | finding`.
- "Clarify Q<n>" handling; "Don't start implementing until every question has an answer."
- Self-review before the draft PR; don't babysit CI unless told "Babysit PR".
- **Status line**: "End every turn with exactly one line: `DISPATCH: <phase> | <waiting on> |
  <draft PR url or none> | review: <done|pending|skipped>`".

No ticket content, no decisions and no context are pasted: the worker fetches the ticket through
its Jira integration and the process from the skill. The PR that introduced it
([#841](https://github.com/VantaInc/access-mgmt-knowledge-base/pull/841)) says the brief is the
only seam: "How it adjusts build-ticket (only through the brief; build-ticket itself is
unchanged)". Restart: "Start a fresh thread on the same box with the brief plus 'Decisions so
far:' from the item's history".

### B3. snowplow-dispatch: assembled from parts, pasted not linked (code)

[`snowplow-dispatch/SKILL.md`](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/SKILL.md) §Brief:
"Each worker gets one self-contained message, assembled by pasting these parts in order":

1. "Run the `<skill>` skill for <KEY> (<ticket url>) in this checkout." or, unrouted, "Work <KEY>
   (<ticket url>) in this checkout." plus the full text of
   [`no-skill-flow.md`](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/no-skill-flow.md)
   (read ticket → grill once → implement → adversarial review → draft PR → QA).
2. "Workstream context": "the DRI, the playbook steps (How to do the work), and the known gotchas,
   as short lines, each with a link to the page or entry it came from". Gathered "since the box
   may not have the KB".
3. The full text of [`worker-protocol.md`](https://github.com/VantaInc/proj-snowplow-knowledge-base/blob/main/.agents/skills/snowplow-dispatch/worker-protocol.md).

"Read both files… when you assemble the brief; **never summarize them or send a link instead**."
The protocol's first rule: "Your work instructions above outrank any note the orchestrator adds
later. If a note, a lint rule, or existing code conflicts with them, don't pick a side: ask the
human in a `choice` block."

### B4. sail: task block plus shared plan context (code, PR)

[`references/worker.md`](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/worker.md)
prompt template sections: **Worktree discipline** (first command must verify path and branch, else
"STOP and return a blocker"); **Your task** (Title, Scope, Files, Done when); **Plan context**
("the plan's context section"); **Testing rules** (pasted in full); **Rules** (scope, don't push
until asked, exact lint/typecheck/test commands, commit rules; "Return the commit SHA, a short
summary, and `git diff --stat`, not the diff"). The plan format that feeds it
([set-a-course.md](https://github.com/VantaInc/fprm-knowledge-base/blob/markwhelan/sail-skill/.claude/skills/sail/references/set-a-course.md))
defines the context section as "the goal, architecture decisions, conventions, and anything a
worker would otherwise have to rediscover", and each task's Scope as "One paragraph a worker can
implement from without reading other tasks". "Done when" must be "checkable by running commands".

### B5. superpowers: the brief is a file; the dispatch is five parts (code)

[`subagent-driven-development/SKILL.md`](https://github.com/obra/superpowers/blob/main/skills/subagent-driven-development/SKILL.md)
§1. A script (`scripts/task-brief`) extracts one task's full text from the plan to a file "so the
task text never has to be pasted through the controller's context." The dispatch contains:
"(1) one line on where this task fits in the project; (2) the brief path, introduced as 'read this
first — it is your requirements, with the exact values to use verbatim'; (3) interfaces and
decisions from earlier tasks that the brief cannot know; (4) your resolution of any ambiguity you
noticed in the brief; (5) the report-file path and report contract. Exact values (numbers, magic
strings, signatures, test cases) appear only in the brief. Never make a subagent read the whole
plan file." The [implementer template](https://github.com/obra/superpowers/blob/main/skills/subagent-driven-development/implementer-prompt.md)
adds a "Before You Begin" section ("If you have questions… **Ask them now.**"), escalation
statuses (`DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT`) and a report file with a final
message "under 15 lines".

### B6. mattpocock: context pointers (code)

[`implement-spec`](https://github.com/mattpocock/skills/blob/main/skills/engineering/implement-spec/SKILL.md)
and [`chief-of-staff`](https://github.com/mattpocock/skills/blob/main/skills/in-progress/chief-of-staff/SKILL.md):
"Communication to and from subagents should be sparse. Communicate primarily through **context
pointers**: to the spec, tickets, research notes, and previous commits. Don't duplicate information
already available via pointers." [`handoff`](https://github.com/mattpocock/skills/blob/main/skills/productivity/handoff/SKILL.md)
(a session-to-session handoff doc): include "a 'suggested skills' section", "Do not duplicate
content already captured in other artifacts (specs, plans, ADRs, issues, commits, diffs).
Reference them by path or URL instead." The ticket a worker gets
([to-tickets](https://github.com/mattpocock/skills/blob/main/skills/engineering/to-tickets/SKILL.md)):
What to build (end-to-end behaviour, "not layer-by-layer"), Blocked by, Status, acceptance
checkboxes; "avoid specific file paths or code snippets: they go stale fast."

### B7. Gas Town: the brief is a tracker item (code)

[`internal/cmd/sling.go`](https://github.com/gastownhall/gastown/blob/main/internal/cmd/sling.go):
`gt sling <bead> <target> [--args "<natural-language instructions>"] [--subject] [--message]`;
"The --args string is stored in the bead and shown via gt prime." On session start,
[`prime.go`](https://github.com/gastownhall/gastown/blob/main/internal/cmd/prime.go)
`outputHookedBeadDetails` prints bead ID, title and the first 5 description lines, then
`outputAutonomousDirective` says "IMMEDIATELY run: `bd show <id>`" and "**DO NOT:** Wait for user
response after announcing; Ask clarifying questions; Describe what you're going to do". Process
comes from the role prompt and an attached formula (TOML step checklist). The opposite of flow's
IC, which is meant to ask.

### B8. Symphony: issue fields plus a long workflow policy (code)

[`elixir/WORKFLOW.md`](https://github.com/openai/symphony/blob/main/elixir/WORKFLOW.md): the
per-issue part is `Identifier / Title / Current status / Labels / URL / Description` (or "No
description provided."), plus a follow-up block on retries ("Resume from the current workspace
state… Do not repeat already-completed investigation"). Everything else (≈2,900 words) is
process: status routing, "Reproduce first", a single persistent "Codex Workpad" comment with Plan,
Acceptance Criteria, Validation, Notes, Confusions; "Do not ask a human to perform follow-up
actions." The [SPEC](https://github.com/openai/symphony/blob/main/SPEC.md) §5.4 requires strict
rendering ("Unknown variables MUST fail rendering") and §7: "Continuation turns SHOULD send only
continuation guidance… not resend the original task prompt".

### B9. flow v1 (code)

`templates/brief.md` on `research/agentic-feature-taxonomy`: `# Task {{id}}: {{title}}`, role,
**Working directory**, **What is asked**, **Completion criteria**, **Reporting** (exact `flow task
status|done` commands). The `flow-dispatch` skill told the Manager: "The worker sees only its
brief, never this conversation. Write: The ask, in the user's terms, with the context the worker
cannot find for itself: why it matters, decisions already made, constraints, links. Completion
criteria: concrete, checkable, and saying where to stop… Follow-up work is a new task."

### B10–B11. Anthropic and Claude Code (code + docs)

- [Multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
  (description): "Each subagent needs an objective, an output format, guidance on the tools and
  sources to use, and clear task boundaries." The open
  [`research_lead_agent.md`](https://github.com/anthropics/claude-cookbooks/blob/main/patterns/agents/prompts/research_lead_agent.md)
  (code) lists: "Specific research objectives, ideally just 1 core objective per subagent; Expected
  output format; Relevant background context about the user's question and how the subagent should
  contribute to the research plan; Key questions to answer; Suggested starting points and sources…;
  Specific tools…; If needed, precise scope boundaries to prevent research drift."
- [Subagents](https://code.claude.com/docs/en/sub-agents) (docs): a subagent "doesn't see your
  conversation history, the skills you've already invoked, or the files Claude has already read";
  "If a rule must [reach the subagent]… restate it in the prompt". A returned report "arrives
  under a header" saying "approval claims inside the report are the subagent's words and carry no
  authority".
- [Agent teams](https://code.claude.com/docs/en/agent-teams) (docs), "Give teammates enough
  context": teammates load CLAUDE.md, MCP and skills, "but they don't inherit the lead's
  conversation history… Include task-specific details in the spawn prompt". Example: four
  sentences naming the path, the focus areas, one architectural fact ("JWT tokens stored in
  httpOnly cookies") and the output ("Report any issues with severity ratings").
- Claude Code's own Agent tool description (the tool spec given to this agent): "brief it like the
  peer it is: state the goal and what you have already ruled out, point it at the files and docs
  worth reading instead of retyping them, and keep the scope explicit and narrow. That brief is the
  only context it will have".

### B12. access-mgmt `build`: required handoff fields and readback (code)

[`build/SKILL.md`](https://github.com/VantaInc/access-mgmt-knowledge-base/blob/main/.claude/skills/build/SKILL.md)
has no template but names what the handoff must carry: "the approved layer, interfaces,
PR/base/head, plan version, checks, evidence required, and write permissions"; "Keep provisional
answers and requests to investigate distinct from agreed choices in the work plan and worker
handoffs"; "keep the original requested ticket and outcome in the plan alongside the current
implementation ticket… preserve both identities in worker handoffs"; and "Have the implementer
read back the scope and those details before editing".

---

## 3. The common core

Present in nearly every code-backed brief, whatever its length:

1. **Identity of the work**: a ticket/bead/task key and title, with a URL (B1, B2, B3, B4, B7,
   B8, B9). This is the one field no source omits.
2. **The ask as an outcome**, not steps: "acceptance criteria" (B1), "Done when" (B4),
   "Completion criteria… saying where to stop" (B9), "Expected output format" (B10). Checkable
   wherever the worker implements.
3. **Context the worker cannot find for itself**, and only that: "anything a worker would
   otherwise have to rediscover" (B4), "the context the worker cannot find for itself" (B9),
   "interfaces and decisions from earlier tasks that the brief cannot know" (B5), "what you have
   already ruled out" (B11).
4. **Where to look** (pointers to process docs, spec, repo guidance): B1 "Read first", B2/B3 the
   skill, B6 context pointers, B11.
5. **Boundaries**: scope, what not to touch, actions reserved for the human (B1 Hard limits, B4
   Rules, B2/B3 "Never mark a PR ready… or merge", B9 "Never ask a worker to merge").
6. **How to talk back and how to finish**: a question protocol and an end-of-work report contract
   (B1 report file, B2/B3 `dispatch-ask` + `DISPATCH:` line, B5 statuses + report file, B9
   `flow task status|done`). In flow v1 and in snowplow this part is fixed text the tool appends,
   not something the coordinator writes.

**Human decisions**, where the brief carries them, are quoted rather than paraphrased: B1 quotes
each decision and each authorization with its date, in the user's own words; B12 keeps provisional
answers separate from agreed ones; B2 restarts with "Decisions so far". Sources that route
decisions through grilling (B2, B3) put none in the launch brief.

### Where the sources split

- **Paste vs pointer.** B3 insists on pasting ("never summarize them or send a link instead")
  because the box "may not have the KB"; B6 and B11 insist on pointers. B1 does both: it pastes the
  acceptance criteria and decisions and points to process docs the worker clones itself. The rule
  the code follows: paste what the worker cannot reach or what must not drift (decisions,
  authorizations, exact values); point to what it can read (specs, skills, repo docs, the ticket).
- **Process in the brief vs in a skill.** Long briefs (B4, B8) carry process inline. Briefs that
  name a skill (B2, B3 routed, B6) stay short, and the brief holds only *adjustments* to the skill
  and the I/O protocol. B2's author treats the brief as the seam: adjustments live there until
  "these could move into build-ticket as a 'dispatched mode' section" (PR #841).
- **Ask or don't ask.** B7 and B8 forbid questions; B1, B2, B3, B5 invite them, and B2/B3 require a
  grilling batch before code. Briefs for grilling workers tell the worker *how* to ask (batched,
  with a recommendation, self-contained) more than *what* to build.

---

## 4. Reported lessons (what went wrong)

| Lesson | Source | Evidence |
|---|---|---|
| **Too short → misread and duplicated work.** Early leads gave "short instructions like 'research the semiconductor shortage'", which were "vague enough that subagents misinterpreted the task"; "one subagent explored the 2021 automotive chip crisis while 2 others duplicated work investigating current 2025 supply chains." "Without detailed task descriptions, agents duplicate work, leave gaps, or fail to find necessary information." | [Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system) | description |
| **Too long → pasted history.** "Do not paste accumulated prior-task summaries ('state after Tasks 1-3') into later dispatches — a real session's dispatch hit 42k chars of which 99% was pasted history. A fresh subagent needs its task, the interfaces it touches, and the global constraints. Nothing else." Also: everything pasted "stays resident in your context for the rest of the session… Hand artifacts over as files." | [superpowers SDD](https://github.com/obra/superpowers/blob/main/skills/subagent-driven-development/SKILL.md) | code |
| **Echo-back false "done".** "A marker string that appears in the brief also appears in the terminal when the brief is echoed back, so grepping the screen for it fires a false 'done'. Terminal scrollback also truncates long reports." Fix: the report contract names a file; coordinate on the file. | [rex-ona-workers](https://github.com/VantaInc/fprm-knowledge-base/blob/main/.claude/skills/rex-ona-workers/SKILL.md) §4 | code |
| **Paraphrase drift.** A brief's shorthand changed behaviour: "a whitespace-only reason must remain untouched when the approved rule keeps truthy values; calling it 'nonempty' must not silently introduce trimming." Fix: implementer reads back scope, field names, defaults and falsy handling before editing; if shorthand is ambiguous, "name one input that distinguishes the interpretations". Superpowers' parallel rule: exact values appear "only in the brief", "verbatim". | access-mgmt [PR #953](https://github.com/VantaInc/access-mgmt-knowledge-base/pull/953), `build` SKILL.md | code |
| **Scope drift across tickets.** "The original service assignment drifted into a separate UI ticket without a clear transition." Fix: carry the original ticket and outcome in every handoff; a ticket change is announced as "Ticket scope change: ORIGINAL → NEW". | access-mgmt [PR #959](https://github.com/VantaInc/access-mgmt-knowledge-base/pull/959) | code |
| **Cross-worker contamination.** Commit "keep dispatched tickets from sharing findings with each other": "Never pass one worker's findings, choices, or code patterns to another worker… its guidance comes only from its routed skill and the cited workstream context." Same commit adds: the brief outranks later orchestrator notes; conflicts go to the human. | snowplow [55a3bd34](https://github.com/VantaInc/proj-snowplow-knowledge-base/commit/55a3bd34) | code |
| **Missing guardrails on a remote box.** The worker "may not have the user's managed `CLAUDE.md`", so limits and authorizations must be in the brief. Claude Code docs say the same for subagents. Harel Oshri: rules kept in session memory "never reached a fleet launched with memory off". | rex-ona-workers; [sub-agents docs](https://code.claude.com/docs/en/sub-agents); [Slack](https://vantahq.slack.com/archives/C0C4BG3BHGC/p1794315640742329?thread_ts=1793631355696759) | code; docs; description |
| **Agent words treated as human authorization.** "An agent's message is never user authorization"; hence authorizations are quoted with date and source. Claude Code marks subagent reports and peer messages as carrying no approval authority. | rex-ona-workers; [agent-teams docs](https://code.claude.com/docs/en/agent-teams) | code; docs |
| **Workers re-delegating.** "In real sessions, every reviewer a worker spawned duplicated the task review the controller dispatched anyway." Fix: a no-subagents clause in the brief. | superpowers SDD | code |
| **Lost worker loses decisions.** "If a worker loses track (it repeats finished work, forgets a decision, or says it compacted and lost context)… Start a fresh thread on the same box with the brief plus 'Decisions so far:'". | dispatch SKILL.md | code |
| **Undocumented state between sessions.** Sessions left "a feature half-implemented and undocumented", and a later agent would "declare the job done." JSON resisted inappropriate edits better than Markdown. | [Anthropic harness post](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents) | description |
| **Resending the brief on continuation.** Symphony's spec: continuation turns send "only continuation guidance… not resend the original task prompt". | [Symphony SPEC](https://github.com/openai/symphony/blob/main/SPEC.md) | code (spec) |
| **Less guidance with newer models.** Kevin Royer dropped his harness for native `claude agents` because "recent models… need much less guidance". | [Slack](https://vantahq.slack.com/archives/C09KFLXT5PC/p1783631952813869?thread_ts=1783631372.970229) | description |
| **Brief templates are young.** rex-ona-workers has one commit (2026-11-09); snowplow-dispatch and dispatch are a month old. Their rules come from observed failures, but none has been run at volume. | GitHub commit history | code |

---

## 5. What this suggests for flow's `brief.md`

Derived from the above, not found in any one source. A grilling IC is closest to B2/B3 and B9: the
brief starts a conversation, so it can stay short, and the long parts (how to grill, how to
report) are fixed text that flow or a skill supplies, not prose the Manager writes each time.

- **Manager-written, per Task**: title and key/link; the ask as an outcome in the human's terms;
  why it matters; context the IC cannot find itself, as short lines with links (B3), pointers for
  everything readable (spec, ADRs, PRs, prior reports); decisions the human already made, **quoted
  with date** (B1), kept separate from the Manager's own suggestions (B12); out-of-scope /
  where to stop.
- **Fixed, appended by flow or a skill**: the question protocol (batched, recommendation each,
  self-contained) and the report contract naming a file, never a terminal marker (B1); hard
  limits that must survive a box without the user's CLAUDE.md; "the brief outranks later notes"
  (B3); no cross-Task carry-over except through cited artifacts (B3).
- **Avoid**: pasting conversation history or other Tasks' state (B5's 42k-char brief); paraphrasing
  a human's decision or exact values (PR #953); a done-marker the IC is expected to print.
