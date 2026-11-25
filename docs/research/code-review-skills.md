# Code review skills against flow's Review dimensions

Research for [Code review skills against flow's Review dimensions](../../.scratch/compose-skills/issues/09-code-review-skills.md)
on the compose-skills map (the ticket is local scratch, not on this branch).

Question: A) do flow's Review dimensions (`claude/plugin/skills/review/dimensions/*.md`, one
question plus an evidence bar each, naming no tool or harness) follow best practice for writing
review skills? B) When `flow:plan` picks a review skill found in the session as a Review
dimension of its own, should the Reviewer hold that skill's findings to flow's evidence bar, or
trust the skill's own criteria and triage?

Read 2026-11-25. Obsidian at `54f20205193a`; obsidian's `.claude/skills` is a symlink to
`.agents/skills`, so paths below use `.agents/skills`.

## Summary

- **A: the evidence bar is best practice.** Anthropic's own guidance for review prompts says a
  single-pass reviewer should get a concrete reporting threshold, not "important" or "high
  severity", and the Code Review service docs suggest cutting false positives by requiring
  evidence (a `file:line` citation) before a class of finding is posted. Flow's "a finding must
  show" lines are exactly that, per dimension. Short files with no tool names also match
  Anthropic's advice to give code review "high freedom".
- **A: the gap is verification.** Every mature review pipeline examined finds first and verifies
  second, often in separate contexts: Anthropic's `code-review` plugin, `/security-review`, the
  managed Code Review service, and obsidian's Frontdoor review. Obsidian's shared `reviewer.md`
  adds grounding rules: read the surrounding file, and Grep before claiming anything about the
  rest of the codebase. Flow's Reviewer checks the bar and triages in one pass with no step that
  confirms a finding against the code.
- **A: smaller gaps.** No exclusion list (pre-existing code, what tooling already enforces,
  concerns the author already addressed), no reasons behind the rules, no examples and no evals.
  Flow's choice of evidence over self-reported confidence scores holds up: Anthropic's current
  `code-review` command dropped its 0-100 score in favour of validation subagents.
- **B: trust the skill's criteria, apply flow's format.** Each skill already filters, some
  heavily (validation subagents, confidence of 8 or more, caps of 4 or 10 findings, P0/P1/P2
  rules). Flow's dimension bars are written for flow's own questions and don't fit a
  security or Frontdoor finding. Anthropic warns that stacking filters on a precision-tuned
  reviewer lowers recall. The Reviewer should keep the skill's verdict on what counts as a finding,
  convert each finding into flow's fields (dimension = skill name, blocking, location, evidence,
  fix), and drop only findings with no location or no checkable evidence.
- **B: what the Reviewer must control is side effects and severity mapping, not the filter.**
  Skills post PR comments (`review-frontdoor-pr`, `/code-review --comment`), send the diff to
  vendors (`greptile-code-review`), submit traces, or can't be started by the model at all
  (`iam-review`).

## Sources and what each does to filter findings

| Skill | Finding stage | Filter before reporting | Severity | Output |
|---|---|---|---|---|
| mattpocock `code-review` | Two sub-agents in parallel: Standards (repo docs plus a 12-smell Fowler baseline) and Spec | None beyond the brief: cite the standard or quote the spec line; smells are "always judgement calls"; skip what tooling enforces; under 400 words per axis | Hard violation vs judgement call | Two reports side by side, never merged or reranked |
| obsidian `local-code-review` + `reviewer.md` | One reviewer, reading `CODE_REVIEW*.md` rules matched to changed paths | Confirm each finding by reading the file (all of it up to 500 lines); Grep before claiming anything about the codebase; drop concerns the PR summary already sets aside; cap at 10 | `block` / `suggest` / `nit`, lowest that fits | `path:line [severity] rule-path` + one paragraph |
| obsidian `review-frontdoor-pr` | P0 and P1 rules always read; P2 rules loaded by `read_when` | Two-step verification per candidate (more context; drop contrived scenarios), a third step for P2 ("one of only four?"), cap P2 at 4 | P0 must fix, P1 needs justification, P2 advisory | Markdown artifact, then posts a PR comment, labels the PR, submits a LangSmith trace |
| obsidian `iam-review` | Checklist of authorization concerns plus three `CODE_REVIEW.md` files | None | Critical / Warning / Suggestion | File and line, severity, what's wrong and fix, rule cited |
| obsidian `greptile-code-review` | External Greptile CLI | Whatever Greptile does; the skill only relays | Greptile's | Greptile's text |
| Anthropic `code-review` plugin | Two CLAUDE.md agents, two bug agents | A validation subagent per bug finding; unvalidated findings are dropped; a false-positive list | None | Terminal list, or inline PR comments with `--comment` |
| Anthropic `/security-review` | One sub-task finds vulnerabilities | A parallel false-positive sub-task per finding with hard exclusions and precedents; drop confidence below 8 of 10 | HIGH / MEDIUM / LOW | File, line, severity, category, exploit scenario, fix |
| Anthropic Code Review service | Several agents in parallel | A verification step against the code, then dedupe and rank | Important / Nit (and others) | Inline PR comments and a check run |

