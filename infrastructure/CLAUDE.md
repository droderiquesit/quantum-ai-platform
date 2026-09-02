# infrastructure/

Terraform 1.9.8 for GCP: the blueprint runtime of ADR 0022, provisioned in
code under ADR 0024. **Not a cargo directory.**

```
terraform fmt -check -recursive .
terraform validate                       # needs `terraform init -backend=false` first
make infra                               # both of the above from the repo root
```

Rules: `.claude/rules/domains/infrastructure.md`.

## Layout

| Path | What |
|---|---|
| `terraform/` | Root module: `main.tf`, `catalogue.tf` (one Cloud Run entry per warm binary), `variables.tf`, `outputs.tf` |
| `terraform/modules/cloudrun` | What must be true of every Cloud Run service: internal ingress, secrets as volumes, a digest-pinned image |
| `terraform/modules/execution-node` | One Compute Engine machine per region under systemd — no external address, no container runtime, shadow mode by default |
| `terraform/modules/egress-proxy` | The TLS-terminating proxy, rendered from `egress/envoy.yaml` as a loopback sidecar and as the node's unit |
| `terraform/modules/trust-zones` | The thirteen zones, default deny in both directions |
| `environments/<env>/terraform.tfvars` | dev, test, stage, prod — the only per-environment inputs; `images.tfvars` holds the digests the pipeline attested |
| `egress/` | The one Envoy bootstrap and the vendored-images list the pipeline mirrors and attests |
| `docker/` | Image definitions |

There is no Kubernetes here. The chart, the manifests and the GitOps
controllers were retired under ADR 0024, and the acceptance suite refuses
their return.

## Before changing anything

- **Never apply without showing the plan.** The guard hook refuses an
  unreviewed apply and a teardown outright. Nothing in this directory has
  been applied by an agent; the first real plan is a human's to read.
- `autonomy_ceiling` may not name a live level. `variables.tf` refuses all
  three at plan time; that validation is load-bearing and mutation-tested.
- No service-account keys. Workload Identity Federation only.
- A validation change needs a real plan proving the gate fires on a bad value
  **and admits a good one**.
- `.terraform/` and `*.tfstate` are denied to reads by `.claude/settings.json`.
  They hold resource topology and secret references.
