# What the tree deploys today, against what the blueprint requires — superseded by ADR 0024

> **Correction, 2026-09-03 — the runtime this document scored no longer
> exists.** This whole document was written against the working tree at
> `bcad2d3`, where `infrastructure/terraform/modules/cluster/` declared a GKE
> cluster, `modules/cloudrun`, `modules/execution-node` and
> `modules/trust-zones` were unwired (`cloudrun/NOT-WIRED.md`,
> `trust-zones/NOT-ENFORCED-HERE.md` describing an identity model of one
> service account per zone bound to a Kubernetes service account), and
> `infrastructure/{helm,gitops,kubernetes}/` held the chart, the Argo CD stack
> and the raw manifests. Under ADR 0024, `808ca32` wired the blueprint runtime
> into the root module and deleted the cluster's Terraform; `67b3e92` and
> `7d79161` deleted the Helm chart, the raw manifests and the Argo CD stack;
> `c924191` wired `modules/egress-proxy`. None of `infrastructure/helm/`,
> `infrastructure/gitops/` or `infrastructure/kubernetes/` exists in the tree
> today — confirmed by `ls infrastructure/` at the time of this correction,
> which lists only `docker`, `egress`, `environments` and `terraform`. The
> `trust-zones` module's per-zone Kubernetes-service-account binding this
> document cites at `main.tf:251-257` is also gone: the module now takes
> `zone_identities` computed in `catalogue.tf` from the workloads the
> catalogue places in each zone — one Cloud Run/GCE service account per
> workload, not one per zone — and its header comment now reads "Wired from
> `infrastructure/terraform/main.tf` under ADR 0024." Every row below that
> cites a deleted path, calls a module "UNWIRED", or lists a resolution step
> for G10 (the trust-zones identity rework, `§Gap matrix` below) describes a
> tree that predates that wiring. The rows are left as scored, because they
> are the record of what was found then and how a migration engineer reasoned
> about it; for the runtime as it is configured today, read
> [`algorik-blueprint-traceability.md`](algorik-blueprint-traceability.md)'s
> Layer 6/7 entry, which was re-scored after `808ca32`. Nothing in the new
> modules has been applied — `execution_nodes = {}` in every environment and
> no `terraform` binary exists in this session either — so "wired" here means
> "reachable from `main.tf`", not "running".


**Scope.** The runtime this repository would produce if its committed
infrastructure were applied, compared resource by resource with the runtime
the Algorik Master Blueprint v10.1-4 requires (the architecture of record per
`docs/adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md:14-16`),
and the dependency-ordered removal and creation sequence a migration engineer
must follow. This document is a specification for that engineer. It authorises
nothing: every step of ADR 0020's sequence still needs recorded human approval
naming the step (`docs/adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md:78-85`,
`docs/adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md:56-70`).

**The paper-trading boundary is out of scope for every row below.** Nothing
here touches Terraform's plan-time refusal
(`infrastructure/terraform/variables.tf:105-116`), `AutonomyLevel::deployable`
in the three composition roots, or `Cell::new` taking no ceiling but paper
(as recorded at `docs/adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md:93-95`).
A row whose paper-trading impact is anything other than "none" is a blocker,
and §Gap matrix marks it so.

## Method and evidence limits

**What was read, in full, by path.** The Terraform root
(`infrastructure/terraform/main.tf`, `variables.tf`, `outputs.tf`) and every
module's `main.tf` (`services`, `network`, `cluster`, `secrets`,
`observability`, `cicd`, `registry`, `evidence`, `data` header, `ai` header,
`edge-cell`, `binaryauthorization`, `console-ingress`, `connectivity`,
`backup`, `scc` header, `identity`), plus the three unwired modules with their
`variables.tf` and companion notes (`cloudrun/NOT-WIRED.md`,
`execution-node/README.md`, `trust-zones/NOT-ENFORCED-HERE.md`) and the node
startup template. The Helm chart (`Chart.yaml`, `values.yaml`,
`values-dev.yaml`, `values-dev-images.yaml`, every template). The GitOps stack
(`argocd/apps/*`, `argocd/base/kustomization.yaml`, `egress-policies.yaml`,
`console-backend.yaml`, `overlays/dev/*`, the `kargo`, `keda` and
`cert-manager` base kustomizations, `vendored-images.txt`). The raw manifests'
README and the commented foot of `kubernetes/base/egress.yaml`. All four
environment tfvars. `.github/workflows/{ci,deploy,infra,vendor}.yml`. The
blueprint sections §4.2, §6.2, §27, §36, §40, §41, §45, §46, §47, §48 and the
companion diagram's text for the trust-zone and DevOps bands. ADRs 0010, 0011,
0017, 0018, 0020, 0022, 0023; the traceability matrix; the blueprint–diagram
reconciliation; the completion plan; the egress design note (`design-egress-path.md`, a session working note supplied with the brief and **not committed to this tree** — cited by section, and every fact taken from it was re-read at its own `path:line` in the tree before being used here); the four
acceptance suites named in the brief plus `documentation.rs`;
`backend/crates/libs/qip-transport/src/http.rs:340-384`.

**What was not done, and why the reader must not infer otherwise.**

- **No binary was executed.** The session that wrote this document has no
  shell. No `terraform`, `helm`, `kubectl`, `gcloud` or `cargo` command was
  run. `docs/plan/completion-plan.md:304-325` records that no `terraform` or
  `helm` binary exists in this environment and no cluster is reachable; that
  is the state this document was written in too. Every claim below is a
  reading of committed text, cited by `path:line`, and nothing was validated
  against the `hashicorp/google ~> 6.12` provider schema
  (`infrastructure/terraform/main.tf:18-21`). A misspelt attribute in any
  module would pass everything that has been checked.
- **Nothing was applied, and nothing is asserted to be running.** "Deploys
  today" means "what the committed files would produce if applied", not
  "what is observed running".
- **There is no evidence in the tree that any pod ever ran.** ADR 0020 step
  1 asks for "a named cluster, a pod list, and a scrape"
  (`docs/adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md:89`)
  and says gathering it is still the one correct action available
  (`:214-216`). `docs/plan/completion-plan.md:143` records that evidence as
  absent. Three prose claims imply a `dev` cluster existed at some time —
  `infrastructure/helm/qip/values-dev-images.yaml:8-20` says its digests
  "were reconciled to what the cluster actually ran";
  `infrastructure/gitops/argocd/overlays/dev/console-ingress.yaml:23` names a
  reserved address; `docs/operations/gitops-exceptions.md:60` describes a
  workload being killed hourly — and none of them is the artefact step 1
  requires. They are treated here as unverified assertions, not as evidence.
- **Three of the four environments have no project.** `test`, `stage` and
  `prod` carry `project_id = "unprovisioned"`
  (`infrastructure/environments/test/terraform.tfvars:30`,
  `stage/terraform.tfvars:33`, `prod/terraform.tfvars:47`), which
  `variables.tf:24-27` refuses at plan time. Only `dev` (`algorik-dev`,
  `us-east4`; `dev/terraform.tfvars:18,29`) could be applied at all.
- **The diagram text was used only for topology and terminology.** Where the
  DOCX and the diagram disagree (K1–K4 in
  `docs/architecture/blueprint-diagram-reconciliation.md:33-106`), the DOCX
  reading is used and the disagreement is listed in §Decisions.

## What the tree deploys today

One table, grouped by the file that declares the resource. "Blueprint
counterpart" names the element of the target runtime that replaces it, or says
that the blueprint has none.

### Terraform root and modules (`infrastructure/terraform`)

