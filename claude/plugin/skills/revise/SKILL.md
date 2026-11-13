---
name: revise
description: Address findings on a PR, from Reviewers or the Director's GitHub comments, by fixing or declining each. Use when a Builder gets a findings file or a PR's comments, and when an IC hands them over.
---

# Revise

A Builder takes one PR's findings and leaves every one either fixed or declined with a reason. It works in the PR's worktree under `flow:build`.

## Principles

- **Every finding gets an outcome:** fixed, or declined with a reason the person who raised it could check.
- **Reviewer findings are judged on their evidence.** Fix what holds; decline what doesn't, and say why.
- **The Director's comments are accepted by default.** A comment phrased as a question is researched, then confirmed (and changed) or denied with the evidence.
- **The spec is the target.** A comment that conflicts with `spec.md` or with another comment is left unchanged and reported to the IC, which decides, or takes a requirement change back to the Director as a spec change.
- **Nothing is posted on GitHub.** Read comments with `gh`; answer only the IC.
- **The body stays current.** Rewrite the PR file with `flow:explain` to describe the PR as it now stands, evidence included, never as a log of the round.

## Process

1. Read the findings: a Reviewer's file under `reviews/`, or the PR's unresolved review comments on GitHub.
2. Fix test-first where the finding is about behaviour, run the repo's checks, and commit.
3. Record each outcome and its reason: under each finding in a Reviewer's file, or, for comments, in a new `reviews/<nn>-<name>-director-<round>.md` with one entry per comment.
4. Rewrite the PR file.
5. Reply in a line or two: the counts fixed and declined, the checks run, and by name any declined blocking finding or comment conflict. The IC reads the file only for those, and may overrule.
