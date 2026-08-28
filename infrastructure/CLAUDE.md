# infrastructure/

Terraform 1.9.8 and Kubernetes manifests for GCP. **Not a cargo directory.**

```
terraform fmt -check -recursive .
terraform validate                       # needs `terraform init -backend=false` first
make infra                               # both of the above from the repo root
```

Rules: `.claude/rules/domains/infrastructure.md`.

## Layout

| Path | What |
|---|---|
| `terraform/` | Root module and `modules/` |
| `environments/<env>/terraform.tfvars` | dev, test, stage, prod — the only per-environment inputs |
| `kubernetes/base/` | Manifests: workloads, secrets (CSI), network policies |
| `docker/` | Image definitions |

## Before changing anything

- **Never apply without showing the plan.** The guard hook refuses an
  unreviewed apply and a teardown outright.
- `autonomy_ceiling` may not name a live level. `variables.tf` refuses all
  three at plan time; that validation is load-bearing and mutation-tested.
- No service-account keys. Workload Identity Federation only.
- A validation change needs a real plan proving the gate fires on a bad value
  **and admits a good one**.
- `.terraform/` and `*.tfstate` are denied to reads by `.claude/settings.json`.
  They hold resource topology and secret references.