| Resource | Where declared | Purpose | Blueprint counterpart |
|---|---|---|---|
| `google_project_service` for seventeen always-on APIs | `modules/services/main.tf:22-69,99-124` | API enablement before any resource needs it | Same (§45.1). `run.googleapis.com` is not in the list (`modules/cloudrun/NOT-WIRED.md:23-27`); `container.googleapis.com` and `gkebackup.googleapis.com` (`services/main.tf:43,68`) have no counterpart |
| VPC `qip-<env>`, `routing_mode = REGIONAL` | `modules/network/main.tf:42-53` | The one network | One global VPC, no peering (§45.1 `VPC (global)`). The module's own note says this half already agrees (`network/main.tf:9-19`) |
| Primary subnet with `pods` and `services` secondary ranges | `modules/network/main.tf:55-84` | GKE node, pod and service addressing | Regional subnets with no secondary ranges; the module names the secondary ranges as transitional (`network/main.tf:33-40`) |
| Cloud Router + NAT, `ALL_SUBNETWORKS_ALL_IP_RANGES` | `modules/network/main.tf:88-108` | Egress for nodes with no external address | "Cloud NAT with allowlist" (§46.2 Network). The all-subnets form is named as transitional (`network/main.tf:36-40`) |
| Firewall `deny-ingress`, `allow-health-checks`, `allow-internal` targeting tag `qip-node` | `modules/network/main.tf:113-172` | Cluster node posture | Per-zone deny-both-ways plus declared paths (§46.1) — the zone module writes these per tag (`modules/trust-zones/main.tf:284-395`) |
| `console_egress` subnet, reserved `api_internal` address, `allow-console-to-api` | `modules/network/main.tf:186-255` | The portal's route from Cloud Run into the VPC (ADR 0018) | Application APIs on Cloud Run behind Private Service Connect (§40.14). The subnet is the pattern the catalogue needs (`modules/cloudrun/NOT-WIRED.md:29-36`) |
| `google_container_cluster.primary` and `google_container_node_pool.primary` | `modules/cluster/main.tf:7-378,380-514` | The GKE cluster: private nodes and endpoint, Calico policy, workload identity pool, CMEK etcd, Binary Authorization `PROJECT_SINGLETON_POLICY_ENFORCE`, Secret Manager CSI add-on, Managed Prometheus, GKE Backup agent, NAP, VPA | **None — the blueprint has no Kubernetes in any phase** (§45.1 lists no GKE; the diagram says "0 Kubernetes clusters, in any phase"). Each cluster feature maps to a Cloud Run or GCE feature in §What the blueprint requires |
| KMS key ring `qip-<env>`, keys `node-encryption`, `secrets` | `modules/secrets/main.tf:28-89` | Customer-managed encryption for etcd and secrets | Cloud KMS (§45.1). `node-encryption` encrypts etcd and has no counterpart; it carries `prevent_destroy` (`secrets/main.tf:48-50`) |
| GKE service identity + KMS grant to the container robot | `modules/secrets/main.tf:60-72` | Lets GKE encrypt etcd | None |
| Node service account `qip-nodes-<env>` with telemetry roles | `modules/secrets/main.tf:98-115` | The kubelet's identity | None; Cloud Run instances and the GCE node run as their workload's own account |
| Workload service accounts `qip-api-<env>`, `qip-fastbrain-<env>`, `qip-deepbrain-<env>` | `main.tf:127-131`; `modules/secrets/main.tf:119-126` | One identity per deployable | One identity per Cloud Run workload (§46.2 Identity), which the `cloudrun` module creates itself (`modules/cloudrun/main.tf:107-116`) |
| Nine `google_secret_manager_secret`s, CMEK, rotation topic | `main.tf:290-311`; `modules/secrets/main.tf:141-240` | Credentials created empty, values out of band | Secret Manager (§45.1) — unchanged |
| Secret accessor grants: API tokens to `api`, envelope key to all three, venue credential gated on `ceiling_reaches_a_venue` | `modules/secrets/main.tf:243-293`; `main.tf:92,340` | Least-privilege secret reads; the venue credential unreadable in every applicable environment | Same shape on Cloud Run: per-mount grants (`modules/cloudrun/main.tf:243-250`); on the node: `venue_credential_bound` requires the same predicate **and** `shadow_mode = false` (`modules/execution-node/main.tf:89-93`) |
| Workload Identity bindings `serviceAccount:<project>.svc.id.goog[qip/<ksa>]` | `main.tf:354-362` | Pods authenticate as their Google account | **None** — a Cloud Run revision names its `service_account` directly (`modules/cloudrun/main.tf:280`); a GCE instance authenticates through the metadata server (`modules/execution-node/main.tf:180-184,338-346`) |
| Console service account and viewer-token grant | `modules/secrets/main.tf:331-388` | The portal reads the platform as `viewer` | portal-api / application API identity (§40.9) |
| Four alert policies gated on `workload_metrics_exist` | `modules/observability/main.tf:18-157` | Kill switch, live fill, persistent breach, permission violation | Observability (§47). Unchanged, but the scrape that would feed them is GKE-only (below) |
| WIF pool `qip-github-<env>`, provider, CI account `qip-ci-<env>`, infra account `qip-infra-<env>` | `modules/cicd/main.tf:17-222` | Keyless pipeline identity | Workload Identity Federation (§45.1). The CI account holds `roles/container.developer` (`cicd/main.tf:96-100`) and the infra account `roles/container.admin` and `roles/gkebackup.admin` (`:156,185`) — three GKE-only grants |
| Artifact Registry `qip-<env>`, immutable tags, CI push, node + workload pull | `modules/registry/main.tf:19-69`; `main.tf:412-415` | The image store | Artifact Registry (§45.1). The node-account pull grant has no counterpart |
| Evidence bucket with locked retention and CMEK | `modules/evidence/main.tf:58-127` | The write-once record | Cloud Storage "audit with retention lock" (§45.1) — unchanged |
| Managed data services, all `false` | `main.tf:448-483`; `modules/data/main.tf:9-16` | Six stores that refuse construction until an adapter exists | Spanner, BigQuery, Memorystore (§45.1) — provisionable later, unchanged in shape |
| Vertex AI metadata, `false` | `main.tf:485-511`; `modules/ai/main.tf:3-14` | Somewhere to train that this build cannot submit to | The blueprint trains on spot GPU with "no Vertex AI" (diagram, training pipeline band) — a counterpart that is a different service |
| Edge cell: subnet with pod/service ranges, account `qip-edge-<cell>-<env>`, WI binding to `qip/qip-edge-<cell>`, evidence and registry grants, deny-egress + per-venue + central-plane firewall on tag `qip-edge-<cell>` | `main.tf:513-541`; `modules/edge-cell/main.tf:32-193` | The regional cell's network and identity; **no compute** | One execution node per region on GCE C3 (§41.4, §41.6) — `modules/execution-node` provisions the equivalent subnet, identity, grants and firewall plus the machine (`execution-node/main.tf:98-236,454-618`) |
| Binary Authorization: KMS signing key, Container Analysis note, attestor, CI grants, policy with default rule and a `cluster_admission_rules` block keyed on `<region>.<cluster>` | `main.tf:557-582`; `modules/binaryauthorization/main.tf:43-240` | Only pipeline-signed images admitted | Binary Authorization (§45.1, §48 Admission). The default rule is what Cloud Run evaluates through `use_default` (`modules/cloudrun/main.tf:275-277`); the cluster rule has no counterpart; a bare GCE instance has no admission point at all (`modules/execution-node/main.tf:29-45`) |
| Console ingress: two global external addresses, IAP grant | `main.tf:590-600`; `modules/console-ingress/main.tf:29-95` | A public URL for Argo CD and Kargo behind IAP | **None** — the blueprint has neither console. Note IAP is switched off at the backend (`infrastructure/gitops/argocd/base/console-backend.yaml:35-36`), so today the URL is password-gated only |
| Partner interconnect and PSC endpoint, both `false` | `main.tf:617-636`; `modules/connectivity/main.tf:59-169` | Colocation path and private Google API access | Private Service Connect (§41.4 Network, §45.1) — the endpoint exists and no environment enables it (`variables.tf:498-501`; no tfvars sets it) |
| Backup: KMS key, GKE Backup service identity and grant, `google_gke_backup_backup_plan` on `cluster_id`, Compute snapshot resource policy | `main.tf:661-695`; `modules/backup/main.tf:79-266` | Journal durability for cell volumes | The snapshot schedule has a counterpart (a disk on the node); the GKE backup plan has none |
| Security Command Center custom detectors, `false` | `main.tf:713-722`; `modules/scc/main.tf:1-25` | Project-scoped SCC | SCC (§45.1) — unchanged |
| Identity Platform config, gated | `main.tf:724-734`; `modules/identity/main.tf:23-91` | Customer sign-in | §40.3 identity (passkeys absent — `docs/architecture/algorik-blueprint-traceability.md:238`) — unchanged |

### The Helm chart (`infrastructure/helm/qip`), reconciled by Argo CD

