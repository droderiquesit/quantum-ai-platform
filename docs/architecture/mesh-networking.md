# Mesh Networking for Regional Cells

This document describes how regional edge cells (`qip-edge-node`) communicate with each other and the central platform in a multi-region deployment.

**Current state:** Single node architecture; mesh networking is implemented in code but not deployed (see `backend/crates/edge/qip-edge/src/mesh.rs`). This document describes the design that will be activated when multiple regions are operational.

## Architecture overview

### Three communication patterns

1. **Central plane → Cell:** Centre sends capital envelopes and strategy intents; cells publish evidence
2. **Cell → Cell:** Peer replication and consensus (optional; depends on trading strategies)
3. **Cell → Venue:** Order submission and fill notifications (via egress proxy on loopback)

### Layer 3: IP networking (Terraform)

**Central connectivity:** Each cell has an ingress rule permitting:
- TCP 443 (HTTPS to Secret Manager, KMS, Artifact Registry)
- TCP 8080 (HTTP/2 gRPC to central plane)
- UDP 443 (QUIC, optional for future)

**Peer connectivity (future):** Optional firewall rules for cell-to-cell state replication on a dedicated port (default 9100).

**Venue connectivity:** Separate ingress rule per venue (one venue = one rule); only when `shadow_mode = false`.

### Layer 4: Protocol (gRPC)

All service-to-service communication uses **gRPC over HTTP/2** on port 8080 (unencrypted within the VPC; encryption is at the Google Cloud network level).

**Why gRPC?**
- Low latency (HTTP/2 multiplexing)
- Streaming (bidirectional for state updates)
- Backpressure (flow control)
- Code generation (Protobuf)

**Why HTTP/2, not HTTP/1.1?**
- The egress proxy uses HTTP/1.1 to upstream vendors (by design; ADR 0024 §4)
- But VPC-internal communication uses HTTP/2 for its multiplexing and streaming

### Layer 5: Application (qip-transport)

The `qip-transport` library (in `backend/crates/libs/qip-transport/`) provides:

- **HTTP/1.1 client:** For vendor APIs (reverse proxy at `http://127.0.0.1:910x`)
- **HTTP/2 server & client:** For internal gRPC

Both use explicit timeouts (no hanging connections).

## Central plane connectivity

### Cell to centre

Each cell maintains a persistent connection to the central platform's API (`qip-api` on Cloud Run):

```
┌──────────────┐       gRPC over HTTP/2        ┌──────────────┐
│              │       TCP 8080                 │              │
│ qip-edge-node│◄────────────────────────────────►│   qip-api    │
│ (cell)       │  (central plane connection)    │ (Cloud Run)  │
│              │                                 │              │
└──────────────┘                                 └──────────────┘
```

