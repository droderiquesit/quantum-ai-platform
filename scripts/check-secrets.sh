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
  # Vendor tokens recognisable by their own shape rather than by the name
  # somebody happened to assign them to.
  #
  # The pattern above only fires on a value assigned to a key it knows, so a
  # credential pasted as `default = "hf_…"` in a tfvars file, or as a bare
  # item in a YAML list, walks straight through it — the key is `default`, or
  # there is no key at all. That is not hypothetical: a Hugging Face user
  # access token belonging to this project was exposed outside the repository,
  # and the only thing standing between that class of mistake and a commit is
  # a check that reads the value.
  #
  # Narrow on purpose, in keeping with the note at the top. Each prefix below
  # is issued by exactly one vendor, carries a fixed length, and contains no
  # separator, so a word in prose cannot reach the minimum: the charset stops
  # at the first `_` or `-`. Three, because these are the three this
  # repository has a reason to hold — Hugging Face is the platform's only
  # model vendor (ADR 0037), GitHub is the one host outside the VPC the
  # management zone may reach, and Google is the cloud. A vendor the platform
  # does not integrate with is left to the key-name pattern above rather than
  # guessed at here.
  #
  # Each prefix is spelled out rather than folded into a character class so
  # that `no_secret_value_appears_in_any_committed_configuration` in
  # `backend/crates/tests/qip-acceptance/tests/security.rs` can assert this
  # file knows every shape it does. The two scanners exist to catch the same
  # mistake in two places, and a prefix taught to one and not the other is a
  # gap wearing a pair of scanners.
  'hf_[A-Za-z0-9]{34,}'
  '(ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9]{36,}'
  'AIza[0-9A-Za-z_-]{35}'
)

readonly EXCLUDE_PATHS=(
  ':(exclude)backend/target'
  ':(exclude)backend/Cargo.lock'
  ':(exclude)scripts/check-secrets.sh'
  # Test fixtures use obviously fake tokens, and flagging them would train
  # everyone to pass --no-verify.
  ':(exclude)*/tests/*'
)

# --history: every line any commit reachable from HEAD ever added.
#
# The scan below reads the tree as it stands, so a credential committed on
# Monday and deleted on Tuesday passes it from Tuesday onwards while every
# clone still carries Monday's blob. This mode reads what each commit *added*
# instead, with the same patterns and the same exclusions, so deleting the
# line does not make the finding go away. That is the point: the fix for a
# committed secret is rotation, and the scan keeps saying so until somebody
# records that it happened.
#
# The record is scripts/secrets-reviewed.txt, one `<commit> <path> <what it
# was>` per line. Without it this mode would be the build ci.yml's comment
# used to warn about, one that fails for ever on an old commit and that
# people learn to ignore. A line with no third field acknowledges nothing:
# the reason is what makes it a record rather than a mute button.
#
# Reports the commit and the path, never the matched line: this output lands
# in a CI log, and a scanner that reprints the secret it found has published
# it a second time.
case "${1:-}" in
  '') ;;
  --history)
    # A shallow clone holds one commit and would report the whole history
    # clean having read none of it.
    if [[ $(git rev-parse --is-shallow-repository) == true ]]; then
      echo "this is a shallow clone, so there is no history here to scan;" >&2
      echo "fetch it first: git fetch --unshallow (fetch-depth: 0 in a workflow)" >&2
      exit 2
    fi
    reviewed="$(git rev-parse --show-toplevel)/scripts/secrets-reviewed.txt"
    alternation=$(IFS='|'; echo "${PATTERNS[*]}")
    commit='' path='' found=0
    # --cc so a line that exists only in a merge's own resolution is read too;
    # a plain `log -p` shows no diff at all for a merge.
    # ponytail: an added line whose own text starts with "++ " reads as a file
    # header and is skipped; parse `--raw -z` if that ever hides a finding.
    while IFS= read -r line; do
      case "$line" in
        'commit '*) commit=${line#commit } ;;
        '+++ '*) path=${line#+++ b/} ;;
        *'cert-manager.io/inject-ca-from-secret:'*) ;;
        *)
          if [[ -f $reviewed ]] && grep -qF -- "$commit $path " "$reviewed"; then
            continue
          fi
          echo "possible secret added by $commit in $path" >&2
          found=1
          ;;
      esac
    done < <(git log --no-color --cc --format='commit %H' HEAD -- . "${EXCLUDE_PATHS[@]}" \
      | grep -a -E "^(commit [0-9a-f]{40}\$|\+\+\+ |\+.*(${alternation}))")
    if [[ $found -eq 1 ]]; then
      echo >&2
      echo "read each with: git show <commit> -- <path>" >&2
      echo "a real one needs rotating, then a line in scripts/secrets-reviewed.txt;" >&2
      echo "deleting it changes nothing, because it is already in the history." >&2
      exit 1
    fi
    echo "secret scan: nothing found in $(git rev-list --count HEAD) commits of history"
    exit 0
    ;;
  *)
    echo "unknown argument '$1'; run with no argument to scan the tree, or --history" >&2
    exit 2
    ;;
esac

found=0
for pattern in "${PATTERNS[@]}"; do
  # One known non-secret shaped like one: cert-manager's cainjector
  # annotation assigns `...-from-secret:` a namespace/name *reference*
  # ("cert-manager/cert-manager-webhook-ca"), which holds nothing, and the
  # vendored upstream manifest carries it verbatim. Filtered by its exact
  # annotation key rather than excluding the vendored file, so a real
  # credential pasted into that file would still be caught. The acceptance
  # suite's copy of this check carries the same single exemption.
  if matches=$(git grep -n -I -E "$pattern" -- . "${EXCLUDE_PATHS[@]}" 2>/dev/null \
    | grep -v 'cert-manager\.io/inject-ca-from-secret:'); then
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
