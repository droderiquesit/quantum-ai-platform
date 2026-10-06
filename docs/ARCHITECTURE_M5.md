# M5 Architecture: First Vertical Slice

**Last updated:** 2026-10-06  
**Status:** Accepted (ADR 0100, ADR 0103–0106)  
**Scope:** One end-to-end event path: tape → edge cell → event broker → ledger → API

## Overview

M5 is the first working vertical slice of the platform: a complete data flow from market input to recorded decision outcome. The slice proves:

1. **Deterministic local decisions** under the cell's risk gate
2. **Event sourcing** with immutable, chained segments
3. **Exact attribution** (double-entry ledger)
4. **Paper trading boundaries** at four independent levels
5. **Replay and auditability** from recorded inputs

The slice runs on a single machine (localhost or one Cloud Run service) and is designed to be extended later without changing the topology.

## Components

### 1. Edge Node (qip-edge-node)

**Role:** The trading cell. Reads a tape of market events, makes local decisions, and produces fills.

**Inputs:**
- Simulated market events (tape file)
- Control messages (halt wires, grants)
- Funding and strategy configuration (in code)

**Outputs:**
- Journal entries (decisions, fills, cancels) → broker partition
- Spool state → drain thread
- Metrics (`/metrics` endpoint)

**Key properties:**
- Runs on `QIP_VENUE_FEED=simulated` only (fence 3)
- Deterministic: same tape + seed = identical output
- Non-blocking: `try_send` to spool, no wait on broker (ADR 0103)
- Halt-aware: reads halt wires every pass, narrows on spool pressure

**Boundaries:**
- Never reads from broker (one-way data flow)
- Cannot send orders to real venues (simulated only)
- Cannot access live capital (paper ceiling enforced by type system)

### 2. Event Fabric Broker (qip-fabricd)

**Role:** Durable log and consumer group manager. Stores fills and control messages in partitions, manages producer offsets and consumer lags.

**Topology:**
```
Partition: cell:newyork-1:reflex
  ├─ Segment 0: records 0–999 (sealed, on disk)
  ├─ Segment 1: records 1000–1999 (sealed, archived)
  └─ Segment 2: records 2000–N (open, receiving)
```

**QoS Classes:**
| Class | Name | Carries | Behavior |
|---|---|---|---|
| P0 | Control | Grants, halt signals, signed watermarks | Never dropped |
| P1 | Outcomes | Fills, cancels, settlement records, chain spans | Never dropped |
| P2 | Journal | Every decision journal entry | Throttled; shed on overload |
| P3 | Research | Episodes, knowledge deltas | Buffered |
| P4 | Telemetry | Derived metrics | Sampled |

**Durability (ADR 0100 §3):**
- **RF1 + fsync.** One replica, one disk. Watermark = last fsynced offset (not appended).
- **Producer-retained.** Edge node's spool keeps fills until broker archives them.
- **Static epoch.** Bumped only on broker restart. No in-tree Raft (C2 blocker).

**Watermarks (ADR 0104):**
Each sealed segment batch is signed with HMAC-SHA256:
```
watermark = HMAC-SHA256(
  key = HKDF(master_key, partition, epoch),
  message = canonical_json(previous_watermark || batch_metadata || records)
)
```
This creates an immutable linked list; changing any byte breaks downstream watermarks.

**Key design:**
- Broker is sole writer of segment chains (multiple writers would require consensus).
- Supports reader group offset tracking (for ledger, for replay, for future consumers).
- Partition key scopes access: each producer can only write its own partition.

### 3. Ledger Writer (qip-ledgerd)

**Role:** Posts fills to durable balances. Maintains double-entry accounting and chain continuity.

**State (all in one atomic WriteBatch):**
```
Balances table:
  account | balance | posted_fills | last_update
  ---
  stock:GOOG | $10,000.00 | 5 | 2026-10-06T14:22:31Z

Partition tails table (deduplication by chain):
  partition_key | last_hash | last_offset | created_at | updated_at
  ---
  cell:newyork-1:reflex | sha256:... | 500 | ... | ...

Consumer offsets table:
  consumer_group | partition | offset | created_at | updated_at
  ---
  ledger | cell:newyork-1:reflex | 500 | ... | ...
```

**Deduplication (ADR 0105):**
On each fill from broker:
1. Read partition tail for the fill's key
2. Compute watermark of the incoming fill
3. If watermark matches tail's `last_hash`, fill is new → post it
4. Else, fill is duplicate → skip it
5. On mismatch (watermark ≠ tail), chain is broken → alert, do not post

**Paper boundary (ADR 0106):**
Every fill with `simulated=false` is refused, even if upstream fences fail. Refusal is logged and counted but does not halt the ledger.

**Read endpoint:**
- `/api/v1/ledger/balances` — all accounts and their balances (decimal text)
- `/api/v1/ledger/partition-tails` — deduplication state for debugging
- `/health` — readiness (connected to broker)

**Single-writer principle (ADR 0100 §2):**
- Only `qip-ledgerd` writes to the ledger store.
- API reads and relays only.
- Central plane (in later slices) will have its own ledger or will consume from this one.

