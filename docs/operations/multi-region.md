# Multi-region

**Written for the retired runtime.** Node pools, overlays and the cluster topology this page reasons about were retired under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md). The finding still holds — a second region is a property of the binaries, not of configuration — and the shape is now one execution node per region from `execution_nodes`, with the central plane on Cloud Run in one region. Rewriting the evidence for that shape is open work.

**A second region is not a set of manifests.** The central plane cannot be run
twice, and the cells are not in the regions they name. Both are properties of
the binaries and the cluster topology, not of the YAML, and no overlay
structure closes either. That is the finding; the rest of this page is the
evidence for it and what the available shape actually costs.

## What is configured today

| | |
| --- | --- |
| Clusters | **One.** `modules/cluster` creates one regional cluster at `var.region`, and `var.region` is a single scalar per environment. All four environment files name `europe-west2`. |
| Cells | **Nine specified, none applied.** Each `edge_cells` entry gets a subnet, a service account, a workload identity binding and egress firewall rules *in its own region* — and no compute. |
| Manifests | **One set.** `infrastructure/kubernetes/base`, with `CELL_ID`, `CELL_REGION` and `CELL_VENUES` substituted at apply time. |

## `CELL_REGION` was a label, not a placement

Until now `CELL_REGION` reached exactly one place that mattered:
`QIP_CELL_REGION`, a string the cell reports about itself. Nothing made it a
statement about where the pods run.

A GKE node pool belongs to its cluster's location. There is no cross-region
node pool, so a cell whose region is not this cluster's region has nowhere in
this cluster to correctly be — and the scheduler, asked to place it anyway,
puts it on the central plane's own nodes. It comes up healthy. It serves
`/health`, it reports `QIP_CELL_REGION` as a region it is not in, and the
central plane's registry records a cell sitting next to a venue on another
continent.

A cell's entire argument is source-adjacency. That is the argument being false
while every indicator says it is true, which is the worst shape this class of
error takes.

`edge-cell.yaml` now carries a `nodeSelector` on
`topology.kubernetes.io/region`, set from the same `CELL_REGION`. It creates no
nodes and fixes nothing — what it does is stop the wrong answer from looking
like the right one:

| cell | region | applied to a `europe-west2` cluster |
| --- | --- | --- |
| `london-1` | `europe-west2` | schedules, as before |
| the other eight | elsewhere | `Pending`, which is the truth |

`Pending` is the correct state for a workload with no nodes.
`docs/operations/external-dependencies.md` already records that
`modules/edge-cell` creates no node pool; what it did not record is that the
pods land somewhere anyway.

> A consequence for the cell runbook: step 8 of
> [deploying-an-edge-cell.md](deploying-an-edge-cell.md) asks you to confirm
> that the node starts, serves `/health` and opens no venue connection. For any
> cell outside the cluster's region that confirmation is now unreachable by
> construction — the pod does not start. That step needs a line, and adding it
> was outside the scope of the change that added the selector.

## The two halves of a cell's egress disagree about topology

`allow-edge-egress` in `edge-cell.yaml` permits the central plane like this:

```yaml
    - to:
        - podSelector:
            matchLabels:
              app: qip-api
```

A NetworkPolicy `podSelector` resolves only inside its own cluster. The
Terraform half of the same rule — `google_compute_firewall.central_plane` in
`modules/edge-cell` — does the same job in CIDRs, from
`local.central_plane_ranges`: the central subnet, the central pod range, and
the private Google API range. That is the form that works between clusters.

So the pod policy is written for a topology where the cell shares a cluster
with the central plane, and the firewall is written for one where it does not.
Whichever a real cell turns out to be, one of the two halves is wrong.

**It fails quietly in the direction that matters.** A cell that cannot reach
the central plane keeps trading inside the envelope it already holds — that is
deliberate, it is why the policy permits the centre rather than requiring it —
so "the cell lost the centre" does not announce itself as an outage. It
announces itself as a cell whose reports stop arriving, which is also what a
cell that is merely busy looks like to the registry.

This page does not fix it, on purpose. Which form is correct depends entirely
on the next section, and writing both rules now would grant the wider one
today for a topology nobody has chosen yet.

## The central plane cannot be run twice

Active-active means two central planes. Read the two brain manifests before
believing that is a deployment question.

