#!/usr/bin/env bash
# Brings this machine's flow checkout up to date: pulls it, links its state/ to the shared
# state directory, builds the binary, and links the binary into $FLOW_BIN_DIR. Safe to run
# again, and meant to run on every machine start. It never clones: clone flow to $FLOW_DIR
# once, then run the clone's copy of this script.
#
#   FLOW_DIR         the flow checkout (default /workspaces/flow)
#   FLOW_STATE_DIR   the shared state directory state/ links to (default /efs-home/flow/state)
#   FLOW_BIN_DIR     the directory the flow binary is linked into (default $HOME/.local/bin)
#   FLOW_SKIP_BUILD  set to 1 to skip the build step
set -euo pipefail

FLOW_DIR="${FLOW_DIR:-/workspaces/flow}"
FLOW_STATE_DIR="${FLOW_STATE_DIR:-/efs-home/flow/state}"
FLOW_BIN_DIR="${FLOW_BIN_DIR:-$HOME/.local/bin}"

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
# Absolute, so the binary link resolves from $FLOW_BIN_DIR.
FLOW_DIR=$(CDPATH='' cd -- "$FLOW_DIR" && pwd)

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

bin="$FLOW_DIR/target/release/flow"
if [[ ${FLOW_SKIP_BUILD:-} != 1 ]]; then
  if ! command -v cargo >/dev/null; then
    fail "build: cargo not found; install a Rust toolchain (https://rustup.rs) or set FLOW_SKIP_BUILD=1"
  fi
  bin_sum=$(checksum "$bin")
  # cargo's own fingerprinting decides whether the binary is stale; an up-to-date build is a no-op.
  cargo build --release --quiet --manifest-path "$FLOW_DIR/Cargo.toml" --target-dir "$FLOW_DIR/target"
  if [[ $(checksum "$bin") == "$bin_sum" ]]; then
    say "build: already"
  else
    say "build: done"
  fi
fi

# Linked even without a build: the link dangles until the first build, which ln does not mind.
bin_link="$FLOW_BIN_DIR/flow"
if [[ -L $bin_link && $(readlink "$bin_link") == "$bin" ]]; then
  say "link: already"
elif [[ -e $bin_link && ! -L $bin_link ]]; then
  fail "link: $bin_link exists and is not a symlink; move it aside so flow can link its binary there"
else
  mkdir -p "$FLOW_BIN_DIR"
  ln -sfn "$bin" "$bin_link"
  say "link: done"
fi

case ":$PATH:" in
  *":$FLOW_BIN_DIR:"*) ;;
  *) say "PATH: add $FLOW_BIN_DIR to your PATH" ;;
esac