| Resource | Where declared | Purpose | Blueprint counterpart |
|---|---|---|---|
| Namespace `qip` (restricted PSS), `default-deny`, DNS, per-workload ingress/egress policies | `templates/namespace.yaml:6-345` | Pod-level default deny | Zone firewall rules and Cloud Run ingress/invoker IAM (§46.1) |
| ConfigMap `qip-config`: `autonomy_ceiling: paper_trading`, `storage_target: memory`, `mesh_cells`, `mesh_peer_london-1`, optional connector keys | `templates/config.yaml:6-130` | The one named place the ceiling and the mesh wire live | Cloud Run `env` map (`modules/cloudrun/main.tf:74-80,329-336`) and the node's `EnvironmentFile` (`modules/execution-node/templates/startup.sh.tftpl:124-132`) |
| `SecretProviderClass` `qip-api-secrets` (six secrets) and `qip-envelope-key` (one) | `templates/secrets.yaml:40-82` | Secret Manager projected as files by the GKE CSI driver | Cloud Run secret volumes, one directory per secret (`modules/cloudrun/main.tf:62-69,378-398`); on the node, `qip-fetch-secret` at boot (`modules/execution-node/README.md:100`) |
| `qip-api` Deployment (no replica count), Service, `qip-api-mesh` Service with `sessionAffinity: ClientIP` and port 9110, PDB, KEDA `ScaledObject` 2–6, optional internal LB Service + console ingress policy | `templates/api.yaml:12-198,200-270,272-295,356-407,523-615` | The API and operator interface; the mesh listener per cell; the console's route | A Cloud Run service. The mesh's per-process state and client-IP pinning (`api.yaml:224-250`) have no Cloud Run counterpart — see §Decisions D16 |
| `qip-fastbrain` Deployment, `replicas: 1`, `Recreate`, requests = limits, no secret but the envelope key | `templates/fastbrain.yaml:31-268` | The central cycle host; "the only workload permitted to reach a venue" (`fastbrain.yaml:3`) | No exact counterpart — the blueprint's hot path lives only in the node. As a warm service it is a Cloud Run service with a floor of one (§Creation order step 4) |
| `qip-deepbrain` Deployment, `replicas: 1`, `Recreate`, 8Gi scratch | `templates/deepbrain.yaml:26-245` | World model, research, optimisation, learning in one binary | Several planes' services at once (Cognition, Intelligence, Optimisation; `docs/architecture/algorik-blueprint-traceability.md:110-151`). One Cloud Run service today; a split is future work |
| `qip-edge-<cell>` StatefulSet ×2, journal PVC 16Gi on class `qip-journal`, Service, PDB, per-cell ingress/egress policies — **rendered only when `cell.enabled`**, which no values file sets | `templates/edge-cell.yaml:7-557`; `values.yaml:48-84`; `values-dev.yaml` (no `cell` block) | The cell's hot path as pods | The execution node (§41.4). ADR 0020 calls this a Deployment (`0020:44`); it is a StatefulSet (`edge-cell.yaml:43`) |
| Egress: ConfigMaps `qip-egress-envoy` and `qip-egress-endpoints`, Service `qip-egress` (9101–9104), four NetworkPolicies — live; ServiceAccount, Deployment, PDB — **commented out** | `templates/egress.yaml:131-746` live; `:819-976` commented (`# kind: ServiceAccount` at `:820`, `# kind: Deployment` at `:835`) | The TLS-terminating reverse proxy the plaintext HTTP client needs; described, never deployed | The blueprint assumes clients speak TLS (§46.2 Network) and has no proxy. The proxy exists because of ADR 0002; its off-GKE form is the first thing to create (§Creation order step 1) |
| `PodMonitoring` for `qip-fastbrain` and `qip-deepbrain` | `templates/monitoring.yaml:31-69` | Managed Prometheus scrape of `/metrics` | Managed Prometheus (§45.1) — but `PodMonitoring` is a GKE resource; Cloud Run and GCE need their own collectors (§Decisions D22) |
| StorageClass `qip-journal`, `Retain`, `WaitForFirstConsumer`, labelled disks; VolumeSnapshotClass commented out | `templates/journal-storage.yaml:38-110,161-168` | Journal disks that outlive their claim | A persistent disk attached to the node — which the module does not yet declare (`modules/execution-node/main.tf:308-314` declares only the boot disk) |
| `ComputeClass qip-burst`, three `VerticalPodAutoscaler`s in `Off` mode | `templates/autoscaling.yaml:28-85` | Node auto-provisioning shape and request recommendation | None; Cloud Run sizing is `cpu`/`memory` per revision (`modules/cloudrun/main.tf:315-327`) |
| `values-dev-images.yaml` — three digests written by the pipeline | `values-dev-images.yaml:21-24`; `.github/workflows/deploy.yml:412-439` | The promoted digest per binary | The `--image <repo>@<digest>` argument of a Cloud Run deploy, and `image_digest` in the module (`modules/cloudrun/variables.tf:203-222`) |

### GitOps (`infrastructure/gitops`) and the raw manifests

| Resource | Where declared | Purpose | Blueprint counterpart |
|---|---|---|---|
| Argo CD v3.5.2, full install less dex, egress policies, console backend | `argocd/base/kustomization.yaml:14-42`; `egress-policies.yaml`; `console-backend.yaml` | The sole writer to the cluster (ADR 0017) | **None** (ADR 0022 `:38-44`). Cloud Deploy in the blueprint (§48) — see §Decisions D11 |
| `AppProject qip`, `Application qip-dev` (auto-sync, prune, self-heal, pinned to one named branch at `dev.yaml:17`), edge template | `argocd/apps/project.yaml`; `dev.yaml:13-32`; `edge.yaml` | What may be reconciled, from where, to where | None |
| dev overlay: vendored image substitution, ManagedCertificate and Ingress on `argocd.136-68-65-168.nip.io` | `argocd/overlays/dev/kustomization.yaml:13-19`; `console-ingress.yaml:16-66` | The console's public URL | None |
| Kargo v1.11.2 (rendered chart) + console | `kargo/base/kustomization.yaml:12-25` | Promotion between environments | None; Cloud Deploy's gradual rollout (§48) is the nearest |
| cert-manager v1.21.1 — "exactly one consumer today: Kargo's webhook certificates" | `cert-manager/base/kustomization.yaml:1-24` | Kargo prerequisite | None |
| KEDA v2.20.2 | `keda/base/kustomization.yaml:6-26` | Event-driven scaling; today only `qip-api`'s CPU trigger | None; Cloud Run scales per revision (`modules/cloudrun/main.tf:298-301`) |
| `vendored-images.txt` (Argo CD, cert-manager ×3, Kargo, KEDA ×3, Redis) and `vendor.yml` | `gitops/vendored-images.txt:25-61`; `.github/workflows/vendor.yml` | Mirror and attest third-party images so the admission policy keeps one rule | The mechanism survives; every current entry is a Kubernetes controller and goes with it. The Envoy image the egress proxy needs would be the first non-Kubernetes entry (`templates/egress.yaml:778-782`) |
| `infrastructure/kubernetes/base/**` — sed-rendered originals, deprecated | `kubernetes/base/README.md:1-36` | "No longer applied by any pipeline step"; the fixture 22 acceptance checks read (`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:3147-3152`) | None. **Last artefact removed** (§Removal order) |

### Workflows and out-of-Terraform deployments

| Resource | Where declared | Purpose | Blueprint counterpart |
|---|---|---|---|
| `ci.yml`: fmt, clippy `-D warnings`, tests `--no-fail-fast`, release build, dependency policy, audit, SBOM, frontends, Trivy, trunk, `terraform fmt`/`validate`/provider check, secrets | `.github/workflows/ci.yml:27-279` | The gate | §48 "Build and test", "Static analysis" — same gates on GitHub Actions rather than Cloud Build |
| `deploy.yml`: gate on ci; matrix of four images (`qip-api`, `qip-fastbrain`, `qip-deepbrain`, `qip-edge-node`) built from one Dockerfile, scanned, pushed by commit tag, signed by digest; `gitops-update` writes `values-<env>-images.yaml` and pushes | `.github/workflows/deploy.yml:84-123,147-335,355-456` | Build, attest, record digests for Argo CD | §48 "Attestation" survives unchanged; "Deploy — services" and "Deploy — node" are the retarget (§Creation order step 7) |
| `infra.yml`: manual plan/up/down, never prod; `down` destroys `module.cluster` and its dependents | `.github/workflows/infra.yml:37-57,271-292` | Iterate the stack; stop the meter | §48 "Infrastructure: OpenTofu, plan reviewed before apply" — Terraform 1.9.8 here (`infra.yml:167-169`) |
| Portal and landing on Cloud Run via `gcloud`/Cloud Build, outside Terraform | `scripts/deploy-frontends.sh:1-70`; `infrastructure/docker/portal.Dockerfile:1`; `docs/operations/gitops-exceptions.md:6-17` | The only Cloud Run workloads today | Experience layer on Cloud Run (§40.5) — transitional Next.js rather than Leptos (ADR 0022 `:46-49`) |
| Environments `dev` (provisioned, `us-east4`), `test`, `stage`, `prod` (unprovisioned) with cells `london-1`+`tokyo-1`, `london-1`, three, `london-1` | `environments/*/terraform.tfvars`; `variables.tf:52` | Four environments | §48 names three: `dev` (Cloud Run only), `sim` (one node + Spanner), `prod` — see §Decisions D17 |

## What the blueprint requires

Status vocabulary: **WIRED** — a root-module block or an applied manifest
produces it; **UNWIRED MODULE** — Terraform exists under `modules/` and
`main.tf` does not call it; **ABSENT** — nothing in the tree produces it.
"Wired" says nothing about whether it has ever been applied (see §Method).

