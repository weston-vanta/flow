# Running skills the model can't invoke

Research for [Running skills the model can't invoke](../../.scratch/compose-skills/issues/02-invoking-locked-skills.md)
on the compose-skills map (the ticket is local scratch, not on this branch).

Question: what does `disable-model-invocation: true` stop in Claude Code, can a Workflow skill
still get an agent to follow such a skill, and what does Claude Code offer for chaining or
composing skills? Affected skills: `to-spec`, `to-tickets`, `wayfinder`, `implement`,
`implement-spec`, `grill-with-docs`.

Tested on Claude Code 2.1.331, 2026-11-25.

## Summary

- `disable-model-invocation: true` stops every path where the model starts the skill: the Skill
  tool in the main session, the Skill tool in any Subagent, preloading through a subagent's
  `skills` field, and discovery (the description is not listed to the model). Only the
  Director typing `/name` runs it.
- The Director's permission covers only the one message in which they typed `/name`, and
  only the main session. It does not reach a Subagent, and it does not reach a locked skill
  that another skill's text asks for. This holds even when the Director started the outer skill.
- Reading the locked skill's `SKILL.md` with the Read tool and following it works mechanically:
  nothing blocks it. But Claude Code's own refusal tells the model "Do not replicate this
  skill's workflow by other means", so the workaround goes against the stated intent, and a
  model that has already seen the refusal declines it.
- `skillOverrides` can't unlock it: `"on"` left the block in place, and plugin skills ignore
  `skillOverrides` entirely. Only editing the frontmatter of a copy unlocks a skill.
- Claude Code has no skill-to-skill reference or dependency mechanism. The documented ways to
  combine skills are: one skill's text telling the model to call the Skill tool for another,
  a subagent's `skills` preload list, `context: fork`, and the Director stacking several
  `/name`s at the start of one message. Plugin `dependencies` only make sure another plugin is
  installed and enabled.
- mattpocock's own skills compose in one direction only: a locked entry-point skill the
  Director types (`wayfinder`, `implement`, `implement-spec`, `grill-with-docs`) calls
  model-invocable skills (`grilling`, `domain-modeling`, `research`, `prototype`, `tdd`,
  `code-review`) through the Skill tool. None of them calls a locked skill.

## What the flag stops

The skills docs list three things the flag does
([Frontmatter reference](https://code.claude.com/docs/en/skills)):

> Set to `true` to prevent Claude from automatically loading this skill. Use for workflows
> you want to trigger manually with `/name`. Also prevents the skill from being preloaded into
> subagents and from running when a session-scoped scheduled task fires with the skill as its
> prompt.

And the invocation-control table in the same page:

> | `disable-model-invocation: true` | Yes | Not on its own | Description not in context, full skill loads when invoked |

On permission from the Director's message (same page):

> If Claude tries when your message doesn't include `/deploy` as a separate word with no
> punctuation attached, Claude Code blocks the call. Expect Claude to suggest running
> `/deploy` yourself.

On subagents ([Subagents: preload skills](https://code.claude.com/docs/en/sub-agents)):

> You can't preload skills that set `disable-model-invocation: true`, since preloading draws
> from the same set of skills Claude can invoke.

> If a listed skill is missing or disabled, […] Claude Code skips it and logs a warning to the
> debug log.

Relevant changelog entries ([CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)):

- 2.1.222: "Improved the refusal when Claude tries to invoke a skill with
  `disable-model-invocation`: Claude is now told to ask you to run the skill instead of
  replicating its workflow"
- 2.1.287: "Improved `/skill` names typed mid-message: Claude is now told they are skills,
  including `disable-model-invocation` ones"
- 2.1.110: "Fixed skills with `disable-model-invocation: true` failing when invoked via
  `/<skill>` mid-message"
- 2.1.129: "`skillOverrides` setting now works: `off` hides from model and `/`,
  `user-invocable-only` hides from model only, `name-only` collapses description"

## Experiments

Setup: a scratch project with these files.

- `toy`: a skill with `disable-model-invocation: true` whose body says "Reply with exactly the
  text ZEBRA-TOKEN-42".
- `wrapper-skill`: a model-invocable skill that says "Call the Skill tool with "toy" and follow
  it".
- `wrapper-read`: a model-invocable skill that says "Read the file .claude/skills/toy/SKILL.md
  and follow its instructions".
- `toy-agent`: a custom subagent with `skills: [toy]` whose body says "reply NO-PRELOAD" if it
  has no preloaded skills.

Each case ran as `claude -p "<prompt>" --model sonnet --output-format stream-json --verbose
--allowedTools "Skill Read Agent"`. The tool calls were read from the stream.

| Case | Prompt | What happened |
| :- | :- | :- |
| A | `Use the toy skill to get the toy token.` | The model said `toy` wasn't in its skill list. It tried `toy-agent`, which replied NO-PRELOAD (the preload was skipped silently). It then ran `wrapper-skill`, whose `Skill(toy)` call was refused. No token. |
| B | `/toy` | Ran the skill. Token returned. |
| C | `please run /toy now` | The model called `Skill(toy)` and it was allowed. Token returned. |
| D | `/wrapper-skill` | `Skill(toy)` refused. No token. The model told the Director to run `/toy` themselves. |
| E | `/wrapper-read` | The model read `toy/SKILL.md` and followed it. Token returned. Nothing blocked it. |
| F | `/wrapper-skill /toy` | Under `-p` only the first skill loads and `/toy` became its argument. `Skill(toy)` was refused. |
| G | Spawn a general-purpose Subagent told to call `Skill(toy)` | Refused inside the Subagent. |
| H | `Spawn a general-purpose subagent to run /toy …` | The main session's message named `/toy`, but the Subagent's `Skill(toy)` was still refused. |
| I | `/wrapper-skill` with `skillOverrides: {"toy": "on"}` in `.claude/settings.local.json` | Still refused. |

The refusal text, the same in every refused case:

> Skill toy cannot be used with Skill tool due to disable-model-invocation. Ask the user to run
> /toy themselves — it cannot be invoked via the Skill tool. Do not replicate this skill's
> workflow by other means — it is reserved for explicit user invocation.

In D, F and I, the model explicitly declined to work around the refusal. Case E worked only
because the model never saw a refusal.

## Ways to compose skills

The documented options:

- **Skill text that calls the Skill tool.** Not a named feature, but the mattpocock skills use
  it throughout. For example, `wayfinder` says "Always call the Skill tool twice, for
  "grilling" and "domain-modeling"", and `implement` says "Call the Skill tool with "tdd""
  (local: `~/.claude/plugins/cache/claude-plugins-official/mattpocock-skills/*/skills/engineering/`).
  It works only when the target is model-invocable.
- **Subagent `skills` preload.** This injects the full text of up to 32 skills at startup.
  Locked skills are skipped
  ([Subagents](https://code.claude.com/docs/en/sub-agents)).
- **`context: fork`.** This runs the skill's own text as a Subagent's prompt. It changes where
  a skill runs, not who may start it ([Skills: run skills in a subagent](https://code.claude.com/docs/en/skills)).
- **Stacking.** "you can stack several skills at the start of one message. Through the Agent
  SDK or `claude -p`, Claude Code loads only the first skill" (up to five more at the terminal
  prompt). This is something the Director types, not something a skill can do
  ([Skills](https://code.claude.com/docs/en/skills)).
- **Plugin `dependencies`.** "Plugins that must be enabled for this one to work", with semver
  ranges resolved against git tags
  ([Plugin dependencies](https://code.claude.com/docs/en/plugins/dependencies)). This handles
  install and enablement only. Calls still go through the Skill tool by namespaced name
  (`plugin-name:skill-name`), so a dependency's locked skills stay locked.
- **`skillOverrides`.** This only hides skills, and "Plugin skills are not affected by
  `skillOverrides`" ([Skills: override skill visibility](https://code.claude.com/docs/en/skills)).
  Case I showed `"on"` doesn't override the frontmatter flag.

## What this means for the affected skills

All six are locked in both installs: plugin 1.3.1 and the 1.3.2 symlinks in
`/workspaces/flow/.claude/skills`. Three skills on the compose-skills worked case are locked:
`to-spec`, `to-tickets` and `wayfinder`. `grilling` and `domain-modeling` are not. For a
Workflow skill there are three honest ways to use a locked skill:

1. Hand the turn to the Director: "run `/to-spec`". This is the behaviour Claude Code itself
   prompts for.
2. Have the Director name it mid-message (`go ahead and /to-spec`), which permits the main
   session for that one message only, never a Subagent.
3. Use an unlocked copy, which is a fork of the skill. That is what the Director's standing
   position wants to avoid.

Reading `SKILL.md` directly works, but it goes against the refusal's explicit instruction and
doesn't work once the model has seen a refusal.

## Not verified

- Stacking at the interactive terminal prompt (only `-p` was tested). The docs say all leading
  skills load there.
- Whether a mid-message `/toy` in an interactive session permits a nested `Skill(toy)` that an
  outer skill's text requests. Case F covered this only under `-p`, where the trailing `/toy`
  became an argument.
- Behaviour under the Agent SDK, and for locked skills in a plugin that is a declared
  dependency (assumed identical: the check is on the Skill tool call).
- The debug-log warning for a skipped preload (not inspected).
- Models other than Sonnet. The block is enforced by Claude Code, but whether a model declines
  the Read workaround depends on the model.
