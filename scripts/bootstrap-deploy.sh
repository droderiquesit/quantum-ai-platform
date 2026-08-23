#!/usr/bin/env bash
# The whole first deployment, as one command.
#
# Run this from Google Cloud Shell (https://shell.cloud.google.com), where
# gcloud, terraform and gh are preinstalled and you are already authenticated,
# or from any machine that has all three. It performs, in order, the manual
# flow documented in docs/security/credentials.md:
#
#   1. checks the tools and the authenticated identity
#   2. enables the project APIs the configuration needs
#   3. lets the current user impersonate the bootstrap service account
#   4. creates the Terraform state bucket if it does not exist
#   5. terraform init + apply, impersonating claude-builder — apply shows its
#      plan and asks before changing anything; this script never auto-approves
#   6. sets the four GitHub Actions *variables* the deploy pipeline reads
#
# What it deliberately never does: create, download or read a service-account
# key. Authentication is your own identity impersonating the bootstrap
# account, so credentials are short-lived and the audit log names a person.
# There is no secret anywhere in this flow — the four pipeline values are
# identifiers, which is why they are variables and not GitHub secrets.
#
# Idempotent: every step either detects its work is already done or is safe
# to repeat, so rerunning after a partial failure is the intended recovery.
set -euo pipefail

readonly ENVIRONMENT="${1:-development}"
case "${ENVIRONMENT}" in
  development | staging | production) ;;
  *)
    echo "usage: $0 [development|staging|production]" >&2
    exit 64
    ;;
esac

readonly REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly TFVARS="${REPO_ROOT}/infrastructure/environments/${ENVIRONMENT}/terraform.tfvars"
readonly TF_DIR="${REPO_ROOT}/infrastructure/terraform"

[[ -f "${TFVARS}" ]] || {
  echo "no tfvars at ${TFVARS}" >&2
  exit 66
}

# Identity values come from the tfvars, so this script cannot disagree with
# the configuration it applies.
tfvar() {
  sed -n "s/^${1}[[:space:]]*=[[:space:]]*\"\(.*\)\"/\1/p" "${TFVARS}" | head -1
}
readonly PROJECT="$(tfvar project_id)"
readonly GITHUB_REPOSITORY="$(tfvar github_repository)"
REGION="$(tfvar region)"
[[ -n "${REGION}" ]] || REGION="europe-west2" # the variable's default
readonly REGION
readonly BOOTSTRAP_SA="claude-builder@${PROJECT}.iam.gserviceaccount.com"
# Project ids are globally unique, so this name is too. Not configurable:
# a state bucket nobody can guess the name of is a state bucket the next
# operator cannot find.
readonly STATE_BUCKET="${PROJECT}-qip-tfstate"

echo "environment:   ${ENVIRONMENT}"
echo "project:       ${PROJECT}"
echo "region:        ${REGION}"
echo "impersonating: ${BOOTSTRAP_SA}"
echo

# --- 1. tools and identity ---------------------------------------------------

missing=()
for tool in gcloud terraform gh; do
  command -v "${tool}" >/dev/null 2>&1 || missing+=("${tool}")
