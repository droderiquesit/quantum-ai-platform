#!/usr/bin/env bash
# Create Kargo's admin credentials, in the two places they belong and
# nowhere else.
#
# The Kargo chart is rendered with `api.secret.name=kargo-api-admin`, so the
# committed manifest carries no credential — this script is the out-of-band
# act that supplies it, the same shape as seed-secret-versions.sh:
#
#   * the admin password itself goes to Secret Manager
#     (algorik-kargo-admin-password), which is where the operator retrieves
#     it from; it is never printed, logged, or written to disk here;
#   * its bcrypt hash and a fresh token-signing key go into the
#     kargo-api-admin Kubernetes Secret the API deployment mounts.
#
# Re-running is refused if either half already exists: replacing the
# password is a deliberate rotation (delete both halves first), not
# something a bootstrap script should do by accident.
#
# Usage: scripts/bootstrap-kargo-admin.sh <environment>

set -euo pipefail

environment="${1:?usage: bootstrap-kargo-admin.sh <environment>}"
tfvars="infrastructure/environments/${environment}/terraform.tfvars"
[ -f "$tfvars" ] || { echo "no tfvars at ${tfvars}" >&2; exit 1; }
project="$(sed -n 's/^project_id[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$tfvars" | head -1)"
[ -n "$project" ] || { echo "${tfvars} carries no project_id" >&2; exit 1; }

unset CLOUDSDK_AUTH_ACCESS_TOKEN

if gcloud secrets describe algorik-kargo-admin-password --project "$project" >/dev/null 2>&1; then
  echo "algorik-kargo-admin-password already exists; rotation is deliberate, refusing." >&2
  exit 1
fi
if kubectl get secret kargo-api-admin -n kargo >/dev/null 2>&1; then
  echo "kargo-api-admin already exists in the cluster; rotation is deliberate, refusing." >&2
  exit 1
fi

password="$(openssl rand -hex 24)"
hash="$(python3 -c 'import bcrypt,sys; print(bcrypt.hashpw(sys.stdin.readline().strip().encode(), bcrypt.gensalt(10)).decode())' <<<"$password")"
signing_key="$(openssl rand -hex 32)"

gcloud secrets create algorik-kargo-admin-password --project "$project" \
  --replication-policy=automatic >/dev/null
printf '%s' "$password" \
  | gcloud secrets versions add algorik-kargo-admin-password --project "$project" --data-file=- >/dev/null
echo "password stored in Secret Manager: algorik-kargo-admin-password"

kubectl create secret generic kargo-api-admin -n kargo \
  --from-literal=ADMIN_ACCOUNT_PASSWORD_HASH="$hash" \
  --from-literal=ADMIN_ACCOUNT_TOKEN_SIGNING_KEY="$signing_key"
echo "kargo-api-admin created in the kargo namespace"
echo "retrieve the password with:"
echo "  gcloud secrets versions access latest --secret algorik-kargo-admin-password --project ${project}"