Sources for the table:

- mattpocock: `.claude/skills/code-review/SKILL.md` in this repo, identical (apart from one
  sentence) to [mattpocock/skills `skills/engineering/code-review/SKILL.md`](https://github.com/mattpocock/skills/blob/main/skills/engineering/code-review/SKILL.md) v1.3.1, and its
  `docs/engineering/code-review.md`.
- Obsidian: `.agents/skills/local-code-review/SKILL.md`, `scripts/ci/src/ai-dx/code-review/reviewer.md`,
  `.agents/skills/review-frontdoor-pr/SKILL.md`, `packages/frontdoor/review-context/review-process.md`,
  `p0.md`, `p2-index.toml`, `.agents/skills/iam-review/SKILL.md`, `.agents/skills/greptile-code-review/SKILL.md`
  and its `scripts/run-greptile-code-review.sh`.
- Anthropic `code-review` plugin: [`plugins/code-review/commands/code-review.md`](https://github.com/anthropics/claude-plugins-official/blob/6985b2d88dc08b4a0d202f5f0301f02e360b9cb1/plugins/code-review/commands/code-review.md).
  Its [README](https://github.com/anthropics/claude-plugins-official/blob/6985b2d88dc08b4a0d202f5f0301f02e360b9cb1/plugins/code-review/README.md)
  still describes a 0-100 confidence score with a threshold of 80, which the command no longer
  uses. The same repo's [`pr-review-toolkit` code-reviewer agent](https://github.com/anthropics/claude-plugins-official/blob/6985b2d88dc08b4a0d202f5f0301f02e360b9cb1/plugins/pr-review-toolkit/agents/code-reviewer.md)
  and `feature-dev` code-reviewer agent still use the score.
- `/security-review`: [`anthropics/claude-code-security-review` `.claude/commands/security-review.md`](https://github.com/anthropics/claude-code-security-review/blob/0c6a49f1fa56a1d472575da86a94dbc1edb78eda/.claude/commands/security-review.md).
  The GitHub Action in the same repo adds a regex hard-exclusion pass and a Claude filter in
  `claudecode/findings_filter.py`, and takes a `false-positive-filtering-instructions` file.
- Code Review service: [Code Review](https://code.claude.com/docs/en/code-review) ("Find, Verify,
  Rank, Post"), [Customize Code Review](https://code.claude.com/docs/en/code-review-customize).
- Built-in `/code-review`: [Review locally](https://code.claude.com/docs/en/code-review-local).
  It is a bundled skill with no `SKILL.md` on disk; its text isn't published. Effort trades
  coverage for confidence (`low` reports only the most confident findings); `--max-findings`
  defaults to 32 where findings go through the `ReportFindings` tool.

## A. Flow's dimensions against best practice

### Where flow matches

- **A concrete bar per dimension.** [Prompting Claude Opus 4.8, "Code review harnesses"](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-4-8#code-review-harnesses):
  "If you do want the model to self-filter in a single pass, be concrete about what the reporting
  threshold is rather than using qualitative terms such as 'important'." [Customize Code Review](https://code.claude.com/docs/en/code-review-customize#choose-what-to-write-in-review-md),
  for findings that turn out wrong: "require evidence before a class of finding is posted. For
  example, 'behavior claims need a `file:line` citation in the source, not an inference from
  naming'". Flow's "a finding must show" lines are concrete and checkable: a quoted requirement
  (intent), a scenario with likelihood and cost (correctness), quoted guidance (standards), a
  sized alternative (simplicity). `/security-review`'s required "exploit scenario" is the same
  idea as correctness's failure scenario.
- **High freedom, concise.** [Skill authoring best practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices#set-appropriate-degrees-of-freedom)
  uses code review as its example of an "open field" task: "Give general direction and trust
  Claude to find the best route". Flow's files run 5 to 12 lines and assume a capable reader.
- **Separate questions.** mattpocock keeps Standards and Spec apart so "one axis [can't mask]
  the other", and every finding carries its dimension in flow. Flow differs in that one Reviewer
  checks several dimensions per pass and triage ranks blocking first across them; that is a
  milder separation than mattpocock's separate sub-agents, not none.
- **Evidence over confidence numbers.** Anthropic's older reviewers asked the model for a 0-100
  or 1-10 confidence and cut below 80 or 8. The current `code-review` command replaced the
  score with validation subagents, and the Code Review service describes a verification step,
  not a score. Flow's evidence travels with the finding, so the Builder and the Director can
  check it; a self-reported number can't be checked.
- **No tool or harness named.** The Code Review service can include a skill's `SKILL.md` body in
  `REVIEW.md` but drops frontmatter, scripts, linked files and `!` commands
  ([Include a skill file](https://code.claude.com/docs/en/code-review-customize#include-a-skill-file)).
  Flow's dimension files would survive that unchanged; obsidian's and mattpocock's skills,
  which run `gh`, `git`, `doc-tools` or spawn sub-agents, would not.

### Where flow falls short

1. **No verification step.** Anthropic's plugin (step 5, a validator per finding),
   `/security-review` (a false-positive sub-task per finding), the Code Review service (a
   "Verify" stage) and obsidian's Frontdoor process (two or three verification steps) all
   confirm each candidate before reporting it. Obsidian's `reviewer.md` gives the grounding rules:
   read the whole file up to 500 lines, Grep before claiming "nothing handles this error" or
   "no caller passes null", don't cite a commit you haven't `git show`n, and "A wrong claim about
   the code is worse than no comment." Anthropic's prompting guide makes the same point
   generally ([Minimizing hallucinations in agentic coding](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices#minimizing-hallucinations-in-agentic-coding):
   "Never speculate about code you have not opened"). Flow's "Running a pass" says to hold
   findings to the bar, which checks a finding's form, not its truth. Anthropic also notes that
   moving filtering out of the finding step "often helps" recall, and that a model told to
   self-filter "might investigate the code just as thoroughly, identify the bugs, and then not
   report findings it determines to be below your stated bar." Flow's step 3 drops sub-bar
   findings silently. **Gap**, the largest one.
2. **No exclusions.** Common to every source: pre-existing issues (Anthropic plugin,
   `/security-review` "newly added by this PR", obsidian "Stay on the changed code"), anything a
   linter or CI enforces (Anthropic plugin, mattpocock, Code Review example `REVIEW.md`),
   concerns the author already addressed in the PR body (obsidian `reviewer.md`), and guards
   for states that can't happen yet (obsidian). Flow's correctness and simplicity cover the last
   one through impact times likelihood, and standards says "Your taste is not a standard", but
   no dimension says to stay on changed code or to skip what tooling enforces. **Gap**, small.
3. **No reasons.** Anthropic ([Add context to improve performance](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices#add-context-to-improve-performance))
   and obsidian's `CODE_REVIEW.skills.md` ("Skill Bodies Should Explain the 'Why'") both say an
   explained rule generalises better. Flow's lines such as "A risk you can't turn into a
   scenario is not a finding" give no reason. The skill-level principle "Evidence, not taste" is
   the only one. **Gap**, small; one clause per rule fixes it.
4. **No examples or evals.** Obsidian rules ship bad/good pairs and its skills ship trigger and
   content evals; Anthropic recommends "at least three evaluations" and examples for
   style-sensitive output. Flow's dimensions have neither, so there is no way to measure whether
   a change to a bar raises or lowers recall. **Gap**, medium, but it is a gap of flow's skills in
   general rather than of the dimension format.
5. **No cap.** Obsidian caps at 10 (and 4 for P2), the Code Review docs suggest a Nit cap, the
   built-in `/code-review` reports up to 32. Flow's findings go to a Builder who must address
   each one, not to a human skimming comments, so a cap matters less. **Not a gap** worth acting on.

## B. Filtering a selected skill's findings

### Why flow's dimension bars shouldn't be applied

- The bars are per-dimension. A finding from `/security-review` ("exploit scenario") or
  Frontdoor ("violates P1 rule `require-correct-frontdoor-package`") meets that skill's own bar.
  No flow dimension file names a bar for "security" or "Frontdoor rules", so the Reviewer would
  either invent one or force-fit correctness's, and drop real findings.
- The skills are already precision-tuned. Anthropic's plugin says "If you are not certain an
  issue is real, do not flag it"; `/security-review` "Better to miss some theoretical issues
  than flood the report"; Frontdoor drops "low-value partial findings". Anthropic's Opus 4.8
  guidance says stacking a stricter bar on a precise reviewer lowers recall: "Precision
  typically rises, but measured recall can fall". A second filter by a Reviewer that didn't do the
  investigation adds the cost of a verification pass without its benefit, since it would judge
  from the finding's text rather than the code.
- The Director chose the skill for its criteria. Re-judging its findings by flow's taste
  undoes that choice. This matches mattpocock's "the repo always overrides" and obsidian's
  "Rule-mandated blockers are authoritative".

### What the Reviewer should still do

- **Normalise into flow's fields.** Dimension = the skill's name; location; evidence (the
  skill's cited rule, scenario or quote); suggested fix. Drop a finding only if it has no
  location or no evidence the Builder can check. Every skill examined already asks for both,
  so this drops almost nothing; it catches malformed output, not judgment.
- **Map severity to blocking.** The skill's top tier is blocking: `block` (obsidian),
  P0 (Frontdoor), Critical (`iam-review`), HIGH (`/security-review`). Everything else is
  non-blocking. Frontdoor's P1 ("requires concrete justification") maps to non-blocking for
  the Reviewer and stays visible to the Builder in Revise. mattpocock has no severity:
  documented-standard "hard violations" map to blocking, smells to non-blocking. Anthropic's
  plugin has no severity, but it reports only validated, high-signal issues, so blocking.
- **Dedupe against flow's own dimensions** in the same pass, keeping the flow finding when both
  say the same thing (mattpocock's Standards and Spec overlap flow's standards and intent almost
  entirely; `flow:plan` could warn about that when offering it).
- **Neutralise side effects.** Run the skill read-only and without posting: Frontdoor posts a
  PR comment, labels the PR and submits a LangSmith trace; Anthropic's command posts with
  `--comment`; `greptile-code-review` sends the diff to a vendor and runs "only after the user
  agrees". Findings live in flow's findings file, so these steps must be skipped or turned off,
  and Greptile's consent belongs at plan approval.
- **Check the skill can run where the Reviewer is.** `iam-review` has `disable-model-invocation:
  true`; per [Running skills the model can't invoke](https://github.com/weston-vanta/flow/blob/research/invoking-locked-skills/docs/research/invoking-locked-skills.md)
  the model can't start it from a Subagent and discovery won't list it. mattpocock's
  `code-review`, Anthropic's plugin and `/security-review` spawn their own sub-agents, which a
  Reviewer Subagent may or may not be able to do depending on the harness. A project skill
  named `code-review` (mattpocock's, in this repo) also replaces the bundled `/code-review`
  ([Change or build on the review](https://code.claude.com/docs/en/code-review-local#change-or-build-on-the-review)),
  so discovery should offer whatever the name resolves to, not assume the built-in.

## Recommendation

For A, keep the format: one question and one concrete evidence bar per file, no tool names.
Add a verification step to "Running a pass" (confirm each candidate against the code and Grep
before claiming anything about the rest of the repo; ideally a separate step or Subagent from
the finding step, which then aims for coverage), a short shared exclusion list (changed code
only; nothing tooling enforces; nothing the PR body already settles), and a reason clause on
each rule.

For B, the Reviewer trusts the selected skill's own criteria and filtering, and applies only
flow's output contract: flow's fields, a severity-to-blocking mapping, dedupe, and a floor of
"has a location and checkable evidence". It runs the skill with its side effects off.
