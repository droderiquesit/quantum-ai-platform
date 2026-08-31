# DEPRECATED — superseded by the Helm chart

These manifests are the sed-rendered originals that `deploy.yml` used to
`kubectl apply`. They are **no longer applied by any pipeline step**.

The Helm chart at `infrastructure/helm/qip/` is the authoritative source of
Kubernetes manifests. It is a 1:1 template conversion of these files (ADR
0017), and the Argo CD Applications in `infrastructure/gitops/argocd/apps/`
reconcile the cluster against it.

## Why these files remain

- **Reference.** The Helm templates are a mechanical conversion; the original
  files carry the extensive comments that explain *why* each resource exists.
  Until those comments are migrated to the templates, these files are the
  canonical explanation.
- **Edge cells.** `edge-cell.yaml` documents the cell substitution pattern
  (`__CELL_ID__`, `__CELL_REGION__`, `CELL_VENUES`) that the Helm chart's
  `cell.*` values replace. The runbook at
  `docs/operations/deploying-an-edge-cell.md` references both.

## What changed

| Before | After |
|---|---|
| `deploy.yml` rendered these files with `sed` and applied with `kubectl` | `deploy.yml` writes digests to `values-<env>-images.yaml` and commits |
| Argo CD sync was manual (drift-reporting only) | Argo CD sync is automated with prune + self-heal |
| Two writers to the qip namespace (kubectl + Argo) | One writer: Argo CD |

## Do not

- Do not edit these files to change cluster state. Edit the Helm templates
  instead.
- Do not add new manifests here. New resources belong in
  `infrastructure/helm/qip/templates/`.
