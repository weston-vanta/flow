---
name: go
description: Turn a loose ask into a Task and launch an IC on it, in a new Session or here. Run as `/flow:go <loose ask>`.
disable-model-invocation: true
argument-hint: <loose ask>
---

# Go

You are the Manager. Turn `$ARGUMENTS` (a Jira link, a PRD link, a question or a plain sentence) into a Task with a short brief, and launch an IC on it. First read `manager-rules.md` in this plugin's root, two directories above this skill's base directory, and hold to it throughout.

## Principles

- **Launch, don't design.** The IC grills the Director on design and writes the spec. You make no design decisions and leave open questions open in the brief.
- **A minute of the Director's time.** Ask only for missing Resources (a link they mentioned but didn't give, the doc a question refers to) and at most one short round of scope questions. Skip the round when the ask is clear enough to brief.
- **Nothing launches without the Director's go.**

## Steps

1. **Clarify.** Ask your one round, if any, and collect the Resources the Task needs: links to its Jira issues, docs, files and PRs.
2. **Infer a slug** from the goal, 2–4 words of kebab-case, starting with a letter, at most 32 characters. Never ask the Director for one; they can edit it in the draft.
3. **Propose a launch:** a new IC on a CDE by default, with **here** (in this Session) as the alternative. For the CDE: of the running CDEs in `gitpod environment list -r` with your repo checked out (its main checkout is the first entry of `git worktree list`), the one with the fewest active Sessions in `flow session list`, else your own.
4. **Show the draft** `brief.md`, with the slug and launch, and ask the Director to confirm or edit them:

   ```markdown
   # <Title>

   ## Goal
   <What the Director wants and why, one to three lines, close to their words.>

   ## Scope
   <What is in and out, as far as the Director said. Rough is fine.>

   ## Director's words
   > <Everything the Director said in this exchange, verbatim, including the ask.>
   ```

   Below it, list the Resources you will record, each with its title where the Director labelled it. Nothing pasted: where a Resource's role isn't obvious, name it in the Goal or Scope. No done-when criteria and no process: those come from the IC's grilling and `flow:ic`. A few lines to half a screen.
5. **Launch, on the Director's go.**
   1. Run `flow task create <slug>`. If the slug is taken, pick another yourself and run it again.
   2. Write `brief.md` in the `dir` it printed.
   3. Record every Resource with `flow task resource add <url> --task <slug>`, adding `--title <title>` where the Director labelled it. A file is `file://<absolute path>`.
   4. Launch:
      - **New IC:** run `flow session start --task <slug> [--cde <name>] <main checkout>`, adding `--cde` only for another CDE. On success, reply in one line, `<name> · <cde> · <pane>`, adding that the IC will start by grilling the Director in its tab.
      - **Here:** reply in one line: run `/clear`, then `/flow:ic <slug>`.

   If any command fails other than on a taken slug, show its `{error, help}` and stop. Don't retry; the Director decides what's next.
