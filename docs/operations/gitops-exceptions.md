# GitOps exceptions

Argo CD is the sole writer to every Kubernetes namespace on the platform.
Two categories of deployment are deliberately excluded from that rule.

## 1. Cloud Run frontends

The console (and any future web frontend) runs on Cloud Run, not Kubernetes.
Cloud Run deploys through `gcloud run deploy` in its own workflow step, not
through Argo CD. The reasons:

- Cloud Run is a managed service with its own revision-based rollout and
  rollback. Argo CD's reconciliation model does not apply: there is no
  cluster to reconcile, and the "desired state" is a container image, not a
  set of manifests.
- The console's ingress is a managed HTTPS load balancer with IAP, not a
  Kubernetes Ingress resource. Argo CD has no visibility into that layer.

If a future frontend moves to Kubernetes (e.g., a containerized Next.js app
on GKE), it gets an Argo CD Application like any other workload.

## 2. One-time GitOps bootstrap

Installing and upgrading Argo CD itself is a `kubectl apply -k` against
`infrastructure/gitops/argocd/overlays/<env>/`. This is the one workload
that cannot be delivered by the reconciler — the reconciler does not exist
until it is applied. The same applies to:

- The `qip` AppProject (`infrastructure/gitops/argocd/apps/project.yaml`)
- The Argo CD Applications themselves (`infrastructure/gitops/argocd/apps/*.yaml`)

These are applied by hand, once per cluster, and committed so their
definitions have the same review trail as everything else. The runbook at
`infrastructure/gitops/argocd/README.md` documents the procedure.

## 3. Edge cells (manual sync, not an exception)

Edge cells are not an exception to GitOps — they *are* GitOps, but with
manual sync. The Argo CD Application for each cell exists in
`infrastructure/gitops/argocd/apps/edge.yaml` (template), and the cell's
desired state is still this repository at a revision. The difference is that
sync is triggered by an operator, not by the reconciler, because bringing a
cell up is a deliberate act that requires a capital envelope, venue ranges
and cell id — decisions the pipeline does not make.

See docs/operations/deploying-an-edge-cell.md for the full procedure.
