# 0020 — Two runtime topologies exist, and the order in which they are resolved

**Status:** accepted — **the direction is now decided** by ADR 0022, which
makes the Algorik blueprint the architecture of record. The sequence below is
therefore the order the migration takes rather than the order it would take.

**Execution remains unauthorised.** A decision about direction is not approval
to execute any step. Every step still requires recorded human approval naming
that step before it begins, exactly as written below; nothing in the direction
being settled changes that, and nothing has been migrated, decommissioned or
provisioned.

## Context

The Algorik Master Blueprint v10.1-4 (§41.4, §41.6, §45.1) describes a runtime
this repository does not have, and this repository has a runtime the blueprint
does not mention.

The blueprint's target is two things and nothing else: roughly seventy Rust
binaries on Cloud Run and Cloud Run Jobs, every one of them scaling to zero,
and a single dedicated Compute Engine machine per region — a C3 or C3D running
the execution node bare under systemd, with no container runtime, no external
IP and `isolcpus` on cores 2 through 15. There is no Kubernetes in it. There is
no service mesh, no Argo CD, no Kargo, no regional VPC peering, and the words
do not appear.

What is in the tree, verified by reading it rather than inferred:

- The Rust platform runs on GKE. `infrastructure/helm/qip/templates/` carries
  `api.yaml`, `fastbrain.yaml`, `deepbrain.yaml`, `edge-cell.yaml` and eight
  more, and ADR 0011 is the decision that put them there.
- Argo CD is the only writer to the cluster, and Kargo promotes between
  environments. ADR 0017 is that decision, taken deliberately, and commits
  `e3c972c`, `69a6287` and `d8b3597` are the cut-over that finished it.
- The frontends already run on Cloud Run. `infrastructure/docker/portal.Dockerfile`
  opens "The portal on Cloud Run", and `google_compute_subnetwork.console_egress`
  in `modules/network/main.tf` provisions the subnet Cloud Run attaches the
  console to for direct VPC egress.
- There is no Compute Engine instance anywhere. When this record was written,
  `grep -rln google_compute_instance infrastructure/` returned nothing; the
  correction below records what it returns now, and that the conclusion
  holds. The blueprint's one permitted VM — the execution node, the piece the
  whole latency argument rests on — does not exist as infrastructure;
  `edge-cell.yaml` runs that workload as a Kubernetes Deployment instead.

So the topology today is already hybrid, and it is hybrid in the opposite
direction from the target: the part the blueprint says must be a bare VM is a
pod, and the part the blueprint says should be Cloud Run is partly Cloud Run
and partly GKE.

## Decision

**Nothing is deleted, and the conflict is recorded rather than resolved by
whoever edits next.**

Three things follow, and they are the whole decision:

0. **The paper-trading boundary is out of scope for this decision.** ADR 0003
   and ADR 0021 govern what the platform may connect to; this record governs
   only where the software runs. No step below changes the former, and a
   migration is never a reason to reopen it.

1. **ADR 0011 and ADR 0017 stand as descriptions of what runs, and are
   superseded in direction by ADR 0022.** When this record was written they
   stood entirely, on the argument that a blueprint is not an ADR and this
   repository's architecture is what its decision records say it is. That
   argument was right and has now been answered the only way it could be: an
   owner turned the blueprint into a decision record. What has not changed is
   that a reader who finds the blueprint and the cluster disagreeing should
   find this file rather than guess which one is stale — the difference is that
   the answer is now "the cluster is transitional" rather than "nobody has
   decided".

