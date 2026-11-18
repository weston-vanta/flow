---
name: explain
description: Principles for explaining your own work to the Director or a human reviewer. Use when writing a PR title or body, a commit message, the plan you present for approval, or the findings you take to the Director about a Stack.
---

# Explain

Covers PR titles, PR bodies, commit messages, the plan an IC presents for approval, and per-Stack findings. It does not govern `spec.md` or `plan.md`, which have their own tests, nor replies in the pane.

## The two failures

1. **Too much detail.** Every line, label and box must help the reader judge the change. Apply the two-sided test:
   - Could a reviewer who wasn't in the grilling judge the work from the explanation and the diff alone? If not, something is missing.
   - Is there any line they would skip? If so, cut it.

   There is no length limit; the test sets the length.
2. **References the reader can't follow.** Define a term before using it. Use the reader's own words: glossary terms, real identifiers, or plain language, never names you coined. Anything only you know (the grilling, the spec, a review, an earlier conversation) is explained in the text or left out.

## Principles

- **One reader:** a staff-level reviewer who wasn't in the grilling. Anticipate at most five questions they would ask about why the change has this shape (why the obvious approach fails, what it gains and costs against the alternative) and answer them in the text, never as an FAQ. Lean toward product requirements, the data model, interfaces and architecture, not how edge cases behave.
- **Forcing facts:** say what forces the shape. A fact is forcing only if the design would be simpler without it: if this were false, what would we delete? Push each one down to the mechanism in the data model, an interface or the architecture, past the consequence to the cause. Keep asking why until the answer is a fact about requirements, interfaces, or architecture: a surface-level fact often hides the deeper one, such as a data model built for one feature now awkwardly supporting others.
- **Only true, checked claims:** evidence that actually ran against the final diff, with anything unverified marked as such. Link only what the reader can open: commits, files on a pushed branch, PRs, docs. The Task directory and worktrees are local to you.
- **Leave out what the reader assumes:** the repo's conventions, and what any competent agent would do anyway. The repo's docs and conventions tell you what its reviewers assume. An assumption that forces a less obvious design is a forcing fact, so state it.
- **Pictures:** use one where the medium renders it and it explains more than the prose it replaces. Plain text or ASCII suits only small ones: a short call tree, a diff sketch, a few boxes. A picture that would be large means the level of abstraction is wrong, or it belongs in an Explainer (`flow:explainer`). Commit messages and other plain-text media usually get none.

## PRs and commits

- **The repo's guidance wins on structure and style.** Look for a PR template, PR or commit rules in `CLAUDE.md` or `AGENTS.md`, and any skill whose description covers PR bodies or commits. Their headings (verbatim, since hooks may check them), order and tone win. With no guidance, these principles alone set the structure. A conflict between the repo and this skill, or missing guidance, gets a line in `notes.md`.
- **Never in a PR body,** unless the repo explicitly asks for it:
  - review findings or review rounds;
  - the story of how the work got done, or a changelog of Revise rounds;
  - a file-by-file list that repeats the diff;
  - links the reader can't open;
  - evidence that didn't run;
  - other PRs' content;
  - promises of future work, unless a planned PR delivers it (then name it).

  Attribution is left to the harness and the repo.
- **Stacks:** each body stands alone. A PR whose purpose only makes sense as part of its Stack opens with one line of Stack context: the Stack's goal and where this PR sits. When it relies on a lower PR, name what it builds on, in words. GitHub draws the Stack, so leave sibling PRs out of the body rather than tabling them. When the Stack has an Explainer, link it.
- **Current, not historical:** a body describes its PR as it stands. Whoever changes the diff (a reshape, a per-Stack follow-up, a Revise round) rewrites the body, evidence included.
- **Title:** the outcome, in imperative mood and glossary words, following the repo's convention when there is one.