| Blueprint element | Section | Present in tree? | Evidence |
|---|---|---|---|
| Cloud Run services for every non-node warm workload, scale to zero, direct VPC egress, secrets as files, image by digest | §41.6, §45.1 | UNWIRED MODULE | `modules/cloudrun/main.tf:254-408`; `modules/cloudrun/NOT-WIRED.md:3-5`. The portal and landing run on Cloud Run outside Terraform (`scripts/deploy-frontends.sh:2`) |
| Cloud Run Jobs for batch work | §41.6, §45.1 | UNWIRED MODULE | `modules/cloudrun/main.tf:423-515` (`kind = "job"`). No binary in the workspace is a batch job (`docs/adr/0010-what-gets-deployed.md:12-19`), so there is nothing to instantiate yet |
| `run.googleapis.com` enabled | §45.1 | ABSENT | Not in `modules/services/main.tf:22-69`; `modules/cloudrun/NOT-WIRED.md:23-27` |
| A subnet for Cloud Run direct VPC egress that GKE does not allocate from | §45.1 | WIRED for the portal only | `modules/network/main.tf:186-204` (`console_egress`, `/26`); the catalogue's own range is ABSENT (`modules/cloudrun/NOT-WIRED.md:29-36`) |
| One execution node per region on C3/C3D high-CPU, systemd, `Restart=always`, blue-green MIG | §41.4, §41.6, §48 | UNWIRED MODULE | `modules/execution-node/main.tf:281-447`; `templates/startup.sh.tftpl:24,196`; `modules/execution-node/README.md:9-14`; no `module "execution_node"` in `main.tf` |
| No container runtime, `isolcpus` 2–15, huge pages, `mlockall`, no swap | §41.4 | UNWIRED MODULE (verified at boot, enforced by the image) | `startup.sh.tftpl:39,48-49`; `README.md:50-67` — governor and C-states are enforced by nobody (`README.md:66`) |
| systemd watchdog | §41.4 | ABSENT | Off by default because the binary sends no `sd_notify` (`modules/execution-node/variables.tf:372-397`) |
| No external IP on the node | §41.4, §46.2 | UNWIRED MODULE | No `access_config` block (`modules/execution-node/main.tf:316-328`); the organisation policy that would forbid one is ABSENT (`modules/trust-zones/NOT-ENFORCED-HERE.md:122-129`) |
| gVNIC, `TIER_1`, compact placement, shielded VM, `TERMINATE` on host maintenance | §41.4, §45.1 | UNWIRED MODULE | `modules/execution-node/main.tf:249-257,322,334-336,351-366` |
| Shadow mode before taking sessions | §41.4, §48 | UNWIRED MODULE | `shadow_mode` default `true` creates no venue rule and no credential binding (`modules/execution-node/main.tf:89-93,548-569`; `variables.tf:146-165`) |
| Machine shape by venue count, 8 to 22 vCPU | §41.4 | UNWIRED MODULE | `modules/execution-node/variables.tf:69-99` — only four shapes admitted; on an 8-vCPU shape §41.3's core map does not fit (`:81-85`) |
| Cloud NAT with allowlist for the node and the zones | §41.4, §46.2 | WIRED, wrong shape; allowlisted forms UNWIRED | Root NAT is `ALL_SUBNETWORKS_ALL_IP_RANGES` (`modules/network/main.tf:102`); `LIST_OF_SUBNETWORKS` forms in `modules/execution-node/main.tf:134-159` and `modules/trust-zones/main.tf:485-528` |
| Private Service Connect for Google APIs | §41.4, §45.1, §46.2 | WIRED, disabled everywhere | `modules/connectivity/main.tf:133-169`; `variables.tf:498-501` default `false`; no tfvars enables it (`NOT-ENFORCED-HERE.md:114-117`) |
| Thirteen trust zones, default deny, sanctioned paths only, IBM egress from Optimisation only, public ingress on two zones only | §46.1, §45.2 | UNWIRED MODULE | `modules/trust-zones/main.tf:37-51,80-97,117-123,129,284-476`; `NOT-ENFORCED-HERE.md:11-21`. Its identity binding is a GKE workload-identity member (`main.tf:251-257`) — written for the transitional runtime |
| Secret Manager, values never in state | §45.1, §46.2 | WIRED | `modules/secrets/main.tf:198-240` |
| Secrets delivered as files, never environment values | §46.2 (implicit), `.claude/rules/01-security-and-safety.md` | WIRED on GKE; UNWIRED on Cloud Run; UNWIRED on the node | `templates/secrets.yaml:15-28`; `modules/cloudrun/main.tf:376-398`; `startup.sh.tftpl:130` |
| Cloud KMS for signing | §45.1, §46.2 | WIRED | `modules/secrets/main.tf:28-89`; `modules/binaryauthorization/main.tf:43-70` |
| Cloud HSM for custody keys; post-quantum corridor signatures | §45.1, §46.2 | ABSENT | No `google_kms_*` with `HSM` protection level in any module read; in-tree HMAC is the standing matter F3 (`docs/architecture/algorik-blueprint-traceability.md:352-372`) |
| Workload Identity Federation, keyless | §45.1, §46.2 | WIRED for the pipeline; runtime identity is GKE-shaped | `modules/cicd/main.tf:24-85`; runtime bindings at `main.tf:354-362` are the GKE form |
| Per-service identities, least privilege | §46.2 | WIRED (three) / UNWIRED (per workload) | `modules/secrets/main.tf:119-126`; `modules/cloudrun/main.tf:93-116` |
| Binary Authorization on Cloud Run | §45.1, §48 | Policy WIRED, cluster-pinned; Cloud Run evaluation UNWIRED | `modules/binaryauthorization/main.tf:188-240`; `modules/cloudrun/main.tf:275-277,431-433` |
| Admission control for the node | §48 | ABSENT by nature | `modules/execution-node/main.tf:29-45`; `README.md:80-104` — the image build must attest what it packaged |
| Artifact Registry | §45.1 | WIRED | `modules/registry/main.tf:19-40` |
| Cloud Build, Cloud Deploy, OpenTofu | §45.1, §48 | ABSENT | GitHub Actions and Terraform 1.9.8 (`.github/workflows/ci.yml:252-254`; `deploy.yml`); frontends are built by Cloud Build through `gcloud` (`scripts/deploy-frontends.sh:6-8`) |
| Cloud Armor, Global HTTPS LB, CDN in front of web and mobile only | §40.5, §45.1 | ABSENT | The only external load balancer is the Argo CD console's (`argocd/overlays/dev/console-ingress.yaml:25-55`), which the blueprint does not have. The portal is reached at its `run.app` hostname (`environments/dev/terraform.tfvars:136-137`) |
| Pub/Sub control fabric shipping the twelve-item payload | §41.5, §45.1 | ABSENT | The only topic is secret rotation (`modules/secrets/main.tf:155-166`); the fabric is the in-tree mesh (`docs/adr/0011-everything-in-rust-on-kubernetes.md:23`) |
| Spanner ledger | §45.1, §46.1 | ABSENT (flag exists) | `variables.tf:442-446` default `false`; every tfvars `enable_spanner = false` |
| Managed Prometheus / Cloud Trace / OpenTelemetry | §45.1, §47 | WIRED on GKE (scrape); OTel spans ABSENT | `modules/cluster/main.tf:299-301`; `templates/monitoring.yaml:31-69`; `docs/architecture/algorik-blueprint-traceability.md:243` |
| Alert policies | §47 | WIRED, gated off | `modules/observability/main.tf:19,55,93,127`; `workload_metrics_exist` commented out in `environments/dev/terraform.tfvars:126` |
| Security Command Center | §45.1 | WIRED, off | `main.tf:713-722` |
| Environments `dev` / `sim` / `prod` | §48 | ABSENT as named | `variables.tf:52` admits `dev`, `test`, `stage`, `prod` |
| A TLS-terminating egress path for a client that refuses `https` | (a consequence of ADR 0002, not a blueprint element) | GKE: described, not deployed; Cloud Run and node: ABSENT | `backend/crates/libs/qip-transport/src/http.rs:366-367`; `templates/egress.yaml:820,835`; `modules/execution-node/README.md:106-133`; `backend/crates/tests/qip-acceptance/tests/egress.rs:1155-1176` asserts the commented state |

## Gap matrix

Every difference between the two tables above, classified. **Paper impact**
names which of the three layers a change could touch; it must read "none" or
the row is a blocker and is marked so. The three layers are Terraform's
refusal (`infrastructure/terraform/variables.tf:105-116`), the composition
roots, and `Cell::new` (`docs/adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md:125-129`).

