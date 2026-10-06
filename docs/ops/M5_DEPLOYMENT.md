# M5 Deployment Guide: First Vertical Slice

This guide covers deploying the M5 first vertical slice (one end-to-end event path) to the dev environment. M5 is designed to be deployed and verified locally first, then optionally to a cloud environment.

**Status:** This covers local deployment and testing. Cloud deployment (dev/test/stage) follows the main `deployment-path.md`.

## Prerequisites

### Local development machine

| Requirement | Version | How to check |
|---|---|---|
| Rust | 1.94.1 from `backend/rust-toolchain.toml` | `rustc --version` |
| Disk space | 20 GB free (build artifacts) | `df -h /` |
| Postgres (optional) | 14+ | Only if testing ledger persistence; omit for memory store |
| Docker (optional) | 20+ | Only if testing broker in a container |

### GCP project (dev only)

If deploying to cloud dev environment (`algorik-platform-dev`):
- Billing must be enabled (`gcloud billing projects describe algorik-platform-dev`)
- Cloud Run API must be enabled
- A state bucket must be initialized with `terraform init`
- See `infrastructure/environments/dev/README.md` for the full setup

## Architecture of M5 (single machine)

```
┌─────────────────────────────────────────────────┐
│ qip-edge-node (QIP_VENUE_FEED=simulated)        │
│  ├─ SimulatedGateway (seeded with tape)         │
│  ├─ Cell (deterministic local risk gate)        │
│  └─ Mirror (journal spool; try_send to ring)    │
├─────────────────────────────────────────────────┤
│ Bounded ring channel (in-memory queue)           │
├─────────────────────────────────────────────────┤
│ qip-fabricd (on localhost:8001)                 │
│  ├─ Broker (partitions, producer table, groups) │
│  └─ Spool pressure halt wire                    │
├─────────────────────────────────────────────────┤
│ qip-ledgerd (on localhost:8002)                 │
│  ├─ Consumer of P1 outcomes chain                │
│  ├─ DurableStore (balances, partition tails)    │
│  └─ Read endpoint (decimal decimals on stdout)  │
├─────────────────────────────────────────────────┤
│ qip-api (on localhost:8000)                     │
│  └─ Relays /api/v1/ledger/* from ledger read    │
└─────────────────────────────────────────────────┘

Data flow:
  tape ──> edge-node ──> try_send ──> ring ──> fabricd (P2, P1)
                                        ├─> ledgerd (P1)
                                        └─> qip-api (relay)

Control flow:
  fixture grant (P0) ──> edge-node ──> fabricd verify
```

## Step 1: Local Build

All commands run from the repository root.

```bash
# Full workspace build (includes tests)
cd backend
cargo build --workspace

# Or, build only the slice binaries
cargo build -p qip-api -p qip-fastbrain -p qip-deepbrain \
  -p qip-edge-node -p qip-fabricd -p qip-ledgerd -p qip-cli

# Expected time: ~2 minutes cold, ~30 seconds warm
# Expected size: ~2 GB in target/debug
```

**Verify build succeeded:**

```bash
ls -la backend/target/debug/qip-{api,edge-node,fabricd,ledgerd} | wc -l
# Should print 4
```

## Step 2: Prepare Configuration

M5 uses a committed catalogue file. No operator configuration is required; the binaries read:

```bash
# Broker configuration (paths are relative to --data-dir)
# Partition: "cell:newyork-1:reflex" (the cell's own journal)
# QoS classes: P0 (control), P1 (outcomes), P2 (market events)

# Broker listens on
export QIP_FABRICD_LISTEN_ADDR=127.0.0.1:8001

# Ledger configuration
export QIP_LEDGERD_LISTEN_ADDR=127.0.0.1:8002
export QIP_LEDGERD_DATA_DIR=/tmp/qip-ledger-data

# Edge node configuration
export QIP_VENUE_FEED=simulated
export QIP_EDGE_NODE_FABRIC_BROKER=http://127.0.0.1:8001

# API configuration
export QIP_API_LISTEN_ADDR=127.0.0.1:8000
export QIP_API_LEDGER_URL=http://127.0.0.1:8002/api/v1/ledger

# Optional: override data directories for test isolation
export QIP_FABRICD_DATA_DIR=/tmp/qip-fabricd-data
export QIP_EDGE_NODE_DATA_DIR=/tmp/qip-edge-data
```

