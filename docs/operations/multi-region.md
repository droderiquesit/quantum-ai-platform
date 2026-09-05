# Multi-region

**A second region is not a set of configuration.** The central plane cannot be
run twice, and that is a property of the binaries, not of the Terraform. The
shape under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)
is the central plane on Cloud Run in one region and one execution node per
region from `execution_nodes`. That is the finding; the rest of this page is
the evidence for it and what the available shape actually costs. None of it
has been applied.

## What is configured today

| | |
| --- | --- |
| Central-plane regions | **One.** Every Cloud Run service in the catalogue is created at `var.region` (`infrastructure/terraform/catalogue.tf:241`), and `var.region` is a single scalar per environment (`variables.tf:30-34`). `dev` names `us-east4`; `test`, `stage` and `prod` name `europe-west2` (`environments/*/terraform.tfvars`). |
| Execution nodes | **None.** `execution_nodes = {}` in all four environments. Each entry, when one exists, carries its own `region` and `zone` (`variables.tf:256-267`), and `modules/execution-node` creates the subnet and the group there. |
| Trust zones | Each declared with the region its subnet lives in (`variables.tf:155-174`). |
| The regional resources underneath | The registry, the evidence bucket, the data and AI resources and the KMS ring are all created at `var.region` (`main.tf:190-199,350-368,370-392`). |

## A node's region is now where it runs

On the retired runtime the region a cell reported was a label, and a cell whose
region had no compute was scheduled onto the central plane's own machines,
came up healthy, and reported a region it was not in. That failure cannot take
that shape here. The startup script writes `QIP_CELL_REGION` from the node
entry's own `region` (`modules/execution-node/templates/startup.sh.tftpl:149`),
and the same value is where the subnet is created (`main.tf:109-115`) and
where the zone must be (`variables.tf:45-48`, refused otherwise). The label and
the placement are one value.

What a node still cannot be is *next to* a venue Google has no region near:
Chicago, NY/NJ and Dubai have none within several hundred kilometres, which
[deploying-an-edge-cell.md](deploying-an-edge-cell.md) names as an
architectural gap with three honest answers.

## The node's route to the centre is one rule, and nothing speaks over it

The node's egress to the central plane is `google_compute_firewall.central_plane`
in `modules/execution-node/main.tf:522-539`, in CIDRs from
`local.central_plane_ranges` — the subnets of the trust zones the catalogue's
workloads attach through, plus the private Google API range
(`infrastructure/terraform/main.tf:128-135`). Derived from the catalogue and
the declared zones rather than typed, so a zone that moves moves the rule.
There is no second, cluster-local form of the rule any more.

The rule permits a conversation nothing holds. The in-tree mesh binds one
listener per cell on its own port and a Cloud Run service publishes exactly
one, so `QIP_MESH_CELLS` is unset on the API and `QIP_MESH_PEER` on the node
(`catalogue.tf:21-27`; ADR 0024, "What it costs"): the API answers
`available: false` and a node starts detached. When the mesh is served,
`QIP_MESH_REGIONS` (`region=grant:cell,cell;…`) on the API files every served
cell under one region so the centre ships each cell a disjoint share of its
region's grant (ADR 0039); left unset, every live grant ships to every cell
and each cycle says so beside the payload.

**It fails quietly in the direction that matters.** A node that cannot reach
the central plane keeps trading inside the envelope it already holds — that is
deliberate; the rule is a path the node uses, not one it depends on
(`main.tf:518-521`) — so "the node lost the centre" does not announce itself as
an outage. It announces itself as a node whose reports stop arriving, which is
also what a node that is merely busy looks like.

## The central plane cannot be run twice

Active-active means two central planes. Read the catalogue before believing
that is a deployment question.

`qip-fastbrain` runs at `concurrency = 1` because it holds per-process state:
one instance's cycle is its own (`catalogue.tf:123-124`). Two instances are two
independent feed consumers building two divergent world models, and — pointed
at a durable `QIP_STORAGE_TARGET` — two writers handing records to one
`ChainArchive`, which assigns its own dense positions as they arrive. Two
independent event sequences interleave into a single chain that still
*verifies*, every entry linking to the one before it, while describing a
history that never happened. A chain that verifies and is wrong is worse than
one that fails.

`qip-deepbrain` runs at `concurrency = 1` for the same reason, more slowly
(`catalogue.tf:171`).

Note what `concurrency = 1` bounds and what it does not: requests per
instance, not instances. The catalogue passes no instance ceiling, so every
service takes `modules/cloudrun`'s defaults of `min_instances = 0` and
`max_instances = 4` (`modules/cloudrun/variables.tf:242-283`). Two brain
instances in one region is a configuration the Terraform admits today;
[scaling and availability](scaling-and-availability.md) says what that costs.
A second region is that failure with a hundred milliseconds between the
copies, and distance removes the last thing that might have arbitrated. There
is no leader election anywhere in this repository — no lease, no fenced
hand-off, no rule for the cycle in flight when leadership moves.

`qip-api` is the one workload that could serve from two regions. What it cannot
do is hold one audit trail from two of them: its rate-limit counters, its cell
registry, its event index and its hash chain are all per process, and the
answer does not improve when the second copy is in Tokyo.

**So it is one question, not two.** The property that makes each brain a
singleton is the property that makes the central plane single-region, and it is
closed in the binaries or not at all.

## Why there is no second region variable

Because a `region_secondary` that the catalogue also deployed to would render
as though a second central plane were a deployment target, and it is not. A
reviewer reads a variable as a claim about what runs. The one scalar is the
honest surface: an environment is one project (`environments/dev/terraform.tfvars:12-14`)
in one region, and a node elsewhere is an entry in a map, not a second plane.

## What active-passive would take

Active-passive is the shape that is actually available: a second region that
runs nothing until a failover. It is still not small.

1. **A second root or a second region variable.** `var.region` is one scalar
   per environment; there is no place in the variable surface to name a second
   region for the central plane. Terraform, not an edit to a running service.
2. **A chain the second region can read.** With `storage_target = "memory"`
   in every environment there is no chain to read at all
   (`variables.tf:302-321`). A passive region that comes up with an empty
   archive has not failed over, it has started a new audit trail beside the
   old one.
3. **Journal snapshots restorable there.** `modules/backup` leaves
   `storage_locations` unset so snapshots land in the nearest multi-region and
   survive the region (`modules/backup/main.tf:99-102`). This one is closest to
   done — and covers nothing until the attachment step in
   [disaster recovery](disaster-recovery.md) has been run.
4. **The regional resources underneath.** The registry, the evidence bucket and
   the KMS ring are all at `var.region`. Pulling images across regions works
   and is slower; a KMS key is regional and a second region needs its own.
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

It is also machines: one `execution_nodes` entry per region, each needing a
boot image nothing in this repository builds
(`modules/execution-node/README.md`, "No image bake exists") and a venue range
nobody has recorded. Three of the nine cannot be a Google region at all.

Correcting the audit is that document's own change to make. It is recorded here
so the two do not quietly disagree in the meantime.

## Related

* [Scaling and availability](scaling-and-availability.md) — why each brain is a
  singleton, and what the catalogue does and does not pin.
* [Deploying a new edge cell](deploying-an-edge-cell.md) — the nine cells, the
  three that are not in their metro, and the node entry.
* [Disaster recovery](disaster-recovery.md) — what is irreplaceable, and the
  journal snapshots.
* [External dependencies](external-dependencies.md) — the standing list; its
  edge-cell section predates ADR 0024.
