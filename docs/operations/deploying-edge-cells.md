# Edge Cell Deployment Across Regions

This document describes the architecture and procedures for deploying regional edge cells (`qip-edge-node`) across multiple Google Cloud regions, connecting them to venues, and integrating them with the central platform.

**Status:** Template for regional deployment. Currently authorised: one node in `dev` (ADR 0035). Multi-region deployment requires:
1. Boot images baked for each region (see "Image baking")
2. Capital allocation per region (see "Capital allocation")
3. Venue connectivity decisions per region (see "Venue connectivity")
4. Mesh networking plan across regions (see "Mesh networking")
5. Order routing strategy (see "Order routing")
6. Production venue registration (see "Venue registration")

## Architecture overview

### Regional cells

Each region runs **exactly one** execution node:

- **Compute Engine instance:** C3 or C3D high-CPU shape (8–22 vCPU per ADR 0024 §41.4)
- **Binary:** `qip-edge-node`, bare Rust under systemd (no container runtime)
- **Storage:** Persistent disk for order journal (snapshots taken by `modules/backup`)
- **Networking:** Private IP only; no external address; gVNIC for TIER_1 networking
- **Health checks:** HTTP `/health` endpoint on internal port (9001 by default)
- **Telemetry:** Prometheus metrics on `/metrics` port (9002 by default) scraped by Ops Agent
- **Secrets:** Capital envelope key fetched to tmpfs via Secret Manager CSI equivalent
- **Egress proxy:** Envoy sidecar on loopback (127.0.0.1:910x) for all outbound TLS
- **Shadow mode:** Structural default (firewall rules do not exist until shadow mode is off)

### Central plane connectivity

Each cell:

- **Publishes:** Evidence (orders sent, fills received, reconciliation breaks) to the central plane
- **Receives:** Capital envelopes (allocation, region share, position updates) from the central plane
- **Falls back:** Can operate independently for up to 7 minutes (default `region_dark_after`) if central plane unreachable
- **Records:** Every decision locally to hash-chained journal before reporting to centre

### Autonomy layers

Three independent layers enforce paper trading (ADR 0003, ADR 0021):

1. **Terraform layer:** `infrastructure/terraform/variables.tf` refuses `supervised_live`, `limited_autonomous_live`, `autonomous_live` at plan time
2. **Composition root layer:** `qip-edge-node` refuses to start with a live ceiling
3. **Type system layer:** `qip-edge` has no `Cell` constructor accepting a live ceiling; shadow mode is structural

## Regional deployment sequence

### Step 1: Prepare the region

**Decide on the region** based on venue proximity, not convenience.

Example regions (ADR 0035 ladder, environments/README.md):
- `us-east4` (Ashburn) — closest to NY/NJ venues (~300km)
- `europe-west2` (London) — closest to London venues
- `us-west1` (Oregon) — closest to US West Coast venues

**Record in architecture decision:**
- Region name and rationale
- Venue list for this region
- Capital allocation (in portfolio base currency)
- Whether this region holds margin, proprietary capital, or both

### Step 2: Bake the boot image

The execution node boots an immutable image (no image families). Images are built by `.github/workflows/image.yml`.

**Prerequisites:**
1. Uncomment `image_bake_subnet_cidr` in `environments/dev/terraform.tfvars` (or whichever environment is building)
2. Run `infra.yml` with `action=up` to create:
   - Baking subnet (/28, no overlap with trust zones or other nodes)
   - Builder service account
   - Cloud Storage staging bucket
   - KMS key for artifact signing

**Procedure:**

```bash
# 1. Prepare tfvars
# In environments/<env>/terraform.tfvars:
image_bake_subnet_cidr = "10.1.0.0/28"  # No overlap with execution nodes' ladder (10.64+)

# 2. Apply infrastructure
make infra  # Creates the bake environment

# 3. Dispatch the image bake workflow
gh workflow run image.yml \
  --ref $(git rev-parse --short HEAD) \
  --field environment=dev \
  --field target_region=us-east4
```

**Output:** Self-link of the baked image (e.g., `projects/algorik-platform-dev/global/images/qip-edge-node-us-east4-20261006-abc123`)

**Image contract** (verified by startup script):

- No container runtime (docker, containerd, podman, crio, runc)
- Kernel command line with `isolcpus=2-<N>` where N = vCPU count - 1
- No swap enabled
- Huge pages preallocated on boot (64 GiB by default)
- `/usr/local/bin/qip-edge-node` executable (the binary)
- `/usr/local/bin/qip-fetch-secret` executable (Secret Manager helper)
- `/usr/local/bin/envoy` executable (static Envoy proxy)
- `google-cloud-ops-agent.service` installed (for telemetry scraping)