## Step 3: Start the Broker

The broker must start first and listen before the edge node connects.

```bash
# Terminal 1: Start broker (blocks)
mkdir -p /tmp/qip-fabricd-data
cd backend
QIP_FABRICD_LISTEN_ADDR=127.0.0.1:8001 \
QIP_FABRICD_DATA_DIR=/tmp/qip-fabricd-data \
timeout 60 ./target/debug/qip-fabricd

# Expected output:
# Broker listening on 127.0.0.1:8001
# Partition "cell:newyork-1:reflex" initialized
# [ready; ctrl-C to stop]
```

**Verify broker health:**

```bash
# Terminal 2: Health check (while broker runs)
curl -s http://127.0.0.1:8001/health | jq .
# Should print: { "status": "ready", "uptime_seconds": <N> }
```

## Step 4: Start the Ledger

In a separate terminal:

```bash
# Terminal 2: Start ledger (blocks)
mkdir -p /tmp/qip-ledger-data
cd backend
QIP_LEDGERD_LISTEN_ADDR=127.0.0.1:8002 \
QIP_LEDGERD_DATA_DIR=/tmp/qip-ledger-data \
timeout 60 ./target/debug/qip-ledgerd

# Expected output:
# Ledger consumer starting; broker at 127.0.0.1:8001
# Connected to broker; consuming from partition "cell:newyork-1:reflex"
# [ready; ctrl-C to stop]
```

**Verify ledger health:**

```bash
# Terminal 3: Health check
curl -s http://127.0.0.1:8002/health | jq .
# Should print: { "status": "ready", "uptime_seconds": <N> }
```

## Step 5: Start the API

```bash
# Terminal 3: Start API (blocks)
cd backend
QIP_API_LISTEN_ADDR=127.0.0.1:8000 \
QIP_API_LEDGER_URL=http://127.0.0.1:8002/api/v1/ledger \
timeout 60 ./target/debug/qip-api

# Expected output:
# API listening on 127.0.0.1:8000
# Ledger relay ready
# [ready; ctrl-C to stop]
```

**Verify API health:**

```bash
curl -s http://127.0.0.1:8000/health | jq .
```

## Step 6: Run the Edge Node (Happy Path)

```bash
# Terminal 4: Start edge node
mkdir -p /tmp/qip-edge-data
cd backend
QIP_VENUE_FEED=simulated \
QIP_EDGE_NODE_FABRIC_BROKER=http://127.0.0.1:8001 \
QIP_EDGE_NODE_DATA_DIR=/tmp/qip-edge-data \
timeout 30 ./target/debug/qip-edge-node

# Expected output:
# Cell starting; venue feed: simulated
# Connected to broker; writing to partition "cell:newyork-1:reflex"
# Pass 1: 3 orders placed, 1 filled
# Pass 2: 2 orders placed, 0 filled
# ...
# [exits after 30 seconds or when tape exhausted]

# Count passes and fills
grep "^Pass" /tmp/qip-edge-data/run.log | wc -l
```

## Step 7: Verify the M5 Tests

Run the acceptance suite for M5 to verify all eight test cases:

```bash
cd backend
cargo test -p qip-acceptance --test slice -- --nocapture --test-threads=1

# Expected output:
# test slice::the_happy_path_runs_end_to_end ... ok
# test slice::duplicate_delivery_is_posted_once ... ok
# test slice::broker_outage_ledger_catches_up ... ok
# test slice::stalled_broker_does_not_stall_decisions ... ok
# test slice::producer_fencing_advances_epoch ... ok
# test slice::ledger_outage_ledger_recovers ... ok
# test slice::replay_produces_identical_chain ... ok
# test slice::spool_returns_to_baseline_after_archive ... ok

# test result: ok. 8 passed; 0 failed
```

**Important:** These tests spawn real processes on localhost. If you run them concurrently with manual servers above, ports will conflict. Stop the manual servers first.

## Step 8: Verify Chain Integrity

After the happy path, inspect the chain:

