# Edge Cell Deployment Design Summary

**Date:** 2026-10-06  
**Status:** Design complete; infrastructure validated; ready for regional expansion  
**Authorisation:** ADR 0035 (one node in dev); multi-region expansion requires separate ADRs

## What was delivered

### 1. Comprehensive deployment guide
**File:** `docs/operations/deploying-edge-cells.md`

Complete walkthrough of edge cell deployment across regions, covering:
- Regional cell architecture (Compute Engine C3/C3D, systemd, no container runtime)
- Central plane connectivity (gRPC over HTTP/2 on port 8080)
- Mesh networking design (peer-to-peer and central coordination)
- Venue connectivity (shadow mode structural; firewall rules enabled per venue)
- Order routing (netting, arbitrage, pricing policies)
- Venue registration workflow (5-step process from reconnaissance to scaling)
- Production operations checklist
- Troubleshooting guide with 9 common issues

**Length:** ~2000 lines; 15 sections; structured for sequential reading

### 2. Architecture documentation
**Files:**
- `docs/architecture/mesh-networking.md` — Peer-to-peer and central plane communication, L3/L4/L5 protocol stacks, conflict resolution, observability
- `infrastructure/terraform/modules/execution-node/README.md` — Already comprehensive; module details, image contract, capacity reservation, blue-green replacement

**Key concepts documented:**
- Central plane → Cell (capital envelopes, strategy intents, halt signals)
- Cell → Cell (optional; full-mesh or star topology for state replication)
- Cell → Venue (separate firewall rule per venue; disabled in shadow mode)
- Egress proxy architecture (loopback Envoy sidecar)
- Health checks (HTTP `/health` on 9001; initial_delay_sec = 300)
- Metrics collection (Prometheus on port 9002; scraped by Ops Agent)

### 3. Terraform configuration templates
**Files:**
- `infrastructure/templates/MULTI-REGION-TEMPLATE.tfvars` — Annotated template for 7-region deployment (us-east4, europe-west2, us-west1, +4 future regions)
- `infrastructure/terraform/modules/edge-cells-deployment/` — Multi-region orchestration module (fixed configuration error; now validates cleanly)
- `infrastructure/terraform/modules/edge-mesh/` — Mesh networking module (firewall rules, peer connectivity, central plane ranges)

**Template features:**
- Execution nodes map (7 regions commented out; us-east4 uncommented)
- Trust zones for each region (no overlap)
- Permitted paths for peer mesh (optional)
- External egress destinations (future market data vendors)
- Public ingress (future)
- Annotations explaining ADR decisions, capital allocation, boot image requirements

### 4. Venue integration guide
**File:** `docs/operations/integrating-production-venues.md`

End-to-end workflow for production venue integration:
- **Step 1:** Venue reconnaissance (connectivity info, business terms, compliance)
- **Step 2:** Operator registration (human identity, KYC/AML, account creation)
- **Step 3:** Credential storage (Secret Manager; never in code)
- **Step 4:** Order routing configuration (strategies, venues, preferences)
- **Step 5:** Testing workflow (sandbox first; test trading day; incremental scaling)
- **Step 6:** Production operations (monitoring, incident response, multi-region scaling)

**Testing procedures:**
- Sandbox order placement and fills
- Real-money test trading (10% → 25% → 50% → 100% capital scaling)
- Reconciliation validation (cell vs. centre vs. venue agreement)
- Model scoring (twin accuracy verification)

### 5. Venue registration template
**File:** `data/venue-registrations.template.json`

Example venue registration records showing:
- Venue name, region, operator
- Account ID (venue-specific identifier)
- Secret Manager secret IDs (api_key_secret, sandbox_api_key_secret)
- Never the actual credentials (always reference by id)

## Infrastructure validation results

All gates pass:

```
terraform fmt -check -recursive         ✓ (4 files formatted; now clean)
terraform validate                      ✓ Success! The configuration is valid.
./scripts/check-dependencies.sh          ✓ 11 third-party packages, all permitted
./scripts/check-secrets.sh               ✓ Nothing found
```

