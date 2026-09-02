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

### The pipeline reaches Cloud Run, and there is no cluster to reach

This section used to say that a private GKE endpoint with no authorised
networks left `deploy.yml` able to authenticate and unable to reach the API
server, and named the Connect gateway as the way out. The cluster, its
endpoint and that question all left the configuration at `808ca32`; ADR 0024
records the runtime that replaced them.

What the pipeline reaches now is the Cloud Run Admin API, which is a Google
API and answers a GitHub-hosted runner the way every other one does: there is
no VPC between the runner and the control plane and nothing to proxy. The
`deploy` job (line numbers in this section and the next are at `02031f1`)
authenticates by workload identity federation with values derived from the
environment's committed tfvars (`.github/workflows/deploy.yml:394`, the same
`derive the identity from the tfvars` step as `:191-224` in the `images`
job) and then, for each catalogue entry, runs `gcloud run services update
--container <name> --image <prefix>/<binary>@<digest>` with the digest the
`images` job signed and attested (`:449-487`). Terraform creates each service
at the digest in `images.tfvars` and ignores the image thereafter
(`infrastructure/terraform/modules/cloudrun/main.tf:558`), so the pipeline is
the only writer to a service's image and there is no reconciler between the
two. The values the pipeline used to read as repository variables —
`GCP_PROJECT`, `GCP_REGION`, `GCP_WORKLOAD_IDENTITY_PROVIDER`,
`GCP_DEPLOY_SERVICE_ACCOUNT`, `GCP_BINAUTHZ_ATTESTOR`,
`GCP_BINAUTHZ_KEY_VERSION`, `GCP_INFRA_SERVICE_ACCOUNT` — are read by no
workflow (`grep -c '${{ vars.' .github/workflows/*.yml` is 0 in every file)
and `no_workflow_depends_on_a_repository_variable` in the acceptance suite
refuses their return.

Nothing has exercised that path. No run of `deploy.yml` has been made against
a project this configuration was applied to, because nothing has been applied
— see "What has not been verified".

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

### A rollout is proven by the serving revision's digest

This section used to be about Deployments, their probes, and a rollout check
at the end of `deploy.yml` that should now pass. There are no Deployments and
no rollout check, and `DOES_NOT_SERVE_YET`, the exemption list it cited, is
no longer in the acceptance suite (`grep -c DOES_NOT_SERVE_YET
backend/crates/tests/qip-acceptance/tests/infrastructure.rs` = 0).

What proves a rollout now is in the same `deploy` step. `gcloud run services
update` blocks until the new revision is Ready and routing and fails the job
when it is not — the rollout wait the GitOps cut-over lost, which
`docs/operations/gitops-exceptions.md` recorded. Then the step reads the
service back, `spec.template.spec.containers[0].image` and
`status.conditions[0].status` (`.github/workflows/deploy.yml:495-503`), and
fails unless the first is exactly the attested image reference and the second
is `True`. `services update` returning is not that proof: a traffic split or
a previous revision still routing would look identical from the exit code.
Only after that does the step append the digest to
`infrastructure/environments/<env>/images.tfvars` and commit the file
(`:542`), so the committed record and the serving revision agree.

Readiness is still what the binary reports, not process liveness:
`qip-fastbrain` and `qip-deepbrain` each serve `/health` and `/ready` on their
own listener, and the fast path revokes readiness for cycles that have
breached its ceiling while the deep path, which has no cycle ceiling, revokes
it only for stopping, halted, stalled, persistently failing and warming.

The execution node is not a Cloud Run service and is not deployed in the
same sense. The step after the services asks each `qip-<env>-exec-*` managed
instance group to replace its instances under the group's own surge-one
policy (`:516-540`) and, with `execution_nodes = {}` in every environment's
tfvars, finds no group and says so rather than inventing one. On a node the
health listener is `QIP_HEALTH_PORT` on the machine
(`infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl:151`).
Bringing one up is still a runbook, and
[that runbook](deploying-an-edge-cell.md) carries ADR 0024's banner rather
than a procedure for this runtime.

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

### Secrets reach a service as mounted files, and a node fetches them at boot

This section used to be "Kubernetes Secrets are created out of band": the
manifests referenced `qip-tokens` and `qip-capital-envelope-key`, nothing
created them, and a person with `kubectl` closed the gap. No Kubernetes
Secret exists any more, nothing is created with `kubectl`, and there is no
CSI driver because nothing needs one.

Every secret a Cloud Run service reads is a Secret Manager volume.
`modules/cloudrun` (line numbers at `02031f1`) mounts each entry of
`secret_mounts` at `/var/run/secrets/qip/<key>/<file>`
(`infrastructure/terraform/modules/cloudrun/main.tf:72-79`; the volume at
`:505-523`, mode 0400), grants the workload's own identity
`roles/secretmanager.secretAccessor` on that secret and no other (`:281-288`),
and puts only the path into the environment, as the `_FILE` variable
`qip_core::secret` reads (`:82-90`). The value never enters the environment.