### 4. API Gateway (qip-api)

**Role:** Relays ledger reads to the outside world, forwards control to the mesh.

**Endpoints:**
```
GET  /health                              — readiness
GET  /api/v1/ledger/balances              — relay from ledger (decimal text)
GET  /api/v1/ledger/partition-tails       — for debugging
POST /api/v1/control/grant                — issue a control grant (signed fixture)
```

**Control path:**
```
Fixture grant (signed with dev key) ──> API ──> broker (P0) ──> edge node verify
```

In M5, the grant is a labelled fixture, not issued by the central plane. In M6+, the central plane will issue grants through the API.

**Security:**
- No secret reaches the API (read from Secret Manager at startup only)
- No routing to real venues (simulated only)
- All state is read-only relay from ledger

## Data Flow

### Happy Path: One Pass

```
1. Edge node reads one market event from tape
        ↓
2. Cell applies deterministic risk gate
        ↓
3. Decision thread places order with simulated venue
        ↓
4. SimulatedGateway fills the order instantly (fills are instant in simulation)
        ↓
5. Decision thread creates Decision::Filled record
        ↓
6. try_send to bounded ring channel (non-blocking; ADR 0103)
        ↓
7. Drain thread reads from ring, seals batch, writes to spool (disk)
        ↓
8. Drain thread produces batch to broker (TCP) on P1 stream
        ↓
9. Broker appends, watermarks, seals segment (every N records or every T seconds)
        ↓
10. Broker sends archived_through hint; spool can release old records
        ↓
11. Ledger consumer reads batch from broker
        ↓
12. Ledger verifies chain continuity (ADR 0105)
        ↓
13. Ledger posts fill to balance sheet in atomic WriteBatch
        ↓
14. API relays balance to `/api/v1/ledger/balances`
        ↓
[Complete]
```

**Latency budget:** 
- Decision thread: < 1 ms (no I/O wait)
- Drain thread: < 10 ms (local disk write + broker TCP send)
- Broker watermark + segment seal: < 100 ms
- Ledger post: < 50 ms
- Total: < 200 ms per fill (single-threaded, no pipelining)

### Backpressure: Spool Full

If drain thread falls behind:
1. Ring channel fills (bounded)
2. Next `try_send` returns `Err::Full`
3. Decision thread treats as spool pressure halt signal
4. Next pass: cell narrows (reduces order size or exposure)
5. Passes continue; no latency stall

This is the key to ADR 0103: backpressure is a *control signal*, not a *wait*.

### Chain Break: Broker Loss

If broker is stopped and restarted:
1. Broker epoch advances (static epoch increments)
2. HMAC key changes (epoch-dependent)
3. Ledger sees watermark mismatch (different key, invalid hash)
4. Ledger skips fill, records break, alerts operator
5. Broker kept the segment; operator can decide:
   - Reset ledger (lose record)
   - Manually advance tail (if chain is genuinely continuous)
   - Investigate log for root cause

## Testing Strategy

The eight real-process tests (ADR 0100 §8) cover:

| Test | Scenario | Verifies |
|---|---|---|
| 1. Happy path | One tape, one pass, end-to-end | Data flows through all four services correctly |
| 2. Outage + recovery | Broker stopped and restarted mid-stream | Ledger detects break, catches up on restart |
| 3. Stalled broker | Broker network delay injected | Cell decisions not stalled; try_send succeeds |
| 4. Duplicate delivery | Broker re-sends same batch | Ledger posts only once (chain continuity) |
| 5. Producer fencing | Edge node epoch advances | Ledger skips old sequences, accepts new ones |
| 6. Ledger outage | Ledger stopped and restarted | Ledger resumes from partition tail; no double-post |
| 7. Replay | Re-drive inputs with ManualClock | Output is byte-for-byte identical to original |
| 8. Spool baseline | Drain completes archive | Spool memory returns to baseline (no leak) |

All tests spawn real processes; none use mocks. All tests use the acceptance-test verdict format (AGREES/DIFFERS).

## Boundaries and Guarantees

### What M5 proves

✓ **Determinism.** Same tape + same seed = identical output (test 7)  
✓ **Idempotency.** Duplicates skipped once (test 4)  
✓ **Atomicity.** Balances and tails updated in one WriteBatch or not at all  
✓ **Chain integrity.** Watermarks verify; no silent corruption (tests 1–7)  
✓ **Paper boundary.** All four fences intact (tests 1–8)  
✓ **Latency isolation.** Decision thread never waits (test 3)

### What M5 does not prove

✗ **Replication.** RF1 only; a disk failure is loss. (Blocked on C2: Tokio/Raft)  
✗ **Multi-region.** One broker, one disk per region. No synchronous replication.  
✗ **Real venues.** Simulated only. `QIP_VENUE_FEED=simulated` enforced at start.  
✗ **Central plane.** Mesh uplink is off. No policy, no grants issued by platform.  
✗ **Cost routing.** No routing decision based on venue cost. Simulated venues only.

These are **intentional limitations**, named in ADR 0100 "What it costs" and "Alternatives rejected". They are blocking on conflicts C2, C4 (ADR 0099) and will be addressed in M6+.

