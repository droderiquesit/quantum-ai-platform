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
  --port 8080 --cpu 1 --memory 512Mi --min-instances 0 --max-instances 3 \
  --set-env-vars "ALGORIK_ENV=development,ALGORIK_POSTURE=paper,ALGORIK_IDENTITY_STORE_DIR=/tmp/algorik-identity" \
  --set-secrets "ALGORIK_SESSION_SECRET=algorik-session-secret:latest" \
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
