#!/usr/bin/env bash
# Delete merged remote branches; archive unmerged ones as tags, then delete.
#
# The owner asked on 2026-10-06 for a fully clean branch list. Deleting a
# branch nobody merged is how work disappears, so nothing here deletes a
# branch whose tip is not already in origin/main or held by an annotated
# `archive/<name>` tag that has been pushed to origin first.
#
# Dry-run by default: it prints what it would do and changes nothing beyond
# the fetch. `--apply` acts.
#
#   scripts/cleanup-branches.sh [--apply] [--open-heads FILE]
#
# --open-heads FILE   one branch per line: the heads of OPEN pull requests,
#                     which are never touched. The script has no GitHub API
#                     access, so this list is the caller's to supply, and
#                     --apply refuses to run without it -- a branch under
#                     review deleted out from under its PR is a closed PR.
#
# Every deletion is first put to .claude/hooks/guard-dangerous-command.py with
# the exact command about to run. A PreToolUse hook sees only the command line
# an agent typed, never the pushes a script makes, so without this step the
# guard would not see these deletions at all. The guard and this script then
# agree by construction rather than by two copies of the same rule.
#
# Exits nonzero if any tag push, deletion, or guard check fails.
set -euo pipefail

readonly REMOTE=origin
readonly PROTECTED=("main" "ccr-0c1bacf8-kla0dd" "HEAD")

apply=false
open_heads_file=""

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed -e '/^set -euo/d' -e 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --apply) apply=true ;;
    --open-heads)
      [ $# -ge 2 ] || { echo "error: --open-heads needs a file" >&2; exit 64; }
      open_heads_file="$2"
      shift
      ;;
    --open-heads=*) open_heads_file="${1#--open-heads=}" ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; usage >&2; exit 64 ;;
  esac
  shift
done

root="$(git rev-parse --show-toplevel)"
cd "$root"
readonly GUARD="$root/.claude/hooks/guard-dangerous-command.py"

declare -A open_heads=()
if [ -n "$open_heads_file" ]; then
  [ -r "$open_heads_file" ] || { echo "error: cannot read $open_heads_file" >&2; exit 66; }
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    case "$line" in "" | "#"*) continue ;; esac
    open_heads["$line"]=1
  done <"$open_heads_file"
elif $apply; then
  echo "error: --apply requires --open-heads FILE (heads of open pull requests)" >&2
  exit 64
else
  echo "warning: no --open-heads given; branches under open PRs are not skipped in this dry run" >&2
fi

[ -r "$GUARD" ] || { echo "error: guard not found at $GUARD" >&2; exit 69; }

git fetch --prune "$REMOTE"

is_protected() {
  local name="$1" p
  for p in "${PROTECTED[@]}"; do
    [ "$name" = "$p" ] && return 0
  done
  return 1
}

# Ask the guard about the exact deletion command. Prints its refusal, if any.
guard_allows() {
  local branch="$1" verdict
  verdict="$(python3 - "$REMOTE" "$branch" <<'PY' | python3 "$GUARD" 2>&1 >/dev/null
import json, shlex, sys
remote, branch = sys.argv[1], sys.argv[2]
command = "git push " + shlex.quote(remote) + " --" + "delete " + shlex.quote(branch)
print(json.dumps({"tool_input": {"command": command}}))
PY
  )" && return 0
  printf '%s' "$verdict" | head -n 1
  return 1
}

today="$(date -u +%Y-%m-%d)"
failures=0
merged_count=0
unmerged_count=0
open_count=0
rows=()

row() { rows+=("$1"$'\t'"$2"$'\t'"$3"); }

while IFS= read -r branch; do
  [ -n "$branch" ] || continue
  is_protected "$branch" && continue
  if [ -n "${open_heads[$branch]+set}" ]; then
    open_count=$((open_count + 1))
    row "$branch" "OPEN-PR" "skipped (head of an open pull request)"
    continue
  fi

  tip="$(git rev-parse --verify -q "refs/remotes/$REMOTE/$branch")" || {
    failures=$((failures + 1))
    row "$branch" "ERROR" "tip not resolvable; left alone"
    continue
  }

  if git merge-base --is-ancestor "$tip" "refs/remotes/$REMOTE/main"; then
    merged_count=$((merged_count + 1))
    class=MERGED
    tag=""
  else
    unmerged_count=$((unmerged_count + 1))
    class=UNMERGED
    tag="archive/${branch//\//-}"
  fi

  if ! $apply; then
    if [ -n "$tag" ]; then
      existing="$(git rev-parse --verify -q "refs/tags/$tag^{commit}" || true)"
      if [ -n "$existing" ] && [ "$existing" != "$tip" ]; then
        row "$branch" "$class" "would FAIL: $tag already exists at ${existing:0:12}, not tip ${tip:0:12}"
      else
        row "$branch" "$class" "would tag $tag at ${tip:0:12}, push it, delete"
      fi
    else
      row "$branch" "$class" "would delete"
    fi
    continue
  fi

  if [ -n "$tag" ]; then
    existing="$(git rev-parse --verify -q "refs/tags/$tag^{commit}" || true)"
    if [ -z "$existing" ]; then
      if ! git tag -a -m "archived by cleanup-branches.sh on $today: unmerged branch $branch" "$tag" "$tip"; then
        failures=$((failures + 1))
        row "$branch" "$class" "FAILED: could not create $tag; branch kept"
        continue
      fi
    elif [ "$existing" != "$tip" ]; then
      # Never move an archive tag: it may be the only record of other work.
      failures=$((failures + 1))
      row "$branch" "$class" "FAILED: $tag exists at ${existing:0:12}, not tip ${tip:0:12}; branch kept"
      continue
    fi
    if ! git push "$REMOTE" "refs/tags/$tag"; then
      failures=$((failures + 1))
      row "$branch" "$class" "FAILED: could not push $tag; branch kept"
      continue
    fi
  fi

  if ! refusal="$(guard_allows "$branch")"; then
    failures=$((failures + 1))
    row "$branch" "$class" "FAILED: guard refused: $refusal"
    continue
  fi

  if git push "$REMOTE" --delete "$branch"; then
    row "$branch" "$class" "${tag:+tagged $tag, }deleted"
  else
    failures=$((failures + 1))
    row "$branch" "$class" "FAILED: delete push failed${tag:+ (tag $tag is pushed)}"
  fi
done < <(git for-each-ref --format='%(refname:strip=3)' "refs/remotes/$REMOTE/")

echo
if [ ${#rows[@]} -gt 0 ]; then
  # Plain bash rather than column(1), which is not installed everywhere.
  width=6
  for r in "${rows[@]}"; do
    name="${r%%$'\t'*}"
    [ ${#name} -gt "$width" ] && width=${#name}
  done
  printf "%-${width}s  %-8s  %s\n" BRANCH CLASS ACTION
  for r in "${rows[@]}"; do
    IFS=$'\t' read -r name class action <<<"$r"
    printf "%-${width}s  %-8s  %s\n" "$name" "$class" "$action"
  done
else
  echo "no candidate branches"
fi
echo
mode=dry-run
$apply && mode=apply
echo "mode=$mode merged=$merged_count unmerged=$unmerged_count skipped-open=$open_count failures=$failures"

[ "$failures" -eq 0 ]
