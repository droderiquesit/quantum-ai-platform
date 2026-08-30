#!/usr/bin/env bash
# Seed first versions into the Secret Manager containers Terraform creates
# empty.
#
# Terraform deliberately creates every secret with no version — a value in
# Terraform is a value in the state file — and the CSI driver projects
# `versions/latest` into the pods. Between those two facts sits an
# operational act nothing else performs: until someone writes version 1,
# every workload that mounts a secret is stuck in ContainerCreating on
# "NotFound ... or has no versions", which is exactly how the first dev
# deployment failed.
#
# This script is that act, made repeatable and safe to re-run:
#
#   * A secret that already has a version is left alone. Rotation is a
#     deliberate, per-secret decision (docs/security/credentials.md), not
#     something a seeding script should do by accident.
#   * Values are generated in-process and piped straight to Secret Manager.
#     Nothing is printed, nothing touches disk, nothing enters shell history.
#   * Only the secrets the platform's SecretProviderClasses actually mount
#     are seeded. The integration credentials (market data, quantum, venue)
#     stay empty until their integration is configured: an absent credential
#     fails closed, and inventing one would turn "integration disabled" into
#     "integration misconfigured".
#
# The generated values are 64 hex characters (32 bytes of entropy), which
# satisfies the API's 32-character token floor and the envelope key's
# 32-byte floor. The capital-envelope key seeded here is a dev paper-trading
# trust root; production's is a human ceremony for the same reason the
# variable exists at all.
#
# Usage: scripts/seed-secret-versions.sh <environment>

set -euo pipefail

environment="${1:?usage: seed-secret-versions.sh <environment>}"
tfvars="infrastructure/environments/${environment}/terraform.tfvars"
[ -f "$tfvars" ] || { echo "no tfvars at ${tfvars}" >&2; exit 1; }
project="$(sed -n 's/^project_id[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$tfvars" | head -1)"
[ -n "$project" ] || { echo "${tfvars} carries no project_id" >&2; exit 1; }

# The stray container variable that overrides real credentials; harmless to
# unset when absent.
unset CLOUDSDK_AUTH_ACCESS_TOKEN

seeded=0
for name in \
  qip-token-monitor \
  qip-token-viewer \
  qip-token-analyst \
  qip-token-approver \
  qip-token-operator \
  qip-capital-envelope-key; do
  secret="${name}-${environment}"
  if gcloud secrets versions list "$secret" --project "$project" \
    --format='value(name)' | grep -q .; then
    echo "${secret}: has a version already; rotation is deliberate, skipping"
    continue
  fi
  openssl rand -hex 32 \
    | gcloud secrets versions add "$secret" --project "$project" --data-file=- >/dev/null
  echo "${secret}: version 1 written"
  seeded=$((seeded + 1))
done
echo "seeded ${seeded} secret(s) in ${project}"
