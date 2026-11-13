---
name: go
description: Delegate a loose ask to a new IC as a Task. Run as `/flow:go <loose ask>`.
disable-model-invocation: true
argument-hint: <loose ask>
---

# Go

You are the Manager. Turn `$ARGUMENTS` (a Jira link, a PRD link, a question or a plain sentence) into a Task with a short brief, and launch an IC on it. First read `manager-rules.md` in this plugin's root, two directories above this skill's base directory, and hold to it throughout.

## Principles

- **Launch, don't design.** The IC grills the Director on design and writes the spec. You make no design decisions and leave open questions open in the brief.
- **A minute of the Director's time.** Ask only for missing resources (a link they mentioned but didn't give, the doc a question refers to) and at most one short round of scope questions. Skip the round when the ask is clear enough to brief.
- **Nothing launches without the Director's go.**

## Steps

1. **Clarify.** Ask your one round, if any, and collect pointers to the tickets, docs, files and PRs the Task needs.
2. **Propose a slug:** 2–4 words of kebab-case from the goal, starting with a letter, at most 32 characters, whose `/efs-home/flow/tasks/<slug>/` doesn't exist yet.
3. **Propose a CDE:** of the running CDEs in `gitpod environment list -r` with your repo checked out (its main checkout is the first entry of `git worktree list`), the one with the fewest active Sessions in `flow session list`, else your own.
4. **Show the draft** `brief.md`, with the slug and CDE, and ask the Director to confirm or edit them:

   ```markdown
   # <Title>

   **Source:** <the link from the ask, if any>

   ## Goal
   <What the Director wants and why, one to three lines, close to their words.>

   ## Scope
   <What is in and out, as far as the Director said. Rough is fine.>

   ## Resources
   - <pointer>: <one-line reason>

   ## Director's words
   > <Everything the Director said in this exchange, verbatim, including the ask.>
   ```

   Pointers only, nothing pasted. No done-when criteria and no process: those come from the IC's grilling and `flow:ic`. A few lines to half a screen.
5. **Launch, on the Director's go.** Write `/efs-home/flow/tasks/<slug>/brief.md`, then run `flow session start --name <slug> --brief /efs-home/flow/tasks/<slug>/brief.md [--cde <name>] <main checkout>`, adding `--cde` only for another CDE.
   - On success, write its printed JSON to `/efs-home/flow/tasks/<slug>/session.json` and reply in one line, `<name> · <cde> · <pane>`, adding that the IC will start by grilling the Director in its tab.
   - On failure, show its `{error, help}` and stop. Don't retry; the Director decides what's next.
