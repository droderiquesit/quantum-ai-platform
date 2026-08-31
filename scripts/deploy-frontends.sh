#!/usr/bin/env bash
# Build and deploy the portal and landing to Cloud Run, in dependency order.
#
# The landing's sign-in buttons carry the portal's URL inlined at build time
# (NEXT_PUBLIC_*), so the portal deploys first and its live URL feeds the
# landing's build. Dockerfiles live in infrastructure/docker/ — the domain
# that owns them — and are staged into each build context for the submit,
# because Cloud Build reads only what the context uploads.
#
# Auth: the caller's gcloud login. Session secret: generated once into
# Secret Manager, never printed, injected by Cloud Run — nothing here or in
# the image carries it.
set -euo pipefail
unset CLOUDSDK_AUTH_ACCESS_TOKEN

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly PROJECT="algorik-dev"
readonly REGION="us-east4"
readonly AR="${REGION}-docker.pkg.dev/${PROJECT}/qip-dev"
SHA="$(git -C "${REPO_ROOT}" rev-parse --short HEAD)"
readonly SHA

# --- session secret: exists once, rotated deliberately, never echoed -------
if ! gcloud secrets describe algorik-session-secret --project "${PROJECT}" >/dev/null 2>&1; then
  gcloud secrets create algorik-session-secret --project "${PROJECT}" --replication-policy=user-managed --locations="${REGION}"
fi
if [[ -z "$(gcloud secrets versions list algorik-session-secret --project "${PROJECT}" --limit=1 --format='value(name)' 2>/dev/null)" ]]; then
  openssl rand -base64 48 | tr -d '\n' | gcloud secrets versions add algorik-session-secret --project "${PROJECT}" --data-file=- >/dev/null
  echo "seeded algorik-session-secret"
fi

# The Identity Platform browser key: public by design, restricted by the
# authorized-domains list Terraform manages. Resolved here rather than
# committed, so the config cannot drift from the project it names.
IDENTITY_API_KEY="$(gcloud services api-keys get-key-string \
  "$(gcloud services api-keys list --project "${PROJECT}" --filter='displayName:Browser' --format='value(uid)' | head -1)" \
  --project "${PROJECT}" --format='value(keyString)')"
readonly IDENTITY_API_KEY
[[ -n "${IDENTITY_API_KEY}" ]] || { echo "no Identity Platform browser key found" >&2; exit 1; }

# --- the console's route to the platform (ADR 0018) ------------------------
#
# Read from the environment's tfvars rather than restated here. Terraform
# reserves this address and the Helm chart's Service claims it; a third literal
# in a shell script is the copy nobody thinks to change, because it is the only
# one that is not configuration. `console_route.rs` fails the build if this
# script ever contains the address instead of reading it.
readonly TFVARS="${REPO_ROOT}/infrastructure/environments/dev/terraform.tfvars"
tfvar() { sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "${TFVARS}" | head -1; }

API_ADDRESS="$(tfvar api_internal_address)"
CONSOLE_SUBNET_CIDR="$(tfvar console_egress_cidr)"
readonly API_ADDRESS CONSOLE_SUBNET_CIDR
# Fail closed and say which half is missing. A console deployed without these
# answers 500 on every gateway call with "QIP_API_BASE_URL is not set" — an
# honest message, and one that describes a deployment fault as a platform one.
[[ -n "${API_ADDRESS}" ]] || { echo "api_internal_address is not set in ${TFVARS}; the console would have no platform to read" >&2; exit 1; }
[[ -n "${CONSOLE_SUBNET_CIDR}" ]] || { echo "console_egress_cidr is not set in ${TFVARS}; the console would have no route into the VPC" >&2; exit 1; }
readonly VPC_NETWORK="qip-dev"
readonly CONSOLE_SUBNET="qip-dev-console-egress"
readonly CONSOLE_SA="qip-dev-console@${PROJECT}.iam.gserviceaccount.com"
# The platform credential, projected as a file rather than an environment
# variable: an environment variable holding a token is readable from
# /proc/<pid>/environ, is inherited by every child, and lands in a crash dump.
# `qip_core::secret` and the console's own `secret.ts` both resolve the _FILE
# form, so this is the same contract the Rust binaries use.
readonly TOKEN_MOUNT="/var/run/secrets/qip/token-viewer"