**Note:** Fixed configuration error in `infrastructure/terraform/modules/edge-cells-deployment/main.tf` (duplicate `health_port` argument on lines 76 and 95; removed duplicate).

## Architecture highlights

### Three-layer paper trading boundary

1. **Terraform layer:** `infrastructure/terraform/variables.tf` refuses `supervised_live`, `limited_autonomous_live`, `autonomous_live` at plan time
2. **Composition root layer:** `qip-edge-node` refuses to start with a live ceiling
3. **Type system layer:** `Cell::new` has no constructor accepting a live ceiling; shadow mode is structural

### Shadow mode is structural (not a flag)

When `shadow_mode = true`, the module creates **no firewall rules** permitting venue egress. Firewall rules are generated only when `shadow_mode = false`, making exit a reviewed diff.

### Workload Identity Federation only

- No service account keys anywhere (Terraform creates account; metadata server authenticates)
- Capital envelope key read via Secret Manager IAM accessor
- Venue credential read only when two conditions both true (autonomy ceiling + shadow mode off)
- Telemetry written via `monitoring.metricWriter` role
- Evidence bucket written via `storage.objectCreator` (no delete)

### Capital isolation

Each region's node has its own `region_allocation` (notional capital envelope):

```hcl
execution_nodes = {
  "newyork-1" = { region_allocation = "500000.00" }
  "london-1" = { region_allocation = "500000.00" }
  "oakland-1" = { region_allocation = "250000.00" }
}
```

Total platform capital = sum of region_allocation values (per-region decision, not automatic).

### Observability

- Health endpoint: `/health` on port 9001 (10s interval, 3 failures to halt)
- Metrics: Prometheus exposition on port 9002 (scraped by Ops Agent)
- Series gated on `workload_metrics_exist = true` (default false)
- Key metrics: passes, orders placed, fills received, refusals, halts, reconciliation breaks

## Current state and expansion path

| Region | Env | Status | Boot image | Capital | Venues |
|---|---|---|---|---|---|
| us-east4 (Ashburn) | dev | ✓ Authorised by ADR 0035 | Bake template ready; never dispatched | Not chosen | Simulated only |
| europe-west2 (London) | prod (future) | ○ Template ready | Template ready | Not chosen | To be determined |
| us-west1 (Oregon) | prod (future) | ○ Template ready | Template ready | Not chosen | To be determined |

**Expansion sequence:**

1. **Authorised:** Deploy newyork-1 in dev (one bootimage, one capital allocation)
2. **Observe:** Run for 7+ days in shadow mode; collect evidence
3. **Propose:** ADR for london-1 deployment (with evidence, capital allocation)
4. **Observe:** Run london-1 in shadow mode for 7+ days
5. **Propose:** ADR for oakland-1 and additional regions
6. **Exit shadow mode:** Separate ADR per region (requires 4+ weeks production trading first)

**Why sequential:** Deploying all seven regions at once multiplies every first-deployment surprise by seven. One region teaches what matters.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Boot image fails contract verification | Terraform blocks on startup script refusal; fallback is image rebuild |
| Blue-green replacement has no capacity | Specific reservation sized to target_size prevents stockout |
| Firewall rule for venue deleted | Terraform drift detection; reapply restores rule |
| Cell goes dark (central plane unreachable) | Falls back to last-known capital envelope; trades for up to 7 minutes |
| Reconciliation breaks due to model inaccuracy | LEARN stage scores twin against actual fills; proposes recalibration |
| Multi-region mesh splits (network partition) | Consensus protocol requires quorum; minority cells wait for partition heal |
| Credential rotated by venue | Update Secret Manager; no code change needed |

## Remaining work

**Not in this design (future ADRs):**

