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
#   3. creates the bootstrap service account if it does not exist, and lets
#      the current user impersonate it
#   4. creates the Terraform state bucket if it does not exist
#   5. terraform init + apply, impersonating claude-builder — apply shows its
#      plan and asks before changing anything; this script never auto-approves
#   6. sets the six GitHub Actions *variables* the deploy pipeline reads
#   7. seeds the six generated secrets with random values, if they are empty
#
# What it deliberately never does: create, download or read a service-account
# key. Authentication is your own identity impersonating the bootstrap
# account, so credentials are short-lived and the audit log names a person.
# There is no secret anywhere in this flow — all six pipeline values are
# identifiers, which is why they are variables and not GitHub secrets.
#
# Idempotent: every step either detects its work is already done or is safe
# to repeat, so rerunning after a partial failure is the intended recovery.
set -euo pipefail

# The four names are the ones `var.environment` validates, and they are short
# on purpose: they are interpolated into Google resource ids with hard length
# limits, and `qip-edge-frankfurt-1-production` is 31 characters against a
# service account's limit of 30. Naming the directory after the value keeps the
# thing you type and the thing that lands in resource names the same string.
readonly ENVIRONMENT="${1:-dev}"
case "${ENVIRONMENT}" in
  dev | test | stage | prod) ;;
  *)
    echo "usage: $0 [dev|test|stage|prod]" >&2
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
for tool in gcloud terraform gh openssl; do
  command -v "${tool}" >/dev/null 2>&1 || missing+=("${tool}")
