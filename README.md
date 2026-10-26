# flow

`flow` is a command-line tool that turns one Claude Code session into a supervisor for
worker sessions running across several cloud development environments. It dispatches
tasks, watches their progress, and reports back, keeping all durable state in a plain-file
state directory.

## Build

```sh
cargo build --release
```

## State directory

`flow` reads and writes state in `$FLOW_STATE` when set, otherwise in `state/` at the root
of this repository. `state/` is expected to be a symlink to shared storage and is not
committed. Every command exits with code 3 when the state directory is missing.

## Docs

- Design: [docs/superpowers/specs/2026-10-26-flow-design.md](docs/superpowers/specs/2026-10-26-flow-design.md)
- Implementation plan: [docs/superpowers/plans/2026-10-26-flow-implementation-plan.md](docs/superpowers/plans/2026-10-26-flow-implementation-plan.md)