| # | Difference | Class | Paper impact | Note |
|---|---|---|---|---|
| G1 | No off-Kubernetes TLS-terminating egress path exists; the GKE one has never run | CREATE | none | Precondition of every warm-service move (§Removal order constraint b). The proxy holds no identity and no credential (`templates/egress.yaml:827-832,980-986`) |
| G2 | `run.googleapis.com` not enabled | CREATE | none | `modules/services/main.tf:22` `always` map |
| G3 | No Cloud Run egress subnet for the catalogue | CREATE | none | Address plan must avoid every range in every environment's tfvars (`environments/dev/terraform.tfvars:54-69,165`) |
| G4 | `cloudrun` module unwired; no warm service on Cloud Run | CREATE | none | One `module "cloudrun"` block per warm binary, one at a time (`NOT-WIRED.md:38-41`) |
| G5 | Three workload SAs created by `modules/secrets` and again by `modules/cloudrun` under the **same account id** | REWIRE | none | `qip-${name}-${env}` (`cloudrun/main.tf:113`) with `name = "api"` equals `${value}-${env}` with `value = "qip-api"` (`secrets/main.tf:123`; `main.tf:128`). Two resources, one id: the second apply fails. Delete the secrets-module accounts when their workload moves, and repoint `evidence`, `data` and `ai` (`main.tf:438-439,476-482,510`) at the `cloudrun` outputs |
| G6 | GKE workload-identity bindings | REMOVE | none | `main.tf:354-362`; replaced by the revision's own `service_account` and the instance's metadata identity (constraint d) |
| G7 | `execution-node` module unwired; cells run as pods (when enabled) | CREATE | none | `shadow_mode = true`; the module accepts no ceiling (`execution-node/main.tf:47-53`); the venue credential predicate must be copied verbatim (`README.md:174,180-190`). **Writing it as `!= "paper_trading"` would make this row a blocker** — it grants the credential to `observation` and `advisory` |
| G8 | The node has no persistent journal disk; only a 100GB boot disk with `auto_delete` | CREATE | none | `execution-node/main.tf:308-314`; the snapshot schedule (`backup/main.tf:224-266`) then has a disk to attach to |
| G9 | No node boot image build (binary, `isolcpus`, huge pages, no runtime, `qip-fetch-secret`) | CREATE | none | `boot_image` is required and refuses a family (`execution-node/variables.tf:101-124`); the image contract is `README.md:95-101` |
| G10 | `trust-zones` unwired; its identity model is one SA per zone bound to a KSA | REWIRE then CREATE | none | `trust-zones/main.tf:234-257` vs one SA per workload (`cloudrun/main.tf:93-106`); see D13 |
| G11 | Root NAT is all-subnets | REWIRE | none | `network/main.tf:102`; narrow to `LIST_OF_SUBNETWORKS` once no GKE subnet needs it |
| G12 | Primary subnet carries pod/service secondary ranges | REWIRE | none | `network/main.tf:67-75`; removing them replaces the subnet — after the cluster is gone |
| G13 | Firewall rules target tag `qip-node` | REMOVE | none | `network/main.tf:133-172,236-255`; zone rules and Cloud Run invoker IAM replace them |
| G14 | Binary Authorization policy pins a cluster rule on `cluster_id` | REWIRE | none | `main.tf:575`; `binaryauthorization/main.tf:234-239` (constraint c) |
| G15 | `global_policy_evaluation_mode = "ENABLE"` justified by GKE system images | REWIRE | none | `binaryauthorization/main.tf:191-198`; the reason no longer applies — re-argue or disable, do not keep silently |
| G16 | Backup plan on `cluster_id`, GKE Backup service identity and grant | REMOVE | none | `backup/main.tf:115-215`; `main.tf:674` (constraint c). The KMS key stays (`prevent_destroy`, `backup/main.tf:98-100`) |
| G17 | Compute snapshot resource policy | KEEP | none | `backup/main.tf:224-266` — attach to the node's journal disk (G8) |
| G18 | CI/infra accounts hold `container.developer`, `container.admin`, `gkebackup.admin` | REWIRE | none | `cicd/main.tf:96-100,156,185`; add `roles/run.developer` (or `run.admin`) and `iam.serviceAccountUser` on each workload SA for the CI account, `roles/run.admin` for infra; drop the three |
| G19 | Node SA `qip-nodes-<env>` and its registry pull grant | REMOVE | none | `secrets/main.tf:98-115`; `main.tf:412-415` |
| G20 | KMS `node-encryption` key and GKE robot grant | REMOVE binding, KEEP key | none | `secrets/main.tf:34-72`; the key cannot be destroyed and costs nothing (`infra.yml:22-25`) |
| G21 | `deploy.yml` records digests for Argo CD | REWIRE | none | `deploy.yml:355-456` → Cloud Run deploy by digest + MIG rolling update (§Creation order step 7); the build, scan and sign steps (`:235-335`) are unchanged |
| G22 | Argo CD, AppProject, Applications, console ingress, IAP grant | REMOVE | none | `argocd/**`; `console-ingress/main.tf:29-95`; `main.tf:590-600`; `variables.tf:747-757`; `services/main.tf:93` |
| G23 | Kargo and cert-manager | REMOVE | none | `kargo/**`; `cert-manager/**` (`cert-manager/base/kustomization.yaml:2-4` — Kargo is its only consumer) |
| G24 | KEDA and the `ScaledObject`; VPAs; `ComputeClass` | REMOVE | none | `keda/**`; `templates/api.yaml:356-407`; `templates/autoscaling.yaml` |
| G25 | `PodMonitoring` scrape | REMOVE, then CREATE the Cloud Run and node collectors | none | `templates/monitoring.yaml`; see D22 |
| G26 | Helm chart, values files, `Chart.yaml` | REMOVE | none | After every template's workload has moved and been observed (ADR 0020 step 5 evidence) |
| G27 | `vendored-images.txt` entries (all Kubernetes controllers) | REWIRE | none | Keep `vendor.yml`; the first surviving entry is the Envoy image if D1 lands on a proxy (`templates/egress.yaml:778-782`) |
| G28 | GKE cluster and node pool, cluster variables, outputs, APIs, taint-recovery and `down` steps | REMOVE | none | `cluster/main.tf`; `main.tf:212-272`; `variables.tf:137-298,694-715`; `outputs.tf:7-17,169-192`; `services/main.tf:43,68`; `infra.yml:201-292` |
| G29 | Edge-cell module, `edge_cells` variable and output | REMOVE | none | `main.tf:513-541`; `variables.tf:323-376`; `outputs.tf:83-100`; `modules/edge-cell/**`. Its WI binding names a KSA (`edge-cell/main.tf:81-85`) that will not exist |
| G30 | `infrastructure/kubernetes/base/**` | REMOVE, LAST | none | Fixture for 22 checks (`infrastructure.rs:3147-3152`); 24 path references across three suites (`infrastructure.rs`, `manifest_wiring.rs:39-48`, `egress.rs:46`) |
| G31 | Console route: internal passthrough LB Service + `allow-api-ingress-from-console` | REWIRE | none | `templates/api.yaml:523-615`; `network/main.tf:206-255`; becomes `invokers = [console SA]` on the API's Cloud Run service (`cloudrun/main.tf:411-419`) with `ingress_posture = "internal"` |
| G32 | Mesh listener per cell with client-IP affinity | REWIRE | none | `templates/api.yaml:213-270`; per-process state (`:233-242`) forces one API instance until the state moves out (D16). The node's `QIP_MESH_PEER` becomes the API's internal address |
| G33 | Environments `test`/`stage` vs blueprint `sim` | KEEP (transitional) | none | `variables.tf:52`; D17 |
| G34 | Pub/Sub control fabric, Spanner ledger, Cloud HSM, OTel spans, Cloud Armor/LB/CDN, Cloud Build/Deploy/OpenTofu, passkeys | KEEP as ABSENT | none | Each is later-phase or a separate decision (D11); none is a precondition for retiring GKE |
| G35 | Frontends on Cloud Run via script | KEEP | none | `scripts/deploy-frontends.sh`; transitional per ADR 0022 `:46-49` |
| G36 | Evidence bucket, secrets, KMS ring, registry, WIF pool, alert policies, SCC, Identity Platform, connectivity | KEEP | none | Unchanged by the migration |
| G37 | Turning `shadow_mode` off, or granting the venue credential anywhere | **not in scope** | would touch layer 1's downstream predicate | Listed so nobody reads a migration row as cover for it. That is ADR 0023 territory (`0023:86-97`) and requires its own approvals |

No row above is a blocker as specified. G7 and G37 name the two edits that
would make one.

## Removal order

Dependency-ordered. A step may not begin before the step it names as its
precondition has its evidence, and — separately — its ADR 0020 approval
(`0020:78-85`). The four hard constraints from the brief are marked (a)–(d).

**Preconditions to any removal.**

- (b) **An off-GKE TLS-terminating egress path exists and has carried a
  request** before any warm service moves. The HTTP client refuses `https`
  by name (`backend/crates/libs/qip-transport/src/http.rs:366-367`); the only
  proxy in the tree is Kubernetes-only and commented out
  (`templates/egress.yaml:819-976`); ADR 0020's own correction says step 5 as
  written would remove the egress path of every migrated service
  (`0020:165-178`). Evidence is the design note's list: a real workload's
  request in the Envoy access log with `%UPSTREAM_CLUSTER%` naming the
  expected cluster, the adapter succeeding, and a refused request to a host
  not on the allowlist (`design-egress-path.md` §5.4 — the scratchpad note;
  the same three are what `.claude/rules/domains/infrastructure.md` calls
  "fires on a bad value and admits a good one").
- The creation steps 1–4 of §Creation order have run for the service being
  moved. Nothing is removed that has not been replaced and observed
  replacing it (ADR 0022 `:64-70`).

**Step R1 — retarget the acceptance suites before any manifest is deleted.**
(a) `infrastructure/kubernetes/base/**` is the fixture for 22 checks
(`backend/crates/tests/qip-acceptance/tests/infrastructure.rs:3147-3152`) and
is read at `infrastructure.rs:818,953,958,969,1628,1774,2259,2303,3176,3353,3356`,
`manifest_wiring.rs:1376,1479,1519,1629,1724,1834,1850` (its header explains
why the base is still walked, `:37-48`), and `egress.rs:46` (`MANIFEST`).
`console_route.rs:37-40` reads `values-dev.yaml` and `templates/api.yaml`.
Each check that asserts a property of the platform — default deny, no root,
limits, no credential in a manifest, the ceiling from a named resource, every
workload covered by policy, the proxy's allowlist, the console admitted only
from its subnet — must be re-pointed at the artefact that will carry the
property on the target runtime (the `cloudrun` module inputs, the zone
firewall rules, the node unit file, the Envoy bootstrap wherever it is
rendered) **before** the file it reads is removed. The design note's table of
what each `egress.rs` test becomes is the model (`design-egress-path.md`
§5.2). A test that cannot be re-pointed without losing what it asserted means
the migration is wrong at that point, not the test
(`.claude/rules/02-change-management.md`, Tests). This step is the parallel
engineer's; it is recorded here because every later step depends on it.

