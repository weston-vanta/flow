# The AXI paradigm for agent-facing CLIs

Research for flow v2 ticket `05-axi-cli-paradigm`. Retrieved 2026-11-07.

## What "axi" is

**AXI = "Agent eXperience Interface"**, a set of 10 design principles for CLIs that AI agents
drive through shell execution, by Kun Chen (GitHub `kunchenguid`).

- Repo: <https://github.com/kunchenguid/axi> (MIT, created 2026-03-21, ~2.3k stars, actively
  maintained; read at commit `37fb6cce0a378706e3fb0eb499e71ab307f0d5b1`).
- Site: <https://axi.md>.
- The normative spec is the installable agent skill
  [`.agents/skills/axi/SKILL.md`](https://github.com/kunchenguid/axi/blob/main/.agents/skills/axi/SKILL.md);
  [`principles.yaml`](https://github.com/kunchenguid/axi/blob/main/principles.yaml) holds the
  canonical titles and summaries. A JS SDK lives in `packages/axi-sdk-js`.
- Reference implementations: [`gh-axi`](https://github.com/kunchenguid/gh-axi) (GitHub) and
  [`chrome-devtools-axi`](https://github.com/kunchenguid/chrome-devtools-axi); the README lists a
  catalog of official and community AXIs (e.g. `JarvusInnovations/gws-axi`).

Other candidates checked: the many `*/axi` repos on GitHub with the identical description are
forks of `kunchenguid/axi`; blog posts (tpiros.dev, tgmarinhopro.com) are secondary write-ups of
it. "AXI" is also ARM's on-chip bus protocol, unrelated. No competing "axi" CLI convention was
found, so `kunchenguid/axi` is almost certainly what the user meant.

Positioning (README): agents today use either CLIs "originally built for humans" or structured
tool protocols like MCP, and "both impose significant overhead"; AXI is "agent-native CLI tools
built from 10 design principles that treat token budget as a first-class constraint."

## The 10 principles (from SKILL.md)

| # | Title | Prescription |
|---|---|---|
| 1 | Token-efficient output | stdout in [TOON](https://toonformat.dev/) (Token-Oriented Object Notation), "~40% token savings over equivalent JSON". Keep internals in JSON; convert at the output boundary. |
| 2 | Minimal default schemas | Lists default to 3-4 fields (id, title, status); long bodies only in detail views; `--fields` to request more; default limits high enough to cover the common case in one call. |
| 3 | Content truncation | Truncate large text (500-1500 chars) but never omit it; show the total size; suggest `--full` only when actually truncated. |
| 4 | Pre-computed aggregates | Include total counts ("30 of 847 total") and cheap derived status ("checks: 3/3 passed") to save follow-up calls. |
| 5 | Definitive empty states | Say "0 closed tasks found" explicitly; empty output makes agents re-run to verify. |
| 6 | Structured errors & exit codes | Idempotent mutations (already closed = no-op, exit 0). Errors on **stdout** in the same structured format, with an actionable `help:` line; never leak dependency errors or stack traces. No interactive prompts. Reject unknown flags (exit 2) and list the valid flags inline; renamed flags get a targeted hint. stdout = data/errors/suggestions, stderr = progress/debug; exit 0 success, 1 error, 2 usage. |
| 7 | Ambient context | An explicit, user-invoked setup command installs a session hook (Claude Code `SessionStart`, Codex, OpenCode) that prints a compact, directory-scoped dashboard at session start; idempotent and path-repairing. Secondary: ship an Agent Skill generated from the same text as the no-args home view, with a CI drift check. |
| 8 | Content first | Running with no arguments shows live state plus a few next-step hints, not a usage manual. |
| 9 | Contextual disclosure | After output, suggest a few complete next commands (`help[n]:` block) with placeholders like `<id>`, carrying forward disambiguating flags; omit them when the output is self-contained; on errors suggest the fixing command, not "see --help". |
| 10 | Consistent way to get help | Home view starts with `bin:` (executable path, `~`-collapsed) and a one-line `description:`; every subcommand has a concise `--help` (flags with defaults, required args, 2-3 examples); `-v`/`-V`/`--version` print the bare version, fast. |

### Command shape

AXI does not mandate a grammar. Its examples use `tool <noun> <verb> [args] --flags`
(`tasks view 42 --full`, `tasks list --state closed`), per-subcommand flag sets, everything
completable by flags alone, and a no-args home view. gh-axi keeps "the same subcommands as `gh`"
(GitHub STUDY.md).

### Output example (SKILL.md section 8)

```
$ tasks
tasks[3]{id,title,status}:
  1,Fix auth bug,open
  2,Add pagination,open
  3,Update docs,closed
help[2]:
  Run `tasks view <id>` to see full details
  Run `tasks create --title "..."` to add a task
```

TOON is an independent format (MIT, Johann Schopplich, <https://github.com/toon-format/toon>):
an indentation-based encoding of the JSON data model with a tabular form for uniform arrays
(`name[N]{fields}:` header, then CSV-like rows). Its site claims 42.6% fewer tokens than JSON at
equal retrieval accuracy; the savings come mostly from uniform arrays of objects.

## Comparison with plain text and JSON conventions

| | Human-style text CLI | `--json` CLI | AXI |
|---|---|---|---|
| Format | Free-form tables and prose | JSON (verbose: repeated keys, braces, quotes) | TOON: structured like JSON, compact like a table |
| Default payload | Whatever humans want | Often every field | 3-4 fields, truncated bodies, plus counts |
| Errors | stderr prose | Varies; often stderr | stdout, structured, with a fix command |
| Empty result | Often blank | `[]` | Explicit sentence with context |
| Discovery | `--help` / man pages up front | Same | Learn by use: home view + `help[]` next steps; concise per-command `--help` |
| Session context | None | None | Optional SessionStart hook dashboard |

The project's evidence (its own benchmarks, Claude Sonnet 4.6 as agent and judge, 5 repeats):

- GitHub, 17 tasks / 425 runs: gh-axi 100% success, $0.050/task; raw `gh` CLI 86%, $0.054;
  GitHub MCP 82-87%, $0.10-0.15
  ([STUDY.md](https://github.com/kunchenguid/axi/blob/main/bench-github/published-results/STUDY.md)).
- Browser, 14 tasks / 490 runs: chrome-devtools-axi 100%, $0.074/task; chrome-devtools-mcp 99%,
  $0.101.

How to read these:

- The CLI-vs-AXI gap comes mainly from aggregates and next-step hints, not from TOON. The study
  says simple lookups perform "similarly"; the largest single gap is `list_labels` (AXI 5/5, CLI
  0/5) "because it includes `totalCount`", and multi-step investigations benefit from the
  suggestions. Input tokens are nearly equal (46K vs 47K per task).
- The MCP gap is mostly schema overhead (137K-176K input tokens vs 46-47K for CLI/AXI).
- Caveats: author-run, one model, read-only public tasks, LLM judge, small samples; no
  independent replication found. Whether the baseline `gh` condition used `--json` was not
  checked.

## Relevance to flow v2 (observations, not decisions)

- The flow v2 map requires everything the web app shows to be available to agents through a CLI,
  with no MCP server. AXI is a CLI-only design aimed at exactly that.
- The principles most clearly worth adopting are cheap and format-independent: a home view
  showing live sessions (8), counts and derived statuses (4), explicit empty states (5),
  structured stdout errors with a fix command and strict flag validation (6), next-step hints
  (9), concise per-command help (10). flow v1 already uses the "ambient context" idea (7)
  through hooks.
- TOON (1) is the most debatable choice: a newer format with fewer parsers than JSON, and the
  benchmark does not isolate its effect. A plausible middle ground is TOON by default with
  `--json` for programs; AXI says nothing against offering JSON as an extra.
- The SDK is JS-only (`axi-sdk-js`); a Rust CLI would implement the conventions itself (whether
  a TOON Rust crate exists was not checked).

## Sources

- <https://github.com/kunchenguid/axi> (README.md, principles.yaml, VISION.md, `.agents/skills/axi/SKILL.md`)
- <https://github.com/kunchenguid/axi/blob/main/bench-github/published-results/STUDY.md>
- <https://axi.md>
- <https://toonformat.dev/>, <https://github.com/toon-format/toon>
