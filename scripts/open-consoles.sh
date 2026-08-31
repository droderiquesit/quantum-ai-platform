#!/usr/bin/env bash
# Print the delivery consoles' published URLs, and open a tunnel as a
# fallback.
#
# Both consoles now have a real address: an HTTPS load balancer behind
# Identity-Aware Proxy, which authenticates the operator's Google identity at
# Google's edge before the request reaches a backend at all. Who may pass is
# `console_operators` in the environment's tfvars — an IAM list in a reviewed
# file, not whoever holds the admin password below. Argo CD's own login still
# happens behind IAP, so each console has two checks rather than one.
#
# The tunnel remains, and is not vestigial. It is the way in when IAP itself
# is the thing that is broken, when a certificate is still provisioning (a
# Google-managed certificate takes up to an hour on first issue), and for an
# operator not yet on the IAM list. port-forward runs over the IAM-gated
# control-plane connection and creates no listening surface for anyone else,
# so it is a safe fallback rather than a hole.
#
# The URLs are read from the cluster rather than hard-coded here, because the
# Ingress is what actually decides them and a copy in a script is a second
# source of truth for a fact that already has one.
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

# The published address, asked of the Ingress that defines it. Absent until
# the console-ingress work is applied, which is why every line below tolerates
# an empty answer rather than failing: the tunnel still works without it.
published() {
  kubectl -n "$1" get ingress "$2" -o jsonpath='{.spec.rules[0].host}' 2>/dev/null || true
}
# Provisioning, Active, or FailedNotVisible. A browser cannot open the URL
# until this says Active, and the first issue takes up to an hour, so printing
# it is the difference between "wait" and "something is wrong".
certificate_state() {
  kubectl -n "$1" get managedcertificate "$2" \
    -o jsonpath='{.status.certificateStatus}' 2>/dev/null || true
}

argocd_host="$(published argocd argocd-console)"
kargo_host="$(published kargo kargo-console)"

if [ -n "$argocd_host" ] || [ -n "$kargo_host" ]; then
  echo
  echo "  Published, behind Identity-Aware Proxy — sign in with the Google"
  echo "  account named in ${tfvars}'s console_operators:"
  [ -n "$argocd_host" ] &&
    echo "    Argo CD   https://${argocd_host}   certificate: $(certificate_state argocd argocd-console)"
  [ -n "$kargo_host" ] &&
    echo "    Kargo     https://${kargo_host}   certificate: $(certificate_state kargo kargo-console)"
  echo
  echo "  A certificate that is still Provisioning means the URL is correct and"
  echo "  not yet servable. Use the tunnel below until it reads Active."
fi

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
