# ADR 0093: The dev control plane's cluster is suspended while no Argo CD release passes the scan gate

- **Status**: Accepted on the authority the owner delegated on 2026-09-19
  (ADR 0081's status line records the delegation) and on the owner's
  instruction of 2026-09-20 to minimise cost. **This suspends a deployment,
  not a decision**: ADR 0036 stands unamended, every manifest stays in the
  tree, and the reversal is one word in one tfvars line.
- **Date**: 2026-09-20
- **Supersedes**: nothing.
- **Related**: ADR 0036 (the control-plane cluster and the GitOps path it
  serves), ADR 0024 (the runtime this replaced), ADR 0040 decision 13 (the
  teardown that emptied the registry), ADR 0028 (the vendoring path and
  why a third-party image is adopted rather than exempted).

## Context

The dev environment holds 238 Terraform-managed resources and **runs no
workload at all**. That is not a claim about idleness; it is the state of
the world, and it took a while to see because two different things are
true at once.

`infra.yml` run 52 (2026-09-20,
<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/35527644424>)
reports `Apply complete! Resources: 0 added, 1 changed, 0 destroyed.` The
infrastructure is applied and stable. The run's conclusion is nonetheless
`failure`, on a later step: `error: deployment "cert-manager-webhook"
exceeded its progress deadline`.

Those two facts belong together. Under ADR 0036 a Cloud Run service is not
a Terraform resource — it is a `RunService` manifest under
`infrastructure/gitops/envs/<env>/`, reconciled by Config Connector, which
Argo CD installs, which the bootstrap step installs. The `removed` blocks
in the root module released the services from state on purpose. So the 32
`module.cloud_run` entries in state are identities, secret grants and
buckets; **not one of them is a running service**, and none has been since
the teardown of 2026-09-13 deleted the four that existed.

The chain from a green apply to a served request therefore passes through
Argo CD, and Argo CD is where it stops.

## What actually blocks it, measured rather than inferred

`vendor.yml` mirrors each line of `infrastructure/egress/vendored-images.txt`
and scans it twice: the full `CRITICAL,HIGH` report for the log, and a
blocking `--severity CRITICAL --exit-code 1 --ignore-unfixed` gate. Run
35528637815 failed that gate on the Argo CD image:

```
usr/local/bin/git-lfs    (gobinary)  Total: 1 (CRITICAL: 1)
usr/local/bin/kustomize  (gobinary)  Total: 1 (CRITICAL: 1)
stdlib  CVE-2025-68121  CRITICAL  Unexpected session resumption in crypto/tls
```

Confirmed independently against the Go vulnerability database as
GO-2026-4337, fixed in `go1.24.13`, `go1.25.7` and `go1.26.0-rc.3`.

The loop runs under `set -euo pipefail`, and Argo CD is the first
control-plane line in the list. So the gate does not merely refuse Argo CD:
**it stops the mirror before redis, Kargo, cert-manager's three images, the
Config Connector operator and the google-cloud-cli are copied at all.** The
registry was recreated empty by the teardown, so cert-manager's pods have
nothing to pull, which is the progress deadline in the log. The failure
that reads like a cert-manager problem is an Argo CD scan result three
steps upstream.

**The obvious remedy does not work, and that was established by measurement
rather than by reading a release note.** Each image's `COPY
/usr/local/bin/<tool>` layer was pulled from quay's v2 API and the Go build
stamp read out of the binary:

| Argo CD tag | kustomize | git-lfs | helm |
|---|---|---|---|
| v3.4.9 | go1.24.0 | go1.25.3 | go1.24.11 |
| v3.5.2 (pinned here) | go1.24.0 | go1.25.3 | — |
| v3.5.3 (newest on the 3.5 line) | go1.24.0 | go1.25.3 | — |
| v3.6.0-rc1 | go1.24.0 | go1.27.0 | go1.27.1 |

v3.5.3 ships byte-identical kustomize and git-lfs layers to the ones
already refused — same size, same stamp. Pinning it would have looked like
remediation in the git history and would have failed the identical gate.
The 3.6 candidate rebuilds git-lfs and helm and still carries kustomize on
`go1.24.0`.

**And there is no newer kustomize to adopt.** The Go module proxy, which
publishes every tag the project has cut, ends at `v5.8.1` dated
2026-02-09 — seven months old, the version Argo CD already ships, the one
built on `go1.24.0`. No pin on either project reaches a fixed build.

That exhausts the remedies this repository can reach. What is left is
(1) upstream kustomize cutting a release on a patched toolchain, (2) this
platform building kustomize from source and copying it over the attested
base, which would make the platform a *builder* of third-party code rather
than a mirror of reviewed bytes and is a decision of its own, or (3) a
scanner exception, which `.claude/rules/00-enterprise-governance.md`
refuses and which the owner's own instruction — "do not silence scanners,
suppress findings, disable security controls, or weaken tests to obtain
passing results" — refuses twice.

## Decision

**A new `suspend` action on `infra.yml` destroys the control-plane cluster
and nothing else, for as long as no published Argo CD image passes the gate.**

```
terraform destroy -target='module.gitops_control_plane[0].google_container_cluster.control_plane'
```

Everything else in dev stands, and everything else in that module stands too:
the etcd key, the three controller identities, the registry DNS zones. The
manifests, the overlays, the bootstrap step, the vendored digests and ADR 0036
are untouched.

### The first version of this decision was `gitops_enabled = false`, and it was refused by a guard that is right

This record said that, it was committed, and run 54 aborted at plan time
(<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/35531628713>,
`failure`, two minutes — too fast to have destroyed anything):

```
│ Error: Instance cannot be destroyed
│   on modules/gitops-control-plane/main.tf line 103:
│  103: resource "google_kms_crypto_key" "etcd" {
│ Resource module.gitops_control_plane[0].google_kms_crypto_key.etcd has
│ lifecycle.prevent_destroy set, but the plan calls for this resource to be
│ destroyed.
```

Closing the flag takes the module's `count` to zero, and that plans a destroy
of every resource in it — including a KMS key whose own comment says why it
may not go: "Destroying the key makes every Secret in etcd unreadable at once,
and the failure appears on the next control-plane restart rather than now."

**The guard is not wrong, it is merely unable to say "unless the cluster is
going too".** `prevent_destroy` is a property of one resource and cannot read
the rest of the plan. So it fires on a plan where the etcd it protects is
being destroyed anyway, which is a false positive — and a false positive on a
control like this is the price of the control, not a reason to remove it. It
is not removed, not made an input, and not routed around with a `-target`
that would smuggle the key out of the plan's scope.

It is worth writing down that the first attempt was wrong, because the shape
of the error is instructive: a switch that reads as "turn the feature off"
was in fact "delete everything the feature ever touched", and the only thing
that noticed was a lifecycle rule on one resource three files down.

### What a durable suspension needs, and why it is not in this commit

The etcd key does not belong inside a module gated on whether a cluster
exists. A KMS key cannot really be deleted — destroying one schedules its
versions and leaves the name claimed for ever, which is the whole argument of
`scripts/terraform-undeletable.py` — so it outlives the cluster by nature and
should outlive it in the configuration too. Moving it to the root beside the
ring, with a `moved` block so the existing key is not destroyed and recreated,
would let `gitops_enabled = false` close cleanly and make the suspension
declarative.

That is a module interface change (`etcd_key_id` in, the resource and its
output out, the `database_encryption` reference and the service-agent grant
repointed) plus a `moved` block that has to be exactly right against live
state, and it is named here rather than done in a hurry at the end of a long
session. Until it lands, the suspension is an act rather than a posture.

**The cost of that, stated plainly: a later plain `up` recreates the cluster
and the meter restarts**, because the configuration still says there should be
one. That is the same contract `down` and `up` already have for the execution
nodes. The tfvars comment beside `gitops_enabled` and the `suspend` step's own
comment both say it, so the next reader meets the caveat wherever they arrive.

**Why this is not a cost decision dressed as a security one, and not the
reverse.** Two independent facts point the same way and either alone would
be weaker:

* The cluster is the dev estate's entire recurring meter. A regional GKE
  Autopilot cluster bills a control-plane fee whether or not a Pod runs,
  and Autopilot bills the system workloads it schedules on top. Nothing
  else in the 238 resources bills at that order: the Cloud Run services do
  not exist, `execution_nodes = {}` in every environment so
  `module.execution_node` is empty, and the remainder is KMS key versions,
  fourteen Secret Manager secrets, a handful of DNS zones, an Artifact
  Registry and a VPC — cents against dollars.
* The cluster cannot do its job and cannot be made to. Keeping it running
  buys no capability today and none tomorrow, because what unblocks it is
  a release somebody else has not cut.

Neither fact makes the other true. Together they mean the suspension costs
nothing but the ten minutes an `up` takes to rebuild.

**Why a targeted destroy and not a teardown.** `teardown` is the owner's act
by name (ADR 0040 decision 13) and takes resources out of state that no `up`
can put back without the reclaim step. One target destroys exactly the
resource that bills and leaves everything else in state, where the next `up`
finds it.

**The `deletion_protection` half, and why it is two applies.** The literal
in `modules/gitops-control-plane/main.tf` is `true`, and the provider reads
it *from state* at delete time — the module's own comment records that a
commit on 2026-09-13 got this wrong for about an hour. So the sequence is:
flip the literal to `false` and apply, which is an in-place update that
writes `false` into state; then `suspend`, which destroys the cluster; then
restore the literal to `true`, so the cluster the next `up` creates is
protected from its first minute. Run 53's `up` step did the first half and
its log carries the evidence —
`~ deletion_protection = true -> false` and
`Apply complete! Resources: 0 added, 1 changed, 0 destroyed`
(<https://github.com/droderiquesit/quantum-ai-platform/actions/runs/35530874039>).
The flag **does not become a module input**. An input is a switch a later
tfvars can throw with nobody deciding anything, which is what
`a_cloud_run_service_cannot_be_deleted_by_a_plan_nobody_read` refuses in
`modules/cloudrun`; a literal flipped and restored leaves no permanent
switch behind.

## What it costs

* **Nothing in dev deploys, and this record does not change that.** It was
  already true before the suspension and is true after it. What the
  suspension removes is the bill, not a capability — but a reader who finds
  this record while looking for why nothing is served must not conclude the
  suspension is the cause. The cause is the scan gate, and the gate is
  right.
* **The GitOps path goes unexercised.** Every day the cluster is down is a
  day the bootstrap, the Argo CD project, the Kargo chain and the post-sync
  proving hook are not run against a real cluster, so drift between the
  manifests and what a cluster would accept accumulates silently. The
  `gitops` and `manifest_wiring` acceptance suites still run and still pin
  the shapes; they cannot catch an API version Google retires.
* **The first `up` after reinstatement is the long one.** Creating an
  Autopilot cluster and waiting for its nodes is the thirty-to-forty minute
  step that run 37 failed inside. That risk is re-taken on reinstatement.
* **The suspension is not durable against a routine `up`.** `gitops_enabled`
  stays `true`, so the configuration still says there should be a cluster and
  the next plain `up` builds one. This is the price of not weakening the etcd
  key's guard, and it is the one cost here that a reader could be caught by
  rather than merely inconvenienced by — so it is written in three places:
  here, in the tfvars beside the flag, and in the `suspend` step itself.
* **The module's other resources stay in state and stay declared**, which is
  the point of targeting one address, but it means Terraform still believes
  in three controller identities and two registry DNS zones whose cluster is
  gone. They cost cents and they are what makes the rebuild a plain create.

## What would make this wrong

Any one of these, and the change is one `up` dispatch:

1. A published Argo CD image whose bundled kustomize reports `go1.24.13` or
   above (or `go1.25.7`, or `go1.26.0-rc.3`). **Re-check by measurement and
   not by a release note**: a kustomize layer whose size has moved off
   12,660,920 bytes is the signal that it was rebuilt at all.
2. An owner decision under remedy (2) above — building kustomize from
   source — which needs its own record, because it changes what "vendored"
   means in this repository.
3. Any work that needs a real cluster in front of it, at which point the
   cost is being paid for something.

## Rejected

* **Pin v3.5.3.** Measured; identical binaries. It would have read as
  remediation in the log and changed nothing.
* **A Trivy exception, an `.trivyignore`, or a per-image acknowledgement
  file.** `vendor.yml`'s own comment records that the acknowledgement file
  which used to exist acknowledged findings in exactly these bundled tools,
  and that "an acknowledgement written for one build silently covering the
  next is how a suppression file becomes a blindfold". The finding is very
  likely unreachable here — Argo CD renders kustomize overlays from a
  committed tree and terminates no TLS session with either binary — and
  that is precisely the argument that writes the exception. It is refused.
* **Move Argo CD below the other seven lines in the list so they mirror
  first.** It would get cert-manager's images into the registry and leave
  the bootstrap failing one step later, at Argo CD itself, having spent a
  change on the appearance of progress.
* **Reverse ADR 0036 and return Cloud Run services to Terraform
  resources.** This is the option that would unblock deployment outright
  rather than pausing the meter, and it is not rejected on its merits —
  it is deferred because it is large. The `gitops` and `manifest_wiring`
  suites, the `removed` blocks, the `RunService` manifests, the prove-
  serving hook and the per-workload overlays all pin the current shape.
  If the upstream blocker outlives this record by long enough to matter,
  that is the decision to take, and it needs its own ADR and its own lane.
* **Leave it running and say nothing.** The estate would keep billing for a
  cluster whose controllers cannot install, and the next reader would find
  a green apply and a failed run and draw the wrong conclusion from each.
* **Remove `prevent_destroy` from the etcd key, or make it an input.** It is
  the one-line change that would have made `gitops_enabled = false` work, and
  it is the reason this ADR has a section about a refused plan instead. The
  guard's false positive costs a dispatch; its absence costs an etcd nobody
  can read, discovered at the next control-plane restart.
* **`terraform state rm` the etcd key before applying, the way `teardown`
  does.** `teardown` is allowed to do that because it is a named act with a
  human behind it and a reclaim step that puts the object back. Generalising
  it into `up` — "quietly drop any protected resource this plan would
  destroy" — turns every `prevent_destroy` in the tree into a comment.
