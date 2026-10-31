#!/usr/bin/env bash
# Rewrites a flow state directory into the formats flow reads from 2026-10-29 on, which no
# longer fill in what older records lack. Exit the supervisor first, so nothing writes state
# while it runs; `flow up` cannot read a lock that still needs migrating. Safe to run again:
# a migrated directory changes nothing.
#
#   migrate-2026-10-29.sh [--dry-run] <state-dir>
#
# --dry-run prints every change and writes nothing. Needs jq.
set -euo pipefail

say() { printf '%s\n' "$*"; }
fail() {
  printf 'migrate: %s\n' "$*" >&2
  exit 1
}

dry_run=0
state=
for arg in "$@"; do
  case $arg in
  --dry-run) dry_run=1 ;;
  -*) fail "unknown option $arg" ;;
  *)
    [[ -z $state ]] || fail "one state directory only"
    state=$arg
    ;;
  esac
done
[[ -n $state ]] || fail "usage: migrate-2026-10-29.sh [--dry-run] <state-dir>"
[[ -d $state ]] || fail "$state is not a directory"
command -v jq >/dev/null || fail "jq is required"
state=${state%/}

labels=()
declare -A counts=()

count() {
  [[ -v "counts[$1]" ]] || labels+=("$1")
  counts[$1]=$((${counts[$1]:-0} + $2))
}

# Replaces $1 with $2, keeping $1's mode.
replace() {
  chmod --reference="$1" "$2"
  mv "$2" "$1"
}

# For the JSON file $1: when jq filter $3 holds, prints change $2 and applies jq filter $4.
change() {
  local file=$1 label=$2 needs=$3 transform=$4 holds
  [[ -f $file ]] || return 0
  holds=$(jq "$needs" "$file") || fail "cannot read $file"
  count "$label" 0
  [[ $holds == true ]] || return 0
  say "${file#"$state"/}: $label"
  count "$label" 1
  ((dry_run)) && return 0
  local tmp
  tmp=$(mktemp "$file.XXXXXX")
  jq "$transform" "$file" >"$tmp"
  replace "$file" "$tmp"
}

# change for each line of the JSON-lines file $1 that holds a wake. A line that does not parse,
# such as one torn by a crash, is kept byte for byte.
change_lines() {
  local file=$1 label=$2 needs=$3 transform=$4 seqs
  [[ -f $file ]] || return 0
  local parsed="(try fromjson catch null) as \$wake"
  seqs=$(jq -rR "$parsed | \$wake | select(. != null and ($needs)) | .seq" "$file")
  count "$label" 0
  [[ -n $seqs ]] || return 0
  while read -r seq; do
    say "${file#"$state"/}: seq $seq: $label"
    count "$label" 1
  done <<<"$seqs"
  ((dry_run)) && return 0
  local tmp
  tmp=$(mktemp "$file.XXXXXX")
  jq -rR "$parsed | if \$wake != null and (\$wake | $needs) then \$wake | $transform | tojson else . end" \
    "$file" >"$tmp"
  if [[ -n $(tail -c1 "$file") ]]; then
    truncate -s -1 "$tmp"
  fi
  replace "$file" "$tmp"
}

lacks_session='(.herdr | type) == "object" and (.herdr | has("session") | not)'
add_session='.herdr.session = "flow"'

for dir in "$state"/tasks/*/; do
  [[ -d $dir ]] || continue
  change "${dir}task.json" "task worktree renamed to cwd" 'has("worktree")' \
    '.cwd = (.cwd // .worktree) | del(.worktree)'
  change "${dir}task.json" "task origin set to dispatched" 'has("origin") | not' \
    '.origin = "dispatched"'
  change "${dir}status.json" "status herdr session set to flow" "$lacks_session" "$add_session"
done

change "$state/supervisor/lock.json" "lock herdr session set to flow" "$lacks_session" "$add_session"

adopted='.obs.type == "adopted-changed"'
change_lines "$state/supervisor/wake-queue.jsonl" "wake adopted-changed renamed to observed-changed" \
  "$adopted" '.obs.type = "observed-changed"'
change_lines "$state/supervisor/wake-queue.jsonl" "wake observed-changed title set to empty" \
  "(.obs.type == \"observed-changed\" or $adopted) and (.obs | has(\"title\") | not)" \
  '.obs.title = ""'
change "$state/supervisor/watch-state.json" "watch-state adopted-changed renamed to observed-changed" \
  'any(.. | objects; .type == "adopted-changed")' \
  'walk(if type == "object" and .type == "adopted-changed" then .type = "observed-changed" else . end)'

for file in "$state"/hosts/*.json; do
  [[ -f $file ]] || continue
  change "$file" "host empty ssh_alias set to the Ona alias" '(.ssh_alias // "") == ""' \
    '.ssh_alias = "\(.env_id).ona.environment"'
  change "$file" "host agent herdr session set to flow" 'any(.agents[]?; has("session") | not)' \
    '.agents |= map(if has("session") then . else .session = "flow" end)'
  change "$file" "host worktree repo set to empty" 'any(.worktrees[]?; has("repo") | not)' \
    '.worktrees |= map(if has("repo") then . else .repo = "" end)'
  change "$file" "host pr repo set to empty" 'any(.prs[]?; has("repo") | not)' \
    '.prs |= map(if has("repo") then . else .repo = "" end)'
done

if ((dry_run)); then
  say "dry run: nothing written"
fi
for label in "${labels[@]}"; do
  say "${counts[$label]} $label"
done
