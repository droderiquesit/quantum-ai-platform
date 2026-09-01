# 0020 — Two runtime topologies exist, and the order in which they are resolved

**Status:** accepted — records a conflict and the sequence for resolving it.
The resolution itself is not yet decided; see "What is still open".

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
  opens "The portal on Cloud Run", and `modules/network/main.tf:143-150`
  provisions the subnet Cloud Run attaches the console to for direct VPC
  egress.
- There is no Compute Engine instance anywhere. `grep -rln
  google_compute_instance infrastructure/` returns nothing. The blueprint's
  one permitted VM — the execution node, the piece the whole latency argument
  rests on — does not exist as infrastructure; `edge-cell.yaml` runs that
  workload as a Kubernetes Deployment instead.

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

1. **ADR 0011 and ADR 0017 stand.** They are not superseded by a document that
   arrived later, because a blueprint is not an ADR and this repository's
   architecture is what its decision records say it is. A reader who finds the
   blueprint and the cluster disagreeing should find this file rather than
   guess which one is stale.

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

## What is still open

**This requires an owner decision that no agent may take.** The question is
whether the platform adopts the blueprint's Cloud Run and bare-metal topology,
or keeps GKE and treats §41.4 and §41.6 as describing a system this one is
deliberately not. Both are defensible; they are not both true; and the cost of
choosing is far smaller than the cost of drifting.

Until that decision is recorded here, **no step of the sequence above may be
started.** The sequence fixes the order migration would take *if* it is
approved; it authorises nothing on its own, and "the only permitted direction
of travel" describes the shape of a future decision rather than granting it.

An agent asked to "make progress on ADR 0020" has, at this moment, exactly one
correct action available: gather step 1's evidence — which GKE workloads have
ever actually run — and bring it to an owner. Everything after that needs a
person, and the paper-trading boundary is untouched by all of it.
