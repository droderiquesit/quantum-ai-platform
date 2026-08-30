#!/usr/bin/env bash
# The dependency policy, enforced.
#
# The platform depends on serde and serde_json and nothing else. Every
# numeric routine, the HTTP server, the hash functions, the RNG, the quantum
# simulator and the HMM are written in-tree.
#
# That is a deliberate trade: more code to maintain, in exchange for a supply
# chain small enough to audit, a build that works offline, and no transitive
# dependency that can change what the platform computes. A pull request adding
# a third dependency has to change this script, which puts the decision in the
# diff where a reviewer will see it.
set -euo pipefail

# serde and serde_json, plus their own transitive closure. Each is listed
# explicitly rather than resolved from the graph, so that when serde changes
# what it depends on the change appears here — which is exactly the event
# worth seeing in a diff.
readonly PERMITTED=(
  # Declared directly.
  "serde"
  "serde_json"
  # serde's derive macro and the proc-macro crates it needs.
  "serde_derive"
  "proc-macro2"
  "quote"
  "syn"
  "unicode-ident"
  # serde 1.0.229 split its core traits into their own crate.
  "serde_core"
  # serde_json's integer and float formatting, and its byte search.
  "itoa"
  "zmij"
  "memchr"
)

# The Rust workspace lives under backend/ (ADR 0016); resolve it from the
# script's own location so the gate passes or fails identically from any cwd.
readonly LOCKFILE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/backend/Cargo.lock"

if [[ ! -f "$LOCKFILE" ]]; then
  echo "backend/Cargo.lock is missing; the build is not reproducible without it" >&2
  exit 1
fi

# Every package in the lockfile that is not one of ours.
mapfile -t locked < <(
  grep '^name = ' "$LOCKFILE" | sed 's/name = "//; s/"//' | sort -u | grep -v '^qip-'
)

violations=()
for package in "${locked[@]}"; do
  permitted=false
  for allowed in "${PERMITTED[@]}"; do
    if [[ "$package" == "$allowed" ]]; then
      permitted=true
      break
    fi
  done
  if [[ "$permitted" == false ]]; then
    violations+=("$package")
  fi
done

if [[ ${#violations[@]} -gt 0 ]]; then
  echo "the dependency policy permits only serde and serde_json." >&2
  echo "these are in the lockfile and are not permitted:" >&2
  printf '  %s\n' "${violations[@]}" >&2
  echo >&2
  echo "if a dependency is genuinely warranted, add it to PERMITTED in this" >&2
  echo "script with a comment saying why. The point is that the decision" >&2
  echo "appears in a diff." >&2
  exit 1
fi

echo "dependency policy: ${#locked[@]} third-party package(s), all permitted"