**Step R2 — stop `deploy.yml` writing GitOps for a service once it deploys
to Cloud Run.** The `gitops-update` job (`deploy.yml:355-456`) writes all four
digests. When a warm service moves, its digest line is dropped from that
loop (`:426`) and the Cloud Run deploy step (§Creation order step 7) takes
over for that binary. Two writers to one workload is the failure ADR 0017
retired (`docs/adr/0017-gitops-delivery.md:35-39`); one binary must have
exactly one deploy path at every commit.

**Step R3 — retire warm services from the chart one at a time, deepbrain,
fastbrain, api, in that order.** Removing a template from the chart is what
deletes the workload: the dev Application prunes and self-heals
(`argocd/apps/dev.yaml:26-29`). Order: `qip-deepbrain` first because nothing
in the namespace depends on it except the API's egress rule
(`templates/namespace.yaml:221-233`) and it holds no listener anything else
dials; `qip-fastbrain` second for the same reason; `qip-api` last because the
console route (`templates/api.yaml:523-615`), the mesh Service the cells dial
(`:253-270`), and both brains' ingress policies (`namespace.yaml:305-344`)
all name it. Each removal follows ADR 0020 step 4's per-service evidence.
Each removed template takes its `SecretProviderClass` entries
(`templates/secrets.yaml`) and its `PodMonitoring` (`templates/monitoring.yaml`)
with it.

**Step R4 — edge cells.** Nothing to drain: `cell.enabled` is `false` and no
values file enables it (`values.yaml:50`; `values-dev.yaml`). The Terraform
cells (`edge_cells` in every tfvars) create subnets, identities and firewall
rules and no compute (`modules/edge-cell/main.tf:10-16`). Remove the
`edge_cell` module block, variable and output (G29) only after the
`execution-node` module is wired for the same cell ids — the node's
`node_id` is the cell id (`modules/execution-node/variables.tf:9-17`) — so
the subnet ranges are freed before the node's own subnet asks for them
(`variables.tf:55-67`).

**Step R5 — KEDA, VPAs, ComputeClass, PodMonitoring.** All are consumers of
workloads removed in R3. `keda/**` goes with the `ScaledObject`
(`templates/api.yaml:356-407`); `templates/autoscaling.yaml` and
`templates/monitoring.yaml` go whole.

**Step R6 — Kargo, then cert-manager.** cert-manager "exists for exactly one
consumer today: Kargo" (`cert-manager/base/kustomization.yaml:2-4`), so it
cannot go first. The Kargo console address `google_compute_global_address.kargo`
(`console-ingress/main.tf:48-55`) and the `bootstrap-kargo-admin.sh` script go
with it.

**Step R7 — Argo CD.** Applications (`argocd/apps/dev.yaml`, `edge.yaml`),
then the AppProject, then the install and its egress policies, then the dev
overlay's Ingress, ManagedCertificate and FrontendConfig
(`argocd/overlays/dev/console-ingress.yaml`), then the `console_ingress`
module block (`main.tf:590-600`), its variables (`variables.tf:747-757`), the
IAP API entry (`services/main.tf:93`) and the `enable_console_ingress` and
`console_operators` lines in `environments/dev/terraform.tfvars:147-153`.
`gitops-exceptions.md` and `argocd/README.md` describe this stack and go with
it (they are `docs/operations`, outside this document's ownership; named so
the runbook check at `infrastructure.rs:3184-3210` is not left naming a
template that no longer exists).

**Step R8 — the Helm chart.** After R3–R7, the remaining templates are
`namespace.yaml`, `config.yaml`, `secrets.yaml`, `journal-storage.yaml` and
`egress.yaml`. They are removed together with `Chart.yaml`, `values*.yaml`
and the `gitops-update` job as a whole (G21, G26). ADR 0020 step 5's evidence
— two consecutive weeks with no traffic on the GKE path (`0020:93`) — is
measured before this step, not after.

**Step R9 — the GKE cluster and everything keyed on it.** `infra.yml down`
already destroys `module.cluster` and its dependents, and names them: the
backup plan and the workload-identity bindings (`infra.yml:271-292`). Then
the code: the `cluster` module block (`main.tf:212-272`), its variables and
outputs (G28), `container.googleapis.com` and `gkebackup.googleapis.com`
(`services/main.tf:43,68`), the GKE robot grant and service identity
(`secrets/main.tf:60-72`), the node account (G19), the CI/infra GKE roles
(G18), and the taint-recovery step (`infra.yml:201-260`).

- (c) **`modules/binaryauthorization` coupling.** `cluster_id` is fed from
  `module.cluster.name` (`main.tf:575`) into `cluster_admission_rules`
  (`binaryauthorization/main.tf:234-239`). Remove that block and the input.
  What replaces it: the policy's `default_admission_rule`
  (`:219-223`), which every Cloud Run service and job evaluates through
  `binary_authorization { use_default = true }`
  (`modules/cloudrun/main.tf:275-277,431-433`). Nothing replaces it for the
  node, because nothing can (`modules/execution-node/main.tf:29-45`): the
  substitute is the image build attesting the binary it packaged
  (`README.md:89-101`), which is a workflow gate, not a policy. The
  `global_policy_evaluation_mode` justification (`:191-198`) is GKE-specific
  and must be re-argued (G15).
- (c) **`modules/backup` coupling.** `cluster_id` from `module.cluster.id`
  (`main.tf:674`) into `google_gke_backup_backup_plan`
  (`backup/main.tf:129-215`). Remove the plan, the GKE Backup service identity
  and its KMS grant (`:115-127`), and `gkebackup.googleapis.com`. What
  replaces it: the module's own second mechanism — the Compute snapshot
  resource policy (`:224-266`) attached to the node's journal disk, which
  the node module must first declare (G8). The `journal_backup` and
  `journal_snapshot_attachment_command` outputs (`outputs.tf:194-238`) are
  rewritten around the node disk rather than `pvc-<uuid>`.
- (d) **The root Workload Identity binding** (`main.tf:354-362`) names
  `serviceAccount:<project>.svc.id.goog[qip/<ksa>]` and `depends_on
  = [module.cluster]`. Remove it. What replaces it: for Cloud Run, the
  revision's `service_account = google_service_account.workload.email`
  (`modules/cloudrun/main.tf:280,440`) — the identity is the account, with no
  federation step; for the node, the instance template's `service_account`
  block and the metadata server (`modules/execution-node/main.tf:180-184,338-346`).
  The comment at `secrets/main.tf:295-301` explaining why the binding lived
  in the root becomes history and should be shortened to say so. The
  `trust-zones` module carries the same GKE-shaped binding
  (`trust-zones/main.tf:251-257`) and must lose it before wiring (G10).

**Step R10 — narrow the network.** Remove the secondary ranges
(`network/main.tf:67-75`; replaces the subnet), narrow the NAT to
`LIST_OF_SUBNETWORKS` (`:102`), and delete the `qip-node` rules (`:133-172`)
and the console→API rule (`:236-255`) once the API is a Cloud Run service
reached by invoker IAM (G31). The console subnet and reserved address stay
until the API moves; the `console_route.rs` suite reads both
(`console_route.rs:37-40`).

**Step R11 — LAST: `infrastructure/kubernetes/base/**`.** (a) Only after R1
has re-pointed every check, `infrastructure.rs:3176-3181` (which requires the
README to name the chart) has been retargeted, and the two doc comments in
`backend/crates/apps/qip-fastbrain/src/config.rs:100` and
`backend/crates/apps/qip-deepbrain/src/config.rs:35` that cite base files have
been corrected by whoever owns `apps/`. Deleting it earlier fails the suite
in three files at once and, worse, removes the only fixture the retargeted
tests are being written against.

## Creation order

Dependency-ordered. Each step names the evidence that lets the next one be
sought; none is authorised by this document.

**Step C1 — the egress path, on the target substrate, first.** Whatever D1
decides, the first artefact is a TLS-terminating reverse proxy that a Cloud
Run service and a GCE node can reach at an `http://` address, with the
port-selects-destination property preserved (`templates/egress.yaml:558-565`).
The design note establishes that a *central* Cloud Run proxy is not a
plaintext address without an internal HTTP load balancer in front of it and
that keeping four listeners multiplies that tier by four
(`design-egress-path.md` §2(a)), and recommends a co-located form — a Cloud
Run sidecar and a `qip-egress.service` unit on the node — with one committed
bootstrap rendered per tier (§3.1). The Envoy image must be mirrored and
attested through `vendor.yml` so Binary Authorization's single default rule
admits it (`templates/egress.yaml:778-782`; `.github/workflows/vendor.yml:139-229`).
Evidence: the three items under §Removal order precondition (b), on the
target substrate. This step has no dependency on any other creation step and
is the one everything else waits on.

