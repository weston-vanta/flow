# Installing and pinning outside skills across CDEs

Question (from `.scratch/compose-skills/issues/03-installing-outside-skills.md`): which ways
does Claude Code offer to install a third-party skill set such as
`mattpocock-skills@claude-plugins-official` so that every CDE and every repo sees exactly one
pinned version? When the same skills are installed twice, which copy wins, and how do
`mattpocock-skills:grilling` and `grilling` resolve?

Researched 2026-11-25 against Claude Code 2.1.331 on the Flow CDE. Sources are the official
docs at code.claude.com (fetched that day), the local `~/.claude` state, and two read-only
`claude -p` runs. Nothing was installed, enabled or changed.

## Local state

| What | Where | Shared across CDEs? |
| :- | :- | :- |
| `~/.claude` | symlink to `/efs-home/.claude` (EFS) | yes |
| `~/.claude/plugins/installed_plugins.json`, `known_marketplaces.json`, `cache/` | under `/efs-home/.claude/plugins` | yes |
| `~/.claude/settings.json` (user settings: `enabledPlugins`, `extraKnownMarketplaces`) | symlink to `~/dotfiles/agents/claude/settings.json`; `~/dotfiles` is on the CDE's overlay root | no, one per CDE (same git repo, separate checkouts) |
| `~/.claude/skills/` (personal skills: `explaining-code`, `my-voice`) | on EFS | yes |
| mattpocock clone (version 1.3.2 per its `.claude-plugin/plugin.json`, HEAD `4ae2317`, no tags) | `~/.local/share/mattpocock-skills`, on the overlay root | no, per CDE |
| `/workspaces/flow/.claude/skills/*` | 26 untracked symlinks into that clone | per clone, and the targets exist only on CDEs that have the clone |
| `mattpocock-skills@claude-plugins-official` | installed at user scope, version 1.3.1, `gitCommitSha 49dd158…` | install record shared; enable state per CDE |

The official marketplace's entry for the plugin is
`{"source": "url", "url": "https://github.com/mattpocock/skills.git", "sha": "49dd158d…"}`
(`~/.claude/plugins/marketplaces/claude-plugins-official/.claude-plugin/marketplace.json`).
So Anthropic, not us, chooses the pinned commit, and the cache directory is named `1.3.1`
because the manifest's `version` at that commit says 1.3.1.

`claude plugin list` on this CDE shows the plugin as `Scope: user`, `Status: ✘ disabled`:
the install record (EFS) is there, but this CDE's `settings.json` has no
`enabledPlugins` entry for it (and the dotfiles history never had one). The plugin was
presumably installed from a CDE whose settings did enable it. That split, one shared install
record and one enable flag per CDE, is the main CDE-specific trap.

## How names resolve when a skill is installed twice

