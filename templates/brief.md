# Task {{id}}: {{title}}

{{role}}

## Working directory

`{{cwd}}`
{{worktree_note}}
## What is asked

{{body}}

## Completion criteria

{{criteria}}

## Reporting

Report through these exact commands, run from your shell.

- At the start, and on each phase change:
  `{{flow}} task status {{id}} working --message "<what you are doing>"`
- When you need a decision you cannot make from this brief or the code:
  `{{flow}} task status {{id}} needs-decision --message "<question>"`
  Then stop and wait for an answer to appear in your terminal.
- When something outside your control blocks you:
  `{{flow}} task status {{id}} blocked --message "<what blocks you>"`
- When work outside this task's scope should run as a sibling task, write its brief to a file,
  then: `{{flow}} task request-dispatch {{id}} --title "<title>" --brief <file> [--host <name>]`
- When every completion criterion is met, write a short Markdown report to a file, then:
  `{{flow}} task done {{id}} --report <path>`

{{contact}}