**Step C2 — `run.googleapis.com`.** Add to the `always` map with the module
that needs it named beside it (`modules/services/main.tf:19-21` pattern;
`NOT-WIRED.md:23-27`). Same commit: the CI account gains the Cloud Run deploy
role and `iam.serviceAccountUser` on each workload account it deploys as
(G18). The infra account's curated role list (`cicd/main.tf:153-191`) gains
`roles/run.admin`; the module's own comment says a missing role fails
mid-apply and that is the accepted price (`:118-123`).

**Step C3 — a subnet for Cloud Run direct VPC egress.** Its own range, not a
share of the primary (`NOT-WIRED.md:29-36`; `modules/network/main.tf:180-185`
gives the same argument for the console). One shared range or one per trust
zone is D13's call. The range must overlap none of: the primary, pod and
service ranges (`variables.tf:119-135`), the console `/26`
(`environments/dev/terraform.tfvars:165`), every cell's three ranges in every
environment (`dev:54-69`, `test:51-61`, `stage:55-77`, `prod:89-156`), and
the thirteen zone ranges to come (`NOT-ENFORCED-HERE.md:145-148`). That
address plan is the step most likely to be underestimated, and it is done
once, before anything below.

**Step C4 — the service catalogue: one `module "cloudrun"` per warm
binary, one at a time.** ADR 0020 step 2 is *one* service on both substrates
(`0020:90`; `NOT-WIRED.md:38-41`). The mapping from today's manifests to the
module's inputs, read off the templates:

| Binary | `traffic_class` | `ingress_posture` / `invokers` | `min`/`max` | `secret_mounts` (from the chart) | `env` (from the chart) | `health_path` |
|---|---|---|---|---|---|---|
| `qip-deepbrain` | `platform` | `internal`; invokers: the API's account | 1 / 1, with `always_on_justification` — it runs a cycle loop, not requests, and two replicas write two evidence sets (`deepbrain.yaml:9-17,34-39`) | envelope key → `QIP_CAPITAL_ENVELOPE_KEY_FILE` (`deepbrain.yaml:179-180`) | `QIP_DEEPBRAIN_HEALTH_ADDRESS`, `QIP_AUTONOMY_CEILING=paper_trading`, `QIP_STORAGE_TARGET=memory`, `QIP_CYCLE_INTERVAL_SECONDS` (`deepbrain.yaml:133-168`; `config.yaml:14,30-31`) | `/health` (`deepbrain.yaml:106-113`) |
| `qip-fastbrain` | `trading` — "the only workload permitted to reach a venue" (`fastbrain.yaml:3`), so the class that can never be published (`cloudrun/main.tf:146-149`) | `internal`; invokers: the API's account | 1 / 1, justified: a singleton by correctness (`fastbrain.yaml:39-64`); `cpu_idle` is false at a floor of one (`cloudrun/main.tf:325`), which is what a loop needs | envelope key (`fastbrain.yaml:238-239`) | `QIP_FASTBRAIN_HEALTH_ADDRESS`, `QIP_AUTONOMY_CEILING`, `QIP_STORAGE_TARGET`; connector keys absent (`fastbrain.yaml:155-224`) | `/health`; readiness is `/ready` (`fastbrain.yaml:140-151`) — the module has one probe path, so D-note: the startup probe must be `/ready` and liveness `/health`, which the module cannot express today (`cloudrun/main.tf:352-373` use one `health_path`) |
| `qip-api` | `platform` (it is the operator interface and the mesh endpoint, not customer traffic; the portal is the customer surface) | `internal`; invokers: the console account (`secrets/main.tf:331-337`) replacing the internal LB (G31) | 1 / 1 until the mesh state moves out of the process (D16); the KEDA 2–6 band (`api.yaml:376-377`) cannot be carried | five tokens + envelope key → the six `_FILE` variables at `api.yaml:141-155` | `QIP_API_ADDRESS`, `QIP_STORAGE_TARGET`, `QIP_AUTONOMY_CEILING`, `QIP_MESH_CELLS` (`api.yaml:85-134`) | **`/api/v1`**, not the module's default `/health` (`api.yaml:166-177`; `cloudrun/variables.tf:331-348`) — a probe at `/health` on this binary is a service that never becomes ready |

Two module properties that make the mapping safe rather than merely possible:
the `_FILE` variables are generated from `secret_mounts` and a plaintext
credential name in `env` is refused (`cloudrun/variables.tf:361-394`), and the
image is refused unless pinned by digest (`:203-222`). One property nothing
enforces: that the generated `_FILE` names are the ones the binary reads —
`manifest_wiring.rs` checks that for the chart (`manifest_wiring.rs:598-720`)
and for nothing in Terraform; §Report names it.

`kind = "job"` stays uninstantiated: no binary in the workspace is a batch
job (`docs/adr/0010-what-gets-deployed.md:12-19`), and a job with nothing to
run is a control that cannot fire.

**Step C5 — the execution node, one per region, shadow mode, per the
module's own wiring block.** `README.md:135-178` gives the root block
verbatim, including the venue-credential predicate that must be copied as it
stands (`:180-190`). It needs a root `execution_nodes` map, empty by default
(`:192-195`); a boot image (G9) — no image pipeline exists in the tree and
Terraform cannot make one; `egress_proxy` from C1 (`variables.tf:245-293`);
a journal persistent disk (G8); `create_egress_nat = true` in any region the
root NAT does not cover (`variables.tf:421-440`). Region set: the cells the
tfvars name are `london-1` (`europe-west2`) and `tokyo-1`
(`asia-northeast1`) in dev; the blueprint's three regions are `us-east4`,
`europe-west2`, `asia-northeast1` (§36). D19. Evidence per ADR 0020 step 3
(`0020:91`): a node holding sessions, quoting nothing, matching the pod's
decisions — with "sessions" meaning the simulated broker or a sandbox
(`0020:98-105`).

**Step C6 — the trust zones.** After G10's rework of the identity binding.
The wiring pass is `NOT-ENFORCED-HERE.md:131-159`: root block, four
variables, the address plan from C3, then tags applied to the things that
carry traffic — which on Cloud Run means the revision's network interface
must carry the zone tag, an input the module does not expose today
(`cloudrun/main.tf:289-296`); on the node it is `node_tag`
(`execution-node/main.tf:62,292`). Start with Optimisation, because that is
the zone whose absence currently gives IBM egress to the research workload
(`NOT-ENFORCED-HERE.md:40-50,149-152`). Evidence: two plans, one refusing an
`ibm-quantum` entry on any zone but `optimisation`, one admitting it there
(`:155-159`).

**Step C7 — retarget `deploy.yml`.** Keep `gate`, the four-image build,
Trivy, push and `sign the pushed image` unchanged (`deploy.yml:84-335`); the
attestation chain is registry-side and survives the topology
(`modules/execution-node/README.md:89-91`). Replace `gitops-update`
(`:355-456`) with:

1. *Services.* Per warm binary already moved: `gcloud run deploy
   qip-<env>-<name> --image <prefix>/<binary>@<digest> --region <region>`,
   identity derived from the tfvars exactly as the `images` job derives it
   (`:187-220`), digest read back from the registry as the sign step does
   (`:303-310`). Binary Authorization is evaluated at that deploy through the
   service's `use_default` (`cloudrun/main.tf:275-277`); an unattested digest
   is refused there, which is the admission gate ADR 0017 said must survive
   (`docs/adr/0017-gitops-delivery.md:38-39`). Because the `cloudrun` module
   also holds `image_digest`, the workflow and Terraform must not both own the
   revision — D20 decides which, and the other reads it.
2. *Node.* Build the boot image with the attested binary and record the image
   self-link; then either `terraform apply` with the new `boot_image` (the
   template is `create_before_destroy`, `execution-node/main.tf:294-296`) or
   `gcloud compute instance-groups managed rolling-action start-update` with
   surge 1 / unavailable 0 / substitute, which is the module's own update
   policy (`:422-429`). Blue-green plus shadow mode is §48 "Deploy — node".
   Binary Authorization has no admission point here; the workflow gate is the
   equality check between the digest attested at `:303-335` and the binary
   packed into the image (`README.md:98-101`).
3. Drop the `edge-node` exclusion comment (`:349-351`) and the Cloud Run
   frontends comment (`:353-354`) as each stops being true.

`no_workflow_depends_on_a_repository_variable`
(`infrastructure.rs:2076`) and `every_step_output_a_workflow_reads_is_one_that_job_writes`
(`:2740`) constrain the rewrite: every value stays a function of the tfvars.

**Step C8 — observability on the new substrates.** Cloud Run has no
`PodMonitoring`; the binaries' `/metrics` needs a collector the tree does not
declare (D22). The node needs a scraper on the machine or a push from the
binary; neither exists. The alert policies stay gated on
`workload_metrics_exist` (`observability/main.tf:19`) until a scrape is
observed on the new substrate — the rule in `.claude/rules/domains/observability.md`
about that flag is unchanged by the migration.

