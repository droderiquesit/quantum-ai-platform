#!/usr/bin/env bash
# Open the delivery consoles — Argo CD and Kargo — and print their URLs.
#
# There is no internet-facing URL for either, deliberately. Both are admin
# control planes for a cluster that has no ingress at all: the nodes have no
# public addresses, the control plane has no public endpoint, and every
# namespace denies ingress by default. A password-authenticated admin console
# on the public internet would be the widest hole in that posture, and it
# would be opened for convenience rather than for a requirement.
#
# So access is a tunnel authenticated by the operator's own Google identity:
# kubectl port-forward runs over the IAM-gated control-plane connection, which
# means the person reaching these consoles is the person IAM already vetted,
# and no listening surface is created for anyone else. The URLs below are real
# and work — they are just local to whoever ran this script.
#
# When algorik.ai lands, the reviewed way to publish these is an HTTPS load
# balancer behind Identity-Aware Proxy with a Google-managed certificate, so
# the same identity check happens at the edge. That is a deliberate commit,
# not something this script should improvise.
#
# Usage: scripts/open-consoles.sh [environment]     (default: dev)
#   Ctrl-C stops both tunnels.

set -euo pipefail

environment="${1:-dev}"
tfvars="infrastructure/environments/${environment}/terraform.tfvars"
[ -f "$tfvars" ] || { echo "no tfvars at ${tfvars}" >&2; exit 1; }
project="$(sed -n 's/^project_id[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' "$tfvars" | head -1)"

# The stray container variable that overrides real credentials elsewhere in
# this repository's scripts; harmless to unset when absent.
unset CLOUDSDK_AUTH_ACCESS_TOKEN

command -v kubectl >/dev/null || { echo "kubectl is not installed" >&2; exit 1; }
kubectl get namespace argocd >/dev/null 2>&1 || {
  echo "no argocd namespace — run scripts/bootstrap-gitops.sh ${environment} first" >&2
  exit 1
}

argocd_password="$(kubectl get secret argocd-initial-admin-secret -n argocd \
  -o jsonpath='{.data.password}' 2>/dev/null | base64 -d || true)"

echo
echo "  Argo CD   https://localhost:8443     user: admin"
if [ -n "$argocd_password" ]; then
  echo "            password: ${argocd_password}"
else
  echo "            password: the argocd-initial-admin-secret has been rotated or removed;"
  echo "            recover it with 'kubectl -n argocd get secret argocd-initial-admin-secret'"
fi
echo
echo "  Kargo     https://localhost:8444     user: admin"
echo "            password: gcloud secrets versions access latest \\"
echo "                        --secret algorik-kargo-admin-password --project ${project}"
echo
echo "  Both serve a self-signed certificate — the browser warning is expected;"
echo "  the tunnel underneath is the IAM-gated control-plane connection."
echo
echo "  Ctrl-C to close both tunnels."
echo

# Both tunnels die with this script, including on Ctrl-C, so a closed terminal
# never leaves a forwarded admin console listening on someone's laptop.
trap 'kill 0' EXIT INT TERM
kubectl port-forward svc/argocd-server -n argocd 8443:443 >/dev/null 2>&1 &
kubectl port-forward svc/kargo-api -n kargo 8444:443 >/dev/null 2>&1 &
wait