`qip-fastbrain` runs **one** replica with `strategy: Recreate`, and the reason
is written at the top of its file: two replicas are two independent feed
consumers building two divergent world models, and — pointed at a durable
`QIP_STORAGE_TARGET` — two writers handing records to one `ChainArchive`,
which assigns its own dense positions as they arrive. Two independent event
sequences interleave into a single chain that still *verifies*, every entry
linking to the one before it, while describing a history that never happened.
A chain that verifies and is wrong is worse than one that fails.

`qip-deepbrain` runs one replica for the same reason, more slowly.

A second region is a second replica with a hundred milliseconds between them.
Distance does not weaken that argument; it removes the last thing that might
have arbitrated. There is no leader election anywhere in this repository — no
lease, no fenced hand-off, no rule for the cycle in flight when leadership
moves — and `scaling-and-availability.md` already says that no manifest change
substitutes for it.

`qip-api` is the one workload that could serve from two regions. What it cannot
do is hold one audit trail from two of them: its rate-limit counters, its cell
registry, its event index and its hash chain are all per process. The foot of
`api.yaml` now works through what a second copy of that chain costs, and the
answer does not improve when the second copy is in Tokyo.

**So it is one question, not two.** The property that makes each brain a
singleton is the property that makes the central plane single-region, and it is
closed in the binaries or not at all.

## Why there is no overlay directory

Because an `overlays/europe-west2` and an `overlays/asia-northeast1` would render
as though a second central plane were a deployment target, and it is not. A
reviewer reads directory structure as a claim about what runs.

`infrastructure/kubernetes/overlays` used to exist and was empty, and the
acceptance suite now pins what made it worth removing: every manifest must live
in the one directory the deploy pipeline renders, because a manifest outside it
is a set of resources a reviewer reads as deployed and that nothing applies. An
overlay tree with a second central plane in it would be that failure with the
stakes raised — not a manifest nothing applies, but a topology nothing can run.

## What active-passive would take

Active-passive is the shape that is actually available: a second region that
runs nothing until a failover. It is still not small.

1. **A second cluster.** `var.region` is one scalar per environment; there is
   no place in the variable surface to name a second region for the central
   plane. Terraform, not YAML.
2. **A chain the second region can read.** Everything in the foot of
   `api.yaml`. A passive region that comes up with an empty archive has not
   failed over, it has started a new audit trail beside the old one.
3. **Journal snapshots restorable there.** Compute Engine snapshots are
   restorable across regions, so this one is closest to done — see
   [disaster recovery](disaster-recovery.md) for the attachment step and what
   is still missing above it.
4. **The regional resources underneath.** The Artifact Registry, the evidence
   bucket and the etcd CMEK are all created at `var.region`. A second cluster
   pulling images across regions works and is slower; a KMS key is regional and
   a second cluster needs its own.
5. **A decision about what failover means here**, which is the one that is not
   infrastructure. [Disaster recovery](disaster-recovery.md) is unambiguous
   that positions are reconciled from the venue and never restored from a
   backup, and after a regional failover *every* position is in that state. So
   a passive region does not come up trading. What it comes up as is somewhere
   to stand while reconciling — which is worth having, and is a much smaller
   claim than active-active.

## The claim this corrects

`docs/architecture/current-state-audit.md` records multi-region deployment as
Partial, and says instantiating the nine cells is "credentials, an `apply`, and
the venue address ranges that `venues = {}` currently, correctly, declines to
guess."

It is also nodes — and nodes in eight of the nine regions is eight more
clusters, or eight of something that is not a GKE node pool. Three of the nine
cannot be a Google region at all: Chicago, NY/NJ and Dubai have none within
several hundred kilometres, which
[deploying-an-edge-cell.md](deploying-an-edge-cell.md) already names as an
architectural gap with three honest answers.

Correcting the audit is that document's own change to make. It is recorded here
so the two do not quietly disagree in the meantime.

## Related

* [Scaling and availability](scaling-and-availability.md) — why each brain is a
  singleton, which is the same refusal this page reaches.
* [Deploying a new edge cell](deploying-an-edge-cell.md) — the nine cells, the
  three that are not in their metro, and the runbook the selector affects.
* [Disaster recovery](disaster-recovery.md) — what is irreplaceable, and the
  journal snapshots.
* [External dependencies](external-dependencies.md) — the standing list,
  including that edge cells have no nodes.