The execution node has no volume to mount, so
`infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl:107-131`
fetches at boot instead: `qip-fetch-secret` writes the capital envelope key
to a tmpfs at mode 0400, and the venue credential only if the module bound
one, which the paper ceiling and shadow mode each prevent on their own. The
binary reads `QIP_CAPITAL_ENVELOPE_KEY_FILE` (`:154`). The helper is one the
boot image is contracted to ship (`:91`), and no image bake exists in this
repository.

What is still out of band is the values. Terraform creates every secret
container empty — no value appears in any `.tf`, and
`no_secret_value_appears_in_the_terraform` refuses one — so a service whose
secret has no version cannot start. `scripts/bootstrap-deploy.sh` seeds the
six self-generated values once, the role tokens and the envelope key, and
never overwrites an existing one; the market-data key, the venue credential
and the quantum token come from a vendor, a broker and a provider, and no
script can invent them.

### There is no ingress controller

Still true, and no longer a gap: there is no `ingress-nginx`, no namespace
for it to run in, and no ingress resource of any kind in front of the API.
That is the configuration rather than something it has yet to complete.

Every catalogue entry is passed `ingress_posture = "internal"`
(`infrastructure/terraform/catalogue.tf:284` at `02031f1`), which
`modules/cloudrun` renders as `INGRESS_TRAFFIC_INTERNAL_ONLY`
(`infrastructure/terraform/modules/cloudrun/main.tf:67`); the module has no
input that produces `INGRESS_TRAFFIC_ALL`. The API answers a caller inside
the VPC only, and only one named in its `invokers` list, which is the
console's identity (`catalogue.tf:64`, `modules/cloudrun/main.tf:563`,
`infrastructure/terraform/outputs.tf:280`); the console reaches it by URL
over its own VPC egress, and no load balancer sits between them.
`public_ingress = {}` in all four environments' tfvars, so the one firewall
rule that would open a zone to Google's load-balancer ranges
(`infrastructure/terraform/modules/trust-zones/main.tf:446`) is created zero
times. The only forwarding rule in the tree is Private Service Connect for
Google APIs (`infrastructure/terraform/modules/connectivity/main.tf:149`),
which is an egress path, not an entrance. Nothing in this configuration
publishes any service to the internet, and a change that did would be a
`public_ingress` entry a reviewer can read.

### GitHub settings are not in this repository

Required reviewers on the `prod` environment. They are the control that makes
"production is never deployed automatically" more than a convention in a
workflow file, and they are a setting rather than a file. This section used to
list four repository variables as well; the workflows now derive every such
value from the committed tfvars, and the acceptance suite refuses a workflow
that reads one — see `docs/security/credentials.md` §2.

### The pipeline's role is project-wide, on Cloud Run rather than a cluster

This section used to say `roles/container.developer` reaches every namespace
in the project and that the RBAC binding to narrow it was not written. The
role went with the cluster.

The deploy account, `qip-ci-<env>`
(`infrastructure/terraform/modules/cicd/main.tf:17-22`), holds
`roles/run.developer` at the project (`:95-99`): it can update any Cloud Run
service in the project and describe its revisions, and it cannot set a
service's IAM policy, so it can change what runs and not who may call it. It
is `roles/iam.serviceAccountUser` on each workload's own identity, granted
per workload in `infrastructure/terraform/modules/cloudrun/main.tf:245-251`
(at `02031f1`) rather than at the project, because a project-wide grant is
the right to act as every identity in the project, the infra account
included. `roles/artifactregistry.writer` on the repository
(`infrastructure/terraform/modules/registry/main.tf`, `ci_push`) and a custom
role of three `instanceGroupManagers` permissions for the node's rolling
replacement (`modules/cicd/main.tf:110-127`) complete it.

The width that remains is the same shape as before: `run.developer` at the
project is every service in the project, not the entries the catalogue names.
Narrowing it would be a per-service binding in place of the project one, and
that is not written — as the namespace-scoped binding it replaces was not
either.

---

## What has not been verified

None of this has been applied, planned or validated. ADR 0024's "Nothing was
applied" section records the machine that made every commit on this branch:
`terraform`, `gcloud`, `helm` and `kubectl` are all `not found`. So
`terraform init`, `terraform validate`, `terraform plan` and `terraform
apply` have not run; no `gcloud run` command has run; no Cloud Run revision
has been created, no image has been attested, and no service has been proven
Ready by the step that would prove it. `kubectl --dry-run`, which the earlier
text listed as unavailable, has no equivalent to be unavailable any more; the
nearest thing to a dry run of this runtime is `terraform plan`, and none has
been produced.

What ran in place of `validate` is the structural pass ADR 0024 describes —
balanced braces, every module source present, every `var.` reference
declared, every module argument declared and every required one passed. It
proves less than `validate` would, with no provider schema and no type
checking, and the first `terraform init` may find something it did not.

Everything else here is checked structurally, by tests that read the files:
`backend/crates/tests/qip-acceptance/tests/infrastructure.rs`. Those tests
can tell that a security property has been deleted. They cannot tell that
the configuration would apply, or that a service would serve.
