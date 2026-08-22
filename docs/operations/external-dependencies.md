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

### Images are not signed

The cluster sets `binary_authorization = PROJECT_SINGLETON_POLICY_ENFORCE`,
which means it refuses an image that is not attested. Nothing in this repository
produces an attestation. A deploy would push an image successfully and then be
refused at admission.

Closing it needs a Binary Authorization policy, an attestor backed by a KMS
signing key, and a signing step in the pipeline after the push.

### Three of the four workloads do not serve

`qip-fastbrain` checks its agent roster and returns. `qip-deepbrain` runs one
cycle and returns. `qip-edge-node`'s crate is still being written —
`crates/edge/qip-edge` says "Under construction" and declares no binary.

Their Deployments, Services and ports describe the shape they are being built
towards. Applied today, Kubernetes would restart each container as it exits.
They carry no liveness or readiness probe for that reason: a probe against an
endpoint that does not exist looks like coverage and is not.

`crates/tests/qip-acceptance/tests/infrastructure.rs` holds this as an explicit,
shrinking list. The test fails when a binary on it gains a serving loop, which
is what forces the exemption to be removed rather than forgotten.

### Edge cells have no nodes

`modules/edge-cell` creates a cell's subnet, identity, IAM and egress firewall.
It does not create a node pool. Until one exists carrying the cell's tag — the
tag every firewall rule in the module targets — the cell has nowhere to be
scheduled, and its egress rules constrain nothing.

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

Four repository variables (`GCP_WORKLOAD_IDENTITY_PROVIDER`,
`GCP_DEPLOY_SERVICE_ACCOUNT`, `GCP_PROJECT`, `GCP_REGION`), and required
reviewers on the `production` environment. The first two come from
`terraform output`. The reviewers are the control that makes "production is
never deployed automatically" more than a convention in a workflow file, and
they are a setting rather than a file.

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
`crates/tests/qip-acceptance/tests/infrastructure.rs`. Those tests can tell that
a security property has been deleted. They cannot tell that the configuration
would apply.
