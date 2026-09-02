# GitOps exceptions — retired

**This runbook described a delivery mechanism that no longer exists.** Argo
CD, Kargo, KEDA, cert-manager, the Helm chart and the Kubernetes manifests
were retired under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md),
which records the owner's instruction, what replaced each of them, and what
was never applied. Nothing below is a procedure anybody should follow.

The one thing this document held that was not a procedure was its fourth
section: the record that, after the GitOps cut-over, nobody verified a
promotion — a build that crash-looped on boot produced a green pipeline. That
gap is closed rather than moved. `.github/workflows/deploy.yml`'s `deploy` job
moves each Cloud Run service with `gcloud run services update`, which blocks
until the revision is Ready and routing, then reads the serving revision back
and fails the run unless it carries the digest that was signed. The acceptance
check `a_promotion_names_who_verifies_it` now reads that job rather than this
file.

What each part of the retired stack became:

| Retired | Replaced by |
|---|---|
| Argo CD Applications, `infrastructure/gitops/argocd` | `infrastructure/terraform/catalogue.tf` — one Cloud Run service per deployable |
| Kargo promotion, `infrastructure/gitops/kargo` | The `deploy` job in `deploy.yml`; digests recorded in `infrastructure/environments/<env>/images.tfvars` |
| KEDA scaling, `infrastructure/gitops/keda` | Cloud Run's own scale-to-zero, bounded by `min_instances`/`max_instances` in `modules/cloudrun` |
| cert-manager | Nothing: it existed for Kargo's webhook certificates |
| The Helm chart, `infrastructure/helm/qip` | `catalogue.tf` and `modules/cloudrun` |
| The manifests, `infrastructure/kubernetes/base` | The same, plus `modules/execution-node` for the cell that was a StatefulSet |
| The Cloud Run "exception" for the frontends | No longer an exception: everything is Cloud Run, and `scripts/deploy-frontends.sh` still deploys the portal and landing |
| Edge cells synced by hand | `execution_nodes` in the tfvars — empty in every environment until a venue decision exists |
