# infrastructure/

Terraform 1.9.8 for GCP: the blueprint runtime of ADR 0022, provisioned in
code under ADR 0024. **Not a cargo directory.**

```
terraform fmt -check -recursive .
terraform validate                       # needs `terraform init -backend=false` first
terraform fmt -check -recursive environments   # the tfvars; run from infrastructure/
make infra                               # all three of the above, from the repo root
```

The third is a separate command because the first two do not reach the tfvars:
`fmt` above is scoped to `terraform/`, and `validate` checks the configuration
rather than the values handed to it — it never opens a `.tfvars` at all. So an
environment file that was not valid HCL used to pass every gate here and fail
at the moment somebody applied. `terraform fmt` parses HCL to reformat it, so
pointing it at `environments/` is the whole check; `ci.yml`'s infrastructure
job has run exactly that since `a100d9a`.

`make infra` now runs it too, as the `tf-tfvars` target between `tf-fmt` and
`tf-validate`. This paragraph said it did **not** — true when written, and it
stayed on the page after the gap was closed, which is precisely how an agent
here gets told a closed gap is open. If you change the Makefile's infra
targets, change this sentence in the same diff.

None of the three screens a *value*: a project id carrying `$(…)` is valid
HCL and is canonically formatted. What refuses that is the validation on the
variable it is passed to — see below.

Rules: `.claude/rules/domains/infrastructure.md`.

## Layout

| Path | What |
|---|---|
| `terraform/` | Root module: `main.tf`, `catalogue.tf` (one entry per warm binary — the source of truth for both the identity Terraform creates and the manifest Argo CD applies), `variables.tf`, `outputs.tf` |
| `terraform/modules/cloudrun` | Each workload's identity, its secret grants and the buckets its files are published to. The service itself left for a manifest under ADR 0036; the root's `removed` blocks release it from state without destroying it |
| `terraform/modules/gitops-control-plane` | The GKE Autopilot cluster per environment that runs Config Connector, Argo CD and Kargo and no trading binary; private endpoint, Binary Authorization on, etcd under the ring's key; the three controller identities (ADR 0036) |
| `terraform/modules/execution-node` | One Compute Engine machine per region under systemd — no external address, no container runtime, shadow mode by default |
| `terraform/modules/egress-proxy` | The TLS-terminating proxy, rendered from `egress/envoy.yaml` as a loopback sidecar and as the node's unit |
| `terraform/modules/trust-zones` | The thirteen zones, default deny in both directions; the management zone may reach GitHub and nothing else outside the VPC |
| `environments/<env>/terraform.tfvars` | dev, test, stage, prod — the only per-environment inputs. There is no `images.tfvars` any more: what an environment serves is `gitops/envs/<env>/kustomization.yaml` |
| `gitops/` | ADR 0036's delivery path: vendored controller manifests under `bootstrap/`, one `RunService` per catalogue workload under `envs/<env>/`, the Argo CD project and Applications, the Kargo chain — `gitops/README.md` |
| `egress/` | The one Envoy bootstrap and the vendored-images list the pipeline mirrors and attests — ten images now, eight of them the control plane's |
| `docker/` | Image definitions |

There is one Kubernetes cluster here, and it runs controllers. ADR 0024
retired the runtime that scheduled the platform's binaries as Pods; ADR 0036
brings back Argo CD and Kargo on a control-plane cluster that reconciles
Cloud Run services through Config Connector, and the acceptance suite keeps
refusing a `qip-*` image in any Pod spec. Terraform's provider set is still
`google` and `google-beta` and nothing else.

## Before changing anything

- **Never apply without showing the plan.** The guard hook refuses an
  unreviewed apply and a teardown outright. This file used to say nothing
  here had ever been applied; that stopped being true and the sentence
  outlived it. `dev` has been applied by `infra.yml`'s `up`, dispatched by
  a person — the workflow's own comments record the runs that found each
  missing permission — and `deploy.yml` run 33891084271 moved the three
  catalogue services to the digests `gitops/envs/dev/kustomization.yaml`
  now names (they were `environments/dev/images.tfvars` until ADR 0036
  moved the record). Observed from outside the project on 2026-09-04, without a
  credential: `qip-dev-api`, `qip-dev-fastbrain` and `qip-dev-deepbrain`
  answer Google Frontend's internal-ingress 404 (a hostname with no
  service answers a different page, with no `server` header), and
  `qip-dev-openobserve` answers `308 -> /web/` anonymously. What is still
  true is the rule: an agent shows the plan and a person applies, and
  `docs/ops/missing-infrastructure-register.md` records what each plan
  and each observation found.
- `autonomy_ceiling` may not name a live level. `variables.tf` refuses all
  three at plan time; that validation is load-bearing and mutation-tested.
- **`modules/execution-node/templates/startup.sh.tftpl` runs as root on the
  node and `templatefile` escapes nothing.** Every `${...}` in it is a
  substitution into a systemd `EnvironmentFile`, a unit file, a YAML label
  block or a shell command; a newline in one appends a line nobody reviewed
  and a `$(…)` in one that lands in a double-quoted word is a command. The
  guard is a validation on each of the module's variables in
  `modules/execution-node/variables.tf`, not anything the template can do:
  a quoted heredoc cannot survive its own terminator appearing in its payload.
  `isolated_cpus` and `shadow_mode` are the two bounded structurally instead.
  Adding a substitution without adding its validation reopens the hole.
- No service-account keys. Workload Identity Federation only.
- A validation change needs a real plan proving the gate fires on a bad value
  **and admits a good one**.
- `.terraform/` and `*.tfstate` are denied to reads by `.claude/settings.json`.
  They hold resource topology and secret references.
