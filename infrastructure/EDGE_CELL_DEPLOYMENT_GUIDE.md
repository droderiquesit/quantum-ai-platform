# Edge Cell Multi-Region Deployment Guide

This guide covers deploying the seven regional edge cells that form the platform's trading backbone.

## Overview

Each edge cell is a dedicated Compute Engine machine in a specific region, running the `qip-edge-node` binary under systemd. Cells decide orders locally within capital envelopes granted by the central plane and communicate with:

- **Central Plane** — receives capital envelopes, sends evidence and exposure updates
- **Venues** — market data feeds, order submission, fill reports
- **Other Cells** — cross-region capital sharing and mirror coordination

The blueprint (`docs/adr/0022`) specifies:
- One C3 or C3D machine per region (8–22 vCPU depending on venue count)
- No container runtime — binary runs bare under systemd
- One Compute Engine execution node per region (no Kubernetes)
- Default deny on all firewall rules; enable only what is needed

## Deployment Sequence

### Phase 1: Single Region (Shadow Mode)

**Objective:** Deploy one cell in the primary region (usually us-east4), observe its behavior, and confirm it matches the central plane's decisions.

**Time:** 2–4 weeks of continuous operation.

**Steps:**

1. **Create boot image** (if not already done)
   ```bash
   gh workflow run image.yml \
     -f region=us-east4 \
     -f architecture=c3-highcpu-22
   ```
   Wait for the run to complete. The workflow outputs the image self-link.

2. **Update `environments/dev/terraform.tfvars`:**
   ```hcl
   # Uncomment and set the boot image from step 1
   boot_image = "projects/<project>/global/images/qip-edge-<timestamp>"
   
   execution_nodes = {
     "us-east4" = {
       region              = "us-east4"
       zone                = "us-east4-a"
       subnet_cidr         = "10.240.0.0/24"
       node_count          = 1
       machine_type        = "c3-highcpu-22"
       shadow_mode         = true  # ← Shadow mode first
       health_port         = 8080
       watchdog_seconds    = 0
       venues = {
         # Simulated broker only in shadow mode
         "sim" = { cidr = "10.0.0.0/8", port = 443 }
       }
       region_allocation   = "500000"  # 500k base allocation
       isolated_cpus       = "2-21"    # Derived from machine_type
       create_egress_nat   = false     # No external egress needed
     }
   }
   ```

3. **Plan and review:**
   ```bash
   cd infrastructure
   terraform plan -var-file=environments/dev/terraform.tfvars | head -100
   ```
   Review that the plan creates:
   - One Compute Engine subnetwork (10.240.0.0/24)
   - One service account (qip-exec-us-east4-dev)
   - One instance template
   - One managed instance group (target_size = 1)
   - Health check rules
   - Google APIs egress rule
   - **No venue egress rules** (shadow mode)

4. **Apply:**
   ```bash
   terraform apply -var-file=environments/dev/terraform.tfvars
   ```
   Wait for the group to converge. The instance should:
   - Boot in ~2 minutes
   - Pass health checks in ~5 minutes
   - Report `HEALTHY` in the managed instance group

5. **Verify health:**
   ```bash
   gcloud compute instance-groups list-instances qip-dev-exec-us-east4 \
     --zone us-east4-a --project <project>
   gcloud compute instances describe qip-dev-exec-us-east4-<hash> \
     --zone us-east4-a --project <project>
   ```

6. **Observe** for 2–4 weeks:
   - Watch the telemetry: `qip_edge_work_passes_total`, `qip_edge_fills_confirmed_total`
   - Compare decisions against the GKE pod running the same binary in shadow mode
   - Review reconciliation breaks (`qip_central_reconciliation_breaks_total`) — some are expected

   See `.claude/rules/domains/observability.md` for telemetry documentation.

### Phase 2: Exit Shadow Mode

Once observation is complete:

1. **Change one line in `terraform.tfvars`:**
   ```hcl
   shadow_mode = false  # ← Remove venue isolation
   ```

2. **Plan to see the new rules:**
   ```bash
   terraform plan -var-file=environments/dev/terraform.tfvars | grep -A 5 "firewall"
   ```
   The plan should show:
   - Venue egress rules (one per venue the node will reach)
   - Health ingress rules (from central plane and other cells)
   - **No change to the instance itself**

3. **Apply:**
   ```bash
   terraform apply -var-file=environments/dev/terraform.tfvars
   ```