done
if ((${#missing[@]} > 0)); then
  echo "missing: ${missing[*]}" >&2
  # Cloud Shell resets /usr when its VM recycles — the home directory
  # survives, installed packages do not. So "it worked this morning" and
  # "terraform: command not found" are both true, and the fix is to install
  # again (Cloud Shell prints the commands when you run the missing tool) or
  # to persist it via $HOME/.customize_environment:
  # https://cloud.google.com/shell/docs/configuring-cloud-shell
  echo "on Cloud Shell a recycled VM loses installed packages; reinstall the" >&2
  echo "missing tool (running it prints the install commands) and rerun this." >&2
  exit 69
fi

# `command -v` is not enough for terraform: Cloud Shell ships a stub that
# passes the check and prints apt install instructions instead of running.
# One bootstrap trusted it, captured several lines of that advisory from
# `terraform output`, and set them as the workload-identity variable — a
# value that passed a non-empty check and failed authentication with an
# error naming an invalid audience. Ask the binary what it is before
# believing it.
if ! terraform version 2>/dev/null | head -1 | grep -q '^Terraform v'; then
  echo "the 'terraform' on PATH is not terraform (Cloud Shell installs a stub" >&2
  echo "that prints install instructions). Install the real one and rerun:" >&2
  echo "  sudo apt update && sudo apt install -y terraform" >&2
  exit 69
fi

# Checked here, not at step 6 where it is used. Failing after a twenty-minute
# apply for a login that takes thirty seconds would mean running the whole
# script twice; nothing before step 6 depends on GitHub, but the point of one
# command is that it finishes.
if ! gh auth status >/dev/null 2>&1; then
  echo "gh is not authenticated. Run: gh auth login   (then rerun this script)" >&2
  exit 77
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

# --- 3. the bootstrap account ------------------------------------------------

# Created here if it does not exist. The account is defined in
# docs/security/credentials.md as the project-admin identity that applies
# Terraform — so its role is owner, and the safety of that is not the role but
# the access path: nobody holds a key to this account, and using it means
# being someone the audit log names, impersonating it for an hour.
#
# Owner rather than a hand-picked list, deliberately. The configuration
# creates IAM bindings, service accounts, a workload identity pool, KMS keys,
# clusters and buckets; a curated role list that misses one permission fails
# in the middle of an apply with an error naming a permission rather than a
# decision, which is exactly the failure this script exists to prevent.
if ! gcloud iam service-accounts describe "${BOOTSTRAP_SA}" >/dev/null 2>&1; then
  echo "creating bootstrap service account ${BOOTSTRAP_SA}…"
  if ! gcloud iam service-accounts create claude-builder \
    --display-name="qip bootstrap: applies Terraform, used via impersonation only"; then
    echo >&2
    echo "could not create ${BOOTSTRAP_SA}; a project owner must create it:" >&2
    echo "  gcloud iam service-accounts create claude-builder" >&2
    exit 77
  fi
fi

# A freshly created account is not immediately visible to the IAM APIs:
# creation returns success and the next call answers "does not exist". That is
# Google's documented eventual consistency, not a fault, so wait for the
# account to become describable before granting anything to or on it.
visible=false
for _ in $(seq 1 18); do
  if gcloud iam service-accounts describe "${BOOTSTRAP_SA}" >/dev/null 2>&1; then
    visible=true
    break
  fi
  echo "waiting for ${BOOTSTRAP_SA} to become visible…"
  sleep 5
done
if [[ "${visible}" != true ]]; then
  echo "${BOOTSTRAP_SA} was created but has not become visible after 90s; rerun this script" >&2
  exit 75
fi

# The owner grant sits outside the creation branch on purpose. A previous run
# that created the account and then failed here would otherwise leave an
# account with no roles that every rerun skips past — an apply that fails
# twenty minutes later, on the first resource, with a permissions error naming
# nothing useful. Re-adding an existing binding is a no-op, so this costs an
# already-correct project nothing. Retried, because the same propagation delay
# that affects describe can refuse the first grant.
granted=false
for _ in $(seq 1 6); do
  if gcloud projects add-iam-policy-binding "${PROJECT}" \
    --member="serviceAccount:${BOOTSTRAP_SA}" \
    --role="roles/owner" --condition=None --quiet >/dev/null 2>&1; then
    granted=true
    break
  fi
  echo "granting roles/owner to ${BOOTSTRAP_SA} failed; retrying in 10s…"
  sleep 10
done
if [[ "${granted}" != true ]]; then
  echo >&2
  echo "could not grant roles/owner on ${PROJECT} to ${BOOTSTRAP_SA}." >&2
  echo "a project owner must run:" >&2
  echo "  gcloud projects add-iam-policy-binding ${PROJECT} \\" >&2
  echo "    --member=\"serviceAccount:${BOOTSTRAP_SA}\" --role=\"roles/owner\"" >&2
  exit 77
fi

# Grant yourself token-creator on the bootstrap account. Needs to succeed only
# once; if you lack the authority to grant it, the error below says who to
# ask. Retried for the same propagation reason as the grant above.
impersonation=false
for _ in $(seq 1 6); do
  if gcloud iam service-accounts add-iam-policy-binding "${BOOTSTRAP_SA}" \
    --member="user:${ACCOUNT}" \
    --role="roles/iam.serviceAccountTokenCreator" \
    --condition=None --quiet >/dev/null 2>&1; then
    impersonation=true
    break
  fi
  echo "granting impersonation failed; retrying in 10s…"
  sleep 10
done
if [[ "${impersonation}" != true ]]; then
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

# Variables, not secrets: all six are identifiers that appear in resource
# names anyway. See docs/security/credentials.md for why that distinction is
# load-bearing.
#
# Six, not four. `deploy.yml` refuses to build unless the two Binary
# Authorization variables are set — an unattested image is one the cluster's
# admission policy rejects, so a pipeline missing them would push images and
# then fail at the point where no pod starts. Setting four of six here left
# that failure for the first person to press the button.
wip="$(terraform -chdir="${TF_DIR}" output -raw workload_identity_provider)"
deploy_sa="$(terraform -chdir="${TF_DIR}" output -raw deploy_service_account)"
attestor="$(terraform -chdir="${TF_DIR}" output -raw binary_authorization_attestor)"
key_version="$(terraform -chdir="${TF_DIR}" output -raw binary_authorization_key_version)"
# The seventh: what infra.yml impersonates to plan, apply and tear down the
# stack from the repository. See modules/cicd for what bounds it.
infra_sa="$(terraform -chdir="${TF_DIR}" output -raw infra_service_account)"

# The infra account's one object-delete grant, scoped to the state bucket.
# Overwriting Terraform state requires storage.objects.delete, and the
# project-level role the account holds deliberately lacks it so nothing it
# runs can delete from the evidence bucket. Here rather than in Terraform:
# the state bucket is created by this script (state cannot bootstrap itself),
# and the acceptance suite refuses delete-capable storage roles in any .tf.
gcloud storage buckets add-iam-policy-binding "gs://${STATE_BUCKET}" \
  --member="serviceAccount:${infra_sa}" \
  --role="roles/storage.objectAdmin" --quiet >/dev/null

declare -A pipeline_variables=(
  [GCP_PROJECT]="${PROJECT}"
  [GCP_REGION]="${REGION}"
  [GCP_WORKLOAD_IDENTITY_PROVIDER]="${wip}"
  [GCP_DEPLOY_SERVICE_ACCOUNT]="${deploy_sa}"
  [GCP_BINAUTHZ_ATTESTOR]="${attestor}"
  [GCP_BINAUTHZ_KEY_VERSION]="${key_version}"
  [GCP_INFRA_SERVICE_ACCOUNT]="${infra_sa}"
)

# Shape-checked, not just non-empty. Non-empty let a Cloud Shell stub's
# install advisory through as the workload-identity provider, so each value
# is held to the one form it can legitimately take. Multi-line refuses first:
# no identifier here contains a newline, and the advisory was nothing but.
for name in "${!pipeline_variables[@]}"; do
  value="${pipeline_variables[$name]}"
  [[ -n "${value}" && "${value}" != *$'\n'* ]] || {
    echo "the value for ${name} is empty or spans lines; refusing to set it:" >&2
    echo "${value}" >&2
    exit 65
  }
done
[[ "${wip}" =~ ^projects/[0-9]+/locations/global/workloadIdentityPools/.+/providers/.+$ ]] || {
  echo "workload_identity_provider is not a provider resource name: ${wip}" >&2
  exit 65
}
for account in "${deploy_sa}" "${infra_sa}"; do
  [[ "${account}" =~ ^[a-z][a-z0-9-]*@[a-z0-9-]+\.iam\.gserviceaccount\.com$ ]] || {
    echo "not a service-account email: ${account}" >&2
    exit 65
  }
done
[[ "${key_version}" =~ ^projects/.+/cryptoKeyVersions/[0-9]+$ ]] || {
  echo "binary_authorization_key_version is not a key version name: ${key_version}" >&2
  exit 65
}

if gh auth status >/dev/null 2>&1; then
  for name in "${!pipeline_variables[@]}"; do
    gh variable set "${name}" --repo "${GITHUB_REPOSITORY}" --body "${pipeline_variables[$name]}"
  done
  echo "pipeline variables set on ${GITHUB_REPOSITORY}"
else
  echo
  echo "gh is not authenticated (run: gh auth login). Set the variables yourself:"
  for name in "${!pipeline_variables[@]}"; do
    echo "  gh variable set ${name} --repo ${GITHUB_REPOSITORY} --body \"${pipeline_variables[$name]}\""
  done
fi

# --- 7. the generated secrets ------------------------------------------------

# Terraform creates the secret containers empty — their values are never in
# Terraform, so a leaked state file leaks no credential. Six of them are
# self-generated random values with no external party involved, and a secret
# with no version fails the CSI mount, leaving every pod in ContainerCreating.
# So the six are seeded here, once: an existing value is never overwritten,
# because replacing the capital-envelope key mid-flight would strand every
# grant signed under the old one. Rotation is a deliberate act — add a version
# and restart the workloads.
#
# The vendor credentials — qip-market-data-key, qip-venue-credential,
# qip-quantum-token — are deliberately not here. They come from a data vendor,
# a broker and a quantum provider respectively; a random value would not be a
# credential, and the venue credential is unreadable under paper trading
# anyway.
for secret in qip-token-operator qip-token-approver qip-token-analyst \
  qip-token-viewer qip-token-monitor qip-capital-envelope-key; do
  qualified="${secret}-${ENVIRONMENT}"
  if [[ -z "$(gcloud secrets versions list "${qualified}" --limit=1 --format='value(name)' 2>/dev/null)" ]]; then
    openssl rand -base64 48 | gcloud secrets versions add "${qualified}" --data-file=- >/dev/null
    echo "seeded ${qualified}"
  else
    echo "${qualified} already has a value; left as it is"
  fi
done

echo
echo "done. The infrastructure is applied and the pipeline can authenticate."
echo
echo "images deploy automatically when ci passes on the default branch, or on demand with:"
echo "  gh workflow run deploy --repo ${GITHUB_REPOSITORY} -f environment=${ENVIRONMENT}"
echo
echo "the cluster's autonomy ceiling is $(tfvar autonomy_ceiling): supplying a"
echo "venue credential does not enable live trading, and nothing in this script"
echo "can raise the ceiling."