done
if ((${#missing[@]} > 0)); then
  echo "missing: ${missing[*]}" >&2
  echo "Cloud Shell has all three preinstalled: https://shell.cloud.google.com" >&2
  exit 69
fi

ACCOUNT="$(gcloud config get-value account 2>/dev/null || true)"
if [[ -z "${ACCOUNT}" ]]; then
  echo "gcloud is not authenticated. Run: gcloud auth login" >&2
  exit 77
fi
echo "authenticated as ${ACCOUNT}"
gcloud config set project "${PROJECT}" --quiet

# --- 2. project APIs ---------------------------------------------------------

# The list from docs/security/credentials.md, plus iamcredentials — without
# it the impersonation below fails with an error that does not name it.
echo "enabling APIs (idempotent, may take a minute on first run)…"
gcloud services enable \
  container.googleapis.com compute.googleapis.com \
  artifactregistry.googleapis.com secretmanager.googleapis.com \
  cloudkms.googleapis.com iam.googleapis.com \
  iamcredentials.googleapis.com cloudresourcemanager.googleapis.com \
  monitoring.googleapis.com logging.googleapis.com storage.googleapis.com

# --- 3. impersonation --------------------------------------------------------

# Grant yourself token-creator on the bootstrap account. Needs to succeed only
# once; if you lack the authority to grant it, the error below says who to ask.
if ! gcloud iam service-accounts add-iam-policy-binding "${BOOTSTRAP_SA}" \
  --member="user:${ACCOUNT}" \
  --role="roles/iam.serviceAccountTokenCreator" \
  --condition=None --quiet >/dev/null 2>&1; then
  echo >&2
  echo "could not grant impersonation of ${BOOTSTRAP_SA} to ${ACCOUNT}." >&2
  echo "a project owner must run:" >&2
  echo "  gcloud iam service-accounts add-iam-policy-binding ${BOOTSTRAP_SA} \\" >&2
  echo "    --member=\"user:${ACCOUNT}\" --role=\"roles/iam.serviceAccountTokenCreator\"" >&2
  exit 77
fi
# IAM grants propagate asynchronously; a fresh one can take ~1 minute to bite.
# If the apply below fails with 403 on generateAccessToken, just rerun.

# --- 4. the state bucket -----------------------------------------------------

# Created here and not by Terraform, because state cannot bootstrap itself.
# Versioned, so a corrupted state file has a history to recover from.
if ! gcloud storage buckets describe "gs://${STATE_BUCKET}" >/dev/null 2>&1; then
  echo "creating state bucket gs://${STATE_BUCKET} in ${REGION}…"
  gcloud storage buckets create "gs://${STATE_BUCKET}" \
    --location="${REGION}" --uniform-bucket-level-access
  gcloud storage buckets update "gs://${STATE_BUCKET}" --versioning
else
  echo "state bucket gs://${STATE_BUCKET} already exists"
fi

# --- 5. terraform ------------------------------------------------------------

export GOOGLE_IMPERSONATE_SERVICE_ACCOUNT="${BOOTSTRAP_SA}"

terraform -chdir="${TF_DIR}" init -backend-config="bucket=${STATE_BUCKET}"

# No -auto-approve, deliberately. The plan terraform prints and the yes you
# type are the review; a bootstrap script that skips them is a script one bad
# tfvars away from applying the wrong environment.
terraform -chdir="${TF_DIR}" apply -var-file="${TFVARS}"

# --- 6. the pipeline variables -----------------------------------------------

# Variables, not secrets: all four are identifiers that appear in resource
# names anyway. See docs/security/credentials.md for why that distinction is
# load-bearing.
wip="$(terraform -chdir="${TF_DIR}" output -raw workload_identity_provider)"
deploy_sa="$(terraform -chdir="${TF_DIR}" output -raw deploy_service_account)"

if gh auth status >/dev/null 2>&1; then
  gh variable set GCP_PROJECT --repo "${GITHUB_REPOSITORY}" --body "${PROJECT}"
  gh variable set GCP_REGION --repo "${GITHUB_REPOSITORY}" --body "${REGION}"
  gh variable set GCP_WORKLOAD_IDENTITY_PROVIDER --repo "${GITHUB_REPOSITORY}" --body "${wip}"
  gh variable set GCP_DEPLOY_SERVICE_ACCOUNT --repo "${GITHUB_REPOSITORY}" --body "${deploy_sa}"
  echo "pipeline variables set on ${GITHUB_REPOSITORY}"
else
  echo
  echo "gh is not authenticated (run: gh auth login). Set the variables yourself:"
  echo "  gh variable set GCP_PROJECT --repo ${GITHUB_REPOSITORY} --body \"${PROJECT}\""
  echo "  gh variable set GCP_REGION --repo ${GITHUB_REPOSITORY} --body \"${REGION}\""
  echo "  gh variable set GCP_WORKLOAD_IDENTITY_PROVIDER --repo ${GITHUB_REPOSITORY} --body \"${wip}\""
  echo "  gh variable set GCP_DEPLOY_SERVICE_ACCOUNT --repo ${GITHUB_REPOSITORY} --body \"${deploy_sa}\""
fi

echo
echo "done. The infrastructure is applied and the pipeline can authenticate."
echo
echo "images deploy automatically when ci passes on main, or on demand with:"
echo "  gh workflow run deploy --repo ${GITHUB_REPOSITORY} -f environment=${ENVIRONMENT}"
echo
echo "the cluster's autonomy ceiling is $(tfvar autonomy_ceiling): supplying a"
echo "venue credential does not enable live trading, and nothing in this script"
echo "can raise the ceiling."
