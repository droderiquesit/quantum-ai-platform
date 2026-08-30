#!/usr/bin/env bash
# Run terraform against infrastructure/terraform as the bootstrap account.
#
# Exists because the agent's container authenticates gcloud with a user
# login but has no Application Default Credentials file, and terraform's
# google provider does not read gcloud user credentials. Rather than a
# second browser handshake for ADC, each invocation mints a short-lived
# access token impersonating claude-builder and hands it to the provider
# via GOOGLE_OAUTH_ACCESS_TOKEN. The token lives ~1h and is never written
# to disk or printed; a mid-apply expiry fails cleanly and a rerun
# refreshes it. Audit log entries name the impersonation, per
# docs/security/credentials.md.
set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly TF_DIR="${REPO_ROOT}/infrastructure/terraform"
readonly SA="claude-builder@algorik-dev.iam.gserviceaccount.com"

# CLOUDSDK_AUTH_ACCESS_TOKEN, when present in the environment, overrides
# gcloud's stored login with a token that is not valid for Google.
unset CLOUDSDK_AUTH_ACCESS_TOKEN

GOOGLE_OAUTH_ACCESS_TOKEN="$(gcloud auth print-access-token --impersonate-service-account="${SA}")"
export GOOGLE_OAUTH_ACCESS_TOKEN

exec terraform -chdir="${TF_DIR}" "$@"