**Step C9 — environments.** `dev` is the only environment that can be
applied. The blueprint's `dev` is "Cloud Run only. No node, no Spanner"
(§48), which matches C4 without C5; its `sim` is "one node, replay harness,
simulated venues" — the shadow node of C5 belongs in a `sim`-shaped
environment the tree does not have (D17).

## Terminology reconciliation

Default: **no rename.** The tree's names are load-bearing in environment
variables, service-account ids, test names and ADRs; renaming them is a
change to code and identity, not to documents. The mapping is recorded
instead.

| Tree name | Blueprint name | Where each is used | Rename? |
|---|---|---|---|
| edge cell, `Cell`, `qip-edge-node`, `QIP_CELL_ID`, `qip-edge-<cell>` | execution node, `algorik-node`, region | `templates/edge-cell.yaml:269`; `modules/edge-cell/main.tf:69`; `modules/execution-node/variables.tf:9-17` ("the two names are the same name"); §41.2, §36 | No. The node module already takes the cell id as `node_id` and sets `QIP_CELL_ID` (`startup.sh.tftpl:124`) |
| central plane, the centre | "Global — exists once"; central intelligence in `us-east4` | `main.tf:132-140`; §4.2 (blueprint lines 285-305); §36 (line 2925) | No |
| `qip-fastbrain` / `qip-deepbrain` | No plane names a "brain". `qip-deepbrain` composes Cognition, Intelligence, Optimisation and part of Valuation; `qip-fastbrain` hosts the central paper cycle, which the blueprint has only inside the node | `templates/deepbrain.yaml:1-7`; `docs/architecture/algorik-blueprint-traceability.md:110-151`; `design-egress-path.md` §1.4 | No. The `cloudrun` module's `plane` label (`variables.tf:73-89`) will be a lie for either binary until it is split; D18 |
| capital envelope | capital grant (payload item 7) | `templates/edge-cell.yaml:21-27`; §41.5 line 3651 | No |
| the mesh, `qip-transport`, `QIP_MESH_PEER` | control fabric (Pub/Sub) | `docs/adr/0011-everything-in-rust-on-kubernetes.md:23`; §45.1 line 4023 | No; the substitution is a decision of record |
| policy payload | the twelve-item shipping payload | `docs/architecture/algorik-blueprint-traceability.md:225-231`; §41.5 | No |
| kill switch (one) | kill switches (two independent paths) | `modules/observability/main.tf:15-49`; §46.2 line 4151 | No; the second wire is backlog B15 (`docs/plan/completion-plan.md:239`) |
| `qip-api` | portal-api plus the application APIs, *and* the operator interface, *and* the mesh endpoint | `templates/api.yaml:1,213-231`; §40.9 | No; note that the blueprint splits what this binary joins |
| console / portal, landing | investor portal / portal-web, public website | `docs/adr/0018-...:7`; §40.6-40.7 | No |
| evidence bucket | "audit with retention lock" | `modules/evidence/main.tf:1-23`; §45.1 line 4019 | No |
| `ingestion-discovery` (zone module) | DOCX "Ingestion"; diagram "Ingestion and discovery" | `modules/trust-zones/main.tf:40`; K4 in `blueprint-diagram-reconciliation.md:95-106` | No; the module chose the diagram's name |
| egress proxy, `qip-egress` | No name — the blueprint's clients speak TLS | `templates/egress.yaml:1-16`; §46.2 | No; it is an ADR 0002 consequence, not a blueprint element |
| `paper_trading` (ceiling) | shadow mode (a node that "connects, ingests, evaluates and gates, but discards orders") | `templates/config.yaml:12-14`; §48 line 4235; `modules/execution-node/variables.tf:146-165` | **No, and do not conflate them.** Shadow mode is a per-deployment observation state that a later diff ends; the ceiling is the platform's boundary. The node module keeps them apart on purpose (`main.tf:47-53`) |
| environments `dev`/`test`/`stage`/`prod` | `dev`/`sim`/`prod` | `variables.tf:52`; §48 lines 4225-4231 | Not now; D17 |
| Terraform, GitHub Actions | OpenTofu, Cloud Build, Cloud Deploy | `infra.yml:167-169`; §48 line 4199-4201 | No; D11 |
| `venues` map (`cidr`, `port`) | venue adapters, "credentials, region-scoped, IP-restricted at venue" | `variables.tf:355-358`; §36.1 line 2941 | No |

## Decisions this document does not make

Each is the owner's. The assumption column is what the parallel engineer is
proceeding under, so that a different answer is a visible change rather than a
silent one. Numbering continues `docs/plan/completion-plan.md:283-298`.

| # | Decision | Assumption the engineer proceeds under |
|---|---|---|
| D1 | The egress path variant: (a) central Cloud Run proxy behind an internal HTTP load balancer, (b) co-located sidecar and node unit, (c) PSC/SWP as complement, (d) a TLS crate in `qip-transport` | (b), per the design note's recommendation (`design-egress-path.md` §3), with (c) adopted underneath it; (d) not taken. Whatever is chosen, the port-selects-destination property is kept and the proxy holds no identity |
| D2 | In-tree HMAC vs a vetted crypto crate (F3) | Unchanged; nothing in this migration adds a caller of `hmac_sha256` |
| D4 | Switch the GKE proxy on | Not switched on. The first proxy that runs is the off-GKE one. Consequence stated plainly: ADR 0020 step 2's "same request served by both" (`0020:90`) can compare the *warm service's* response but not an egress path, because the GKE side has none — step 2's evidence is weaker than written unless D4 is also taken |
| D5 | ADR 0020 steps 1–5, each approved by name | Writing Terraform that is unwired, or wired behind an empty map or a `false` flag so that the plan creates nothing, is not a step. A plan that creates compute, a service or a route is a step and waits for its approval |
| D7 | K3 — what the application zone may reach | The DOCX's narrower reading (`blueprint-diagram-reconciliation.md:69-93`): `application-identity` reaches the ledger to read and raises intents; the zone module's `core_paths` already encodes it (`trust-zones/main.tf:80-84`) |
| D9 | Market-data and chain hostnames and their licensing posture | None added. Those listeners stay absent from every rendering of the bootstrap |
| D11 | Whether §48's OpenTofu / Cloud Build / Cloud Deploy row is CONTRADICTS or transitional | Transitional: GitHub Actions and Terraform stay; `gcloud run deploy` and a MIG update from the workflow stand in for Cloud Deploy's rollout. Cloud Deploy's gradual rollout with automatic rollback (§48) is not reproduced |
| D13 | Identity model for the zones: one account per workload (`cloudrun`) or one per zone (`trust-zones`) | Per workload. The zone is a subnet and a tag; the zone module's per-zone account and KSA binding (`trust-zones/main.tf:234-257`) are removed and its ledger/fabric grants take workload account emails as members. One shared Cloud Run subnet per zone, not one per workload |
| D14 | Which environment first | `dev`, the only one with a project (`environments/dev/terraform.tfvars:18`) |
| D15 | Floors of one for the two brains versus the blueprint's scale-to-zero | Floor of one with `always_on_justification`; the blueprint's scale-to-zero is for request-driven services and these binaries run loops (`fastbrain.yaml:298-315`) |
| D16 | The mesh on Cloud Run | `qip-api` at `max_instances = 1` until the per-process mesh state moves out (`templates/api.yaml:233-250`). The cell's `QIP_MESH_PEER` becomes the API's internal address; there is no client-IP affinity to lose at one instance |
| D17 | A `sim` environment (§48) versus `test`/`stage` | Not created. The shadow node lands in `dev`'s tfvars under `execution_nodes` when C5 is approved, which departs from §48's "dev: no node" |
| D18 | The `plane` label for `qip-deepbrain` and `qip-fastbrain` | `intelligence` for deepbrain, `capital-and-risk` for fastbrain, each with a comment saying the binary spans planes; the label is for the bill, not a claim about the boundary |
| D19 | The region set for execution nodes | The cells the tfvars already name, not §36's three; `us-east4` (dev's own region) gets no node until a cell is declared there |
| D20 | Who owns a Cloud Run revision's image: the workflow (`gcloud run deploy`) or Terraform (`image_digest`) | The workflow deploys; Terraform holds the digest as a variable read from the same tfvars-derived source and `ignore_changes` is *not* added, so a drift shows in the next plan rather than being hidden. To be argued in the wiring pass |
| D21 | `global_policy_evaluation_mode` after GKE | Left `ENABLE` with its justification rewritten; a change to `DISABLE` is a separate reviewed diff with a plan showing what it refuses |
| D22 | How Cloud Run and node metrics reach Managed Prometheus | Not decided; nothing in the tree collects from either substrate. `workload_metrics_exist` stays `false` |
| D23 | Whether the node's boot image is built in CI or by hand, and with what tool | Not decided. It is not a Rust dependency and needs no ADR under ADR 0002; it does need a pinned, reproducible build and a place the self-link is recorded |
