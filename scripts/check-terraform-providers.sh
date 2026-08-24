#!/usr/bin/env bash
# Every provider a Terraform module uses, that module must declare.
#
# `terraform validate` does not catch this. A module can reference a provider
# it never named in `required_providers` and validate cleanly; only `plan`
# refuses it, and `plan` needs credentials. So the failure mode is a
# configuration that passes CI and fails on the day somebody applies it —
# which is exactly what happened here: `modules/ai` used `google-beta` for the
# Vertex metadata store, nothing declared it, `validate` said Success, and only
# a real plan against the provider revealed it.
#
# This is a static check on purpose. It needs no credentials, no `terraform
# init` and no network, so it runs in CI on every pull request rather than at
# the moment of an apply.
#
# What it does not catch: a module using a resource type that exists only in a
# non-default provider without an explicit `provider =` argument. Detecting
# that means knowing every provider's resource list, which is a schema lookup
# and therefore needs `init`. The explicit reference is the common case and the
# one that bit us.
set -euo pipefail

readonly ROOT="${1:-infrastructure/terraform}"
failures=0

# `provider = google-beta` on a resource, or `google-beta = google-beta` inside
# a root module's `providers = { … }` block. Both name a provider the module at
# that path has to declare.
providers_referenced() {
  grep -rhoE '^[[:space:]]*provider[[:space:]]*=[[:space:]]*[a-z][a-z0-9_-]*' "$1"/*.tf 2>/dev/null |
    sed -E 's/.*=[[:space:]]*//' | sort -u
}

providers_declared() {
  # Names inside a `required_providers` block are its `name = {` entries.
  #
  # Brace depth, not "the next closing brace": each entry is itself a block, so
  # a naive scanner stops at the end of the first provider and reports every
  # later one as undeclared. This check caught exactly that in its own first
  # draft, which is the argument for the depth counter rather than a comment
  # saying to be careful.
  awk '
    /required_providers[[:space:]]*\{/ { inside = 1; depth = 1; next }
    inside {
      opens  = gsub(/\{/, "{")
      closes = gsub(/\}/, "}")
      if (depth == 1 && $0 ~ /=[[:space:]]*\{/) {
        name = $0
        gsub(/[[:space:]]/, "", name)
        sub(/=.*/, "", name)
        if (name != "") print name
      }
      depth += opens - closes
      if (depth <= 0) inside = 0
    }' "$1"/*.tf 2>/dev/null | sort -u
}

check_module() {
  local dir="$1" name="$2"
  local referenced declared missing
  referenced="$(providers_referenced "$dir")"
  [ -z "$referenced" ] && return 0
  declared="$(providers_declared "$dir")"

  while read -r provider; do
    [ -z "$provider" ] && continue
    # The default `google` provider is inherited implicitly and is declared at
    # the root; a submodule referencing it by name still ought to declare it,
    # but the common and dangerous case is a *non-default* provider.
    if ! printf '%s\n' "$declared" | grep -qx "$provider"; then
      echo "  ${name}: references provider '${provider}' and does not declare it in required_providers" >&2
      missing=1
    fi
  done <<< "$referenced"

  [ -n "${missing:-}" ] && return 1
  return 0
}

echo "terraform provider declarations, under ${ROOT}"

if ! check_module "$ROOT" "root"; then
  failures=$((failures + 1))
fi

for dir in "$ROOT"/modules/*/; do
  [ -d "$dir" ] || continue
  if ! check_module "$dir" "modules/$(basename "$dir")"; then
    failures=$((failures + 1))
  fi
done

if [ "$failures" -ne 0 ]; then
  echo >&2
  echo "${failures} module(s) reference a provider they do not declare." >&2
  echo "Add it to that module's required_providers block. A module that does" >&2
  echo "not declare a provider it uses inherits nothing and fails at plan," >&2
  echo "after passing validate — so this fails on apply day rather than today." >&2
  exit 1
fi

echo "every referenced provider is declared"
