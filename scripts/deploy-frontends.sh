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
# Secret Manager, never printed, projected by Cloud Run as a file — nothing
# here, in the image, or in the process's environment carries it.
#
# Both services are deployed by digest and opted into the project's Binary
# Authorization policy, the way deploy.yml deploys the catalogue: a service
# that does not opt in is never evaluated against the policy, and these two
# face the internet. The policy requires an attestation, so each digest is
# signed here before it is deployed — the same attestor and key version the
# pipeline signs with, read from Terraform rather than restated — and the
# routed revision is read back afterwards to prove it serves those bytes.
set -euo pipefail
unset CLOUDSDK_AUTH_ACCESS_TOKEN

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO_ROOT
readonly PROJECT="algorik-dev"
readonly REGION="us-east4"
readonly AR="${REGION}-docker.pkg.dev/${PROJECT}/qip-dev"
SHA="$(git -C "${REPO_ROOT}" rev-parse --short HEAD)"
readonly SHA

# --- what gets signed is a reviewed commit, not whatever is on disk ---------
#
# `gcloud builds submit` uploads the directory as it is, and `attest` below
# puts the pipeline's own attestor and key version on the digest that comes
# back. Run from a tree with uncommitted edits, that signs bytes no commit
# holds and no review saw, under a `:${SHA}` tag naming a commit they are not
# — an operator identity minting the pipeline's signature for unreviewed
# code. So, before anything is built: the tree must be clean, and HEAD must
# already be contained in the remote default branch, because a branch only
# its author has read is not reviewed either. Both refusals say what to do.
if [[ -n "$(git -C "${REPO_ROOT}" status --porcelain)" ]]; then
  echo "the working tree has uncommitted changes; the attestor signs only what a reviewed commit holds. Commit them, merge to the default branch, and run this again from a clean checkout of it:" >&2
  git -C "${REPO_ROOT}" status --short >&2
  exit 1
fi
# The remote's default branch: from the tracking ref when the clone recorded
# one, from the remote itself when it did not, and `main` as the last resort.
DEFAULT_BRANCH="$(git -C "${REPO_ROOT}" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || true)"
DEFAULT_BRANCH="${DEFAULT_BRANCH#origin/}"
if [[ -z "${DEFAULT_BRANCH}" ]]; then
  DEFAULT_BRANCH="$(git -C "${REPO_ROOT}" ls-remote --symref origin HEAD 2>/dev/null \
    | sed -n 's|^ref: refs/heads/\([^[:space:]]*\)[[:space:]]HEAD$|\1|p' | head -1)"
fi
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
readonly DEFAULT_BRANCH
git -C "${REPO_ROOT}" fetch --quiet origin "${DEFAULT_BRANCH}"
if ! git -C "${REPO_ROOT}" merge-base --is-ancestor HEAD "origin/${DEFAULT_BRANCH}"; then
  echo "HEAD (${SHA}) is not contained in origin/${DEFAULT_BRANCH}; merge it there first. The pipeline's attestor signs what the default branch holds, not what one operator has checked out" >&2
  exit 1
