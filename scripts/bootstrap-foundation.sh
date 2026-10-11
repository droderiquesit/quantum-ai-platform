#!/usr/bin/env bash
# The tenant, from nothing, as one command (ADR 0118).
#
#   scripts/bootstrap-foundation.sh <ORG_ID|none> <BILLING_ACCOUNT_ID> <PREFIX>
#
# Run it from Google Cloud Shell (https://shell.cloud.google.com) as the
# owner, in a clone of this repository. It is the one step in the platform's
# life that needs organisation-level rights — create a folder, create
# projects, link billing, set organisation policy — and so the one step no
# workflow performs. Everything after it runs from infra.yml and deploy.yml
# under Workload Identity Federation. In order, it:
#
#   1. checks its arguments, the tools, and who you are authenticated as
#   2. creates the seed project `<PREFIX>-seed` if it does not exist, links
#      billing, and enables the APIs the foundation root calls
#   3. creates the foundation's state bucket in the seed if it does not exist
#   4. terraform init, adopts the seed into state once, and plans
#      infrastructure/foundation — the plan is shown, and nothing is applied
#      until you type `yes`; this script never auto-approves
#   5. records dev's project id and the number Google assigned it in
#      infrastructure/environments/dev/terraform.tfvars and the manifests that
#      embed them, and shows you the diff
#   6. chains to scripts/bootstrap-deploy.sh dev, which creates the platform
#      root's state bucket, its workload-identity pool and accounts, and seeds
#      its secrets — and which asks before it applies, too
#   7. prints exactly what exists now, and the commit to push
#
# What it never does: create, download or read a service-account key; pass
# -auto-approve; push to git; touch a project it did not name. ORG_ID and
# BILLING_ACCOUNT_ID are account identifiers and are written nowhere — not to
# a file, not to the repository; they reach Terraform as -var arguments for
# the length of one plan.
#
# Idempotent: every step detects work already done, so rerunning after a
# partial failure is the intended recovery. Arguments are refused, never
# corrected: a project id quietly repaired is a project nobody named.
set -euo pipefail

usage() {
  echo "usage: $0 <ORG_ID|none> <BILLING_ACCOUNT_ID> <PREFIX>" >&2
  echo "  ORG_ID              numeric organisation id (gcloud organizations list), or 'none'" >&2
  echo "                      for an account with no organisation (no folder, no org policy)" >&2
  echo "  BILLING_ACCOUNT_ID  XXXXXX-XXXXXX-XXXXXX (gcloud billing accounts list)" >&2
  echo "  PREFIX              4-24 chars; projects are <PREFIX>-dev, <PREFIX>-seed, ..." >&2
  exit 64
}

