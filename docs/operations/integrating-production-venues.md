# Integrating Venues (provider sandbox, paper trading only)

This document describes how a venue is integrated into a regional edge cell
deployment. **Every step here ends in a provider sandbox or the simulated
gateway. Nothing here places a live order, and no step or phase moves the
platform to real money.**

Paper trading is not a phase (ADR 0003). A live order path requires an
accepted ADR — the proposed ADR 0107, "what would have to be true before
anything moves beyond paper trading", sets out the prerequisites and is not
accepted — so this guide has no "go live" step to follow, and an operator
who finds one elsewhere should treat it as a defect and report it.

This file used to be titled "Integrating Production Venues" and ended in
"flip to production credentials", a test day "with real money", and "4+ weeks
of production trading". Those were instructions to cross the paper boundary
in a document an operator reads as procedure; they are removed rather than
qualified.

**Prerequisite:** One execution node running in shadow mode for at least 7
days (ADR 0035).

## Overview

Venue integration involves five workstreams:

1. **Venue reconnaissance:** Read the venue's sandbox API documentation
2. **Network connectivity:** Ensure the cell can reach the sandbox endpoint
3. **Credential storage:** Store sandbox credentials as files, never env values
4. **Order routing:** Configure which strategies use this venue
5. **Sandbox testing:** Validate behaviour against the provider sandbox

The cell's **capital envelope** is the throttle on paper; the sandbox's own
**order volume limits** are its own throttle. Neither is automatic.

## Step 1: Venue reconnaissance

### Gather sandbox connectivity information

Read the venue's sandbox (demo / certification) API documentation and collect:

| Item | Example | Why |
|---|---|---|
| **Sandbox gateway hostname** | `sandbox.venue.example` | For DNS; used by egress proxy |
| **Published IP ranges** | `192.0.2.0/24` (documentation range) | For firewall rules in terraform |
| **Gateway port** | 443 (HTTPS) or 8443 | Firewall rule port |
| **Fill notification channel** | WebSocket on same host, or separate URL | Architecture decision |
| **Authentication method** | API key in header, mutual TLS, etc. | `qip-brokers` adapter design |
| **Rate limits** | Sandbox order and message limits | Cell's `QIP_VENUE_*` settings |
| **Settlement terms** | T+2, as the venue models them | Risk gate configuration |
| **Market hours** | Sandbox session hours | Scheduling and envelope availability |
| **Sandbox SLA** | Fill confirmation latency | Monitoring threshold; twin calibration |

### Understand the sandbox terms

1. **Who holds the sandbox account?** One named person, never a shared
   credential. Record the role, not personal details, in this repository.
2. **What fee schedule does the sandbox model?** These feed the
   `cost_router` so the twin's costs match what the venue would charge.
3. **What does the sandbox not model?** Queue position, partial fills, and
   latency are often simplified. Record the gaps; the LEARN stage scores the
   twin against sandbox fills, and a gap nobody wrote down reads as accuracy.

## Step 2: Register for the provider sandbox

**One named operator registers for the venue's sandbox.** This person creates
the sandbox credentials and is the escalation point if sandbox orders behave
unexpectedly.

1. Request sandbox (demo / certification) access through the venue's
   documented process.
2. The venue issues sandbox credentials.
3. The operator places a test order **in the sandbox** and confirms fills
   arrive.
4. The operator stores the credential (Step 3).

**Documentation:** record the registration in `data/venue-registrations.json`
(schema in `data/venue-registrations.template.json`). Record the **secret id**
and the operator's **role** — never a personal email address, a venue account
identifier, or a credential.

## Step 3: Store sandbox credentials securely

**Never commit an API key to this repository, even in a comment or commit
message.**

Credentials are stored in Google Cloud Secret Manager and reach the execution
node as files at boot time (ADR 0024), never as environment values.

```bash
# For each credential the sandbox issues (API key, or mTLS cert and key):
gcloud secrets create <venue>-sandbox-key \
  --replication-policy="automatic" \
  --data-file=/path/to/sandbox-api.key
```

### Update the venue registration file

```json
{
  "records": [
    {
      "venue_name": "<venue>",
      "region": "<gcp-region>",
      "operator_role": "<role, e.g. cell operator>",
      "registration_date": "<YYYY-MM-DD>",
      "sandbox_api_key_secret": "<venue>-sandbox-key",
      "notes": "Provider sandbox only. Paper trading (ADR 0003)."
    }
  ]
}
```

The file records the **secret id** (the name in Secret Manager), not the
value. There is no field for a production credential, deliberately.

### Grant the cell access

The execution node's service account reads the secret through the binding
`modules/execution-node` creates. If a manual grant is needed during
diagnosis:

```bash
gcloud secrets add-iam-policy-binding <venue>-sandbox-key \
  --member=serviceAccount:<execution-node-service-account> \
  --role=roles/secretmanager.secretAccessor
```