Docs ([Skills: resolve skills that share a name](https://code.claude.com/docs/en/skills)):

- Enterprise beats personal (`~/.claude/skills`), personal beats project (`.claude/skills`)
  for the same bare name.
- "A plugin skill and a skill at any of the locations above: Both load, because plugin
  skills are namespaced as `/plugin-name:skill-name`." A plugin skill never shadows a bare
  one.
- Symlinked entries are followed, and a target reached through several locations loads once.
- Nested `.claude/skills` in subdirectories load lazily and get a path prefix
  (`/apps/web:deploy`).

Observed with `claude -p` (asking for the verbatim skill listing, no tools):

- From an empty scratch dir: no mattpocock skills at all (the plugin is disabled here and
  the project symlinks aren't in scope).
- From `/workspaces/flow` with `--plugin-dir` pointing at the cached 1.3.1 copy (a
  session-only load, no settings written): both `grilling` (project symlink, 1.3.2) and
  `mattpocock-skills:grilling` (plugin, 1.3.1) are listed, and likewise for
  `domain-modeling`, `tdd`, `pr`, `research` and the rest. Claude sees two differently
  versioned copies side by side and picks by description.

So with today's double install, nothing "wins": the bare name is the 1.3.2 symlink and the
prefixed name is the 1.3.1 plugin, and both are offered whenever the plugin is enabled.

Between two plugins with the same manifest name, [Plugin loading: name
conflicts](https://code.claude.com/docs/en/plugins/loading.md#name-conflicts) gives the
order: managed-settings id, then `--plugin-dir`/`CLAUDE_CODE_PLUGIN_DIRS`, then an
installed marketplace plugin, then a skills-directory plugin (personal over project), then a
claude.ai synced plugin.

### `disable-model-invocation`

Of the five skills the map composes (`to-spec`, `to-tickets`, `wayfinder`, `grilling`,
`domain-modeling`), three set `disable-model-invocation: true` in both 1.3.1 and 1.3.2:
`to-spec`, `to-tickets` and `wayfinder` (also `grill-with-docs`, `triage`, `implement`,
`handoff` and others). The docs: the description is "not in context", "Claude can't invoke
the skill on its own", and if it tries "Claude Code blocks the call". The exception is the
user typing the skill's name in their message, which "counts as your permission for that
message". The setting also stops the skill being preloaded into subagents. The `claude -p`
listings above confirm these skills are absent from Claude's skill list. A Workflow skill
therefore cannot call these Guidance skills through the Skill tool on its own; the Director
has to type them, or flow has to read their `SKILL.md` files directly.

## The options

### A. Official marketplace plugin at user scope (today's partial setup)

- Install once (records and cache land on EFS, so every CDE has the files). Enable with
  `"mattpocock-skills@claude-plugins-official": true` in user `enabledPlugins`, which on
  these CDEs means the dotfiles `settings.json`, the same place `flow@flow` is enabled.
- Every repo sees it; skills are prefixed `mattpocock-skills:`.
- Pinning: the version is whatever sha Anthropic's catalog names. Auto-update is on by
  default for `claude-plugins-official`, so a new sha in the catalog moves every CDE after
  the next session start ([Install: keep plugins
  updated](https://code.claude.com/docs/en/plugins/install.md#keep-plugins-updated)). You can
  turn auto-update off for the marketplace (per `known_marketplaces.json`, which is on EFS,
  or `autoUpdate` on an `extraKnownMarketplaces` entry), but you can't pin the catalog
  itself: a SHA `ref` "is refused for a marketplace with a reserved name, such as
  `claude-plugins-official`"
  ([marketplace reference](https://code.claude.com/docs/en/plugins/marketplace-reference.md#fields-by-type)).
  Today that pin is 1.3.1, behind upstream's 1.3.2.

### B. Our own marketplace entry pinned by sha

- Add an entry to a marketplace we control, for example the `flow` marketplace already at
  `/efs-home/flow/claude`, with
  `{"source": "github", "repo": "mattpocock/skills", "sha": "<40-char sha>"}` (or `url` /
  `git-subdir` with `sha`). `sha` wins over `ref`
  ([plugin sources](https://code.claude.com/docs/en/plugins/marketplace-reference.md#plugin-sources)).
- The plugin is copied into `cache/flow/mattpocock-skills/<version>/` (on EFS). Version
  comes from the manifest's `version`, then the entry's, then the commit sha
  ([how the version is computed](https://code.claude.com/docs/en/plugins/loading.md#how-claude-code-computes-the-version)).
  Moving the pin means editing the sha; if upstream forgets to bump `version`, also set
  `version` on the entry or omit both, or users stay on the cached copy.
- Third-party marketplaces don't auto-update by default, and a `directory` marketplace is
  read in place with no refresh step, so nothing moves until flow's install script ships a
  new `marketplace.json`.
- Enabled the same way as A, in user `enabledPlugins` in each CDE's settings. When user
  settings declare the marketplace in `extraKnownMarketplaces` and enable the plugin, Claude
  Code clones missing marketplaces and fetches enabled plugins that aren't cached in the
  background after session start, and an external-source plugin is fetched only when user,
  untracked local, `--settings` or managed settings enable it
  ([plugin loading](https://code.claude.com/docs/en/plugins/loading.md#plugins-and-marketplaces-that-arent-on-disk-at-session-start)).
  So a dotfiles entry is enough on a fresh CDE.
- Same prefix as A: the plugin id is `mattpocock-skills@flow` and skills are
  `mattpocock-skills:<skill>`. If A stays installed and enabled too, both copies share the
  manifest name `mattpocock-skills`; their skill prefixes would collide and the conflict
  rules apply (not tested).

### C. Project scope (`.claude/settings.json` in each repo)

- `enabledPlugins` committed in the repo reaches everyone who clones it, but "doesn't
  download it to their machines": each clone needs
  `claude plugin install <id> --scope project` once, unless a user-scope install already
  covers it ([install scopes](https://code.claude.com/docs/en/plugins/install.md#choose-an-install-scope)).
  Project-scope plugins also need the workspace trust dialog, which `-p` doesn't satisfy
  ([plugin loading](https://code.claude.com/docs/en/plugins/loading.md#trust-check-and-component-limits-for-repository-plugins)).
- Only that repo sees it, so it doesn't meet "every repo". Precedence is local > project >
  user per plugin id, so a repo can still turn a user-scope plugin off with `false`.

### D. Personal skills under `~/.claude/skills` (EFS)

- `~/.claude/skills` is on EFS, so a skill placed there appears on every CDE and in every
  repo with no per-CDE setting. Entries can be symlinks.
- Pinning is by whatever the files are: a clone on `/efs-home` checked out at a tag or sha,
  with per-skill symlinks from `~/.claude/skills`. Skills are bare-named (`grilling`) and
  beat a same-named project skill. Edits are picked up live in the session.
- Variant: a folder under `~/.claude/skills/` that has `.claude-plugin/plugin.json` loads as
  a skills-directory plugin, `<name>@skills-dir`, with no install step
  ([create: make a plugin load in every session](https://code.claude.com/docs/en/plugins/create.md#make-a-plugin-load-in-every-session)).
  The mattpocock repo root has that manifest, so one symlink
  `~/.claude/skills/mattpocock-skills -> /efs-home/<clone>` should load it as
  `mattpocock-skills@skills-dir` with prefixed skills. An installed marketplace plugin of
  the same manifest name beats it. Not verified: whether the symlinked plugin folder
  resolves, and whether its manifest's `skills` array is honoured there.
- No updates arrive by themselves; flow (or the Director) moves the checkout.

### E. Per-repo `.claude/skills` symlinks (today's other copy)

- Bare names, per repo, untracked here, and pointing at a per-CDE path
  (`~/.local/share/...` on the overlay root). On a CDE without that clone the links dangle.
  Fails both "every CDE" and "every repo" without per-repo, per-CDE setup.

## Summary

| Option | Every CDE | Every repo | Who pins | Name |
| :- | :- | :- | :- | :- |
| A. official plugin, user scope | files yes; enable per CDE (dotfiles) | yes | Anthropic's catalog sha, auto-updates | `mattpocock-skills:x` |
| B. own marketplace entry + `sha` | files yes; enable per CDE (dotfiles) | yes | us, via the entry's sha | `mattpocock-skills:x` |
| C. project scope | per clone install | only that repo | as A or B | `mattpocock-skills:x` |
| D. `~/.claude/skills` on EFS | yes, no setting | yes | us, via the clone's checkout | `x` (or prefixed as a skills-dir plugin) |
| E. per-repo symlinks | no (per-CDE target) | no | the clone's checkout | `x` |

To see exactly one version, only one of these may be active: today A (disabled here, maybe
enabled elsewhere) and E both exist.

## Not verified

- Whether the plugin is enabled on other CDEs (only this CDE's settings were read).
- Option B and the skills-dir variant of D end to end; the collision when A and B are both
  enabled.
- Whether a Workflow skill's instruction ("run `/to-spec`") counts as user permission for a
  `disable-model-invocation` skill; the docs tie the exception to the user's own message.
- The Claude Code changelog was only skimmed (its first 100k of 1.4M characters). The one
  relevant entry: 2.1.331 says marketplaces declared in managed `extraKnownMarketplaces`
  will auto-update by default from on or after 2026-11-30, which doesn't affect user-settings
  marketplaces.
