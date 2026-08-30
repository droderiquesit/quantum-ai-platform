#!/usr/bin/env bash
# How many tests pass, how many fail, and a non-zero exit if any do.
#
# This script exists because of a specific mistake. The obvious way to count
# the suite is to sum the `test result: ok. N passed` lines — and a test binary
# that *fails* prints `test result: FAILED.` instead, so a failing target
# contributes nothing to that sum and disappears. The count comes back lower
# and still looks like a clean number, which is the worst way for a measurement
# to be wrong: it under-reports rather than erroring, and the report written
# from it says the suite is green.
#
# So: match both outcomes, add up both columns, and exit on cargo's status
# rather than on whether the parse found anything.
set -uo pipefail

readonly OUTPUT="$(mktemp)"
trap 'rm -f "$OUTPUT"' EXIT

# `--no-fail-fast` so every target runs; without it the first failing binary
# stops the others and the totals are a lower bound nobody labelled as one.
# The workspace lives under backend/ (ADR 0016).
cd "$(dirname "${BASH_SOURCE[0]}")/../backend"
cargo test --workspace --all-features --no-fail-fast >"$OUTPUT" 2>&1
readonly STATUS=$?

# Both outcomes, both columns.
passed=$(grep -oE 'test result: [A-Za-z]+\. [0-9]+ passed' "$OUTPUT" \
  | awk '{s+=$(NF-1)} END {print s+0}')
failed=$(grep -oE '[0-9]+ failed' "$OUTPUT" | awk '{s+=$1} END {print s+0}')

echo "tests: ${passed} passed, ${failed} failed"

if [[ "${failed}" -gt 0 || "${STATUS}" -ne 0 ]]; then
  echo
  echo "failing tests:" >&2
  grep -E '^test .* \.\.\. FAILED$' "$OUTPUT" | sort -u >&2 || true
  echo >&2
  echo "the suite is red. Do not quote the passing count without the failing" >&2
  echo "one; a number that omits it describes a platform that does not exist." >&2
  exit 1
fi

echo "the suite is green"
