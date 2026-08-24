# Provisioning a managed Google service

**Do not turn one on until its adapter exists.** Every managed store in
`infrastructure/terraform/modules/data` and the Vertex module in
`modules/ai` defaults to `false`, and the default is the correct value for
this build. Enabling one gives you a healthy, empty, billable instance that no
code in this repository can open.

Check before you enable:

```
terraform -chdir=infrastructure/terraform plan \
  -var-file=env/<environment>.tfvars \
  | grep -A20 enabled_without_an_adapter
```

If your service is named in that output, the deployment is ahead of the code.

## What a first deployment needs

Two variables have no default and no way to be guessed.

`project_id` is the one you already know. `project_number` is the project's
numeric id, which is **not** derivable from it:

```
gcloud projects describe <project_id> --format='value(projectNumber)'
```

Google's own service agents are named by number — Secret Manager publishes
rotation notices as `service-<number>@gcp-sa-secretmanager.iam.gserviceaccount.com`
— so the IAM grant that makes rotation work at all needs this value and cannot
infer it.

## The order things must happen in

1. `terraform init` and `terraform apply`. This creates the cluster, the
   network, the KMS key ring, the service accounts, the secrets **and no
   secret values**.
2. Write each secret's value out of band — `gcloud secrets versions add`.
   Terraform creates the secret and never a version, because a value in state
   is a leaked credential. A deployment whose secrets are empty will start and
   refuse to do anything that needs one, which is the intended failure.
3. Apply the Kubernetes manifests.
4. Read each pod's start-up banner. Every binary prints what it will and will
   not do before it does anything, including what it persists and what it
   loses on restart.

## Which storage target to set

`QIP_STORAGE_TARGET` accepts `memory`, `file` and `engine`. The six managed
targets are ports: naming one stops the process rather than upgrading it, and
that refusal is deliberate.

A workload can only use `engine` or `file` if it has somewhere to write. The
root filesystem is read-only in every manifest, so:

| Workload | Volume | Target |
|---|---|---|
| `qip-edge-node` | 16Gi claim, retained | `engine` |
| `qip-api` | `emptyDir` only | `memory` |
| `qip-fastbrain` | `emptyDir` only | `memory` |
| `qip-deepbrain` | `emptyDir` only | `memory` |

`StorageSettings::preflight` round-trips a value at start-up, so a target
pointed at an unwritable root fails at boot rather than at the first audit
write. If a pod will not start and the banner mentions storage, that is this
check working.

**The API's hash chain is the gap worth knowing about.** It archives the event
log's chain across restarts, and with `memory` it archives to nothing. Closing
that needs either a claim of its own — which makes it a StatefulSet — or the
Cloud Storage adapter that `StorageTarget::CloudStorage` still refuses.

## What is deliberately not provisioned

`modules/data/NOT-PROVISIONED.md` has the full reasoning. In short: Pub/Sub,
because ADR 0011 replaced it with the in-tree mesh and a topic with no
publisher is indistinguishable from a broken one; Dataflow, because no code
submits a job; and Confidential VMs, because enabling them alongside a crate
named `qip-confidential` would let the name and the configuration together
imply a guarantee neither provides.

The single Pub/Sub topic that does exist carries no platform data. Secret
Manager will not accept a rotation schedule without somewhere to announce a
rotation is due.

## Why the flags are all off

The platform implements three storage targets and refuses six by name, each
with the credential or configuration it still needs. Infrastructure that runs
ahead of code produces the most expensive kind of wrong answer: a diagram that
reads as a working capability, a bill that arrives monthly, and an empty
database nobody notices until someone queries it.

Turning a flag on is a claim that the adapter exists and is wired. Until
someone can make that claim, absent is the honest state.

## The egress rule a live adapter needs

The namespace runs default-deny on both ingress and egress. Every workload has
an egress policy naming exactly what it may reach, and **a workload with no
matching rule does not get an error — it gets a connection that hangs.**

That matters more now than it did, because the platform has live adapters. An
*unconfigured* adapter refuses at start-up and says what it needs, which is the
loud, correct failure. A *configured* adapter behind a denied egress produces a
hang, which looks like a slow vendor.

`allow-market-data-egress` is written out in `namespace.yaml`, commented, ready
to uncomment. Two things must be true first:

1. `VENDOR_CIDR` is the range the vendor **publishes**. A range resolved from a
   DNS lookup is a rule that works until the vendor moves a host, and
   `0.0.0.0/0` is not a vendor range — a workload that can reach the internet
   can exfiltrate the position history it was trained on.
2. It points at a **TLS-terminating proxy**, not the vendor. `qip_transport::http`
   has no TLS stack and refuses `https` by name, so a credential sent straight
   to a public vendor crosses the internet in clear text. The proxy holds the
   vendor's certificate.

The venue path is the same shape and already templated per cell as
`VENUE_CIDR`/`VENUE_PORT` in `edge-cell.yaml`, because a venue rule belongs to
one cell: two cells selected only by `app` would each inherit the other's
venues, which is a wider grant than either was given.
