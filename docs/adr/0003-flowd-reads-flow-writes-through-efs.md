# `flowd` serves reads; Writers outside it write the Store directly

flow is one Rust library with two binaries: `flow`, the CLI that runs on every CDE, and `flowd`, a daemon on the Flow CDE that runs the PR poller, serves the browser-only TypeScript frontend, and exposes a localhost read API returning exactly the CLI's JSON. Writers outside `flowd` (Claude Code hooks, `flow wt create`) never send facts to `flowd`. They write the Store files on the shared `/efs-home` through the same Store module, because `flowd` is reachable only on the Flow CDE's localhost, and because Sessions should keep reporting while `flowd` is down. `flowd` does not watch the Store: inotify does not see writes made by other NFS clients, and every read is served fresh from the Store.

## Considered Options

- **Cron-run one-shot poller plus a Node server that runs the CLI as a child process**: nothing to keep alive, but there is no home for Writers that must hold connections open (a herdr-socket `liveness` subscriber, a change stream), and there is a server-side Node runtime.
- **Writers send facts to `flowd` over the network**: one writer process for the Store, but it would need a listener reachable from every CDE, and facts would be lost whenever `flowd` is down.

## Consequences

- The Store must take concurrent writes from several CDEs at the file level; `flowd` cannot serialize them.
- A future change stream has to detect changes by polling the Store, not by filesystem events.
