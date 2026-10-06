# Integrating Production Venues

This document describes the end-to-end workflow for integrating a production venue into a regional edge cell deployment.

**Prerequisite:** One execution node running in shadow mode for at least 7 days (ADR 0035).

## Overview

Venue integration involves five parallel workstreams:

1. **Venue onboarding:** Register with the venue's operator
2. **Network connectivity:** Ensure the cell can reach the venue
3. **Credential storage:** Securely store venue API keys
4. **Order routing:** Configure which strategies use this venue
5. **Testing and rollout:** Validate before exiting shadow mode

The cell's **capital envelope** is the throttle; the venue's **order volume limits** are its own throttle. Neither is automatic.

## Step 1: Venue reconnaissance

### Gather connectivity information

Contact the venue (or read their API documentation) and collect:

| Item | Example | Why |
|---|---|---|
| **Order gateway hostname** | `api.nasdaq.com` | For DNS; used by egress proxy |
| **Published IP ranges** | `209.85.230.0/24`, `209.85.231.0/24` | For firewall rules in terraform |
| **Order gateway port** | 443 (HTTPS) or 8443 | Firewall rule port |
| **Fill notification channel** | WebSocket on same host, or separate URL | Architecture decision |
| **Authentication method** | API key in header, mutual TLS, etc. | `qip-brokers` adapter design |
| **Rate limits** | 1,000 orders/second, 100 MB/s | Cell's `QIP_VENUE_*` settings |
| **Account requirements** | Min. balance, approved for algorithmic trading | Registration blocker |
| **Settlement terms** | T+2 trades, margin available | Risk gate configuration |
| **Market hours** | 9:30–16:00 EST | Scheduling and envelope availability |
| **Venue's SLA** | Fill confirmation < 100ms p95 | Monitoring threshold |

### Understand the business contract

1. **Who registers?** Identify one person who will hold the account (never a service account or shared credential). This person is accountable for compliance and audit.

2. **What are the fees?** Collect the fee schedule (maker/taker, per-share or per-transaction, rebates if any). These feed the `cost_router` at order time.

3. **What compliance is required?**
   - Anti-money laundering (AML) checks on the registered person
   - Account approval for algorithmic trading (often requires written test plan)
   - Market conduct training (depends on jurisdiction)
   - Real-time surveillance obligations (some venues require monitoring or callbacks)

4. **What's the liability model?** If the cell places an erroneous order, who reverses it? (Venues differ on this.)

## Step 2: Register the operator

**One named operator registers with the venue.** This person:

- Holds the account identity and is legally liable for the account's conduct
- Creates the API credentials (the venue issues them)
- Is the escalation point if orders go wrong
- Signs compliance attestations

**Procedure:**

1. **Contact the venue's institutional sales team** (not self-service portal; venues want to know their automated traders)
   - Introduce the platform: "Algorik, a quantitative trading platform"
   - State the intent: "We want to place equity orders on your exchange"
   - Name the operator: "Alice Johnson will be the account holder"

2. **Venue sends:** Account application form (requires KYC/AML info on Alice)

3. **Operator completes:** Application (typically 1–2 weeks for approval)

4. **Venue creates:** Account and issues credentials (usually an API key or certificate)

