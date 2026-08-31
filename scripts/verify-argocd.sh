#!/usr/bin/env bash
# Say what Argo CD is actually doing, and what to do about it.
#
# Every failure this script reports has happened on this platform, and each
# one looked the same from the outside — an Application stuck on `Unknown`
# with the real reason three `kubectl -o jsonpath` invocations away. The
# reasons were: DNS that never resolved (NodeLocal DNSCache keeps the kube-dns
# ClusterIP as the destination, which no pod selector matches), a repository
# the reconciler had no credential for, and an AppProject that forbade the
# cluster-scoped kinds the chart contains, so Argo owned resources it could
# not touch. This prints the state and names the fix rather than leaving the
# next person to rediscover them.
#
# Read-only. It changes nothing.
#
# Usage: scripts/verify-argocd.sh [application]     (default: qip-dev)

set -uo pipefail

app="${1:-qip-dev}"
unset CLOUDSDK_AUTH_ACCESS_TOKEN

fail=0
note() { printf '  %s\n' "$*"; }

command -v kubectl >/dev/null || { echo "kubectl is not installed" >&2; exit 1; }

echo
echo "argocd controllers"
if ! kubectl get namespace argocd >/dev/null 2>&1; then
  note "the argocd namespace does not exist."
  note "FIX: scripts/bootstrap-gitops.sh dev"
  exit 1
fi
not_ready="$(kubectl get deploy -n argocd --no-headers 2>/dev/null \
  | awk '{split($2, r, "/"); if (r[1] != r[2]) print "  " $1 " " $2}')"
if [ -n "$not_ready" ]; then
  note "not every controller is available:"; echo "$not_ready"
  note "FIX: kubectl describe pod -n argocd <name> — check image pull (Binary"
  note "     Authorization denies anything the vendor workflow has not attested)"
  note "     and NetworkPolicy egress in infrastructure/gitops/argocd/base."
  fail=1
else
  note "all controllers available"
fi

echo
echo "repository access"
repo="$(kubectl get application "$app" -n argocd -o jsonpath='{.spec.source.repoURL}' 2>/dev/null)"
if [ -z "$repo" ]; then
  note "no Application named ${app} in the argocd namespace."
  note "FIX: kubectl apply --server-side -f infrastructure/gitops/argocd/apps/"
  exit 1
fi
note "repoURL: ${repo}"
case "$repo" in
  https://*)
    note "anonymous HTTPS — no credential needed while the repository is public"
    ;;
  *)
    if kubectl get secret -n argocd -l argocd.argoproj.io/secret-type=repository \
         -o jsonpath='{.items[*].data.sshPrivateKey}' 2>/dev/null | grep -q .; then
      note "an ssh credential is present"
    else
      note "this URL needs a credential and none is configured."
      note "FIX: either make the repository readable anonymously and use its"
      note "     https:// URL, or create a repository Secret carrying the key."
      fail=1
    fi
    ;;
esac

echo
echo "application"
sync="$(kubectl get application "$app" -n argocd -o jsonpath='{.status.sync.status}' 2>/dev/null)"
health="$(kubectl get application "$app" -n argocd -o jsonpath='{.status.health.status}' 2>/dev/null)"
revision="$(kubectl get application "$app" -n argocd -o jsonpath='{.status.sync.revision}' 2>/dev/null)"
note "sync=${sync:-<none>}  health=${health:-<none>}"
[ -n "$revision" ] && note "reading revision ${revision}"

# Check whether automated sync is enabled.
automated="$(kubectl get application "$app" -n argocd -o jsonpath='{.spec.syncPolicy.automated}' 2>/dev/null)"
if [ -n "$automated" ]; then
  note "automated sync is enabled (prune + self-heal)"
else
  note "automated sync is NOT enabled — sync is manual"
  note "NOTE: central-plane Applications (dev, test, stage, prod) should have"
  note "      automated sync enabled. Only edge cells use manual sync."
fi

case "$sync" in
  Synced)
    note "the cluster matches the chart at that revision"
    ;;
  OutOfSync)
    if [ -n "$automated" ]; then
      note "OutOfSync with automated sync enabled — the reconciler should"
      note "be syncing. Check the Argo CD controller logs:"
      note "  kubectl logs -n argocd deployment/argocd-application-controller"
      fail=1
    else
      note "OutOfSync — expected for manual-sync Applications. Run:"
      note "  argocd app sync $app"
      note "or click Sync in the Argo CD UI."
    fi
    ;;
  *)
    note "the reconciler cannot compare. The condition below says why:"
    fail=1
    ;;
esac

conditions="$(kubectl get application "$app" -n argocd -o jsonpath='{range .status.conditions[*]}{.type}: {.message}{"\n"}{end}' 2>/dev/null)"
if [ -n "$conditions" ]; then
  echo "$conditions" | while IFS= read -r line; do note "$line"; done
  case "$conditions" in
    *"dial udp"*|*"lookup"*|*"i/o timeout"*)
      note "FIX: DNS. Under NodeLocal DNSCache the query keeps the kube-dns"
      note "     ClusterIP as its destination, so a pod-selector rule never"
      note "     matches — allow-dns needs the VIP as an ipBlock. See"
      note "     infrastructure/gitops/argocd/base/namespace.yaml."
      ;;
    *"authenticate"*|*"publickey"*|*"Permission denied"*)
      note "FIX: repository access — see the section above."
      ;;
  esac
fi

echo
echo "resources"
unknown="$(kubectl get application "$app" -n argocd \
  -o jsonpath='{range .status.resources[?(@.status=="Unknown")]}{.kind}/{.name}{"\n"}{end}' 2>/dev/null)"
total="$(kubectl get application "$app" -n argocd \
  -o jsonpath='{.status.resources[*].kind}' 2>/dev/null | wc -w)"
note "${total} tracked"
if [ -n "$unknown" ]; then
  note "these are owned by the Application and cannot be reconciled:"
  echo "$unknown" | while IFS= read -r line; do note "  $line"; done
  note "FIX: the AppProject forbids the kind. A cluster-scoped kind needs an"
  note "     entry in clusterResourceWhitelist in"
  note "     infrastructure/gitops/argocd/apps/project.yaml."
  fail=1
else
  note "none Unknown — every tracked resource is one the project permits"
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "Argo CD is reading the repository and reconciling correctly."
else
  echo "Argo CD is not fully working — see the FIX lines above."
fi
exit "$fail"