[[ $# -eq 3 ]] || usage
readonly ORG_ARG="$1"
readonly BILLING_ACCOUNT_ID="$2"
readonly PREFIX="$3"

# --- 1. arguments, tools, identity -------------------------------------------

# The same three rules infrastructure/foundation/variables.tf enforces,
# character for character, so this script refuses exactly what a plan would
# refuse — before it has created a project and a bucket rather than after.
if [[ "${ORG_ARG}" == "none" ]]; then
  ORG_ID=""
elif [[ "${ORG_ARG}" =~ ^[0-9]{1,20}$ ]]; then
  ORG_ID="${ORG_ARG}"
else
  echo "ORG_ID must be the numeric organisation id, or 'none'; got '${ORG_ARG}'." >&2
  exit 65
fi
readonly ORG_ID
if [[ ! "${BILLING_ACCOUNT_ID}" =~ ^[0-9A-F]{6}-[0-9A-F]{6}-[0-9A-F]{6}$ ]]; then
  echo "BILLING_ACCOUNT_ID must look like 0A1B2C-3D4E5F-6A7B8C (upper-case hex, no 'billingAccounts/')." >&2
  exit 65
fi
if [[ ! "${PREFIX}" =~ ^[a-z][a-z0-9-]{2,22}[a-z0-9]$ ]]; then
  echo "PREFIX must be 4-24 characters: a lowercase letter, then lowercase letters, digits" >&2
  echo "and hyphens, not ending in a hyphen ('<PREFIX>-stage' must fit Google's 30)." >&2
  exit 65
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly REPO_ROOT
readonly FOUNDATION_DIR="${REPO_ROOT}/infrastructure/foundation"
readonly DEV_TFVARS="${REPO_ROOT}/infrastructure/environments/dev/terraform.tfvars"
readonly SEED="${PREFIX}-seed"
readonly DEV_PROJECT="${PREFIX}-dev"
readonly STATE_BUCKET="${SEED}-foundation-tfstate"

# Every committed file that names dev's project id or embeds its number. The
# id is the build registry's host project too (ADR 0115), so the later
# environments' kustomizations and the Kargo chain name it as well. The
# acceptance tests that pin the same fact are listed so the tree stays green
# after a rewrite. ADRs and runbooks are history and are not rewritten.
readonly RECORDED_PATHS=(
  infrastructure/environments/dev/terraform.tfvars
  infrastructure/environments/README.md
  infrastructure/gitops
  scripts/deploy-frontends.sh
  docs/ops/runbooks/owner-switches.md
  backend/crates/tests/qip-acceptance/tests/gitops.rs
  backend/crates/tests/qip-acceptance/tests/infrastructure.rs
)

tfvar() {
  sed -n "s/^${1}[[:space:]]*=[[:space:]]*\"\{0,1\}\([^\"]*\)\"\{0,1\}[[:space:]]*$/\1/p" "${DEV_TFVARS}" | head -1
}
COMMITTED_DEV="$(tfvar project_id)"
readonly COMMITTED_DEV
COMMITTED_NUMBER="$(tfvar project_number)"
readonly COMMITTED_NUMBER
REGION="$(tfvar region)"
[[ -n "${REGION}" ]] || REGION="europe-west2" # the platform root's default
readonly REGION

missing=()
for tool in gcloud terraform git jq; do
  command -v "${tool}" >/dev/null 2>&1 || missing+=("${tool}")
done
if ((${#missing[@]} > 0)); then
  echo "missing: ${missing[*]}" >&2
  echo "Cloud Shell has all four; a recycled Cloud Shell VM can lose installed" >&2
  echo "packages — rerun the tool to see its install command, then rerun this." >&2
  exit 69
fi
# Cloud Shell's `terraform` can be an install-advisory stub that passes
# `command -v` (scripts/bootstrap-deploy.sh says what that once cost).
if ! terraform version 2>/dev/null | head -1 | grep -q '^Terraform v1\.9\.'; then
  echo "terraform on PATH is not Terraform 1.9.x (the foundation pins ~> 1.9.0):" >&2
  terraform version 2>&1 | head -1 >&2 || true
  echo "Install 1.9.8 from https://releases.hashicorp.com/terraform/1.9.8/ and rerun." >&2
  exit 69
fi

ACCOUNT="$(gcloud config get-value account 2>/dev/null || true)"
if [[ -z "${ACCOUNT}" ]]; then
  echo "gcloud is not authenticated. Run: gcloud auth login" >&2
  exit 77
fi
# Terraform reads application-default credentials, not gcloud's login. Cloud
# Shell provides them for the signed-in user; elsewhere they are a separate
# login, and without one the first plan fails on a credentials error.
if ! gcloud auth application-default print-access-token >/dev/null 2>&1; then
  echo "no application-default credentials for Terraform. Run:" >&2
  echo "  gcloud auth application-default login" >&2
  exit 77
fi

echo "authenticated as: ${ACCOUNT}"
if [[ -n "${ORG_ID}" ]]; then
  echo "mode:             organisation (folder + org policies)"
else
  echo "mode:             no organisation (projects under billing only; no folder, no org policy)"
fi
echo "seed project:     ${SEED}"
echo "dev project:      ${DEV_PROJECT}"
echo "state bucket:     gs://${STATE_BUCKET} (${REGION})"
echo

confirm() {
  local answer
  read -r -p "$1 Type 'yes' to continue: " answer
  [[ "${answer}" == "yes" ]] || {
    echo "stopped; nothing further was changed." >&2
    exit 1
  }
}

# The committed tfvars name the project the workflows will act on. A prefix
# that does not produce that id would create one project and point every
# workflow at another, so the mismatch is resolved — by rewriting the
# committed files, shown and confirmed — before anything is created.
if [[ "${COMMITTED_DEV}" != "${DEV_PROJECT}" ]]; then
  echo "dev's committed project id is '${COMMITTED_DEV}', but PREFIX gives '${DEV_PROJECT}'."
  if [[ "${COMMITTED_NUMBER}" != "0" ]]; then
    echo "and dev's tfvars already record project_number ${COMMITTED_NUMBER}, so they describe a" >&2
    echo "project that exists. Retire it first (docs/ops/runbooks/new-project-from-scratch.md)" >&2
    echo "or use the prefix it was created with." >&2
    exit 65
  fi
  echo "This rewrites '${COMMITTED_DEV}' to '${DEV_PROJECT}' in every committed file that names it:"
  (cd "${REPO_ROOT}" && git grep -l -F "${COMMITTED_DEV}" -- "${RECORDED_PATHS[@]}" | sed 's/^/  /')
  confirm "Rewrite them?"
  (cd "${REPO_ROOT}" && git grep -l -F "${COMMITTED_DEV}" -- "${RECORDED_PATHS[@]}" |
    while IFS= read -r file; do
      sed -i "s/${COMMITTED_DEV}/${DEV_PROJECT}/g" "${file}"
    done)
fi

# --- 2. the seed project -------------------------------------------------------

if gcloud projects describe "${SEED}" >/dev/null 2>&1; then
  echo "seed ${SEED} exists; adopting it"
else
  echo "creating seed project ${SEED}"
  parent=()
  [[ -n "${ORG_ID}" ]] && parent=(--organization="${ORG_ID}")
  gcloud projects create "${SEED}" --name="${SEED}" "${parent[@]}" --quiet
fi
# Billing first: a bucket cannot be created in a project with no billing.
gcloud billing projects link "${SEED}" --billing-account="${BILLING_ACCOUNT_ID}" --quiet >/dev/null
echo "enabling the seed's APIs (idempotent)…"
gcloud services enable --project="${SEED}" \
  serviceusage.googleapis.com cloudresourcemanager.googleapis.com \
  cloudbilling.googleapis.com billingbudgets.googleapis.com \
  orgpolicy.googleapis.com iam.googleapis.com storage.googleapis.com

# --- 3. the foundation's state bucket ------------------------------------------

if gcloud storage buckets describe "gs://${STATE_BUCKET}" >/dev/null 2>&1; then
  echo "state bucket gs://${STATE_BUCKET} exists"
else
  echo "creating state bucket gs://${STATE_BUCKET}"
  gcloud storage buckets create "gs://${STATE_BUCKET}" --project="${SEED}" \
    --location="${REGION}" --uniform-bucket-level-access \
    --public-access-prevention --quiet
fi
# Versioned so a bad apply's state can be rolled back by a person.
gcloud storage buckets update "gs://${STATE_BUCKET}" --versioning --quiet >/dev/null

# --- 4. plan, show, ask, apply -------------------------------------------------

tf() { terraform -chdir="${FOUNDATION_DIR}" "$@"; }
tf_vars=("-var=billing_account_id=${BILLING_ACCOUNT_ID}" "-var=prefix=${PREFIX}")
[[ -n "${ORG_ID}" ]] && tf_vars+=("-var=org_id=${ORG_ID}")

tf init -input=false -backend-config="bucket=${STATE_BUCKET}"
# The seed exists before the root that manages it, so it is imported once.
# A second run finds it in state and skips this.
if ! tf state list 2>/dev/null | grep -qx 'google_project.seed'; then
  echo "adopting ${SEED} into the foundation's state"
  tf import -input=false "${tf_vars[@]}" google_project.seed "${SEED}"
fi

plan_file="$(mktemp -t foundation-plan.XXXXXX)"
trap 'rm -f "${plan_file}"' EXIT
tf plan -input=false "${tf_vars[@]}" -out="${plan_file}"
echo
echo "Above is everything the foundation will create or change. Read it."
confirm "Apply exactly this plan?"
# A saved plan applies exactly what was shown and nothing else; it is not
# -auto-approve, which would plan again and apply whatever it found.
tf apply -input=false "${plan_file}"

# --- 5. record dev's project id and number ---------------------------------------

projects_json="$(tf output -json projects)"
number="$(jq -r '.dev.project_number // empty' <<<"${projects_json}")"
created_id="$(jq -r '.dev.project_id // empty' <<<"${projects_json}")"
if [[ ! "${number}" =~ ^[1-9][0-9]{5,19}$ || "${created_id}" != "${DEV_PROJECT}" ]]; then
  echo "the foundation reports dev as '${created_id}' number '${number}'; expected ${DEV_PROJECT}" >&2
  echo "and a project number. Is dev enabled in infrastructure/foundation/terraform.tfvars?" >&2
  exit 70
fi

current="$(tfvar project_number)"
if [[ "${current}" == "${number}" ]]; then
  echo "dev's tfvars already record project number ${number}"
elif [[ "${current}" == "0" ]]; then
  echo "recording ${DEV_PROJECT}'s number ${number}"
  sed -i "s/^project_number = 0\$/project_number = ${number}/" "${DEV_TFVARS}"
  # The two manifests that embed the number: IAP's service agent, which is
  # the console's one invoker, and the API URL the console dials.
  sed -i "s/service-0@gcp-sa-iap\.iam\.gserviceaccount\.com/service-${number}@gcp-sa-iap.iam.gserviceaccount.com/" \
    "${REPO_ROOT}/infrastructure/gitops/envs/dev/invokers.yaml"
  sed -i "s/qip-dev-api-0\./qip-dev-api-${number}./" \
    "${REPO_ROOT}/infrastructure/gitops/envs/dev/portal.yaml"
else
  echo "dev's tfvars record project_number ${current}, but ${DEV_PROJECT} is ${number}." >&2
  echo "One of the two is stale; nothing was rewritten. Resolve it by hand before continuing." >&2
  exit 70
fi
if [[ "$(tfvar project_number)" != "${number}" ]]; then
  echo "failed to record the number in ${DEV_TFVARS}; is the line exactly 'project_number = 0'?" >&2
  exit 70
fi

# --- 6. the platform root's bootstrap -------------------------------------------

echo
echo "chaining to scripts/bootstrap-deploy.sh dev — it shows its own plan and asks again"
"${REPO_ROOT}/scripts/bootstrap-deploy.sh" dev

# --- 7. what exists now ----------------------------------------------------------

folder="$(tf output -raw folder_id 2>/dev/null || true)"
echo
echo "================================================================================"
echo "Created or adopted (re-running this script changes none of it):"
if [[ -n "${folder}" && "${folder}" != "null" ]]; then
  echo "  folder        folders/${folder}  (org policies: see controls_not_applied below)"
fi
echo "  seed project  ${SEED}  — foundation state in gs://${STATE_BUCKET}"
jq -r 'to_entries[] | "  project       \(.value.project_id)  number \(.value.project_number)  (\(.key))"' <<<"${projects_json}"
echo "  budgets       $(tf output -json budgets | jq -r 'to_entries | map("\(.key)=\(.value)") | join(", ")')  (alerts at 50/90/100%)"
echo "  platform      ${DEV_PROJECT}-qip-tfstate, pool qip-github-dev, account qip-infra-dev,"
echo "                claude-builder, generated secrets (scripts/bootstrap-deploy.sh)"
echo "Not applied:"
tf output -json controls_not_applied | jq -r '.[] | "  - \(.)"'
echo
echo "The repository now records dev's project. Commit and push it — the workflows"
echo "read only committed tfvars:"
(cd "${REPO_ROOT}" && git status --short -- "${RECORDED_PATHS[@]}" | sed 's/^/  /')
echo
echo "  git checkout -b record-${DEV_PROJECT}"
echo "  git commit -am 'Record ${DEV_PROJECT} and its project number (ADR 0118)'"
echo "  git push -u origin record-${DEV_PROJECT}   # then merge it through review"
echo
echo "Then: infra.yml plan -> up (dev), deploy.yml — docs/ops/runbooks/new-project-from-scratch.md"
echo "================================================================================"