5. **Operator tests** (in venue's sandbox first): Places a test order, confirms fills work

6. **Operator stores credential securely** (see Step 3 below)

**Documentation:** Create a file `data/venue-registration-<venue>.md` recording:
- Registration date
- Operator name and email
- Venue account id
- Any special terms or account restrictions

## Step 3: Store credentials securely

**Never commit an API key to this repository, even in a comment or commit message.**

Credentials are stored in Google Cloud Secret Manager. The execution node reads them as files at boot time.

### Create secrets in Cloud Secret Manager

```bash
# For each credential the venue issues (e.g., API key, mTLS cert):

# 1. Create the secret
gcloud secrets create nasdaq-prod-key \
  --replication-policy="automatic" \
  --data-file=- <<< "$(cat /path/to/api.key)"

# (If mTLS certificate, store both cert and key)
gcloud secrets create nasdaq-prod-cert \
  --replication-policy="automatic" \
  --data-file=- <<< "$(cat /path/to/client.crt)"

gcloud secrets create nasdaq-prod-key-cert \
  --replication-policy="automatic" \
  --data-file=- <<< "$(cat /path/to/client.key)"

# 2. Record which secret ids you created
echo "nasdaq-prod-key nasdaq-prod-cert nasdaq-prod-key-cert" > credentials-created.txt
```

### Update the venue registration file

Edit `data/venue-registrations.json`:

```json
{
  "records": [
    {
      "venue_name": "nasdaq",
      "region": "us-east4",
      "operator": "alice@example.com",
      "registration_date": "2026-10-06",
      "account_id": "ACME-12345",
      "api_key_secret": "nasdaq-prod-key",
      "sandbox_api_key_secret": "nasdaq-sandbox-key",
      "notes": "Production account for newyork-1"
    }
  ]
}
```

**Important:** The file records the **secret id** (the name in Secret Manager), not the value.

### Grant the cell access

The execution node's service account must be able to read the secrets:

```bash
# Automatic via Terraform (modules/execution-node/main.tf):
# The venue_credential binding is created when shadow_mode is off

# To manually grant (if needed):
gcloud secrets add-iam-policy-binding nasdaq-prod-key \
  --member=serviceAccount:qip-exec-newyork-1-prod@algorik-platform-prod.iam.gserviceaccount.com \
  --role=roles/secretmanager.secretAccessor
```

## Step 4: Configure order routing

Order routing is configured at two levels:

### At the cell: Declare the venue in tfvars

```hcl
# In environments/prod/terraform.tfvars

execution_nodes = {
  "newyork-1" = {
    # ... other fields ...
    venues = {
      "nasdaq" = { cidr = "209.85.230.0/24", port = 443 }
      "nyse"   = { cidr = "209.85.231.0/24", port = 443 }
      "simulated" = { cidr = "127.0.0.1/32", port = 9001 }  # Fallback
    }
  }
}
```

### At the centre: Assign strategies to this region

Each strategy's deployment manifest declares which region(s) it runs in:

```yaml
# In infrastructure/gitops/envs/prod/<strategy>.yaml

apiVersion: qip.algorik.ai/v1
kind: Strategy
metadata:
  name: arbitrage-us-equities
spec:
  placement:
    regions: ["us-east4"]  # This strategy only runs in newyork-1
  venues:
    - nasdaq
    - nyse
  # ... rest of strategy config ...
```

### At the cell: Order placement preferences

Each order the centre sends includes a **venue preference** (one or more venues). The cell respects this preference subject to:

- **Orderbook depth:** If the order is large, the cell may split across venues
- **Fees:** If one venue is significantly cheaper, the cell prefers it
- **Throughput:** If one venue fills faster, the cell favours fill certainty
- **Feasibility:** Risk gate checks (e.g., account balance, position limits)

The cell's routing decision is recorded in the order journal with the reason.

## Step 5: Testing workflow

### Test in sandbox first

Most venues offer a sandbox (demo) environment with the same API but no money risk.

```bash
# 1. Get the sandbox credentials from the venue
# 2. Store in Secret Manager as a separate secret
gcloud secrets create nasdaq-sandbox-key --data-file=- <<< "..."

# 3. Configure the cell to use sandbox (in QIP_BROKERS env)
# This is an environment configuration, not a Terraform change

# 4. Place test orders in sandbox
# Observe: order confirms, fills, reconciliation
# Typical throughput: 10–50 orders/second (test API limits)

# 5. When satisfied: Flip to production credentials
```

### Execute a test trading day (with real money, limited size)

Once sandbox tests pass:

1. **Capital envelope:** Start with a small allocation (e.g., 10% of regional cap)
   ```hcl
   region_allocation = "50000.00"  # 10% of 500k
   ```

2. **Exit shadow mode:** In a targeted, reviewed change:
   ```hcl
   execution_nodes = {
     "newyork-1" = {
       shadow_mode = false  # NEW: Out of shadow mode
       # ... rest ...
     }
   }
   ```
   This change enables the firewall rules permitting venue egress.

3. **Run for one trading day:** Monitor closely.
   - Orders placed and confirmed?
   - Fills received?
   - Any refusals or errors?
   - Reconciliation matches?

4. **Review results:** Meeting, with:
   - Cell operator
   - Strategy owner
   - Compliance officer
   - Risk manager

5. **Scale up:** Increase capital allocation incrementally (10% → 25% → 50% → 100%)

### Monitoring during test trading

Real-time:
```bash
# Watch fills
watch -n 1 'gcloud logging read "resource.type=gce_instance AND jsonPayload.venue=nasdaq" --limit=5 --format=table(timestamp,jsonPayload.message)'

# Watch refusals
gcloud logging read "resource.type=gce_instance AND jsonPayload.gate=GATE_LIVE_VENUE" --limit=20

# Watch reconciliation
curl -s http://<cell-private-ip>:9002/metrics | grep reconciliation
```

Post-trading:
```bash
# Export fills and reconcile against venue's API
# Use: cell's /journal endpoint (if available) + venue's REST API

# Score the twin against actual fills (LEARN stage)
# Check: "Did the model predict this fill correctly?"
```

## Step 6: Production operations

### Pre-production checklist

- [ ] Operator trained on cell operations (how to halt, scale capital, etc.)
- [ ] Incident runbook written (what to do if orders are stuck or fills don't come)
- [ ] Venue's trading hours marked in internal calendar
- [ ] Venue's support contact (phone number, email) is posted
- [ ] Cell telemetry is scraped (ADR 0032 — workload_metrics_exist = true)
- [ ] Alert policies for this venue are configured
- [ ] Reconciliation break rate is < 0.1% (systematic errors understood)
- [ ] Two operators have practiced manual order cancellation (if needed)

### Post-launch monitoring

**Daily:**
- Check cell health and pass count
- Verify fills are reconciling
- Monitor capital utilization (should stay < 90% of envelope)

**Weekly:**
- Review reconciliation breaks (any systematic issues?)
- Check venue's published security incidents or API changes
- Score the twin (is the model still accurate?)

**Monthly:**
- Review trading activity report (orders, fills, P&L)
- Check venue fees are correct
- Audit credential rotation (if applicable)

### Scaling to multiple regions

Once the first region is proven (4+ weeks of production trading):

1. **Repeat Steps 1–5 for the next region** (e.g., london-1 for europe-west2)
2. **New region gets its own capital allocation** (independent decision)
3. **Strategies gradually deployed to new region** (only after both cells proven)
4. **Monitor cross-region capital management** (centre distributes across regions)

## Troubleshooting

### Cell cannot reach the venue

**Symptom:** Orders placed but immediately refused with `GATE_CONNECTIVITY` or similar.

**Checks:**
1. **Firewall rule exists:**
   ```bash
   gcloud compute firewall-rules describe qip-prod-exec-newyork-1-venue-nasdaq
   ```

2. **DNS resolution works:**
   ```bash
   ssh <cell> nslookup api.nasdaq.com
   ```

3. **Egress proxy reaches the venue:**
   ```bash
   ssh <cell> curl -v https://api.nasdaq.com/health 2>&1 | grep -i "connected"
   ```

**Fixes:**
- Venue's IP range changed? Update `cidr` in tfvars and reapply.
- Egress proxy bootstrap missing venue's hostname? Add to `egress_allowed_upstreams` in tfvars.
- Network path blocked upstream (ISP, VPC, firewall)? Check with network team.

### API authentication fails

**Symptom:** Orders placed but venue rejects with 401 Unauthorized.

**Checks:**
1. **Credential is readable by the cell:**
   ```bash
   ssh <cell> cat /run/qip/secrets/venue-credential
   ```

2. **Credential is current (not revoked or expired):**
   ```bash
   # Contact venue's support
   ```

3. **Cell is passing the credential in the right place:**
   ```bash
   ssh <cell> curl -v https://api.nasdaq.com/orders \
     -H "Authorization: Bearer $(cat /run/qip/secrets/venue-credential)"
   ```

**Fixes:**
- Credential was rotated by the venue? Update Secret Manager
- Credential was stored incorrectly? Re-create the secret
- Cell expects a different header? Update the `qip-brokers` adapter

### Orders are placed but fills don't come back

**Symptom:** `qip_edge_orders_placed_total` increases but `qip_edge_fills_confirmed_total` does not.

**Checks:**
1. **Venue's fill notification API is working:**
   - Venue's test API accepts orders?
   - Venue's WebSocket is broadcasting fills?
   - Venue's REST endpoint `/orders` returns the placed orders?

2. **Cell is listening for fills:**
   ```bash
   ssh <cell> curl http://localhost:9002/metrics | grep fills
   ```

3. **Cell's order book contains the orders:**
   ```bash
   # Check the order state via health endpoint or logs
   ```

**Fixes:**
- Venue changed their fill notification format? Update adapter
- Cell's subscription to fill channel failed? Restart the cell
- Venue is rate-limiting? Reduce order rate or increase QIP_VENUE_RATE_LIMIT

### Reconciliation breaks keep happening

**Symptom:** `qip_central_reconciliation_breaks_total` or `qip_edge_reconciliation_breaks_total` is high.

**Types of breaks:**

| Break | Cause | Fix |
|---|---|---|
| Cell reports more fills than centre | Cell received extra fills (fills duplicated or cell/centre race condition) | Check venue's fill API for duplicates; compare timestamps |
| Centre sees fills cell didn't report | Centre saw fills that cell hasn't reported yet | Increase report frequency; check cell→centre network |
| Fill quantity mismatch | Cell partial-filled differently than centre expected | Normal for venues with complex order types; not necessarily an error |

**Diagnosis:**
```bash
# Get the most recent breaks from central plane logs
gcloud logging read "resource.type=cloud_run_revision AND jsonPayload.reconciliation_break" --limit=10

# Compare with cell's orders
ssh <cell> curl http://localhost:9002/metrics | grep -i reconciliation
```

## References

- `infrastructure/terraform/modules/execution-node/`: Node provisioning
- `backend/crates/edge/qip-routing/`: Order routing engine
- `backend/crates/libs/qip-brokers/`: Venue adapters
- `docs/operations/deploying-edge-cells.md`: Cell deployment guide
- ADR 0035: One execution node, in shadow mode
- ADR 0008: Edge cells decide alone
- ADR 0003: Paper trading by default