fi

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
# The API is a Cloud Run service (ADR 0024) and its URL is Terraform's to
# report: `api_internal_base_url` is `module.cloud_run["api"].uri`, internal
# ingress, answering only the console's identity over the VPC. It is read from
# `terraform output` rather than restated here, because a literal in a shell
# script is the copy nobody thinks to change — it is the only one that is not
# configuration. The GKE runtime reserved an internal-load-balancer address in
# the tfvars for this (`api_internal_address`); that key is gone with the load
# balancer, and a script still reading it deployed a console whose upstream
# was empty. `console_route.rs` fails the build if this script ever carries
# the address, or reads a key the tfvars no longer set.
#
# The subnet the console egresses through is still a tfvars value, because
# Terraform creates it from that value and nothing else names it.
readonly TFVARS="${REPO_ROOT}/infrastructure/environments/dev/terraform.tfvars"
readonly TF_ROOT="${REPO_ROOT}/infrastructure/terraform"
tfvar() { sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p" "${TFVARS}" | head -1; }

# `terraform output` reads state, so this needs the same `terraform init`
# infra.yml performs against the environment's state bucket; it is not a
# plan and mutates nothing.
API_BASE_URL="$(terraform -chdir="${TF_ROOT}" output -raw api_internal_base_url 2>/dev/null || true)"
CONSOLE_SUBNET_CIDR="$(tfvar console_egress_cidr)"
readonly API_BASE_URL CONSOLE_SUBNET_CIDR
# Fail closed and say which half is missing. A console deployed without these
# answers 500 on every gateway call with "QIP_API_BASE_URL is not set" — an
# honest message, and one that describes a deployment fault as a platform one.
[[ -n "${API_BASE_URL}" ]] || { echo "terraform output api_internal_base_url is empty; the API's Cloud Run service has not been applied, so the console would have no platform to read" >&2; exit 1; }
[[ -n "${CONSOLE_SUBNET_CIDR}" ]] || { echo "console_egress_cidr is not set in ${TFVARS}; the console would have no route into the VPC" >&2; exit 1; }

# The attestor and the key version that signs for it, from the same state.
# deploy.yml derives both from the tfvars; here they are one `terraform
# output` away and read from there, because a name constructed in a second
# place is a second place for it to be wrong. An empty output means the policy
# module has not been applied — and then there is no policy to opt into, so
# refusing here is more honest than deploying a service that reads as
# evaluated against nothing.
ATTESTOR="$(terraform -chdir="${TF_ROOT}" output -raw binary_authorization_attestor 2>/dev/null || true)"
KEY_VERSION="$(terraform -chdir="${TF_ROOT}" output -raw binary_authorization_key_version 2>/dev/null || true)"
readonly ATTESTOR KEY_VERSION
[[ -n "${ATTESTOR}" ]] || { echo "terraform output binary_authorization_attestor is empty; the admission policy has not been applied, so nothing could sign for these images" >&2; exit 1; }
[[ -n "${KEY_VERSION}" ]] || { echo "terraform output binary_authorization_key_version is empty; there is no key version to sign with" >&2; exit 1; }

readonly VPC_NETWORK="qip-dev"
readonly CONSOLE_SUBNET="qip-dev-console-egress"
readonly CONSOLE_SA="qip-dev-console@${PROJECT}.iam.gserviceaccount.com"
# The network tag the console-egress firewall rules target —
# `local.console_egress_tag` in modules/network/main.tf, the deny-egress at
# 65000 and the allow to the restricted VIP above it. A firewall rule binds an
# interface by tag, and a Cloud Run direct-VPC-egress interface carries a tag
# only if the deploy passes `--network-tags`: without it both rules bind
# nothing, the implied allow-all egress at 65535 is what applies, and every
# tool reports the subnet as denied. Derived from the environment the tfvars
# name, because the module builds the tag from the same value.
ENVIRONMENT="$(tfvar environment)"
readonly ENVIRONMENT
[[ -n "${ENVIRONMENT}" ]] || { echo "environment is not set in ${TFVARS}; the console-egress network tag cannot be derived and the console's egress rules would bind nothing" >&2; exit 1; }
readonly CONSOLE_EGRESS_TAG="qip-${ENVIRONMENT}-console-egress"
# The landing's own identity, created by `modules/secrets` beside the
# console's. Without `--service-account` Cloud Run runs a service as the
# project's default compute account — shared by everything that names none,
# so a grant to it for one workload is a grant to all of them. The landing
# holds no grant at all; the point of naming it is that this stays true.
readonly LANDING_SA="qip-dev-landing@${PROJECT}.iam.gserviceaccount.com"
# Every credential, projected as a file rather than an environment variable:
# an environment variable holding a secret is readable from
# /proc/<pid>/environ, is inherited by every child, and lands in a crash dump.
# `qip_core::secret` and the console's own `secret.ts` both resolve the _FILE
# form, so this is the same contract the Rust binaries use. The session
# secret — the key that signs the console's cookies — once rode the same
# `--set-secrets` argument as an environment value, four lines below a comment
# saying why it must not; a `--set-secrets` entry is a file only when its
# left-hand side is a path, so both are paths.
readonly TOKEN_MOUNT="/var/run/secrets/qip/token-viewer"
readonly SESSION_SECRET_MOUNT="/var/run/secrets/algorik/session-secret"

# --- the discipline deploy.yml applies to every catalogue image ------------
#
# The digest the registry holds for a tag. Read back from the registry rather
# than from the build's own output, because what is signed has to be what
# Cloud Run will pull. A tag is mutable; an attestation names bytes.
digest_of() {
  local tag="$1" digest
  digest="$(gcloud artifacts docker images describe "${tag}" \
    --project "${PROJECT}" --format='value(image_summary.digest)')"
  [[ -n "${digest}" ]] || { echo "the registry reports no digest for ${tag}; the build did not push it" >&2; exit 1; }
  printf '%s' "${digest}"
}

# Attest an image by digest, unless these exact bytes already are. A re-run
# for an unchanged commit is a normal thing to do, and `sign-and-create`
# refuses to write an attestation that already exists — so an existing one is
# success rather than a conflict. `beta` because the command moved there in
# current gcloud, as deploy.yml notes.
attest() {
  local image="$1" existing
  existing="$(gcloud container binauthz attestations list \
    --project "${PROJECT}" --attestor "${ATTESTOR}" --attestor-project "${PROJECT}" \
    --artifact-url "${image}" --format='value(name)')"
  if [[ -n "${existing}" ]]; then
    echo "${image} is already attested."
    return 0
  fi
  gcloud beta container binauthz attestations sign-and-create \
    --project "${PROJECT}" --artifact-url "${image}" \
    --attestor "${ATTESTOR}" --attestor-project "${PROJECT}" \
    --keyversion "${KEY_VERSION}"
}

# Prove the revision that is *serving* runs the digest that was signed.
# `gcloud run deploy` returning is not that, and neither is reading
# `spec.template` — that is what was asked for, which a traffic split that
# never moved satisfies just as well. So the revisions read are the ones
# `status.traffic` routes to, and every container of each is compared.
prove_serving() {
  local service="$1" image="$2" routed=0 revision percent serving
  while IFS=, read -r revision percent; do
    [[ "${percent:-0}" -gt 0 ]] || continue
    routed=$((routed + 1))
    serving="$(gcloud run revisions describe "${revision}" \
      --project "${PROJECT}" --region "${REGION}" \
      --flatten='spec.containers[]' --format='value(spec.containers.image)')"
    if [[ "${serving}" != "${image}" ]]; then
      echo "${service} routes traffic to ${revision}, which runs '${serving}'; asked for ${image}" >&2
      exit 1
    fi
    echo "${service} serves ${image} from ${revision}"
  done < <(gcloud run services describe "${service}" \
    --project "${PROJECT}" --region "${REGION}" \
    --flatten='status.traffic[]' \
    --format='csv[no-heading](status.traffic.revisionName,status.traffic.percent)')
  [[ "${routed}" -gt 0 ]] || { echo "${service} routes traffic to no revision, so nothing serves ${image}" >&2; exit 1; }
}

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

PORTAL_IMAGE="${AR}/algorik-portal@$(digest_of "${AR}/algorik-portal:${SHA}")"
readonly PORTAL_IMAGE
attest "${PORTAL_IMAGE}"

# `--allow-unauthenticated` is Cloud Run's ingress, not the portal's: the
# landing must be able to send an anonymous browser to the sign-in page. The
# portal's gateway proxies qip-api with the viewer token it mounts below, so
# what stands between the internet and the platform is the portal's own
# session check, and `ALGORIK_AUTH_REQUIRED=true` is what turns it on. Left
# out once, the gateway answered anyone with the platform's data on a token
# nobody had to present; `infrastructure.rs` refuses the deploy without it.
echo "deploying algorik-portal…"
gcloud run deploy algorik-portal \
  --project "${PROJECT}" --region "${REGION}" \
  --image "${PORTAL_IMAGE}" \
  --binary-authorization=default \
  --allow-unauthenticated \
  --service-account "${CONSOLE_SA}" \
  --port 8080 --cpu 1 --memory 512Mi --min-instances 0 --max-instances 3 \
  --network "${VPC_NETWORK}" --subnet "${CONSOLE_SUBNET}" \
  --network-tags "${CONSOLE_EGRESS_TAG}" \
  --vpc-egress private-ranges-only \
  --set-env-vars "ALGORIK_ENV=development,ALGORIK_POSTURE=paper,ALGORIK_AUTH_REQUIRED=true,ALGORIK_IDENTITY_PROJECT_ID=${PROJECT},ALGORIK_IDENTITY_API_KEY=${IDENTITY_API_KEY},QIP_API_BASE_URL=${API_BASE_URL},QIP_API_TOKEN_FILE=${TOKEN_MOUNT},ALGORIK_SESSION_SECRET_FILE=${SESSION_SECRET_MOUNT}" \
  --set-secrets "${SESSION_SECRET_MOUNT}=algorik-session-secret:latest,${TOKEN_MOUNT}=qip-token-viewer-dev:latest" \
  --quiet
prove_serving algorik-portal "${PORTAL_IMAGE}"
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

LANDING_IMAGE="${AR}/algorik-landing@$(digest_of "${AR}/algorik-landing:${SHA}")"
readonly LANDING_IMAGE
attest "${LANDING_IMAGE}"

echo "deploying algorik-landing…"
gcloud run deploy algorik-landing \
  --project "${PROJECT}" --region "${REGION}" \
  --image "${LANDING_IMAGE}" \
  --binary-authorization=default \
  --allow-unauthenticated \
  --service-account "${LANDING_SA}" \
  --port 8080 --cpu 1 --memory 256Mi --min-instances 0 --max-instances 3 \
  --quiet
prove_serving algorik-landing "${LANDING_IMAGE}"
LANDING_URL="$(gcloud run services describe algorik-landing --project "${PROJECT}" --region "${REGION}" --format='value(status.url)')"
readonly LANDING_URL

echo
echo "portal:  ${PORTAL_URL}"
echo "landing: ${LANDING_URL}"
echo "next: add both hostnames to identity_authorized_domains in the dev tfvars and apply."