### Paper trading fences (ADR 0003, 0021, 0100, 0106)

| Layer | Check | Enforced by | Reversal condition |
|---|---|---|---|
| 1. Terraform | Live ceiling refused at plan time | `variables.tf`, `required_variable_validation` | ADR amending this record |
| 2. Composition root | Live ceiling refused at start-up | `AutonomyLevel::deployable` in each binary | ADR giving permission |
| 3. Type system | No paper-trading constructor with live | `Cell::new()` type signature | Unsafe code + ADR; both denied here |
| 4. Ledger | Fill with simulated=false refused | `qip-ledgerd` data validation | ADR amending this record |

All four must fail independently to allow a live order. Breaking any one does not bypass the others.

## Extension Points (for M6+)

The M5 slice is designed to be extended without reshaping:

### Central Plane Integration
```
Central → qip-fabricd (policy on P0) → edge-node
           ↓
         qip-ledgerd (ledger reads, posts fills)
```
The fabric bandwidth and ledger deduplication are already sized for central input.

### Multi-Region
```
Region 1 broker ── registry sync ──> Archive storage (S3)
Region 2 broker ─← registry sync ─── Archive storage (S3)
```
Each region runs an independent M5 slice; archive provides durability across regions.

### Provider sandboxes
```
edge-node → broker (P2 journal) → central → sandbox connector → provider sandbox
```
A provider sandbox may stand in for the simulated venue; it is still paper trading. No connector to a production venue exists or is planned: paper trading is not a phase (ADR 0003), and a live path would require an accepted ADR (the proposed ADR 0107) and code changes, not a deployment variant.

## Deployment Variants

### Local Development
```
All four binaries on localhost:8000–8002
Memory store for ledger (no persistence)
QIP_VENUE_FEED=simulated
Tape input from file or fixture
```

### Cloud (Dev)
```
qip-api on Cloud Run
qip-fabricd and qip-ledgerd on Cloud Run (future: execution node)
qip-edge-node runs as Cloud Run Job or locally
Memorystore for ledger (still no persistence for M5)
QIP_VENUE_FEED=simulated
```

### Shadow Mode (M7+)
```
Same as cloud, but:
Real venue connector reads live ticks (no orders sent)
Central plane computes what it would trade
Edge node produces hypothetical fills
Ledger posts to a shadow balance sheet
Operator reviews hypothetical trades; shadow mode is not a step towards live trading
```

## Performance Characteristics

### Latency
- Decision thread: < 1 ms (no I/O)
- E2E per fill: < 200 ms (single-threaded, no pipelining)
- Replay: < 10 ms per record (ManualClock, no sleep)

### Throughput
- Decision: 1 order every 1–10 ticks (venue-dependent)
- Broker: 10,000 records/sec (no bottleneck in M5)
- Ledger: 1,000 posts/sec (write-batched; no contention)

### Durability
- Spool: all records until `archived_through`
- Segments: all sealed records until expiry
- Balances: all posted fills forever

### Scalability
- M5 is single-machine, single-threaded decision.
- Bottleneck: decision thread CPU (not I/O).
- Scaling to multiple cells requires per-cell fabric partition (future work).

## Related Architecture Decisions

| ADR | Topic | Impact on M5 |
|---|---|---|
| 0003 | Paper trading by default | Fence 1–3 mandated |
| 0021 | No live capital path | Fence 1–3 enforced structurally |
| 0099 | v11.6 / v2.1 as record | Requirement set for M5 scope |
| 0100 | Event fabric design | Entire M5 topology |
| 0103 | Non-blocking try_send | Decision latency isolation |
| 0104 | HMAC-SHA256 watermarks | Chain integrity proof |
| 0105 | Chain continuity dedup | Idempotency without coordination |
| 0106 | Ledger simulated=false refuse | Fourth paper fence |

## Glossary

**Chain continuity:** The hash of each batch includes the hash of the previous, creating an immutable linked list. A break (mismatched hash) means the segment was corrupted or the ledger has seen an out-of-order batch.

**Deduplication by watermark:** Instead of tracking offsets, the ledger tracks the hash of the last batch and verifies that the next batch's previous_hash matches. This survives broker loss (which may re-send a batch).

**Spool pressure:** When the ring channel is full, the next `try_send` fails. The decision thread treats this as a signal to narrow (reduce orders) in the next pass. Not a wait; a control signal.

**Watermark chain:** Each batch seals with an HMAC-SHA256 of itself and the previous batch's watermark. Verifying the chain requires knowing the HMAC key; reading the chain is public.

**Fence:** An independent layer that refuses a live order. M5 has four fences at different levels (Terraform, process startup, type system, data validation).

## References

- `docs/adr/0100-*.md` — Event fabric design
- `docs/adr/0103–0106.md` — M5 sub-decisions
- `docs/ops/M5_DEPLOYMENT.md` — Step-by-step deployment
- `docs/ops/M5_RUNBOOK.md` — Operational procedures
- `docs/implementation/critical-path.md` — M5 packet breakdown (57 packets, 10 waves)
- `backend/crates/tests/qip-acceptance/tests/slice.rs` — Eight real-process tests
