# Edge Cells Deployment Orchestration

Orchestrates deployment of multiple regional edge cells, each running on a dedicated Compute Engine machine, with mesh networking and venue connectivity.

## Architecture

The platform runs as **source-adjacent edge cells** (ADR 0008): one execution node per region, sitting next to the venues it trades. Cells decide locally within capital envelopes granted by the central plane.

**This module composes nothing and creates nothing.** It is the topology
contract: it validates a set of cells (every zone inside its region, every
cell configured for a venue, every regional capital ceiling a positive
number) and derives the values the root hands to the modules that do create
resources:

1. **Execution nodes** (`modules/execution-node`) — composed by the root's
   `module "execution_node"`, one C3 or C3D machine per region
2. **Edge mesh** (`modules/edge-mesh`) — takes `central_plane_ranges` and
   `psc_addresses` from this module's outputs; not yet composed by the root

It used to call both modules itself. A module calling a module is invisible to
`terraform_contract.rs`'s correspondence scan, which reads only the root's
calls, and that is how this one came to pass `isolated_cpus` and
`telemetry_endpoint` to an `execution-node` that declares neither — a
configuration `terraform validate` refuses and nothing ever ran. It would also
have been a second set of nodes beside the root's. Composition is the root's
job, one level deep.

`tests/multi_region_deployment.tftest.hcl` plans it against a mocked provider
and proves each gate both refuses and admits: `terraform init -backend=false
&& terraform test` from this directory.

## Deployment Steps

### 1. Provision (Shadow Mode)

Deploy with `execution_nodes` map populated and `shadow_mode = true` for all nodes:

```hcl
execution_nodes = {
  "cell-us-east4" = {
    region         = "us-east4"
    zone           = "us-east4-a"
    subnet_cidr    = "10.240.0.0/24"
    node_count     = 1
    machine_type   = "c3-highcpu-22"
    shadow_mode    = true
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
  }
}
```

In shadow mode:
- Machines boot and run the binary
- Health checks prove the node is working
- Telemetry is written to Cloud Monitoring
- No venue connectivity is possible — firewall rules are empty
- The node's decisions are compared against the central plane's pod

### 2. Observe

Once a node has run for 2–4 weeks in shadow mode:
- Verify its health is stable
- Confirm its decisions match the central plane pod's decisions on the same calendar day
- Read the reconciliation breaks and confirm they are expected (e.g., order-id assignment differences)

### 3. Exit Shadow Mode

Change one line in tfvars: `shadow_mode = true` → `shadow_mode = false`.

This creates:
- Venue egress firewall rules, opening paths to the configured venues — the
  simulated venue and provider sandboxes only; every cell is paper-only
  (ADR 0003) and `qip-edge-node` refuses any other venue feed at start-up
- Venue ingress rules (coordination between cells and counterparties)
- Health ingress rules, allowing the central plane and other cells to reach it

The node cannot take venue sessions until shadow mode is off. Turning it off is a reviewed diff.

## Multi-Region Deployment

All seven regional cells follow the same pattern:

```hcl
execution_nodes = {
  # primary region — first deployed, most mature
  "cell-us-east4" = { … shadow_mode = false … }
  # secondary regions — staged deployment, each observed before next
  "cell-us-west1" = { … shadow_mode = true … }
  "cell-europe-west1" = { … shadow_mode = true … }
  …
}

cross_region_mirrors = [
  {
    from_region              = "us-east4"
    to_region                = "us-west1"
    rtt_ms                   = 42
    inventory_band_pct       = 2
    dislocation_threshold_pct = 10
  }
]
```

Each cell is independent until its mirror configuration is written. At that point:
- The cell deploys the cross-region configuration in `node.env`
- It knows which venues are in other regions
- It reads the latency and inventory band from the configuration
- It begins updating the `qip_edge_region_share_bound` metric with what the centre has allocated

## Required Gates

Every edge cell deployment must pass:

1. **Security Controls** — the `qip-acceptance` suite confirms:
   - Paper-trading boundary is intact (cells cannot reach live venues)
   - Firewall rules are least-privilege
   - Service accounts have minimal IAM grants
   - Secrets are read from Secret Manager, never environment variables

2. **Partition Resilience** — tests confirm:
   - A halted cell keeps working inside its envelope
   - Stale envelopes expire and stop a cell
   - Reconciliation breaks are caught and logged

3. **Blue-Green Replacement** — operational verification:
   - The managed instance group replaces a failing node
   - A replacement does not lose sessions
   - Telemetry continues uninterrupted

4. **Evidence** — the deployment captures:
   - Boot logs proving the node came up healthy
   - Health check graphs showing continuous readiness
   - Reconciliation break metrics proving consistency with the centre

## Cost and Billing

Each execution node bills for:
- Compute Engine machine (C3/C3D, per-second billing, discount available)
- 100GB persistent disk (per-second billing)
- Snapshot retention (if configured in `modules/backup`)
- Blue-green surge instance during updates (not reserved by default; ADR 0099 C8)
- Egress bandwidth (heavily outbound for market data; venue-specific)
- NAT gateway (if `create_egress_nat = true`; only some regions need it)

There is no:
- Container registry pull on every boot (image is immutable, pre-baked)
- Kubernetes cluster (GKE removed; ADR 0024, ADR 0036)
- Service mesh (Istio, Linkerd)
- Additional load balancers (direct routes)

The cost per region is dominated by the machine shape and venue data feeds, not orchestration.
