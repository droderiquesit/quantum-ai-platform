# External dependencies

What this build does not have, what each missing thing would need, and what
depends on it.

Several crates point their `Error::Unavailable` messages at this file. That is
the pattern the platform uses for a managed service it has a port for and no
credentials to reach: a complete trait, a working local adapter, and an error
naming exactly what production must supply. Nothing falls back silently, because
a deployment pointed at BigQuery that quietly served a JSON file would pass its
smoke tests and lose every write.

This file exists to make the same statement about the infrastructure. The
Terraform in `infrastructure/terraform` provisions three managed services —
Artifact Registry, a Cloud Storage evidence bucket, and the KMS keys — and
deliberately provisions none of the others. Declaring Terraform for a service
nothing can reach would create infrastructure that costs money, widens the
attack surface and serves no request.

---

## 1. The data mesh

`qip-mesh` declares six ports. Each has an in-memory and a file-backed adapter
that work today, and a managed target that returns `Error::Unavailable`. The
required configuration below is the string the code itself reports, so the two
cannot drift.

| port | managed target | provisioned | what it needs |
| --- | --- | --- | --- |
| `evidence` | Cloud Storage, object lock | **yes** | already created: see `modules/evidence` |
| `lakehouse` | BigLake / Iceberg | no | GCP project, a BigLake metastore catalog, a Cloud Storage warehouse bucket, and a service account with BigLake Admin and Storage Object Admin |
| `analytical` | BigQuery | no | GCP project, dataset, and a service account with BigQuery Data Editor and BigQuery Job User |
| `hot_series` | Bigtable | no | GCP project, Bigtable instance and table, and a service account with Bigtable User |
| `master_data` | Spanner | no | GCP project, Spanner instance and database, and a service account with Cloud Spanner Database User |
| `graph` | Spanner Graph | no | the same project, instance and database with a property graph defined, and a service account with Cloud Spanner Database User |

The evidence port is the one that is provisioned, because it is the one whose
local adapter is already real: `EvidenceStore` is write-once by construction —
no delete, no overwrite, a second write of different bytes refused by digest.
The bucket underneath it is configured to agree: versioning, a locked retention
policy, uniform bucket-level access, customer-managed encryption, and
`roles/storage.objectCreator` for the workloads that write. Not `objectUser`,
not `objectAdmin`, not `storage.admin` — each of those carries
`storage.objects.delete`, and an append-only store whose writer can delete is
not append-only.

`qip-storage` declares a second, older set of targets for the same reason:
BigQuery, Cloud Storage, AlloyDB, Spanner, Bigtable and Memorystore, each with
its own `required_configuration`. AlloyDB needs an instance, a database and IAM
database credentials; Memorystore needs an instance address and VPC
connectivity. None is provisioned.

## 2. Language models and embeddings

`qip_ai::language::RemoteModel` reports itself unavailable whether or not a
credential is present, and says why: **this build has no TLS-capable HTTP
client**. The platform implements its own HTTP server in-tree and has no
outbound TLS transport, so a model credential alone would not be enough.

A deployment that wants a hosted model needs three things, not one: the
credential named by `RemoteModelConfig::credential_env`, outbound HTTPS to the
provider's endpoint, and a transport. The transport is the part that is a code
change rather than a configuration one.

`qip_ai::embedding` ships a lexical feature-hashing embedder that needs no
credentials and is honestly not a semantic one. A learned model registers
through the same `Embedder` trait.

**No Vertex AI, and no egress path to one, is provisioned.** This matters for
the fast brain specifically, and the guarantee is weaker than it looks:

* `qip-fastbrain` mounts no secret and sets no model endpoint, and its binary
  refuses to start if any agent it hosts holds `call_language_model`.
* But `qip-fastbrain` **does** link `qip-ai` transitively, through both
  `qip-kernel` and `qip-agents`. The compile-time boundary that protects the
  safety-critical engines does not protect this binary.
