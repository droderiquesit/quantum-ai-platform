# ADR 0711: The three NCC rows wait on the second-region record, and the parked patch stays unapplied until that record names the hub's monthly cost

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-GCP-8056, R12-GCP-8057, R12-GCP-8058

## Context

Three rows ask for Network Connectivity Center. R12-GCP-8056 wants VPC spokes
with route filters. R12-GCP-8057 wants a dedicated mesh hub exporting only
narrow Reflex and Fabric prefixes. R12-GCP-8058 wants a star hub with centre
and edge groups for service, data, AI and management traffic. All three are
MISSING. No `google_network_connectivity_*` resource is in the tree.

ADR 0035 runs one execution node in one region, so there is nothing for a
hub to join. ADR 0114 decision 4 keeps the GKE control plane to controllers
only, and ADR 0024 serves the service plane from Cloud Run, so no service,
data or AI traffic needs a central inspection hub. The acceptance test
`gcp_placement::no_network_connectivity_hub_or_vpc_peering_is_declared_before_the_second_region`
refuses a hub declaration even when it is gated off. Lane L6 wrote the hubs
and a mocked harness, and the parent session parked them on 2026-10-24 as
`docs/blueprint/lanes/l6-ncc-hubs-parked.patch`. The module had been written
and withdrawn once before, on PR 239.

Decision 4 of the owner's 2026-10-25 decisions declined the hubs until a
second region is decided, and the 2026-10-31 rulings on these rows repeated
it and extended it to the VPC spokes. ADR 0710 decision 2 already records the
same wait for the sibling row R12-GCP-10009, including the rule that every
spoke, star spokes included, must carry an include or exclude range. A hub and
its data transfer are billed, so the choice is the owner's. This record gives
the three rows one home and writes no code.

## Decision

1. **R12-GCP-8056: owner.** No VPC spoke, hub or peering is declared. The
   three VPCs in `modules/network` stay unjoined, which is the isolation they
   exist for. The spokes land with the record that supersedes ADR 0035 for a
   second region (R12-GCP-8035) and names the hub's monthly cost. No figure is
   chosen here (provisional: none). The row stays MISSING.
2. **R12-GCP-8057: owner.** The mesh hub is not declared. With one region the
   latency mesh has nothing to join. When the second-region record is
   accepted, the hub is applied from the parked patch, exports only the narrow
   control-fabric prefixes ADR 0156 section 6(c) admits, and carries that
   exclusion as a validation that refuses a wide prefix. The row stays
   MISSING.
3. **R12-GCP-8058: owner.** The star hub is not declared. One region and no
   GKE service plane means no traffic needs central inspection. The star hub
   lands only with the second-region record and its named monthly cost, and
   not before. The row stays MISSING.
4. **Guard: unchanged.** `gcp_placement.rs` is not edited, and the patch is not
   applied, to let any of these through. The guard's condition moves only
   inside the second-region record, in the same change that lands a rule check
   the record names. This record amends no Accepted record and supersedes
   nothing.
5. **Filters: required at landing.** When the hubs land, every spoke on every
   hub, the star's included, must carry an include or exclude range, as ADR
   0710 decision 2 requires. A validation must refuse a filterless spoke. The
   test that feeds it one must be seen to fail before the validation is added.
   A reviewed plan must show the hubs, the spokes and their filters. Nothing
   here is a quantum item, so ADR 0150 is not touched.

## What it costs

All three rows stay MISSING and none can be shown by a plan. The isolation of
the three VPCs holds, and no cross-region path exists to inspect or to route.
The parked patch is known to filter its latency spokes but not its star
spokes, which decision 5 fixes only at landing. The work is written twice if
the patch ages before the second region is approved. No billed infrastructure
is added and no live order path or paper-trading layer is touched.

## What would make this wrong

If the owner approves a second region, decisions 1 to 3 are replaced by that
record, which must also name the hub's cost and move the guard. If a service,
data or AI plane other than Cloud Run is funded in the one region, the star
hub could be needed before a second region exists, and that needs an ADR
amending ADR 0024. If the parked patch drifts from the network module so far
that it no longer applies, the record that lands the hubs must regenerate it
and not edit the guard. Nothing here is built or tested; this record only
orders the questions.