# The session secret is created by this script, so its grant belongs to this
# script too — Terraform owns the account and the platform tokens, and neither
# tool reaches into what the other created.
gcloud secrets add-iam-policy-binding algorik-session-secret \
  --project "${PROJECT}" --member "serviceAccount:${CONSOLE_SA}" \
  --role roles/secretmanager.secretAccessor --quiet >/dev/null

# --- portal ----------------------------------------------------------------
echo "building portal image ${AR}/algorik-portal:${SHA}…"
cp "${REPO_ROOT}/infrastructure/docker/portal.Dockerfile" "${REPO_ROOT}/frontend/Dockerfile"
trap 'rm -f "${REPO_ROOT}/frontend/Dockerfile" "${REPO_ROOT}/frontend/landing/Dockerfile"' EXIT
gcloud builds submit "${REPO_ROOT}/frontend" \
  --project "${PROJECT}" --region "${REGION}" \
  --tag "${AR}/algorik-portal:${SHA}" --quiet

echo "deploying algorik-portal…"
gcloud run deploy algorik-portal \
  --project "${PROJECT}" --region "${REGION}" \
  --image "${AR}/algorik-portal:${SHA}" \
  --allow-unauthenticated \
  --service-account "${CONSOLE_SA}" \
  --port 8080 --cpu 1 --memory 512Mi --min-instances 0 --max-instances 3 \
  --network "${VPC_NETWORK}" --subnet "${CONSOLE_SUBNET}" \
  --vpc-egress private-ranges-only \
  --set-env-vars "ALGORIK_ENV=development,ALGORIK_POSTURE=paper,ALGORIK_IDENTITY_PROJECT_ID=${PROJECT},ALGORIK_IDENTITY_API_KEY=${IDENTITY_API_KEY},QIP_API_BASE_URL=http://${API_ADDRESS}:8080,QIP_API_TOKEN_FILE=${TOKEN_MOUNT}" \
  --set-secrets "ALGORIK_SESSION_SECRET=algorik-session-secret:latest,${TOKEN_MOUNT}=qip-token-viewer-dev:latest" \
  --quiet
PORTAL_URL="$(gcloud run services describe algorik-portal --project "${PROJECT}" --region "${REGION}" --format='value(status.url)')"
readonly PORTAL_URL
echo "portal: ${PORTAL_URL}"

# --- landing ---------------------------------------------------------------
echo "building landing image ${AR}/algorik-landing:${SHA} against ${PORTAL_URL}…"
cp "${REPO_ROOT}/infrastructure/docker/landing.Dockerfile" "${REPO_ROOT}/frontend/landing/Dockerfile"
gcloud builds submit "${REPO_ROOT}/frontend/landing" \
  --project "${PROJECT}" --region "${REGION}" \
  --config /dev/stdin <<CONFIG
steps:
  - name: gcr.io/cloud-builders/docker
    args:
      - build
      - --build-arg
      - PORTAL_URL=${PORTAL_URL}
      - --tag
      - ${AR}/algorik-landing:${SHA}
      - .
images:
  - ${AR}/algorik-landing:${SHA}
CONFIG

echo "deploying algorik-landing…"
gcloud run deploy algorik-landing \
  --project "${PROJECT}" --region "${REGION}" \
  --image "${AR}/algorik-landing:${SHA}" \
  --allow-unauthenticated \
  --port 8080 --cpu 1 --memory 256Mi --min-instances 0 --max-instances 3 \
  --quiet
LANDING_URL="$(gcloud run services describe algorik-landing --project "${PROJECT}" --region "${REGION}" --format='value(status.url)')"
readonly LANDING_URL

echo
echo "portal:  ${PORTAL_URL}"
echo "landing: ${LANDING_URL}"
echo "next: add both hostnames to identity_authorized_domains in the dev tfvars and apply."