* And its network policy permits `199.36.153.8/30`, which is private Google
  access — one address range for *every* Google API, `aiplatform.googleapis.com`
  included. A `NetworkPolicy` cannot distinguish them.

Closing the network half needs a **VPC Service Controls perimeter** around the
project with `aiplatform.googleapis.com` outside it, or an egress policy that
restricts the fast brain's subnet to the specific restricted-VIP services it
needs. That perimeter is not created by this configuration. Until it is, the
statement that the fast path cannot reach a model rests on the start-up check
and on there being no credential and no transport — which is true today and is
not enforced by the network.

## 3. Market data

No licensed feed is available to this build. `qip-market-ingestion` ships a
synthetic environment marked `LicensingClass::Synthetic`, which the object model
refuses to admit to a production decision, plus a replay adapter.

A real deployment needs a consolidated tape or direct venue feeds, a news and
filings provider, a fundamentals vendor and a macro data source — each with its
own licence, entitlement terms and connectivity.

**No Pub/Sub topics are provisioned**, because nothing in the workspace
publishes to one. `qip-events` is an in-process bus with a file-backed log.
Provisioning a topic nothing writes to would be the same mistake as
provisioning a warehouse nothing can query.

## 4. Quantum hardware

`qip-quantum` simulates. The `qip-quantum-token` secret exists in every
environment so the deployment is uniform, and **no IAM binding grants any
workload access to it** — there is no hardware backend configured to use it. It
is a placeholder, and it should either gain a reader when a backend is chosen or
be removed.

## 5. Venue connectivity

The `venues` map on an edge cell is empty in every shipped environment. The
address ranges a venue publishes are not guessable, and an empty map means the
cell reaches no venue at all — the correct state for a cell whose connectivity
nobody has confirmed.

Filling it in needs the venue's own connectivity documentation or an extranet
provider, and in most cases a cross-connect that is not a cloud resource.

---

# Gaps in the infrastructure itself

These are not missing third-party services. They are things this configuration
describes but does not complete, and each would stop a deployment.

### The pipeline cannot reach the cluster

`enable_private_endpoint = true` with `authorised_networks = []` means the
control plane is reachable only from inside the VPC. A GitHub-hosted runner is
not. As written, `deploy.yml` will authenticate successfully and then fail to
reach the API server.

The three ways out, in rough order of preference: a self-hosted runner inside
the VPC; the GKE Connect gateway, which proxies the API server through Google's
control plane and needs `roles/gkehub.gatewayEditor` and a fleet membership; or
an authorised network for the runner's egress addresses, which GitHub-hosted
runners do not have stable ones of.

None is configured. This is the single thing that stands between this
configuration and a deployment that runs.

### Images are signed now, and it is worth knowing by whom

This used to say that nothing produced an attestation. The gap was worse than
that sentence: the cluster set `binary_authorization =
PROJECT_SINGLETON_POLICY_ENFORCE` and no policy resource existed, so Google
evaluated the *implicit* policy, whose default rule is `ALWAYS_ALLOW`. The
cluster was not refusing unsigned images and waiting for a signer. It was
enforcing a policy that admitted everything, while the configuration read as
though a control was in place.

`infrastructure/terraform/modules/binaryauthorization` closes it: an asymmetric
KMS signing key in the platform's existing key ring, a Container Analysis note,
an attestor holding the public half, and a policy whose default rule is
`REQUIRE_ATTESTATION` with `ENFORCED_BLOCK_AND_AUDIT_LOG` — plus the same rule
pinned to the cluster, so loosening the default later does not quietly loosen
the cluster that trades. `deploy.yml` signs each image by digest after the
push. The attestor and key version it signs with are not repository variables
any more: the `derive the identity from the tfvars` step
(`.github/workflows/deploy.yml:191-224`) constructs both from the
environment's committed `project_id` and `region`, and
`no_workflow_depends_on_a_repository_variable` in the acceptance suite refuses
a `${{ vars.` in either workflow. `docs/security/credentials.md` §2 has the
reasoning.