4. **Verify the node can reach the provider sandboxes:**
   Leaving shadow mode opens network paths; it does not leave paper trading.
   The cell's ceiling is paper by construction (ADR 0003), and a live order
   path requires an accepted ADR (the proposed ADR 0107 sets out what would
   have to be true first). Monitor:
   - Order placement: `qip_edge_orders_placed_total{venue}`
   - Fills: `qip_edge_fills_confirmed_total{venue}`
   - Refusals: `qip_edge_refusals_total` (should stay low)

### Phase 3: Scale to Seven Regions

Once the primary cell is stable outside shadow mode (still paper trading against simulated venues and provider sandboxes):

1. **Add secondary regions to `terraform.tfvars`:**
   ```hcl
   execution_nodes = {
     "us-east4" = {
       # … from phase 2, shadow_mode = false
     }
     "us-west1" = {
       region              = "us-west1"
       zone                = "us-west1-a"
       subnet_cidr         = "10.241.0.0/24"
       node_count          = 1
       machine_type        = "c3-highcpu-22"
       shadow_mode         = true  # ← Shadow mode until observed
       # … other config …
     }
     # Add chicago-1, newyork-1, etc.
   }
   
   # Define cross-region mirrors (ADR 0039, §31.1)
   cross_region_mirrors = [
     {
       from_region              = "us-east4"
       to_region                = "us-west1"
       rtt_ms                   = 42
       inventory_band_pct       = 2
       dislocation_threshold_pct = 10
     }
     # Add more as needed
   ]
   ```

2. **Repeat Phase 1 for each new region:**
   - Deploy in shadow mode
   - Observe for 2–4 weeks
   - Confirm decisions match the central pod
   - Exit shadow mode with a tfvars change

3. **Handle colocated regions separately:**
   Some cells (chicago-1, newyork-1, dubai-1) are not in GCP regions. These run on Compute Engine machines in nearby regions connected via partner interconnect. See `modules/connectivity` for the network path.

## Configuration Reference

### Machine Types

The blueprint permits only C3 and C3D high-CPU shapes, 8–22 vCPU:

| Shape | vCPU | RAM | Use Case |
|-------|------|-----|----------|
| c3-highcpu-8 | 8 | 32 GB | Low-venue deployments, testing |
| c3-highcpu-22 | 22 | 88 GB | Full production, many venues |
| c3d-highcpu-8 | 8 | 32 GB | Local SSD storage |
| c3d-highcpu-16 | 16 | 64 GB | Local SSD storage |

**Important:** On 8-vCPU shapes, the isolated CPU range (2–7) gives only 6 cores for 23 modules. This fits but is tight. On 22-vCPU shapes, the range (2–21) gives 20 cores, which is comfortable.

See `modules/execution-node/README.md` for §41.3 thread assignment details.

### Venue Configuration

