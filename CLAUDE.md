# flow

This repo builds flow v2: one Rust crate with a shared library (`src/lib.rs`) and two
binaries, the `flow` CLI (`src/bin/flow.rs`) and the `flowd` daemon (`src/bin/flowd.rs`).

- Use the vocabulary of `GLOSSARY.md` in code, docs and conversation. Hard-to-reverse
  decisions are recorded in `docs/adr/`.
- Build and check with `cargo build`, `cargo test` and `cargo clippy --all-targets`.
- Tests run the binaries as black boxes through `tests/common` (`TestStore`), which points
  `FLOW_STORE` at a temp dir. Assert on argv, stdin and environment in, stdout JSON and exit
  code out, never on Store files or library internals. The one use of the library in tests
  is writing backdated state through the Store's `put_facet` with an explicit `at`.
- `flow` prints JSON on stdout; errors are `{error, help}` JSON with exit code 1 (error) or
  2 (usage).
- The web app is TypeScript in `web/src`, bundled by esbuild (its only dev dependency) into
  `web/dist` (`index.html`, `favicon.svg`, `app.js`, `app.css`), which `flowd` embeds
  through its `ASSETS` table. `web/dist` is committed, so `cargo build` needs no Node;
  `build.rs` fails the build with a clear message if a bundle file is missing. After changing `web/src`, run
  `npm --prefix web ci && npm --prefix web run build` and commit `web/dist` with the change.
- `node --test` (from the repo root or `web/`, after `npm --prefix web ci`) runs the view-model
  tests in `web/src/*.test.ts` and fails if `web/dist` is stale. Node runs the `.ts` files
  directly, so imports name `.ts` files and the code uses only erasable TypeScript syntax.
