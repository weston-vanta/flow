---
name: align
description: The IC's Align phase - grill the Director to an approved spec.md, then get the plan approved. Run from flow:ic.
---

# Align

Agree with the Director what the Task delivers before any work is shaped, then agree how it is cut. These are two approvals: first `spec.md`, then the plan.

## Principles

- **Find facts yourself.** Read the brief, the Task's Resources (`flow task get`), and the repo's `CLAUDE.md`, glossary and ADRs. Anything the code, docs or Jira issues can answer is yours to look up, not a question.
- **Ask only what only the Director can decide:** requirements, scope, ambiguity, data model, architecture and accepted tradeoffs.
- **Grill in rounds.** Each round is a short list of numbered questions, each with your recommended answer and a line of why, so the Director can answer by number and mostly agree. Each round builds on the last answers.
- **Stop when nothing open would change the spec.**

## Process

1. Read and find facts. If the brief already points at a spec sufficient to build from, say so and ask to skip the grilling; on a yes, write `spec.md` from it and go to step 3.
2. Grill until nothing open would change the spec.
3. Write `spec.md` in the Task directory and ask the Director to approve it. Revise until they do. It holds:
   - the goal;
   - what is in and out of scope;
   - the requirements, as checkable statements in words;
   - the data model, architecture and design decisions, each with the tradeoff accepted;
   - the deliverable: PRs, a spec, or an answer.

   Its test is not a template: could the Director judge the draft PRs against it without having been in the grilling? For a spec or answer deliverable, Align ends here.
4. Draft `plan.md` with `flow:plan`. Present it for a reader who wasn't in the grilling, written with `flow:explain`, and ask two things: does the Director approve it, and do they want Resources made from it (Jira issues matching the plan through `acli jira`, an Explainer, a doc through `gws`, or others). Revise until they approve. The approval, whose picture shows the Stacks, counts as the Director asking for a Stack.
5. Create those Resources through Subagents, record each as `flow:ic` says, and write the builder briefs with `flow:plan`. Pair each PR in `plan.md` that has a Jira issue with its key, whether a Subagent created the Jira issue or the Director named an existing one to adopt.
