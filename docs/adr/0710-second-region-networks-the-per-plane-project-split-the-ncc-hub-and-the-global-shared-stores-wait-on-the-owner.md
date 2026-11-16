# ADR 0710: The per-plane project split, the NCC hub, per-region networks and the global managed stores each wait on a named owner input, and nothing is built here

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-GCP-10001, R12-GCP-10009, R12-GCP-8036, R12-GCP-8037

## Context

Four rows ask for structure that only pays once there is more than one region or
a funded managed store, and the owner ruled on each on 2026-10-31.

- R12-GCP-10001 wants folders and projects that isolate every environment and
  plane. ADR 0118 (Accepted) fixes one project per environment under a platform
  folder. The per-plane split exists only as the uninstantiated
  `modules/resource-hierarchy`, and creating it needs an organisation id and
  billing account that only the owner holds.
- R12-GCP-10009 wants a Network Connectivity Center hub and spokes with route
  filters. ADR 0035 (one execution node, one region) and a guard in
  `gcp_placement.rs` refuse a hub before a second region. The parked patch
  `l6-ncc-hubs-parked.patch` filters its latency spokes but not its star spokes.
- R12-GCP-8036 wants a Reflex, Fabric and Service network, a landing bucket,
  collectors and egress per execution region. The network module and NAT exist
  for the one region; the landing bucket waits on `enable_cloud_storage`, which
  is false in all four tfvars. Its note also says a Service cluster conflicts
  with ADR 0024, where Cloud Run serves the service plane.
- R12-GCP-8037 wants the ledger, registry, catalog and identity services as
  single global resources. Identity and the artifact registry exist. The Spanner
  ledger, a Knowledge Catalog and a managed model registry are unfunded
  (owner decision 5).

## Decision

1. **R12-GCP-10001: owner.** ADR 0118 stands, one project per environment. The
   per-plane clause is not reworded away. When the owner supplies an
   organisation id to `scripts/bootstrap-foundation.sh`, a later record amends
   ADR 0118 to wire `modules/resource-hierarchy` into the foundation root and
   migrates one plane at a time, each with a plan shown first. Until then the
   module stays unused by any root and the row stays PARTIAL.
2. **R12-GCP-10009: owner.** No hub, spoke or peering is declared now. The hub
   waits for the record that supersedes ADR 0035 with a second region and names
   the hub's monthly cost (R12-GCP-8035, R12-GCP-8057). Hybrid spokes also wait
   on an ordered circuit (R12-GCP-8059). When the hub lands, every spoke,
   including star spokes, must carry an include or exclude range, and a
   validation must refuse an unfiltered spoke; a test feeding a filterless star
   spoke must be seen to fail first. The `gcp_placement.rs` guard moves only in
   that same record. This decision supersedes nothing.
3. **R12-GCP-8036: figure.** The figure needed is the list of execution regions
   and the monthly ceiling for them, to come from R12-GCP-8035. No figure is
   chosen here. ADR 0035 stands: one region and no second one declared or
   funded. In that region the three networks and controlled egress are the
   existing build. The landing bucket is not enabled until `enable_cloud_storage`
   is set by an owner-approved change and a writer exists for it. Whether a
   per-region OpenObserve (ADR 0028) or the in-process push (ADR 0121) meets
   "regional collectors" is decided by the second-region record, and the row is
   not reworded to pass on either.
4. **R12-GCP-8036 service plane: decline.** The "Service VPC/GKE cluster" wording
   is declined as written. ADR 0024 stands and Cloud Run serves the service
   plane, so no per-region cluster is planned.
5. **R12-GCP-8037: owner.** Decision 5 of the 2026-10-31 owner decisions stands.
   The Spanner ledger, Knowledge Catalog and managed model registry are not
   funded before dev runs. Meanwhile the model registry is the one the platform
   journals behind the signed release path. Each store reopens only by an ADR
   stating a measured need and a monthly ceiling, and then as one global
   resource that a plan shows exactly once. The ceiling is not chosen here
   (provisional: none).

## What it costs

All four rows stay open: 10001 and 8036 and 8037 PARTIAL, 10009 MISSING. No
plane-level blast-radius split exists, so a dev mistake is bounded by the
environment project and not by a plane project. There is no regional landing
storage or second region, so the per-region plan check cannot be shown. No
managed catalog or registry exists, so model lineage rests on the signed
release journal alone. The parked NCC patch carries a known gap (unfiltered star
spokes) that is recorded but not fixed. No billed infrastructure is added.

## What would make this wrong

If the owner supplies an organisation id and billing account before dev serves
from its single project, deferring the plane split would be needless delay and
decision 1 would be reopened. If a second region is approved, the hub, regional
networks and landing bucket stop being deferrable and decisions 2 and 3 are
replaced by that record. If a measured need for lineage or a ledger appears
before dev runs, decision 5 gives way to the ADR it names. If a Service plane
other than Cloud Run is ever funded, decision 4 needs an ADR amending ADR 0024.
Nothing here is built or tested; this record only orders the questions.
