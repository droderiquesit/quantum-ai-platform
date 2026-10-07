# M5 Operational Runbook

Quick reference for operators running the M5 first vertical slice. This is a narrower, faster reference than the full `RUNBOOK.md`; refer there for broader context.

## What is running

M5 runs four processes on localhost (dev) or Cloud Run (production):

| Service | Role | Health endpoint | Logs |
|---|---|---|---|
| `qip-fabricd` | Event broker (partitions, segments, archive) | `GET http://broker:8001/health` | stdout |
| `qip-ledgerd` | Fill ledger (double-entry postings) | `GET http://ledger:8002/health` | stdout |
| `qip-api` | API gateway (ledger relay) | `GET http://api:8000/health` | Cloud Logging (prod) |
| `qip-edge-node` | Edge cell (local risk, strategy) | None; exit code only | stdout |

The edge node runs as a CLI tool, not a service. It reads a tape, makes decisions, and exits.

## Health checks

### Quick check (all services)

```bash
for svc in broker:8001 ledger:8002 api:8000; do
  curl -s http://$svc/health | jq .status
done
# Expected: ready, ready, ready
```

### Deep check (by service)

**Broker:**
```bash
curl -s http://broker:8001/broker/stats | jq '{partitions, segments_sealed, produce_rate_records_per_sec, archived_bytes_total}'
# Should show: non-zero segments, non-zero produce rate
```

**Ledger:**
```bash
curl -s http://ledger:8002/api/v1/ledger/balances | jq '.[] | {account, balance, posted_fills}'
# Should show: accounts with posted_fills > 0
```

**API:**
```bash
API=http://api:8000
curl -s "$API/api/v1/ledger/balances" | jq '. | length'
# Should match ledger's count
```

## Common operations

### 1. Check if a fill reached the ledger

A fill goes: edge-node → broker (P1 outcomes) → ledger → balance sheet.

```bash
# On the broker, check if the fill was produced
curl -s http://broker:8001/broker/stream/cell:newyork-1:reflex/records \
  | jq '.[] | select(.payload.kind == "Filled") | .payload'

# On the ledger, check if it was posted
curl -s http://ledger:8002/api/v1/ledger/balances \
  | jq '.[] | select(.account == "stock:GOOG") | {balance, posted_fills}'
```

### 2. Restart a single service

```bash
# Stop (if running as a process)
pkill -f qip-fabricd

# Or via Cloud Run
gcloud run services update-traffic qip-dev-fabricd --to-revisions LATEST=0
# Wait a few seconds
gcloud run services update-traffic qip-dev-fabricd --to-revisions LATEST=100

# Verify it came back
curl -s http://broker:8001/health
# Should show: { "status": "ready", ... }
```

### 3. Replay a segment to audit the chain

Replay re-drives the edge node's decisions against the broker's recorded segment, byte-for-byte.

```bash
# Run replay
qip replay \
  --broker http://broker:8001 \
  --partition cell:newyork-1:reflex \
  --compare

# Output format:
# AGREES=8 DIFFERS=0  ← All 8 records reproduced identically
# AGREES=7 DIFFERS=1  ← 1 record differed (corruption or bug)
```

**If DIFFERS > 0:**

The segment was corrupted or the strategy changed. Compare the two runs:

```bash
qip replay \
  --broker http://broker:8001 \
  --partition cell:newyork-1:reflex \
  --diff \
  --output /tmp/replay-diff.json

cat /tmp/replay-diff.json | jq '.differences[] | {record_index, field, expected, actual}'
```

### 4. Check spool memory usage (edge node)

The edge node's spool buffers fills until the broker archives them. It should return to baseline.

```bash
# On the edge node machine
du -sh /tmp/qip-edge-data/spool
# Healthy: < 1 MB
# Unhealthy: > 10 MB (drain thread may be stuck)

# Check if drain is making progress
tail -n 5 /tmp/qip-edge-data/run.log | grep "archive"
# Should see: "archived_through: <offset>" advancing
```

### 5. Verify paper trading boundary

All four fences must be intact:

