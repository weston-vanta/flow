# flow

flow shows every Claude Code Session on every CDE, each with the Stack of PRs it owns: to the
user in a local web app served by `flowd`, and to agents as JSON through the `flow` CLI. It only
observes; it dispatches and supervises nothing.

This is flow v2, under construction. Terms are defined in [GLOSSARY.md](GLOSSARY.md) and the
hard-to-reverse choices in [docs/adr/](docs/adr/).

## Install

On the Flow CDE, `scripts/install` builds the frontend and both release binaries and installs
`flow` and `flowd` into `/efs-home/flow/bin` (or the directory given as its argument),
replacing each by rename. A running `flowd` keeps its old binary until restarted. It also
installs the Claude Code plugin marketplace in `claude/` to `/efs-home/flow/claude` (beside the
bin directory): marketplace `flow`, plugin `flow`, whose hooks run `flow hook` on every Session
event. Each CDE's dotfiles enable `flow@flow` through `extraKnownMarketplaces` (a `directory`
source at `/efs-home/flow/claude`) and `enabledPlugins`, put the bin directory on PATH, and
point agents at `flow guide`, which prints [the agent note](docs/agent-note.md).