**Configuration (in cell's environment):**

```bash
QIP_CENTRAL_PLANE_URL=http://qip-api-prod-<project-number>-us-east4.run.app:8080
QIP_CELL_ID=newyork-1
```

**Heartbeat:** Cell sends a status message every 30 seconds (default):

```json
{
  "cell_id": "newyork-1",
  "timestamp": "2026-10-06T15:30:42Z",
  "journal_height": 12345,
  "capital_envelope": { "current": "450000.00", "allocated": "500000.00" },
  "reconciliation_breaks": 0,
  "halt_status": { "halted": false },
  "orders": { "placed_since_last_report": 42, "filled": 41 }
}
```

**Centre's responses:**

1. **Capital envelope update:** "Your allocation is now $450k; here's the new region share"
2. **Strategy intents:** "Place these orders with this sizing"
3. **Halt signal:** "Stop trading immediately"
4. **Policy update:** "New deterministic gate; use this logic for feasibility"

**Fallback:** If the centre is unreachable for > 7 minutes (default `region_dark_after`), the cell:
- Continues to execute existing strategies
- Refuses new orders from the centre
- Records the outage in the journal
- Resumes when centre becomes reachable

### Centre to cell

The centre (`qip-api`, `qip-fastbrain`, `qip-deepbrain`) maintains a read-only view of each cell's state:

```
     qip-api                qip-fastbrain            qip-deepbrain
       │                         │                         │
       ├────────────────────────────────────────────────────┤
       │         gRPC subscribe to cell reports            │
       │        (unidirectional stream, cell → centre)      │
       │                                                     │
       └──────────────────────────────────────────────────────→ cell
```

Cell's stream includes:

- Order placements (with venue, size, price, timestamp)
- Fill confirmations (with venue, quantity, fill price, timestamp)
- Reconciliation breaks (differences between what cell reported sent vs. centre's records)
- Halt events (kill-switch, policy gates, journal pressure)

The centre uses this stream to:

- **Score the twin** (LEARN stage): Compare actual fills vs. counterfactual model prediction
- **Assess feasibility:** Check if a venue remains liquid/reachable
- **Manage capital:** Adjust allocations if a region is over-utilized
- **Detect errors:** Reconciliation breaks alert to model inaccuracy or systematic pricing errors

## Peer-to-peer connectivity (future)

**Current state:** Not deployed; design is in place for when strategies require cross-region coordination.

### When is peer replication needed?

Some trading strategies benefit from **shared order book state across regions**:

Example: Arbitrage between US and European exchanges.

- Cell in US sees a trade opportunity: buy US equity at $100, sell equivalent in Europe at €95
- Cell in Europe sees the reciprocal: buy European equity at €94, sell in US at $101
- **Without coordination:** Both cells may place the same trade independently (inventory duplication)
- **With coordination:** Cells agree on who goes long/short; one books the arbitrage profit

### Mesh topology

**Current plan (future ADR):** Star topology with the centre as hub.

```
┌────────────┐
│newyork-1   │
│  (us-east4)│
└──────┬─────┘
       │
       ├─ gRPC on port 9100
       │
    ┌──┴──────────────┐
    │                 │
┌───┴──────┐   ┌─────┴────┐
│london-1  │   │ oakland-1 │
│europe-w2 │   │ us-west1  │
└──────────┘   └───────────┘
```

Each cell connects to every other cell (full mesh) for state replication:

```
QIP_MESH_CELLS=newyork-1,london-1,oakland-1
QIP_MESH_PORT=9100
QIP_MESH_REPLICATION_INTERVAL_MS=100  # How often to sync
```

### Peer messages

When a cell places an order, it broadcasts to peers:

```json
{
  "event_type": "OrderPlaced",
  "cell_id": "newyork-1",
  "order": {
    "id": "order-12345",
    "strategy": "arbitrage-us-equities",
    "side": "BUY",
    "quantity": 10000,
    "venue": "nasdaq",
    "price": 100.50
  }
}
```

Peers respond with acknowledgment (received and journalled) or rejection (I can't replicate this).

Rejections are recorded:

- **Capacity:** Peer has no capital envelope to replicate this order
- **Journal:** Peer's disk is full or not writable
- **Policy:** Peer's policy gate refuses this order type
- **Timeout:** No response within 1 second

Consensus is reached when a quorum of peers have acknowledged (at least 2 of 3, etc.).

### Conflict resolution

If two cells simultaneously place orders that violate a coordinated constraint (e.g., max position), the **earlier timestamp wins**:

```
Cell A: OrderPlaced, timestamp 1234.500
Cell B: OrderPlaced, timestamp 1234.501
```

Cell A's order is applied; Cell B's order is rejected by the consensus protocol.

## Network design (Terraform)

### Trust zones and regions

Each region has its own trust zones (subnets) in the same VPC:

```hcl
# infrastructure/terraform/variables.tf

trust_zones = {
  "application-identity" = {
    region      = "us-east4"
    subnet_cidr = "10.0.32.0/24"
  }
  "application-identity-london" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.48.0/24"
  }
}
```

### Execution nodes' subnets

Each node gets its own subnet from the regional ladder (see `infrastructure/environments/README.md`):

```hcl
execution_nodes = {
  "newyork-1" = {
    region      = "us-east4"
    subnet_cidr = "10.64.0.0/24"      # us-east4 block
  }
  "london-1" = {
    region      = "europe-west2"
    subnet_cidr = "10.68.0.0/24"      # europe-west2 block
  }
}
```

### Firewall rules for peer communication

When peer replication is enabled, add a `permitted_paths` entry:

```hcl
permitted_paths = {
  "cells-mesh" = {
    from  = "execution-nodes"
    to    = "execution-nodes"
    mode  = "allow"
    ports = [9100]
    note  = "Peer-to-peer state replication across regions"
  }
}
```

Terraform generates rules permitting:
- newyork-1 (10.64.0.0/24) → london-1 (10.68.0.0/24) on port 9100
- london-1 (10.68.0.0/24) → newyork-1 (10.64.0.0/24) on port 9100
- ... and so on for each pair

### Central plane ranges

Automatically derived from the trust zones where catalogue workloads are deployed:

```hcl
# In infrastructure/terraform/main.tf
central_plane_ranges = concat(
  [
    for zone in distinct([for workload in local.cloud_run_catalogue : workload.trust_zone]) :
    var.trust_zones[zone].subnet_cidr
    if contains(keys(var.trust_zones), zone)
  ],
  [local.private_google_apis],  # 199.36.153.8/30
)
```

Each node gets a firewall rule permitting egress to `central_plane_ranges` on ports 443 and 8080.

## Observability

### Latency between regions

**Metric:** `qip_mesh_latency_ms{peer_cell}`

- Histogram of round-trip time for mesh messages
- Buckets: [1ms, 5ms, 10ms, 50ms, 100ms, +Inf]
- Alert if p95 latency > 50ms for any peer

### Message loss

**Metric:** `qip_mesh_drops_total{peer_cell,reason}`

- Dropped peer messages (timeout, invalid, network error)
- Reasons: `timeout`, `invalid_signature`, `network_error`, `backpressure`
- Alert if drop rate > 1% for any peer

### Replication lag

**Metric:** `qip_mesh_replication_lag_ms{peer_cell}`

- How far behind a peer's journal is from the local journal
- Alert if lag > 1000ms for any peer

## Troubleshooting

### Cell cannot reach central plane

**Symptom:** `qip_edge_halted{source="central_connectivity"}` = 1

**Checks:**

1. **DNS resolution:**
   ```bash
   ssh <cell> nslookup qip-api-prod-12345-us-east4.run.app
   ```

2. **Firewall rule exists:**
   ```bash
   gcloud compute firewall-rules describe qip-prod-exec-newyork-1-central-plane
   ```

3. **Central plane service is running:**
   ```bash
   gcloud run services list --filter="name:qip-api"
   ```

4. **Network connectivity test:**
   ```bash
   ssh <cell> curl -v http://qip-api-prod.run.app:8080/health
   ```

**Fixes:**
- Firewall rule deleted? Reapply Terraform
- Central plane service crashed? Redeploy with `deploy.yml`
- Network path broken? Check VPC routing and NAT

### Peer connection fails

**Symptom:** `qip_mesh_drops_total{peer_cell="london-1"}` increasing

**Checks:**

1. **Peer is running:**
   ```bash
   gcloud compute instances describe qip-prod-exec-london-1 --zone=europe-west2-a
   ```

2. **Peer's health check passes:**
   ```bash
   ssh <peer-cell> curl http://localhost:9001/health | jq .
   ```

3. **Firewall rule exists:**
   ```bash
   gcloud compute firewall-rules describe qip-prod-exec-newyork-1-cells-mesh
   ```

4. **Latency is acceptable:**
   ```bash
   # Check qip_mesh_latency_ms metric
   gcloud monitoring time-series list --filter 'metric.type="custom.googleapis.com/qip_mesh_latency_ms"'
   ```

**Fixes:**
- Peer down? Restart or replace it (blue-green update)
- Firewall rule deleted? Reapply Terraform
- Regional latency too high? Consider topology redesign (add regional hubs vs. full mesh)

## References

- ADR 0008: Edge cells decide alone
- ADR 0024: Blueprint runtime provisioned in code
- Blueprint §41.2–§41.5: Execution nodes and inter-cell communication
- `backend/crates/edge/qip-edge/src/mesh.rs`: Mesh implementation
- `backend/crates/libs/qip-transport/`: Transport layer
- `infrastructure/terraform/modules/trust-zones/`: Firewall rules and VPC configuration
