#!/usr/bin/env bash
# Brings this machine's flow checkout up to date: pulls it, links its state/ to the shared
# state directory, and builds the binary. Safe to run again, and meant to run on every machine
# start. It never clones: clone flow to $FLOW_DIR once, then run the clone's copy of this script.
#
#   FLOW_DIR         the flow checkout (default /workspaces/flow)
#   FLOW_STATE_DIR   the shared state directory state/ links to (default /efs-home/flow/state)
#   FLOW_SKIP_BUILD  set to 1 to skip the build step
set -euo pipefail

FLOW_DIR="${FLOW_DIR:-/workspaces/flow}"
FLOW_STATE_DIR="${FLOW_STATE_DIR:-/efs-home/flow/state}"

say() { printf 'install: %s\n' "$*"; }
fail() {
  printf 'install: %s\n' "$*" >&2
  exit 1
}

# A checksum of file $1, empty when it does not exist.
checksum() { if [[ -e $1 ]]; then cksum <"$1"; fi; }

if ! grep -qx 'name = "flow"' "$FLOW_DIR/Cargo.toml" 2>/dev/null; then
  fail "$FLOW_DIR is not a flow checkout (its Cargo.toml does not declare name = \"flow\"); clone flow there or set FLOW_DIR"
fi

before=$(git -C "$FLOW_DIR" rev-parse HEAD)
if git -C "$FLOW_DIR" pull --ff-only --quiet; then
  if [[ $(git -C "$FLOW_DIR" rev-parse HEAD) == "$before" ]]; then
    say "pull: already"
  else
    say "pull: done"
  fi
else
  say "pull: warning: git pull --ff-only failed; continuing with the checkout as it is" >&2
fi

if [[ -d $FLOW_STATE_DIR ]]; then
  say "state dir: already"
else
  mkdir -p "$FLOW_STATE_DIR"
  say "state dir: done"
fi
# Absolute, so the link works from anywhere and compares equal to its target on the next run.
FLOW_STATE_DIR=$(CDPATH='' cd -- "$FLOW_STATE_DIR" && pwd)

link="$FLOW_DIR/state"
if [[ -L $link ]]; then
  if [[ $(readlink "$link") == "$FLOW_STATE_DIR" ]]; then
    say "symlink: already"
  else
    ln -sfn "$FLOW_STATE_DIR" "$link"
    say "symlink: done"
  fi
elif [[ -d $link ]]; then
  fail "$link is a real directory, not a symlink; move its contents to $FLOW_STATE_DIR and remove it"
elif [[ -e $link ]]; then
  fail "$link exists and is not a symlink; remove it"
else
  ln -sfn "$FLOW_STATE_DIR" "$link"
  say "symlink: done"
fi

if [[ ${FLOW_SKIP_BUILD:-} != 1 ]]; then
  if ! command -v cargo >/dev/null; then
    fail "build: cargo not found; install a Rust toolchain (https://rustup.rs) or set FLOW_SKIP_BUILD=1"
  fi
  bin="$FLOW_DIR/target/release/flow"
  bin_sum=$(checksum "$bin")
  # cargo's own fingerprinting decides whether the binary is stale; an up-to-date build is a no-op.
  cargo build --release --quiet --manifest-path "$FLOW_DIR/Cargo.toml" --target-dir "$FLOW_DIR/target"
  if [[ $(checksum "$bin") == "$bin_sum" ]]; then
    say "build: already"
  else
    say "build: done"
  fi
fi

case ":$PATH:" in
  *":$FLOW_DIR/target/release:"*) ;;
  *) say "PATH: add $FLOW_DIR/target/release to your PATH" ;;
esac
