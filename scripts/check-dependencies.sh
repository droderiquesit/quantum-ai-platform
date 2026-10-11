#!/usr/bin/env bash
# The dependency policy, enforced (ADR 0109).
#
# Until 2026-10-11 the platform depended on serde and serde_json and nothing
# else (ADR 0002, 0009). The owner then accepted ADR 0109: third-party crates
# are admitted from a committed allow-list, scripts/dependency-allowlist.txt,
# each line naming the record that admitted it. What has not changed is the
# point of this script: adding a dependency must change a committed file, so
# the decision is in the diff where a reviewer will see it, and a package that
# arrives transitively without anyone choosing it fails here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
LOCKFILE="$ROOT/backend/Cargo.lock"
ALLOWLIST="$ROOT/scripts/dependency-allowlist.txt"
readonly LOCKFILE ALLOWLIST

if [[ ! -f "$LOCKFILE" ]]; then
  echo "backend/Cargo.lock is missing; the build is not reproducible without it" >&2
  exit 1
fi
if [[ ! -f "$ALLOWLIST" ]]; then
  echo "scripts/dependency-allowlist.txt is missing; without it nothing is permitted" >&2
  exit 1
fi

# The allow-list: the first word of every line that is not blank or a comment.
mapfile -t permitted < <(sed 's/#.*//' "$ALLOWLIST" | awk 'NF { print $1 }' | sort -u)
# A line with no reason is refused: an admission nobody can explain is the
# supply-chain failure this file exists to make visible.
unexplained=$(grep -Ev '^[[:space:]]*(#|$)' "$ALLOWLIST" | grep -vc '#' || true)
if [[ "$unexplained" -gt 0 ]]; then
  echo "dependency-allowlist.txt has $unexplained line(s) with no '# ADR — reason' comment" >&2
  exit 1
fi

# Every package in the lockfile that is not one of ours.
mapfile -t locked < <(
  grep '^name = ' "$LOCKFILE" | sed 's/name = "//; s/"//' | sort -u | grep -v '^qip-'
)

violations=()
for package in "${locked[@]}"; do
  found=false
  for allowed in "${permitted[@]}"; do
    if [[ "$package" == "$allowed" ]]; then
      found=true
      break
    fi
  done
  if [[ "$found" == false ]]; then
    violations+=("$package")
  fi
done

if [[ ${#violations[@]} -gt 0 ]]; then
  echo "these packages are in backend/Cargo.lock and not in scripts/dependency-allowlist.txt:" >&2
  printf '  %s\n' "${violations[@]}" >&2
  echo >&2
  echo "if a crate is warranted, add it — and every package it brings — to the allow-list" >&2
  echo "with the ADR that admits it and why (ADR 0109)." >&2
  exit 1
fi

echo "dependency policy: ${#locked[@]} third-party package(s), all permitted"