## Step 4: Configure order routing

### At the cell: declare the venue in tfvars

```hcl
execution_nodes = {
  "<cell>" = {
    # ... other fields ...
    venues = {
      "<venue>"   = { cidr = "192.0.2.0/24", port = 443 }  # sandbox endpoint
      "simulated" = { cidr = "127.0.0.1/32", port = 9001 }
    }
  }
}
```

The execution-node module refuses a venue range of the whole internet
(`0.0.0.0/0`, `::/0`); declare the sandbox's published range, not a wide one.

### At the centre: assign strategies to this region

Each strategy's deployment manifest declares which region(s) it runs in and
which venues it may route to. The cell routes subject to depth, fees,
throughput, and the risk gates; every routing decision is journaled with its
reason.

## Step 5: Sandbox testing

Most venues offer a sandbox with the same API and no money at risk. That is
the only external execution target this platform uses.

```bash
# 1. Configure the cell to use the sandbox credential (Step 3).
# 2. Place test orders in the sandbox.
#    Observe: order confirms, fills, reconciliation.
# 3. Score the twin against sandbox fills in the LEARN stage.
```

The cell's pass path runs only against the simulated gateway (`run_pass`
takes `&mut SimulatedGateway`), and start-up refuses a simulated feed paired
with any other gateway. A sandbox adapter is exercised by its own adapter
tests, not by a deployed pass.

### Monitoring sandbox runs

```bash
# Watch refusals — GATE_LIVE_VENUE firing means something tried to route to
# a live-class venue and was refused; treat every occurrence as an incident.
gcloud logging read "resource.type=gce_instance AND jsonPayload.gate=GATE_LIVE_VENUE" --limit=20

# Watch reconciliation
curl -s http://<cell-private-ip>:9002/metrics | grep reconciliation
```

## Step 6: Operating a sandbox-integrated cell

### Checklist

- [ ] Operator trained on cell operations (how to halt, adjust the paper envelope)
- [ ] Incident runbook written (orders stuck, fills missing)
- [ ] Sandbox session hours recorded
- [ ] Cell telemetry is scraped (`workload_metrics_exist = true` only with evidence)
- [ ] Reconciliation break rate understood (systematic errors explained)

### Routine review

- **Daily:** cell health and pass count; fills reconciling.
- **Weekly:** reconciliation breaks; venue sandbox API changes; twin score.
- **Monthly:** paper activity report; fee model still matches the venue's schedule.

### Adding further regions

Once a region's sandbox integration is proven, repeat Steps 1–5 for the next
region. Each region gets its own paper envelope; strategies move to a new
region only after both cells are proven against their sandboxes.

## Troubleshooting

### Cell cannot reach the sandbox

**Symptom:** orders refused with a connectivity gate.

1. Firewall rule exists:
   `gcloud compute firewall-rules describe <firewall-rule-name>`
2. DNS resolves: `ssh <cell> nslookup <sandbox-host>`
3. Egress proxy reaches it: `ssh <cell> curl -v https://<sandbox-host>/health`

**Fixes:** update the venue `cidr` in tfvars if the sandbox range changed;
add the hostname to `egress_allowed_upstreams`.

### Sandbox authentication fails

**Symptom:** the sandbox rejects with 401.

1. The credential file is present and readable by the cell process.
2. The credential is current (sandbox keys are often rotated).
3. The adapter passes the credential where the sandbox expects it.

Do not print the credential while diagnosing; check presence and length.

### Orders are placed but fills don't come back

**Symptom:** `qip_edge_orders_placed_total` increases but
`qip_edge_fills_confirmed_total` does not.

Check the sandbox's fill channel, the cell's subscription to it, and the
cell's order book via the health endpoint or logs.

### Reconciliation breaks keep happening

**Symptom:** `qip_central_reconciliation_breaks_total` or
`qip_edge_reconciliation_breaks_total` is high.

| Break | Cause | Fix |
|---|---|---|
| Cell reports more fills than centre | Duplicated fills, or a cell/centre race | Check the sandbox fill API for duplicates; compare timestamps |
| Centre sees fills the cell didn't report | Cell report lag | Increase report frequency; check cell→centre network |
| Fill quantity mismatch | Partial fills modelled differently | Often expected with sandbox order types |

## References

- `infrastructure/terraform/modules/execution-node/`: node provisioning
- `backend/crates/edge/qip-routing/`: order routing engine
- `docs/operations/deploying-edge-cells.md`: cell deployment guide
- ADR 0003: paper trading by default and in fact — not a phase
- ADR 0035: one execution node, in shadow mode
- ADR 0008: edge cells decide alone
- ADR 0107 (proposed): prerequisites before anything moves beyond paper trading
