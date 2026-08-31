#!/usr/bin/env bash
# Stand up the delivery and scaling stack on a cluster, from the repository
# alone.
#
# Everything this script applies is committed: the vendored upstream
# manifests, the network posture, and the per-environment overlays under
# infrastructure/gitops/. It exists so that "how does a new environment get
# Argo CD, Kargo, cert-manager and KEDA" has exactly one answer that cannot
# drift from what dev runs, instead of a trail of shell history. Every step
# is idempotent; re-running converges.
#
# What it deliberately does NOT do:
#   * mirror or attest images — the vendor workflow owns that, and a cluster
#     whose registry lacks the attested digests will refuse these pods at
#     admission, which is the correct order to discover that in;
#   * create credentials — bootstrap-kargo-admin.sh (run here, safely
#     re-runnable) and the Argo CD repository deploy key (README) are
#     deliberate acts with their own records;
#   * touch the qip namespace — the platform itself deploys through
#     deploy.yml today and through Argo CD after the ADR 0017 cut-over.
#
# Usage: scripts/bootstrap-gitops.sh <environment>

set -euo pipefail

environment="${1:?usage: bootstrap-gitops.sh <environment>}"
for overlay in cert-manager argocd kargo keda; do
  [ -d "infrastructure/gitops/${overlay}/overlays/${environment}" ] || {
    echo "no ${overlay} overlay for '${environment}' — add infrastructure/gitops/${overlay}/overlays/${environment} first" >&2
    exit 1
  }
done

unset CLOUDSDK_AUTH_ACCESS_TOKEN

# cert-manager first: Kargo's webhook certificates come from it, and a Kargo
# applied before the CA is ready flaps until it is.
kubectl apply --server-side --force-conflicts -k "infrastructure/gitops/cert-manager/overlays/${environment}"
kubectl rollout status deploy/cert-manager-webhook -n cert-manager --timeout=300s

kubectl apply --server-side --force-conflicts -k "infrastructure/gitops/argocd/overlays/${environment}"

kubectl apply --server-side --force-conflicts -k "infrastructure/gitops/kargo/overlays/${environment}"
# Refuses to overwrite existing credentials, so this is safe on every run.
scripts/bootstrap-kargo-admin.sh "${environment}" || true

kubectl apply --server-side --force-conflicts -k "infrastructure/gitops/keda/overlays/${environment}"

# The reconciler's assignment: what it may deploy and the one Application.
# Sync stays manual until the ADR 0017 cut-over commit.
kubectl apply --server-side -f infrastructure/gitops/argocd/apps/project.yaml
kubectl apply --server-side -f infrastructure/gitops/argocd/apps/dev.yaml

echo
echo "bootstrap applied. Remaining deliberate acts, each with its own record:"
echo "  * Argo CD repository read: add the deploy key per infrastructure/gitops/argocd/README.md"
echo "  * Argo CD UI:  kubectl port-forward svc/argocd-server -n argocd 8443:443  (admin / argocd-initial-admin-secret)"
echo "  * Kargo UI:    kubectl port-forward svc/kargo-api -n kargo 8444:443       (admin / Secret Manager: algorik-kargo-admin-password)"