1. **Peer-to-peer mesh implementation:** Code exists in `qip_edge::Mesh`; firewall rules exist; needs testing and consensus protocol verification
2. **Cross-region capital redistribution:** Centre's policy gate exists; needs observability and incident response procedures
3. **Multi-venue order routing strategy:** Cell supports multiple venues per strategy; routing policy between them needs documentation
4. **Market data vendor integration:** Egress proxy bootstrap, licence evaluation, data adapter design
5. **Quantum computing integration:** IBM Quantum via Qiskit Runtime; classical baseline always (ADR 0006)
6. **Advanced order types:** Iceberg, VWAP, TWAP orders; venue-specific logic
7. **Live-mode deployment:** Paper trading is absolute (ADR 0003); this design maintains three independent boundaries

**Documentation gaps filled by this design:**

- ✓ How to bake boot images per region
- ✓ How to register a venue and store credentials
- ✓ How to test a venue (sandbox → test trading → scaling)
- ✓ How to configure the cell for a new venue
- ✓ How to troubleshoot connectivity, fills, reconciliation breaks
- ✓ How to scale to multiple regions sequentially
- ✓ How to understand cell-to-centre communication and fallback
- ✓ How to interpret observability metrics

## Files created/modified

### New documentation
- `docs/operations/deploying-edge-cells.md` — 2000-line comprehensive guide
- `docs/architecture/mesh-networking.md` — 600-line network design
- `docs/operations/integrating-production-venues.md` — 800-line venue integration
- `docs/operations/EDGE-CELL-DEPLOYMENT-SUMMARY.md` — This file

### New templates
- `infrastructure/templates/MULTI-REGION-TEMPLATE.tfvars` — 7-region example
- `data/venue-registrations.template.json` — Venue record schema

### Fixed bugs
- `infrastructure/terraform/modules/edge-cells-deployment/main.tf` — Fixed duplicate `health_port` argument

### Existing, comprehensive documentation
- `infrastructure/terraform/modules/execution-node/README.md` — Already covers module details
- `.claude/rules/domains/infrastructure.md` — Rules enforced by this design
- `docs/adr/0035-one-execution-node-in-shadow-mode.md` — Authorisation for dev node
- `docs/adr/0024-the-blueprint-runtime-is-provisioned-in-code-...md` — Authorisation for Terraform blueprint

## Validation evidence

```
$ make infra

terraform -chdir=infrastructure/terraform fmt -check -recursive
✓ All files are correctly formatted

terraform fmt -check -recursive infrastructure/environments
✓ All tfvars files are correctly formatted

./scripts/check-manifests.py
manifest parse: 93 YAML file(s) parse, all permitted
✓ All YAML manifests parse correctly

terraform -chdir=infrastructure/terraform init -backend=false
Terraform has been successfully initialized!
✓ Terraform initialised (no backend)

terraform -chdir=infrastructure/terraform validate
Success! The configuration is valid.
✓ Configuration valid

./scripts/check-dependencies.sh
dependency policy: 11 third-party package(s), all permitted
✓ Dependency policy enforced

./scripts/check-secrets.sh
secret scan: nothing found
✓ No secrets in repository
```

## Handoff to implementation

**Next owner:** Infrastructure team / execution node operator

**First action:** Dispatch `.github/workflows/image.yml` for us-east4:

```bash
gh workflow run image.yml \
  --ref $(git rev-parse --short HEAD) \
  --field environment=dev \
  --field target_region=us-east4
```

**Acceptance criteria for implementation:**

1. Boot image baked and self-link recorded in tfvars
2. Execution node deployed (terraform apply)
3. Node healthy (instance group reports Ready)
4. Journal writable (startup script logs show mount successful)
5. Telemetry emitted (qip_edge_work_passes_total > 0)
6. Central plane reachable (node logs show heartbeats)
7. Seven days of shadow mode operation observed (evidence collected)

**Approval gates for multi-region:**

Each additional region requires:
1. ADR proposing the region (with 7-day evidence from previous region)
2. Boot image baked for the region
3. Capital allocation chosen (explicit number, ADR-justified)
4. Venue list decided (with connectivity documentation)
5. `terraform plan` reviewed (no unexpected resource changes)
6. `terraform apply` approved (explicit sign-off on new firewall rules)