```bash
# Fence 1: Terraform refuses live ceilings
grep "autonomy_ceiling" infrastructure/environments/dev/terraform.tfvars
# Should show: autonomy_ceiling = "paper_trading"

# Fence 2: Composition roots refuse live
./target/debug/qip-api --help | grep autonomy
# (No live option shown; only paper_trading)

# Fence 3: Type system (code review)
grep -n "impl Cell" backend/crates/edge/qip-edge/src/cell.rs \
  | head -1 \
  | xargs -I {} grep -A 20 "pub fn new" backend/crates/edge/qip-edge/src/cell.rs
# Should show: fn new(...) -> Cell { ... autonomy_ceiling = PaperTrading ... }

# Fence 4: Ledger refuses live
curl -s http://ledger:8002/api/v1/ledger/status | jq .refuses_live_fills
# Should show: true
```

## Incident playbooks

### Broker is down (cannot connect)

**Symptom:** Edge node cannot send fills; fills pile up in spool.

**Diagnosis:**

```bash
# Check if process is running
ps aux | grep qip-fabricd | grep -v grep

# Check if listening
netstat -tlnp | grep 8001
# or on macOS:
lsof -i :8001

# Check logs
tail -n 20 /var/log/qip-fabricd.log
```

**Recovery:**

```bash
# Restart broker (clears all in-flight data; uses persisted segments)
pkill -f qip-fabricd
sleep 2
qip-fabricd &

# Verify it came back
curl http://broker:8001/health

# Edge node will catch up (spool drains)
watch -n 1 "du -sh /tmp/qip-edge-data/spool"
# Press Ctrl-C when size stabilizes
```

**If it doesn't restart:**

```bash
# Check disk space
df -h /tmp
# If < 10% free, archive and compress old segments

# Check data directory permissions
ls -ld /tmp/qip-fabricd-data
chmod 755 /tmp/qip-fabricd-data
```

### Ledger chain break (fills not posted)

**Symptom:** Broker shows fills but ledger balances don't move; ledger logs show "Chain break detected".

**Diagnosis:**

```bash
# On broker, get the last fill's offset
curl -s http://broker:8001/broker/stream/cell:newyork-1:reflex/records?limit=1 \
  | jq '.[-1] | {offset, previous_hash}'

# On ledger, get the tail
curl -s http://ledger:8002/api/v1/ledger/partition-tails \
  | jq '.[] | select(.partition == "cell:newyork-1:reflex")'

# Compare previous_hash values
# If they don't match, the chain is broken
```

**Recovery (test only):**

If this is a test and you want to reset:

```bash
# Delete ledger data (loses all posted fills)
rm -rf /tmp/qip-ledger-data
pkill -f qip-ledgerd

# Restart ledger
qip-ledgerd &

# It will restart at offset 0 (or the broker's earliest)
curl http://ledger:8002/health
```

**Recovery (production):**

Do not delete production ledger data. Instead:

1. **Check if broker lost data.** A broker restart with a new epoch might cause this.
   ```bash
   # Restart broker, which starts a new epoch
   pkill -f qip-fabricd
   sleep 2
   qip-fabricd &
   ```
   The ledger will see the epoch change and reset its tail.

2. **Manual tail advance (only if chain is genuinely continuous).** A skilled operator can verify the chain is intact and manually set the tail:
   ```bash
   qip ledger --data-dir /data/qip-ledger-data \
     set-partition-tail \
     --partition cell:newyork-1:reflex \
     --hash <correct-hash> \
     --offset <correct-offset>
   ```
   Do not guess. Verify the hash against the broker's segment first.

### Edge node is slow or hanging

**Symptom:** The node processes only 1–2 passes per minute instead of 10+.

**Diagnosis:**

```bash
# Check CPU and memory
ps aux | grep qip-edge-node | grep -v grep

# Check broker latency
time curl -s http://broker:8001/broker/stats > /dev/null
# Should be < 100 ms

# Check spool size (may be exerting backpressure)
du -sh /tmp/qip-edge-data/spool

# Check if it's just waiting for tape (EOF)
tail -n 1 /tmp/qip-edge-data/run.log
```

**Recovery:**

If broker is slow, restart it. If spool is full, the node is intentionally narrowing. If tape is exhausted, the node will exit normally (not an error).