### Step 3: Configure capital allocation

**Decide on the capital envelope per region.** This is:

- The maximum notional capital the region may hold across all strategies
- A positive decimal string (e.g., "1000000.00" for 1M in base currency)
- Never defaulted; always an explicit decision

**Record in `execution_nodes` tfvars entry:**

```hcl
execution_nodes = {
  "newyork-1" = {
    region           = "us-east4"
    zone             = "us-east4-b"
    subnet_cidr      = "10.64.0.0/24"      # From the ladder; no overlap with trust zones
    machine_type     = "c3-highcpu-16"     # Venue count determines size
    boot_image       = "projects/algorik-platform-dev/global/images/qip-edge-node-us-east4-20261006-abc123"
    region_allocation = "1000000.00"       # Notional capital allocation
    
    venues = {
      "nasdaq" = { cidr = "209.85.230.0/24", port = 443 }  # Published address range + port
      "nyse"   = { cidr = "209.85.231.0/24", port = 443 }
    }
    
    shadow_mode            = true           # Structural default; observes but does not trade
    create_egress_nat      = true           # Simulated venues have no internet route; live ones need NAT
    default_pricing        = ""             # Empty until pricing strategy is deployed
    strategy_plan_path     = ""             # Empty until strategy plan is loaded
    cross_region_mirror_path = ""           # Empty until cross-region rules are published
  }
}
```

### Step 4: Plan and apply

**Before applying:**

```bash
# Run all infrastructure gates
make infra

# Review the plan
terraform -chdir=infrastructure plan -var-file=environments/dev/terraform.tfvars

# The plan must show:
# 1. google_compute_subnetwork for the node
# 2. google_service_account for the node's identity
# 3. google_compute_instance_template with the image and startup script
# 4. google_compute_instance_group_manager (managed instance group)
# 5. google_compute_firewall rules (health checks, Google APIs, central plane, venues in shadow mode)
# 6. google_compute_reservation for capacity guarantee
```

**Apply:**

```bash
terraform -chdir=infrastructure apply -var-file=environments/dev/terraform.tfvars
```

**Post-apply verification:**

1. **Node health:** Instance group should report "healthy" within 5 minutes (initial_delay_sec = 300)
   ```bash
   gcloud compute instance-groups managed describe qip-dev-exec-newyork-1 --zone=us-east4-b
   ```

2. **Journal writable:** Node logs should show successful tmpfs mount and journal initialization
   ```bash
   gcloud compute instances get-serial-port-output qip-dev-exec-newyork-1 --zone=us-east4-b | tail -100
   ```

3. **Telemetry emitted:** Ops Agent should be scraping metrics
   ```bash
   curl -s http://<node-private-ip>:9002/metrics | head -20
   ```

4. **Central plane reachable:** Node should log heartbeats to the platform
   ```bash
   # Check the central plane's ingest_cell_report counter
   # This requires observability to be ingested (ADR 0032 / workload_metrics_exist=true)
   ```

### Step 5: Attach journal snapshot schedule

The execution node's persistent disk holds the hash-chained order journal. Snapshots are automatic (see `modules/backup`), but the attachment binding must be created after first boot:

```bash
# Get the journal snapshot attachment command from Terraform output
terraform -chdir=infrastructure output journal_snapshot_attachment_command

# Example output:
# gcloud compute disks add-labels qip-dev-exec-newyork-1 \
#   --labels=qip_journal=true \
#   --zone=us-east4-b
```

## Mesh networking

**Current state:** Single node (dev). Mesh exists in code only (`qip_edge::Mesh` in `backend/crates/edge/qip-edge/src/mesh.rs`).

**When deploying multiple regions:** Each regional cell must be able to reach the central plane and, optionally, other regional cells for peer replication.

### Central plane connectivity

**From node to centre:** TCP 443 and 8080 (gRPC over HTTP/2 and HTTP/1.1)

Rules are generated automatically by the root module:

```hcl
# In infrastructure/terraform/main.tf
central_plane_ranges = concat(
  [
    for zone in distinct([for workload in local.cloud_run_catalogue : workload.trust_zone]) :
    var.trust_zones[zone].subnet_cidr
    if contains(keys(var.trust_zones), zone)
  ],
  [local.private_google_apis],
)
```

Each node gets a firewall rule permitting egress to these ranges on 443 (HTTPS to Secret Manager, KMS, Artifact Registry) and 8080 (internal gRPC to the API).