2. **The resolution order is fixed even though the resolution is not.** If the
   platform does migrate, it migrates in this sequence, because each step's
   evidence is what makes the next one safe.

   **Every step below requires recorded human approval naming that step before
   it begins.** The evidence column says what must be true for approval to be
   *sought*; it is never itself the authorisation. That distinction is the
   point of this paragraph: an agent can gather step 1's evidence entirely on
   its own, and an ordering that gated on artefacts alone would let it read
   its own output as permission and provision compute that holds venue
   sessions with no person having decided anything. Evidence earns a
   conversation, not a machine.

   | # | Step | Why it is here | Evidence that closes it |
   |---|---|---|---|
   | 1 | Establish which GKE workloads have ever actually run | A decommission plan written against manifests rather than against running pods removes something that was load-bearing | A named cluster, a pod list, and a scrape |
   | 2 | Move one scale-to-zero warm service to Cloud Run, running both | The cheapest reversible proof that the identity, secret and egress path works off GKE | The same request served by both, and the Secret Manager CSI equivalent proven on Cloud Run |
   | 3 | Stand up one execution node on GCE C3 in shadow mode | §41.4 requires it and nothing in the tree provisions it. Shadow mode first, because a node that takes venue sessions before it has been observed is the failure this whole ordering exists to avoid | A node holding sessions, quoting nothing, matching the pod's decisions |
   | 4 | Cut the remaining warm services over, one at a time | | Per service |
   | 5 | Only then retire the Helm chart, Argo CD and Kargo | | Two consecutive weeks with no traffic on the GKE path |

   Steps 3 through 5 are Phase 16 work in the blueprint's own roadmap and are
   not started here.

   **Step 3 is bounded by the paper-trading rules and does not relax them.**
   "Holding venue sessions" in that row means what it means everywhere else in
   this repository: the simulated broker or a provider sandbox. ADR 0003, ADR
   0021 and `.claude/rules/01-security-and-safety.md` govern it, a migration
   changes none of them, and moving a workload from a pod to a VM is not an
   occasion to revisit what that workload may connect to. Nothing in this
   sequence authorises a live venue, and no step of it may be used as the
   reason to.

3. **Neither topology may become undocumented.** While both exist, both are
   described. That is what this file is for.

## What it costs

Carrying two topologies costs real money and real attention. Two delivery
paths must both keep working; a change to the api's configuration has to be
made twice, once in `values.yaml` and once wherever Cloud Run reads it; and
the identity model differs — Workload Identity on GKE and a Cloud Run service
identity — so a permission granted in one place is not granted in the other,
and the failure looks like a bug rather than a missing grant.

Recording the conflict rather than resolving it also costs the thing every
deferred decision costs: somebody will read the blueprint, not find this file,
and start migrating. The index entry is the mitigation and it is a weak one.

The alternative was worse. Deleting the GitOps cut-over three commits after it
landed, on the authority of a document that names no migration path and no
cutover evidence, would have removed a working delivery mechanism and left the
platform with neither.

## What would make this wrong

- **An execution node running on GCE C3 in shadow mode, approved and
  observed.** At that point the blueprint's central topological claim is real,
  GKE is carrying the part it was never meant to carry, and step 5 becomes
  overdue rather than premature.

  This row is about a node that *has been approved into existence*, not a node
  somebody may build. It is not a licence to create one in order to satisfy it
  — a reversal condition that could be brought about unilaterally by the
  reader is not a reversal condition, it is an instruction.
- **The blueprint being adopted as the repository's architecture of record.**
  It has not been. If an owner decides it is, ADR 0011 and ADR 0017 must be
  superseded explicitly and this file becomes the migration plan rather than a
  conflict record.
- **A second warm service being written for Cloud Run without step 2's
  evidence.** That is the drift this ordering exists to prevent: two
  permanent runtimes acquired one convenient exception at a time.

### Corrections recorded after acceptance

Two facts in this record have been overtaken by the tree, and one step of
the sequence has a consequence the sequence did not state. None of them
changes the decision, the direction or the authorisation rule; they are
recorded here so that a reader who checks the record against the tree finds
the difference explained rather than concluding the record is stale.

- **The Compute Engine grep is no longer empty, and the conclusion still
  holds.** Commit `6cde5d7` added `modules/execution-node/`, which declares a
  `google_compute_instance_template` and a `google_compute_instance_group_manager`
  for the §41.4 node. No `google_compute_instance` resource exists, and the
  root module does not call that module — `infrastructure/terraform/main.tf`
  wires neither `execution-node`, `cloudrun` nor `trust-zones` — so no plan
  this repository can produce provisions a VM. The node exists as an unwired
  module, which is a different fact from not existing and a different fact
  from step 3 having begun; step 3 has not begun, and an unwired module is
  not the evidence that row asks for.
