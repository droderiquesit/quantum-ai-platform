# infrastructure/

Terraform 1.9.8 for GCP: the blueprint runtime of ADR 0022, provisioned in
code under ADR 0024. **Not a cargo directory.**

```
terraform fmt -check -recursive .
terraform validate                       # needs `terraform init -backend=false` first
terraform fmt -check -recursive environments   # the tfvars; run from infrastructure/
make infra                               # all three of the above, from the repo root
terraform test                           # the gates, planned; from each directory below
```

**`terraform test` is the fourth gate and `make infra` does not run it.** Say
so out loud because the gap is the kind that gets missed: `ci.yml`'s
infrastructure job discovers every `*.tftest.hcl` under `infrastructure/` and
runs it, and the Makefile's `infra` target does not. Four directories hold one
today — `terraform/`, `terraform/modules/{network,public-edge,trust-zones}` —
and each needs its own `terraform init -backend=false` first.

What it buys is the thing the other three cannot give. `validate` checks that
the configuration parses and that its references resolve; it evaluates no
`validation` block and no `lifecycle.precondition`, which is where every
safety refusal here lives. Those were asserted only as text until 2026-09-14,
and text cannot tell a rule that works from a rule that cannot run: the prefix
check on `console_egress_cidr` read correctly, had a Rust test asserting it
"both fires and admits", and killed the plan outright by handing a null to
`split` — in the three environments where the value is null, which is three of
the four. See ADR 0069.

Every harness mocks its providers and runs `command = plan`, so none needs a
credential, reaches a project or creates anything — which is the only way to
plan a configuration whose environments have been torn down.

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
| `terraform/modules/public-edge` | Cloud Armor, the global HTTPS load balancer, Cloud CDN (§40.5, §40.14). Creates nothing in any environment — `hostnames` is empty in all four. Its content is refusals: a backend may front only `public-edge` or `application-identity`, and there is no listener on port 80. `README.md` beside it says what it does not hold |
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
  outlived it. `dev` was applied by `infra.yml`'s `up`, dispatched by
  a person — the workflow's own comments record the runs that found each
  missing permission — and `deploy.yml` run 33891084271 moved the three
  catalogue services to the digests `gitops/envs/dev/kustomization.yaml`
  names (they were `environments/dev/images.tfvars` until ADR 0036
  moved the record). Observed from outside the project on 2026-09-04, without a
  credential: `qip-dev-api`, `qip-dev-fastbrain` and `qip-dev-deepbrain`
  answered Google Frontend's internal-ingress 404 (a hostname with no
  service answers a different page, with no `server` header), and
  `qip-dev-openobserve` answered `308 -> /web/` anonymously.
  **Torn down 2026-09-13 on the owner's instruction** (ADR 0040 decision
  13): `infra.yml`'s `teardown` action, in three dispatches, deleted the
  four Cloud Run services, the control-plane cluster and 166
  Terraform-managed resources; 55 free entries remain — API enablement,
  the workflow's own identity, an empty VPC and two subnets Google's
  `serverless-ipv4-*` addresses still hold — and nine KMS keys, five
  `force_destroy = false` buckets and the state bucket stand outside state.
  The register in `docs/DELIVERY-STATUS.md` lists them. Nothing is deployed
  anywhere now. What is still
  true is the rule: an agent shows the plan and a person applies, and
  `docs/DELIVERY-STATUS.md` records what each plan and each observation
  found. It absorbed the missing-infrastructure register on 2026-09-07,
  and this line pointed at the deleted path until 2026-09-08.
- `autonomy_ceiling` may not name a live level. `variables.tf` refuses all
  three at plan time; that validation is load-bearing and mutation-tested, and
  since 2026-09-14 it is also *planned*: `terraform/tests/paper-boundary.tftest.hcl`
  runs a plan per live rung — one each, because
  `contains("autonomous_live")` is true of `"limited_autonomous_live"` and a
  single case can pass with two of the three admitted — and plans
  `paper_trading` and `observation` to the end, asserting the `live_capable`
  output stays false. That is the admitting half, which no evidence for this
  gate had before.
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
  **and admits a good one**. Add the pair to the module's harness; if the
  module has none, `terraform/modules/network/tests/` is the smallest example
  to copy. `qip-acceptance`'s `terraform_plan` suite fails a harness that
  proves only one of the two halves.
- `.terraform/` and `*.tfstate` are denied to reads by `.claude/settings.json`.
  They hold resource topology and secret references.