**Implementation detail:** The node's `qip_edge::Mesh` connects via the egress proxy (loopback 127.0.0.1:910x) to the central plane's address, which is configured via `QIP_CENTRAL_PLANE_URL` in the node's environment.

### Peer-to-peer connectivity (optional)

**Not currently deployed.** When peers need to share state (cross-region replication, p2p reconciliation):

1. Create firewall rules permitting cell-to-cell traffic on a designated port (e.g., 9100)
2. Update `permitted_paths` in tfvars to declare the routes
3. Configure each node with `QIP_MESH_CELLS` (comma-separated cell IDs and addresses)

Example (future):

```hcl
permitted_paths = {
  "newyork-to-london" = {
    from  = "execution-nodes"
    to    = "execution-nodes"
    mode  = "allow"
    ports = [9100]  # Mesh state replication
    note  = "Cross-region cell-to-cell peer replication"
  }
}
```

## Venue connectivity

### Deciding on venues per region

**Before deploying a cell,** you must decide which venues it will connect to. Venues decide the machine size (more venues = larger shape) and the capital allocation.

**Venue information required (from the venue's connectivity documentation):**

- Published IP address range(s) (CIDR)
- Port(s) for the order gateway
- Protocol (always TCP for order submission and fill subscription)
- Authentication method (see "Venue registration" below)
- Market data feed (if separate from order gateway)

### Shadow mode and venue rules

**Shadow mode is structural:** Even with venues declared, a node in shadow mode has no firewall rule permitting egress to venue addresses.

```hcl
resource "google_compute_firewall" "venue" {
  for_each = var.shadow_mode ? {} : var.venues  # Empty when shadow_mode = true

  # ... rule details ...
}
```

This means:

- **In shadow mode:** Node starts successfully, observes orders, sizes positions, but `qip-edge::Cell::send` logs refusals at the `GATE_LIVE_VENUE` gate with no rule to attempt outbound connection.
- **Out of shadow mode:** The rules exist; refusals mean the node cannot reach the venue (network issue or authentication failure).

### Turning off shadow mode

**Shadow mode is a separate decision from the infrastructure change.** To exit shadow mode:

1. **Gather evidence:** Run the node for a sustained period (at least 7 days) under shadow mode. Collect:
   - Order book depth and liquidity observations
   - Model accuracy (LEARN stage calibration)
   - Reconciliation break counts
   - Latency histograms

2. **Propose the change:** Write an ADR proposing to exit shadow mode for the specific region and venue list, with evidence attached.

3. **Set `shadow_mode = false` in tfvars** (requires ADR approval):

   ```hcl
   execution_nodes = {
     "newyork-1" = {
       # ... other fields ...
       shadow_mode = false
     }
   }
   ```

4. **Plan and apply:** Terraform will add firewall rules for each venue.

## Order routing

### Strategy placement

Order routing starts with **strategy placement:** which cell(s) run which strategies.

**Current state:** Strategies are placed at the central REASON stage. The cell receives `StrategyIntents` from the central `CostRouter` and executes them.

**Configuration:**

```hcl
# In the strategy's manifest (infrastructure/gitops/envs/<env>/*.yaml)
spec:
  placement:
    regions: ["us-east4"]  # This strategy runs in the cell in this region

# Or, for cells in all active regions:
    regions: ["all"]  # Requires documentation of which regions are active
```

### Order placement within a cell

Within a region, `qip-edge::Cell` runs the **netting** and **arbitrage** engines:

1. **Netting:** Combines orders across strategies into fewer orders (position consolidation)
2. **Arbitrage:** Optionally executes internal crosses between strategies before external order submission
3. **Pricing:** Each order is priced according to `default_pricing` strategy (or per-order policy)

### Venue selection

For each order, the cell determines which venue(s) to use based on:

- **Orderbook depth:** Each venue's observable depth at the order's size
- **Fees:** Venue-specific trading fees and rebates
- **Throughput:** Venue's historical fill confirmation latency
- **Capital reserve:** Available capital in this region's envelope
- **Feasibility:** Deterministic checks (e.g., position limits, account status)

Routing decisions are logged in the order journal with the venue assignment rationale.

### Central plane interaction

After placing an order on a venue:

1. **Report sent:** Cell sends `OrderPlaced` report with venue, quantity, price, timestamp
2. **Fill received:** Venue confirms fills; cell reports `FillConfirmed` to centre
3. **Reconciliation:** Centre compares what cell reports sent vs what cell reports filled; breaks are recorded

## Venue registration

### Before first deployment

For each venue the cell will connect to, a **registration record** must exist in `data/venue-registrations.json`:

```json
{
  "records": [
    {
      "venue_name": "nasdaq",
      "operator": "alice@example.com",
      "account_id": "12345",
      "api_key_secret": "nasdaq-prod-key",
      "sandbox_api_key_secret": "nasdaq-test-key"
    }
  ]
}
```

**Important:**

- `operator`: Name of the person who registered (for audit)
- `api_key_secret`: Secret Manager secret id (NOT the key itself; the id)
- `venue_name`: Must match the key in `venues` map in tfvars

### Credential management

**Workflow:**

1. **Register at the venue** under the operator's own identity (never a service account)
2. **Store the credential** in Google Cloud Secret Manager:
   ```bash
   gcloud secrets create nasdaq-prod-key --replication-policy="automatic"
   gcloud secrets versions add nasdaq-prod-key --data-file=- <<< "$(cat ~/.venue-credentials/nasdaq.key)"
   ```

3. **Record the secret id** in the registration file:
   ```json
   {
     "venue_name": "nasdaq",
     "operator": "alice@example.com",
     "api_key_secret": "nasdaq-prod-key",  # The Secret Manager secret id
     ...
   }
   ```

4. **Grant the node access** (automatic via Terraform):
   ```hcl
   # In modules/execution-node/main.tf
   resource "google_secret_manager_secret_iam_member" "venue_credential" {
     count = local.venue_credential_bound ? 1 : 0
     project   = var.project_id
     secret_id = var.venue_credential_secret_id
     role      = "roles/secretmanager.secretAccessor"
     member    = "serviceAccount:${google_service_account.node.email}"
   }
   ```

5. **Commit the registration record** (with credential id, not the credential itself):
   ```bash
   git add data/venue-registrations.json
   git commit -m "Register operator alice at NASDAQ for us-east4 cell"
   ```

### Secrets never in code

**Never:**
- Put an API key, token, or password in a `.tf` file, `.tfvars`, or code comment
- Set a secret as an environment variable (it appears in `/proc/<pid>/environ`)
- Download a service account key file

**Always:**
- Store credentials in Secret Manager
- Reference the secret by id (not the value)
- Let the platform read secrets as files via the CSI driver (on execution nodes) or Secret Manager mounts (on Cloud Run)

## Observability

### Health endpoint

Each node exposes `/health` on `health_port` (default 9001):

```bash
curl -s http://<node-private-ip>:9001/health | jq .
```

Response indicates:

- Journal writable (disk space, permissions)
- All secrets readable
- Central plane reachable (if mesh_cells configured)
- Venue connections established (if out of shadow mode)

### Metrics (Prometheus)

Metrics are published on `metrics_port` (default 9002):

```bash
curl -s http://<node-private-ip>:9002/metrics | grep qip_edge
```

Key series:

- `qip_edge_work_passes_total` — pass count since boot
- `qip_edge_orders_placed_total{venue}` — orders sent per venue
- `qip_edge_fills_confirmed_total{venue}` — fills received per venue
- `qip_edge_refusals_total{gate}` — orders refused per gate (e.g., GATE_LIVE_VENUE in shadow mode)
- `qip_edge_halted{source}` — halt status (0=running, 1=halted)
- `qip_edge_reconciliation_breaks_total` — discrepancies between cell and centre records

### Logs

Structured logs go to Cloud Logging via the Ops Agent:

```bash
gcloud logging read "resource.type=gce_instance AND resource.labels.instance_id=$(gcloud compute instances describe qip-dev-exec-newyork-1 --zone=us-east4-b --format='value(id)')" --limit=50
```

## Production deployment considerations

### Pre-production checklist

Before deploying to `prod`:

- [ ] One node has run in `dev` for 7+ days in shadow mode
- [ ] Telemetry is scraped (observability ADR 0032 implemented)
- [ ] LEARN stage calibration has converged (model accuracy > baseline on held-out data)
- [ ] Reconciliation break rate is < 0.1% (systematic pricing errors understood)
- [ ] Disaster recovery procedure documented (journal snapshot restore)
- [ ] Incident response runbook written (cell halt procedures, manual override)
- [ ] Two operators trained on cell operations and monitoring
- [ ] Capacity reservation confirmed with venue (confirm available shares, locate within latency SLA)

### Multi-region deployment

Once the first region is proven:

1. **For each additional region:**
   - Repeat "Regional deployment sequence" above
   - Capital allocation is a per-region decision (not automatic)
   - Deploy to test first; observe for 7 days before prod
   - Document venue-specific routing policies (latency, fees, throughput)

2. **Mesh networking:**
   - Enable `QIP_MESH_CELLS` when peer replication is needed
   - Test peer message delivery under loss (simulated partition)
   - Monitor cross-region latency percentiles (p95, p99)

3. **Central plane coordination:**
   - Centre must be configured with all active cell ids
   - Centre's region_share policy distributes capital across cells
   - Centre must handle cells becoming dark (unreachable for > 7 minutes) gracefully

### Blue-green cell replacement

When replacing a cell (e.g., for a binary update):

1. **Terraform generates a new instance template** (name_prefix creates a new one on any change)
2. **Managed instance group creates the new instance** (max_surge_fixed = 1)
3. **New instance passes health checks** (initial_delay_sec = 300)
4. **Old instance is terminated** (replacement_method = "SUBSTITUTE")
5. **Journal persists** (attached to new instance via disk label)

**During replacement:**
- Central plane sees the cell go dark for ~5 minutes (initial_delay_sec)
- Regional capital envelope is paused while new instance comes up
- Strategies should not place orders during this window (centre's responsibility)

## Troubleshooting

### Node won't start

**Startup script exits before systemd unit starts:**

```bash
# Check serial port output (first 50 lines)
gcloud compute instances get-serial-port-output qip-dev-exec-newyork-1 --zone=us-east4-b | head -50
```

**Common issues:**

| Error | Cause | Fix |
|---|---|---|
| "isolcpus not in kernel command line" | Image was not built with kernel parameter | Rebuild image with `isolcpus=2-N` in GRUB |
| "qip-edge-node is missing" | Binary not in image | Rebuild image; check `/usr/local/bin/qip-edge-node` |
| "could not read the capital envelope key" | Secret Manager IAM binding missing or key unavailable | Check service account has `secretmanager.secretAccessor` on the secret |
| "swap is active" | Image has swap partitions | Rebuild image; disable swap in fstab and kernel command line |
| "no huge pages" | Huge pages not preallocated at boot | Rebuild image with huge page kernel parameters |

### Node reports unhealthy

**Health check fails after instance comes up:**

```bash
# Check instance template and its startup script
gcloud compute instance-templates describe qip-dev-exec-newyork-1-abc123 --zone=us-east4-b

# Check the instance's response to health checks
gcloud compute instances list --filter="name:qip-dev-exec-newyork-1" --format="value(status)"

# SSH to the node (via IAP)
gcloud compute ssh qip-dev-exec-newyork-1 --zone=us-east4-b --tunnel-through-iap

# Inside the node:
curl -s http://localhost:9001/health | jq .
systemctl status qip-execution-node
systemctl status qip-egress
journalctl -u qip-execution-node -n 50
```

### Node is healthy but not receiving orders

**Central plane is not sending strategies to this cell:**

1. **Check QIP_CELL_ID matches centre's configuration:**
   ```bash
   ssh <node> 'cat /etc/qip/execution-node/node.env | grep QIP_CELL_ID'
   ```

2. **Check centre knows about this cell:**
   ```bash
   # In the API (requires access)
   curl -s http://qip-api:8080/api/v1/cells | jq '.[] | select(.id=="newyork-1")'
   ```

3. **Check mesh connectivity:**
   ```bash
   ssh <node> 'curl -s http://localhost:9002/metrics | grep qip_edge_work_passes_total'
   # Should be > 0 if passes are running
   ```

### High reconciliation break rate

**Cell and centre disagree on fills:**

1. **Check venue fills are being received:**
   ```bash
   ssh <node> 'curl -s http://localhost:9002/metrics | grep qip_edge_fills_confirmed'
   ```

2. **Check centre's fill booking:**
   - Centre's `LEARN` stage scores fills against the twin (counterfactual)
   - If twin prices are wrong, actual fills look bad
   - ADR 0057 requires two independent vendors for reconciliation

3. **Check venue authentication:**
   - Credential might be revoked or rate-limited
   - Venue might be rejecting fills due to account status

## References

- ADR 0008: Edge cells decide alone
- ADR 0020: Two runtime topologies and the order to resolve them
- ADR 0024: The blueprint runtime is provisioned in code
- ADR 0035: One execution node, in shadow mode, in one region
- Blueprint §41.2–§41.4: Execution node specifications
- `infrastructure/terraform/modules/execution-node/`: Terraform module
- `backend/crates/edge/qip-edge/`: Cell implementation
- `backend/crates/apps/qip-edge-node/`: Binary entry point