- **Step 5 as written removes the egress path of every migrated service.**
  The platform's HTTP client speaks plaintext HTTP/1.1 by design and relies
  on a TLS-terminating egress proxy in front of it. That proxy is described
  only by the Helm chart (`infrastructure/helm/qip/templates/egress.yaml`, and
  its byte-identical copy under `infrastructure/kubernetes/base/`). The Cloud
  Run module routes all egress into the VPC and terminates TLS for nothing,
  so a service moved off GKE has no outbound HTTPS unless an equivalent
  exists off GKE — and retiring the chart at step 5 would then take the
  proxy away from every service already migrated in steps 2 and 4. An egress
  path that does not depend on the chart is therefore a precondition of step
  5, and of step 2's evidence, that the table above does not name. What that
  path is — a proxy on Cloud Run, a Cloud Run service reaching a vendor
  directly, or something else — is an owner decision this correction
  deliberately does not make.
- **The second reversal condition above has occurred.** ADR 0022 adopted the
  blueprint as the architecture of record after that bullet was written;
  the status line and "What was open, and what still is" already say so,
  and the bullet is kept as written so the condition and its outcome can
  both be read.
- **The proxy is not merely Kubernetes-only; it is not running.** In both
  manifest copies the `ServiceAccount` and `Deployment` are committed
  commented out, so Argo CD renders the chart and no proxy pod exists.
  Commit `64b765a` made the egress suite say so rather than uncomment the
  pod before reading it. Step 2's "egress path works off GKE" cannot be
  proven by comparison with a GKE path until one exists to compare against.

## What was open, and what still is

**The direction was open and is now closed.** The question was whether the
platform adopts the blueprint's Cloud Run and bare-metal topology or keeps GKE
and treats §41.4 and §41.6 as describing a system this one deliberately is not.
The owner has adopted the blueprint (ADR 0022). GKE, Argo CD, Kargo, Helm and
KEDA are the transitional runtime; the sequence above is the route.

**What is still open is every single step of it.** Direction and authorisation
are different decisions and this record deliberately keeps them apart, because
conflating them is how a settled destination becomes an agent provisioning
compute nobody asked for. Concretely:

- No step below may be started without recorded human approval naming that
  step. The evidence in each row is what makes it reasonable to *ask*; it is
  never the answer.
- Nothing is removed. ADR 0011 and ADR 0017 still govern what runs, and the
  Helm chart is retired at step 5 and not before — on that step's evidence,
  with approval, after two consecutive weeks of no traffic on the GKE path.
- The paper-trading boundary is untouched by all of it. Step 3's "holding
  venue sessions" means the simulated broker or a provider sandbox, as it does
  everywhere else in this repository.

An agent asked to "make progress on ADR 0020" still has exactly one correct
action available: gather step 1's evidence — which GKE workloads have ever
actually run — and bring it to an owner. That has not changed, and the
direction being decided is not a reason for it to.

### Correction recorded at ADR 0024

The sequence above was executed in code, not step by step and not on the
per-step approvals this record required. The owner's instruction — "create
the new infrastructure while devouring the old", quoted in full in
[ADR 0024](0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
— superseded the per-step approval rule **for the code**: the Cloud Run
catalogue, the execution-node module and the trust zones are wired into the
root module, the cluster's Terraform is removed, `deploy.yml` moves Cloud Run
services, and the chart, the manifests, Argo CD, Kargo and KEDA are gone from
the tree. ADR 0024 maps each commit onto the step it corresponds to.

Three things did not change. Step 1's evidence — which GKE workloads ever
actually ran — was never gathered. Nothing was applied: no terraform, gcloud
or kubectl binary existed on the machine that made the commits, and the
first plan against a project that ran the cluster is a human's to read
before anything is destroyed — the approval rule stands unchanged for that.
And the paper-trading boundary is exactly as this record's point 0 left it.

The egress precondition recorded in the corrections above is met in code:
`modules/egress-proxy` renders the one committed bootstrap as a loopback
sidecar and as the node's unit, so retiring the chart took no proxy away
from anything — there was never one to take. ADR 0011 and ADR 0017 are now
superseded rather than superseded in direction.