```bash
# Inspect sealed segments (if using a file-based store)
ls -lh /tmp/qip-fabricd-data/partitions/*/segments/ | head -5

# Count records in the broker
qip event-fabric inspect --broker http://127.0.0.1:8001

# Verify chain with the replay tool
qip replay \
  --broker http://127.0.0.1:8001 \
  --partition cell:newyork-1:reflex \
  --compare

# Expected: "AGREES=8; DIFFERS=0" (or AGREES=N for N records replayed)
```

## Deployment to Cloud (Dev Environment)

Once verified locally, deploy to `algorik-platform-dev`:

```bash
# Step 1: Ensure infrastructure is initialized
cd infrastructure
terraform -chdir=environments/dev init

# Step 2: Plan the infrastructure
terraform -chdir=environments/dev plan

# Step 3: Apply (or dispatch infra.yml workflow)
# Local apply:
terraform -chdir=environments/dev apply

# Or via GitHub Actions:
gh workflow run infra.yml \
  -f action=up \
  -f environment=dev
```

**Wait for:**
- Cloud Run services to reach Ready status
- `/metrics` endpoints to respond
- Ledger to complete reconciliation from broker

## Rollback

### Local

```bash
# Stop all processes (Ctrl-C in each terminal)
pkill -f qip-fabricd
pkill -f qip-ledgerd
pkill -f qip-api
pkill -f qip-edge-node

# Clean data
rm -rf /tmp/qip-*-data
```

### Cloud (dev)

```bash
# Revert the promotion commit (GitOps rollback)
git revert <promotion-commit-sha>
git push origin main

# Or re-promote earlier freight via Kargo UI
kargo get freight --project qip-dev
kargo promote qip-dev test <earlier-freight-id>

# Verify rollback
gcloud run services describe qip-dev-api
# Should show the previous revision in status.traffic
```

## Troubleshooting

### Broker fails to start

**Error:** `Address already in use`

```bash
# Check which process is listening
lsof -i :8001
# Kill it
kill <PID>
```

**Error:** `Permission denied writing to /tmp/qip-fabricd-data`

```bash
chmod 755 /tmp/qip-fabricd-data
rm -rf /tmp/qip-fabricd-data
mkdir -p /tmp/qip-fabricd-data
```

### Edge node cannot connect to broker

**Error:** `Connection refused: 127.0.0.1:8001`

```bash
# Verify broker is running
curl http://127.0.0.1:8001/health

# Verify edge node environment variable
echo $QIP_EDGE_NODE_FABRIC_BROKER
# Should be: http://127.0.0.1:8001
```

### Ledger reports partition tail mismatch

**Error:** `Chain break detected; previous_hash mismatch`

This is expected if:
1. The broker was restarted and re-sealed the same segment (new epoch).
2. The ledger data was deleted but the broker data was not.

**Recovery:**

```bash
# Option 1: Reset both stores (lose data)
rm -rf /tmp/qip-{fabricd,ledger}-data
# Restart both services

# Option 2: Manually advance the ledger's tail (for testing)
qip ledger --data-dir /tmp/qip-ledger-data \
  set-partition-tail \
  --partition cell:newyork-1:reflex \
  --hash <new-hash> \
  --offset <new-offset>
```

### Spool grows without shrinking (producer retained)

**Expected in the first pass:** The spool fills as the drain thread initializes. It should return to baseline within a few seconds.

**Check spool size:**

```bash
du -sh /tmp/qip-edge-data/spool
# Healthy: returns to baseline (~100 KB) after archive
# Unhealthy: keeps growing (>1 MB)
```

**If unhealthy:**

```bash
# Check if drain thread is alive
ps aux | grep qip-edge-node

# Check if broker is accepting produces
curl http://127.0.0.1:8001/broker/stats | jq .produce_rate

# Check if segments are being archived
ls -lh /tmp/qip-fabricd-data/partitions/*/archive/
```

## Next Steps

After M5 is verified locally and in dev:

1. **M6 (Core platform functioning):** Deploy the ingestion service and risk engine.
2. **M7 (Blueprint functional completion):** Add the central plane and real venues (still in paper mode).
3. **M8 (Reliability/security):** Security review, observability gates, performance tuning.

## References

- ADR 0100: Event fabric design and M5 specification
- ADR 0103–0106: M5-specific sub-decisions
- `docs/implementation/critical-path.md`: M5 packet breakdown
- `docs/ops/deployment-path.md`: Full production deployment flow
- `docs/operations/RUNBOOK.md`: Operator playbooks