```bash
# Force restart
pkill -f qip-edge-node

# Reduce tape size and retry (for testing)
# Or wait for the spool to drain
```

### Segment fails verification (DIFFERS > 0 on replay)

**Symptom:** `qip replay --compare` shows `DIFFERS > 0`.

**Diagnosis:**

```bash
# Get the differing record details
qip replay \
  --broker http://broker:8001 \
  --partition cell:newyork-1:reflex \
  --diff \
  --output /tmp/diff.json

cat /tmp/diff.json | jq '.differences[] | {record_index, field, expected, actual}'

# Compare to the original run's log
grep "record_index: <number>" /tmp/qip-edge-data/run.log
```

**If segment is corrupted:**

```bash
# Archive the bad segment
mv /tmp/qip-fabricd-data/partitions/*/segments/<bad-segment-name> \
   /tmp/qip-fabricd-data/corrupted-segments/

# Restart broker (rebuilds from good segments)
pkill -f qip-fabricd
sleep 2
qip-fabricd &
```

**If strategy changed:**

The replay is comparing against a stale run. This is expected if code was updated. The test `slice::replay_produces_identical_chain` will catch unintended strategy changes.

## Metrics to watch

If observability is configured (`workload_metrics_exist = true`):

| Metric | Healthy range | Alert threshold |
|---|---|---|
| `qip_edge_work_passes_total` | Incrementing every 1-2 sec | Flat for > 30 sec |
| `qip_edge_orders_placed_total` | Incrementing per pass | Flat for > 1 min |
| `qip_edge_refusals_total{gate="live_venue"}` | Zero (paper only) | > 0 (breach) |
| `qip_ledger_paper_boundary_refusals_total` | Zero (paper only) | > 0 (breach) |
| `qip_central_reconciliation_breaks_total` | Zero to low | > 5 (corruption) |

## Logs to watch

### Broker

**Normal:**
```
Broker listening on 127.0.0.1:8001
Partition "cell:newyork-1:reflex" ready
Segment sealed at offset 1000; archived_through 900
```

**Abnormal:**
```
Produce refused: channel full (spool pressure)
Segment seal failed: disk I/O error
```

### Ledger

**Normal:**
```
Ledger consumer starting
Connected to broker
Posted fill [id=abc; balance=$1000.00]
Partition tail updated: offset 1000, hash=sha256:...
```

**Abnormal:**
```
Chain break: previous_hash mismatch
Refusing simulated=false fill
Could not connect to broker (retry every 5 seconds)
```

### Edge node

**Normal:**
```
Cell starting; venue feed: simulated
Pass 1: 3 orders placed, 1 filled, 0 canceled
Pass 2: 2 orders placed, 0 filled, 1 canceled
```

**Abnormal:**
```
Spool full; narrowing exposure
Halt wire engaged: kill_switch
```

## When to escalate

| Condition | Action |
|---|---|
| Paper boundary refusal counter > 0 | Stop the system. Check for a security breach or test misconfiguration. |
| Reconciliation breaks accumulating | Stop the system. Run chain verification. Do not post further fills. |
| Spool growing beyond 100 MB | Restart broker and ledger. Check disk free space. |
| Edge node not progressing for > 5 minutes | Restart edge node. Check broker health. |
| Segment fails verification on replay | Archive the segment. Investigate how it was created. |

## Normal shut-down

To safely stop all M5 services:

```bash
# Stop in reverse order (edge first, API last)
pkill -f qip-edge-node
sleep 2

pkill -f qip-ledgerd
sleep 2

pkill -f qip-fabricd
sleep 2

pkill -f qip-api

# Verify all stopped
ps aux | grep qip- | grep -v grep
# (Should print nothing)
```

## Next steps if something fails

1. **Check the logs.** All processes print to stdout; capture them.
2. **Run health checks.** See "Health checks" above.
3. **Check the paper boundary.** If in doubt, run `qip verify --paper-trading`.
4. **Restart in order.** Broker → Ledger → API → Edge node.
5. **Run the acceptance tests.** `cargo test -p qip-acceptance --test slice` will reproduce the issue in a controlled way.

Reference: `docs/ops/M5_DEPLOYMENT.md` for detailed recovery procedures.
