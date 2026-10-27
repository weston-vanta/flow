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

# Seconds since the epoch at which $1 was last modified (GNU stat, else BSD stat).
mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1"; }

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
  bin="$FLOW_DIR/target/release/flow"
  head_time=$(git -C "$FLOW_DIR" log -1 --format=%ct)
  if [[ -x $bin ]] && (($(mtime "$bin") >= head_time)); then
    say "build: already"
  else
    cargo build --release --quiet --manifest-path "$FLOW_DIR/Cargo.toml" --target-dir "$FLOW_DIR/target"
    # cargo leaves an up-to-date binary untouched; mark it as built for this commit.
    touch "$bin"
    say "build: done"
  fi
fi

case ":$PATH:" in
  *":$FLOW_DIR/target/release:"*) ;;
  *) say "PATH: add $FLOW_DIR/target/release to your PATH" ;;
esac
