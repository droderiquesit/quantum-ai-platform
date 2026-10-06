# Edge Cell Mesh Networking

Provides inter-cell networking and communication for the seven regional edge cells that form the distributed trading platform.

## Architecture

Each edge cell runs on a dedicated Compute Engine machine in its region, adjacent to the venues it trades. Cells decide locally within capital envelopes granted by the central plane and communicate with:

1. **Central Plane** — capital envelopes in, evidence and exposure out
2. **Other Cells** — region-scoped capital share and cross-region mirroring (§31.1, ADR 0039)
3. **Venues** — market data, order submission, fills

This module creates the mesh connecting cells to each other and to the central plane.

## Topology

A `REGIONAL` VPC with one subnet per region per trust zone. Each execution node attaches to its own dedicated subnet, isolating it from zones and from other nodes.

### Mesh Connections

- **Cell-to-Cell**: Private Service Connect endpoints (when regions are distant) or direct VPC peering (when regions are adjacent)
- **Cell-to-Central**: Direct access through central plane subnets and Google APIs
- **Cell-to-Venue**: Zone-specific egress rules, empty in shadow mode

### Firewall Posture

Default deny on all edges. Cells are isolated from each other in shadow mode. Turning off shadow mode creates venue egress rules and, if this is the first cell to reach a venue, creates an ingress rule on that venue's aggregation point.

## Constraints

- No service-to-service mesh (Istio, etc.) — the edge lives over microseconds
- No overlay network — one VPC, regional subnets, native routing
- No inter-region peering — cells are independent by design (ADR 0008)
- No external addresses — cells reach everything through Google Cloud gateways

## Required Evidence

Every cell deployment must demonstrate:

1. **Blue-green replacement** — the group replaces a failing instance without a session loss
2. **Partition resilience** — a cell halted from the centre keeps working inside its envelope
3. **Reconciliation** — breaks between cell and centre are caught and recorded

See `.claude/rules/01-security-and-safety.md` for paper-trading boundary enforcement.