Venues are keyed by identifier (e.g., "sim", or a provider sandbox's name). Every entry is a simulated venue or a provider sandbox; a production exchange is never listed, because the platform does not trade live (ADR 0003):

```hcl
venues = {
  "sim" = {
    cidr = "10.0.0.0/8"  # Simulated broker (always accessible)
    port = 443
  }
}
```

**Shadow Mode:** Firewall rules are empty; the node cannot reach any venue.

**Outside shadow mode:** Firewall rules are created, one per listed venue (simulated or provider sandbox). The node can reach only those venues, and its ceiling is still paper trading.

### Region Allocation

Each cell is granted a capital ceiling for its region, e.g., `"500000"` for $500k. This is:

- The maximum the cell can allocate to strategies in that region
- Bounded by blue-green replacement; if a cell is halted, its allocation stays until it recovers or the envelope expires
- Updated by the central plane via capital envelope rotations

### Cross-Region Configuration

Each cell that mirrors a venue in another region needs:

1. **Mirror declaration** in `cross_region_mirrors`
2. **Environment variable** in `node.env` (written by the startup script)
3. **Boot image** that can read and parse the mirror config file
4. **Firewall rules** (automatic: central plane and mirror-point access to health port)

The central plane discovers mirrors by reading `cross_region_mirror_path` and publishing capital share updates via envelope rotations.

## Troubleshooting

### Instance fails to start

**Symptom:** Instance shows `UNHEALTHY` after 5 minutes.

**Check:**
1. Boot logs: `gcloud compute instances get-serial-port-output <instance>`
2. Verify the image exists and is correct
3. Check kernel parameters: `isolcpus`, huge pages, swap

### Instance starts but health check fails

**Symptom:** Instance runs but `/health` returns 5xx.

**Check:**
1. SSH into the instance via IAP (no external IP)
2. `journalctl -u qip-edge-node -20` for binary logs
3. `systemctl status qip-edge-node` for unit status
4. Check that Secret Manager credentials are readable

### Node runs but does not take orders

**Symptom:** No `qip_edge_orders_placed_total` metrics.

**Check:**
1. Is `shadow_mode = false` in the tfvars?
2. Are venue firewall rules present? `gcloud compute firewall-rules list --filter 'name~qip-exec-<node-id>'`
3. Are venues reachable? Check `qip_edge_refusals_total` for `GATE_LIVE_VENUE`
4. Is the capital allocation > 0? Check `qip_edge_region_share_bound` gauge

### Node health is unstable

**Symptom:** Instance is repeatedly replaced (cycling between `RUNNING` and `CREATING`).

**Cause:** Health check is failing intermittently (GC pause, high load, etc.)

**Fix:**
1. Increase health check timeout or check interval (conservative defaults are 10s/5s)
2. Check CPU utilization — the isolated core may be hitting limits
3. Verify the binary is not crash-looping on startup

### Cross-region replication breaks

**Symptom:** `qip_central_reconciliation_breaks_total` growing.

**Check:**
1. Is the mirror-point cell in the same region? Orders should be confirmed there first.
2. Is the central plane receiving fill reports from both cells?
3. Check network latency: `qip_edge_region_share_bound` may be stale if latency is high

## Monitoring and Alerting

### Key Metrics

- **`qip_edge_work_passes_total`** — increment per cycle (should be smooth, ~100–1000/s)
- **`qip_edge_orders_placed_total{venue}`** — orders sent to each venue
- **`qip_edge_fills_confirmed_total{venue}`** — fills received
- **`qip_edge_refusals_total{gate}`** — orders rejected by pre-trade checks
- **`qip_edge_halted{source}`** — gauges whether the cell is halted (kill switch, policy, polled, journal)
- **`qip_central_reconciliation_breaks_total{direction}`** — mismatches with central plane

### Alert Policies

Example alert: "Cell not trading"
```
metric.type = "custom.googleapis.com/qip_edge_work_passes_total"
resource.labels.node_id = "us-east4"
AND value(rate(metric.value[5m])) < 10
```

Example alert: "High refusal rate"
```
metric.type = "custom.googleapis.com/qip_edge_refusals_total"
AND value(rate(metric.value[5m])) > 1000
```

See `modules/observability/main.tf` for the full alert policy configuration.

## Cost Optimization

### Per-Region Cost Breakdown

| Component | Cost | Notes |
|-----------|------|-------|
| c3-highcpu-22 (on-demand, 1 year) | ~$4,500/mo | Discount available on 1-3 year commitments |
| 100 GB PD-balanced disk | ~$4/mo | Snapshot retention adds ~$20/mo |
| Egress to venues | ~$500-5,000/mo | Depends on market data volume |
| Health monitoring | ~$50/mo | Cloud Monitoring charges |
| **Total per region** | ~$5,000-10,000/mo | Without venue data costs |

### Cost Reduction

- **Reserved instances** (1 year): 40% discount on compute
- **Resource reservation**: Avoids stockout-induced replacement failures (ADR 0099 C8)
- **Local SSD** (c3d vs c3): Adds cost but speeds up cache; not yet needed
- **Egress optimization**: Venue data feeds can be proxied or cached

## Next Steps

1. Deploy first node in dev/us-east4 (Phase 1)
2. Observe for 2–4 weeks with Shadow Mode gate enabled (ADR 0035)
3. Exit shadow mode to reach provider sandboxes, still paper trading (Phase 2) — paper is not a phase (ADR 0003); a live path requires an accepted ADR (proposed ADR 0107)
4. Deploy secondary regions one at a time (Phase 3)
5. Enable cross-region mirroring and capital sharing (ADR 0039)

---

**Related:**
- `.claude/rules/01-security-and-safety.md` — paper-trading boundary
- `.claude/rules/domains/observability.md` — telemetry and alerting
- `docs/adr/0008-edge-cells-decide-alone.md` — edge cell architecture
- `docs/adr/0035-one-execution-node-in-shadow-mode.md` — shadow mode rationale
- `docs/adr/0039-*` — capital sharing and cross-region mirroring