Three things are still out of band, and the module's `OUT-OF-BAND.md` carries
them in full (its first item, the two repository variables, is the stale one;
that file belongs to the module's owner):

  * `binaryauthorization.googleapis.com` and `containeranalysis.googleapis.com`
    enabled on the project — this configuration manages no
    `google_project_service` anywhere;
  * **the signer is the pipeline itself.** Anyone who can make that pipeline run
    a step of their choosing can sign an image of their choosing, so this
    raises the bar from "anything in the registry runs" to "anything the
    pipeline signs runs". A signer the pipeline cannot impersonate needs a
    second identity in a second project, which is not a thing a repository can
    create for itself;
  * the attestation says the pipeline pushed these bytes and nothing else. Not
    that the source was reviewed, not that the dependencies were the ones the
    lockfile names. A SLSA provenance statement would carry those claims and
    this repository produces none.

Key rotation is a deliberate sequence rather than a setting, because Cloud KMS
rotates only symmetric keys automatically. Disabling the old version before
everything signed by it has been rescheduled refuses running images one at a
time, as they happen to move.

### All four workloads now serve

This section used to name the workloads that ran once and exited. There are
none left, and the exemption list in the acceptance suite
(`DOES_NOT_SERVE_YET`) is empty.

`qip-fastbrain` validates its agent roster, then runs an ingest-and-cycle loop
and serves `/health` and `/ready` on its own listener. `qip-deepbrain` does the
same with a research cadence rather than a fast-path one. Both Deployments
carry real probes.

The two endpoints answer different questions on purpose, and the reasoning
differs between the nodes. On the fast path, liveness stays 200 while a cycle
is merely slow — restarting a slow node makes the problem worse — and readiness
turns 503 once cycles have breached the fast-path ceiling for longer than the
breach tolerance, so a node that is alive and not fast leaves rotation instead
of being killed. On the deep path there is **no cycle ceiling at all**: a long
cycle there is research rather than a fault, so slow is never unready. Its
readiness is revoked only for stopping, halted, stalled, persistently failing,
and warming — that last having no fast-path equivalent, because until the first
cycle lands there is no world model to consult.

The rollout check at the end of `deploy.yml` should now pass against both
rather than time out.

`qip-edge-node` used to be the third and is not any more. It binds
`QIP_HEALTH_PORT`, answers every request with the cell's state, and
`edge-cell.yaml` probes `/health` for real. It is still absent from the
rollout check, but for a different reason — the pipeline does not apply
`edge-cell.yaml` at all. Bringing a cell up is
[a runbook](deploying-an-edge-cell.md), because a workload that trades should
not appear unattended.

`backend/crates/tests/qip-acceptance/tests/infrastructure.rs` holds this as an explicit,
shrinking list. The test fails when a binary on it gains a serving loop, which
is what forces the exemption to be removed rather than forgotten.

### Execution nodes exist in code, and every environment declares none

This section used to describe `modules/edge-cell` and the node pool it did not
create. Neither exists any more: the cluster, the edge-cell module and the
manifests left at `808ca32`, `67b3e92` and `7d79161`, and ADR 0024 records
the runtime that replaced them. An edge cell is now one Compute Engine machine
from `modules/execution-node`, not a workload waiting for a node pool.

`infrastructure/terraform/main.tf:467-469` (line numbers throughout this
section are at `851c0ed`) instantiates that module once per
entry in `var.execution_nodes`, and every environment's tfvars leaves the map
empty (`infrastructure/environments/{dev,test,stage,prod}/terraform.tfvars`,
`execution_nodes = {}`). So the plan, if one ran, would create no node. The
module's `README.md:10-17` says why the first entry is a venue decision rather
than a Terraform one: `qip-edge-node` refuses an empty `QIP_VENUES`, and no
venue's published address ranges are recorded anywhere in this repository.

When an entry does exist, the module creates the node's own subnet
(`modules/execution-node/main.tf:109`), a service account with no key
(`:174`), a shielded instance template with no external address (`:292`), a
zonal managed instance group (`:416`), and a firewall that denies all egress
at priority 65000 (`:472`) with named allows for Google APIs (`:495`) and the
central plane (`:522`). The per-venue egress rule is created only when
`shadow_mode` is false (`:551-552`), and the root passes `shadow_mode = true`
unconditionally (`main.tf:492`), so the first node cannot open a venue session
until a reviewer sees that literal change. The module's README carries the
rest: what the startup script verifies about the boot image, what nothing here
enforces, and that no image bake exists.

None of it has been planned or validated. There is no `terraform` binary in
this environment.

### The capital-envelope key is symmetric, and the centre now holds it

This section used to say that no central-plane identity could read
`qip-capital-envelope-key`, and that the absent binding was the honest state
rather than a missing line. The configuration has since taken the other side
of that argument, and this section now describes what is written rather than
what was once refused.

`infrastructure/terraform/catalogue.tf` mounts the key as a file into all
three central Cloud Run services — at `851c0ed`, `api` (`catalogue.tf:96-100`),
`fastbrain` (`:149-151`) and `deepbrain` (`:181-183`) — and
`modules/execution-node/main.tf:200-203` grants
`roles/secretmanager.secretAccessor` on it to each node's service account.
`qip-api` reads it as `QIP_CAPITAL_ENVELOPE_KEY` and signs the envelopes it
dispatches down the mesh with it (`backend/crates/apps/qip-api/src/trust.rs`);
`qip-edge-node` reads the same variable and verifies against it
(`backend/crates/apps/qip-edge-node/src/main.rs:103`).

The signature is still symmetric. `qip_edge::envelope::sign_payload` is
HMAC-SHA256 over the shared secret, and the function says so itself: it
"proves possession of a shared secret, not the identity of a signer". The
consequence the earlier version of this section warned about therefore
holds by construction now: any process that can read the key can mint an
envelope for any node, so a compromised central-plane service could widen
every cell's bound. That is the trade the catalogue makes so the centre can
grant capital at all, and it is worth naming rather than assuming.

What would close it is unchanged and unbuilt: asymmetric signing, with the
central plane holding a private key in KMS, each node holding only the public
half, and a verification path in `qip-edge` that is not HMAC. `grep -rl
'kms\|ed25519\|asymmetric' backend/crates/edge/qip-edge/src` finds only the
comment in `envelope.rs` that points here. Until then, reading a node's copy
of the key grants the ability to mint as well as to verify.

### Kubernetes Secrets are created out of band

`qip-tokens` and `qip-capital-envelope-key` are referenced by the manifests and
created by nothing. That is consistent with the rule that no secret value
appears in Terraform or in a manifest, and it leaves a step that is currently a
person with `kubectl`. A Secret Manager CSI driver or an external-secrets
controller would close it; neither is configured.

### There is no ingress controller

`allow-api-ingress` permits traffic from a namespace labelled `ingress-nginx`.
Nothing in this repository creates that namespace or the controller in it.

### GitHub settings are not in this repository

Required reviewers on the `prod` environment. They are the control that makes
"production is never deployed automatically" more than a convention in a
workflow file, and they are a setting rather than a file. This section used to
list four repository variables as well; the workflows now derive every such
value from the committed tfvars, and the acceptance suite refuses a workflow
that reads one — see `docs/security/credentials.md` §2.

### The pipeline's cluster role is broader than one namespace

`roles/container.developer` is the narrowest predefined role that can apply
manifests. It still reaches every namespace in the project. Narrowing it is a
Kubernetes RBAC binding rather than a Google one, and is not written.

---

## What has not been verified

None of this has been applied, planned or validated. There are no Google Cloud
credentials in the environment this was written in, so `terraform init`,
`terraform validate`, `terraform plan` and `terraform apply` were all
unavailable, and so was `kubectl --dry-run`.

Everything here is checked structurally instead, by tests that read the files:
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs`. Those tests can tell that
a security property has been deleted. They cannot tell that the configuration
would apply.
