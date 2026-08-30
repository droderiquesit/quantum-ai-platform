#!/usr/bin/env bash
# Look for credentials that should never be committed.
#
# Deliberately narrow. A scanner that flags every high-entropy string produces
# a wall of false positives, and a wall of false positives is a scanner people
# learn to skip. These patterns are for things that are unambiguously secrets.
set -euo pipefail

readonly PATTERNS=(
  # Private keys of any kind.
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'
  # AWS access key ids.
  'AKIA[0-9A-Z]{16}'
  # Google service-account keys.
  '"type": "service_account"'
  # A bearer token or password assigned inline, rather than read from the
  # environment. The exclusions below keep the platform's own examples out.
  '(password|passwd|secret|api_key|apikey|token)[[:space:]]*[:=][[:space:]]*"[A-Za-z0-9+/=_-]{20,}"'
)

readonly EXCLUDE_PATHS=(
  ':(exclude)backend/target'
  ':(exclude)backend/Cargo.lock'
  ':(exclude)scripts/check-secrets.sh'
  # Test fixtures use obviously fake tokens, and flagging them would train
  # everyone to pass --no-verify.
  ':(exclude)*/tests/*'
)

found=0
for pattern in "${PATTERNS[@]}"; do
  if matches=$(git grep -n -I -E "$pattern" -- . "${EXCLUDE_PATHS[@]}" 2>/dev/null); then
    echo "possible secret matching /$pattern/:" >&2
    echo "$matches" >&2
    found=1
  fi
done

if [[ $found -eq 1 ]]; then
  echo >&2
  echo "if one of these is a real secret it needs rotating, not deleting:" >&2
  echo "it is already in the history." >&2
  exit 1
fi

echo "secret scan: nothing found"
