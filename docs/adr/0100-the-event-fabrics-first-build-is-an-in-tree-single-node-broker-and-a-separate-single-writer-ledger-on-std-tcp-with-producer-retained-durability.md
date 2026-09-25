# ADR 0100: The event fabric's first build is an in-tree single-node broker and a separate single-writer ledger on std TCP, with producer-retained durability

- **Status**: Accepted, 2026-09-25, as the synthesis of a three-architect
  design panel. It is accepted under the direction the owner gave on
  2026-09-25 ("make reasonable engineering decisions… choose the lowest-risk
  reversible decision and document the assumption"). The same instruction
  that led to ADR 0099 carries this one. **Nothing is built by this record.**
  It fixes the shape the work packets build to.
- **Date**: 2026-09-25
- **Supersedes**: nothing.
- **Amends**:
  - ADR 0091's binary count, from five to seven: `qip-fabricd` and
    `qip-ledgerd`, each argued below under ADR 0091's own test.
  - `.claude/rules/architecture/00-boundaries.md`'s "no lib performs I/O",
    for exactly one lib: `qip-transport`, stated below.
- **Related**: ADR 0099 (the v11.6 target and conflict register C1–C8). ADR
  0002, ADR 0009 and ADR 0001 (dependency posture; untouched). ADR 0089
  (retention classes). ADR 0091 (binaries). ADR 0043 (in-tree SHA-256/HMAC
  allowed; asymmetric signing and TLS not). ADR 0003 and ADR 0021 (paper
  trading; untouched).

## Context

v11.6 §26.1 and v2.1 §7 specify the Algorik Rust Event & Control Fabric: a
native broker (`fabricd`), metadata quorum, client SDK, schema registry,
archive and mirror. It carries control down and journals and outcomes up,
and it is never on the hot path. The first build phase of both blueprints
requires it: v11.6 §30 Phase 0, v2.1 §26 Phase 2 "Rust Fabric MVP". The
first working slice runs through it: v11.6 Phase 1, "one asset/venue, local
fast path, deterministic risk, ledger, paper trading, replay".

The blueprint specifies Tokio, QUIC with mTLS, prost, BLAKE3 and a Raft
crate. ADR 0099's conflict C2 leaves every one of those refused until a
dependency record of its own. The first build therefore has to live inside
`serde` + `serde_json`, blocking `std` I/O and in-tree SHA-256/HMAC. It has
to be shaped so that the later records swap internals without reshaping
components.

A design panel ran on 2026-09-25. Three architects worked independently,
with migration-first, blueprint-fidelity-first and failure-behaviour-first
emphases. Three judges scored the proposals (safety-first 139,
migration-first 134, fidelity-first 130), splitting 2–1 on the winner. A red
team then found seven blockers in the leader, each fixable with an idea from
a losing proposal. This record is the synthesis. The proposals and the
critique are working papers of that session; what they decided is recorded
here.

## Decision

### 1. Where each component lives

"Fabric" already means the **capital** fabric in this tree
(`qip-capital-fabric`, `qip-api/src/fabric.rs`). Everything here is
therefore named **event fabric**: modules `event_fabric`, variables
`QIP_EVENT_FABRIC_*`, metrics `qip_event_fabric_*`. The binary keeps the
blueprint's name `qip-fabricd`, so the trace to the blueprint is literal.

| Blueprint component | Home | Why there |
|---|---|---|
| `FabricEnvelope`, `StreamPolicy`, QoS classes, `SchemaId`, record and batch codec (CRC32C, SHA-256 chain) | `qip-events::event_fabric` (I/O-free) | `qip-events` already owns `AnyEvent`, `Topic`, retention classes and the schema registry. The envelope **wraps** `AnyEvent`, as `StreamEnvelope` does. No second envelope. |
| Reflex journal contract (`Decision`, including `Filled` with side, quote unit and fee) | `qip-contracts::reflex` | The ledger and the API must read it without depending on `qip-edge` (`api_boundary.rs`). The chain digest hashes canonical JSON, so every verifier agrees on bytes rather than on a Rust type's field order. |
| Segment store and archive | `qip-storage::segment` | Beside the WAL engine, whose frame and torn-tail rules it copies. The **same** type is the edge spool. There is one on-disk format, used by the spool, the wire and broker segments. A batch is written once by the drain and appended as-is. |
| Broker core: partitions, producer table, groups, QoS admission | `qip-streaming::event_fabric` | Beside `DurableLogTransport` and the ports it implements. It is a library, so every guarantee gets a property test without a process. |
| Client SDK and protocol | `qip-transport::event_fabric` | The in-tree HTTP/1.1 client, retry, breaker, bounded queue and spool are already there, and `qip-edge` already depends on the crate. |
| HTTP/1.1 **server** | moved from `qip-api/src/http.rs` to `qip-transport::server` | A second hand-written server parser would be new attack surface. The reviewed, bounded one moves mechanically, and `qip-api` keeps a re-export. **This is the one amendment to 00-boundaries**: `qip-transport` is the in-tree protocol stack and the only lib that owns sockets, in both directions. It already owns the client side. No other lib gains I/O. |
| `fabricd` | **new binary `apps/qip-fabricd`** | See §2. |
| Ledger writer | **new binary `apps/qip-ledgerd`**, with posting logic in `qip-portfolio` (pure double-entry) | See §2. |
| Reflex ring → spool → drain | `apps/qip-edge-node::event_fabric` | The node's composition, behind the existing `qip_edge::journal::Mirror` seam. The `qip-edge` crate gains no fabric type, so FABRIC-003 holds structurally. |
| `fabricctl`, `fabric-inspect`, verifiers | `qip event-fabric …` subcommands in `qip-cli` | `qip-cli` is already the operator tool. Verifiers follow `qip replay`'s AGREES=0 / DIFFERS=3 discipline. |
| `fabric-mirror`, `fabric-bridge`, Bigtable/BigQuery/Spanner sinks | not built | Mirror: BLOCKED(C8). Bridge: not needed. Managed sinks: BLOCKED(C4, C2). |

### 2. Two new binaries, argued under ADR 0091's own test

ADR 0091 admits a binary for "a writer of the log or a clock of its own".

- **`qip-fabricd`** is both. It is the sole writer of every partition's batch
  chain, and it runs its own clock for segment roll, retention and archive. It
  cannot live in `qip-api`, which scales out and to zero: a broker there is two
  writers or none. It cannot live in a brain, because v2.1 §22 requires the
  fabric to keep journaling when a brain dies. It cannot live on the reflex
  host, because FABRIC-003 keeps the broker off the hot path.
- **`qip-ledgerd`** is the sole writer of the ledger chain. Two judges and the
  red team rejected placing it in `qip-api`, for three reasons: ADR 0091 names
  the API "the one workload that can scale"; it runs with a memory store on
  Cloud Run; and `api_boundary.rs` says financial state "does not belong here".
  It serves its own read endpoint, and `qip-api` relays the view as decimal
  text. Keeping it separate from the broker keeps two chains in two custodies.
  That is custody separation, not cryptography (ADR 0043's third gap stands).

### 3. Durability, stated as what it is

- **RF1 with `fsync` before every acknowledgement.** One broker, one disk, per
  region. The high watermark is the last *fsynced* offset, never the last
  appended one.
- **Static leader epoch**, persisted and bumped at every start. There is no
  consensus. In-tree replication with in-sync replicas and automatic failover
  was proposed and **rejected**: it is hand-rolled distributed coordination,
  the thing the tree's own doctrine calls "ADR 0009's mistake wearing
  different clothes" (`qip-transport/src/spool.rs`). Replication waits for
  C2's consensus record.
- **Producer-retained durability.** The producer's spool keeps every record
  until the broker reports that the sealed segment holding it has been
  archived. The report is `archived_through` on every produce acknowledgement
  and metadata response. The spool budget is sized against the seal cadence,
  and an end-to-end test asserts spool bytes return to baseline after
  archive.
- **The gap is named.** FABRIC-077 and FABRIC-086 ("no acknowledged record
  lost to a broker, zone or device failure") are **not met** by this build,
  and the matrix scores them BLOCKED(C2). Anyone reading "the fabric" must not
  read it as replicated.

### 4. Ordering, idempotency and fencing

- Per-partition order only. There is no global order, and none is claimed.
  Each cell's journal is its own partition key, so a gap or integrity break
  parks one cell rather than every cell that shares a partition.
- **Producer identity is `(producer_id, epoch)`. The sequence is assigned at
  drain time**, dense per `(stream, partition)`, and is not the spool
  position. P2 shedding therefore cannot punch holes the broker would refuse
  forever. A shed window is an explicit `Gap` record the broker accepts.
- **Deduplication** is a same-epoch sequence match whose payload hash must
  equal the cached batch's, within a bounded window. A new epoch **fences**
  the old one, and sequences carry over across epochs, so a restarted node
  neither duplicates nor loses. Event ids use a persisted, monotonic session
  counter. A second-granular start time is not enough, because it collides in
  a crash loop.
- The ledger deduplicates by **chain continuity per partition key**. It does
  not use a bare `offset <= consumed` rule, which would drop a legitimately
  re-appended record after broker loss. Its durable state is balances, chain
  tails and consumer offsets in one `DurableStore` `WriteBatch`, never an
  in-memory `EventLog` that fills on a date.

### 5. Quality-of-service classes

These are declared in a committed stream catalogue. Nothing rests on a broker
default (FABRIC-051).

| Class | Carries | Overload behaviour |
|---|---|---|
| P0 control | capital grants, policy, halt, package announcements, signed ledger watermarks | never dropped; the producer is refused and narrows |
| P1 outcomes | fills, cancels, settlement records, `ChainSpan` continuity records | never dropped; the producer is refused and narrows |
| P2 market journal | every reflex journal entry (ADR 0089's `EventAnchored` class) | throttled; an explicit `Gap` if shed |
| P3 research | episodes, knowledge deltas | backlog allowed |
| P4 telemetry | derived telemetry events | sampled or shed first |

Raw ticks stay `Transient` under ADR 0089. What is journaled is book state
at each of the cell's own decisions, not a feed firehose.

### 6. The hot path never waits

The decision thread hands a batch to a bounded channel with `try_send`, and
that is all it does. A spool thread writes segments, and a drain thread
produces. Control compilation, halt-wire polling and file reads move off the
decision thread. Spool pressure becomes a fourth **reading-style** halt wire,
applied every pass and failing engaged, in the same discipline as the
existing polled halt. It narrows sizing and then halts new exposure before
the spool budget is exhausted; cancels and confirms continue. Passes are
driven by a timer, not by health probes.

### 7. Security, honestly scoped

- Plaintext TCP inside the VPC. mTLS is BLOCKED(C2), and the matrix says so.
- **Bearer-token identity per producer**, following the precedent in
  `qip-api/src/auth.rs`, with a key-scoped produce ACL: a cell can write only
  its own partition key.
- **P0 watermarks and grants are HMAC-signed** through the existing
  sign/verify path. An unauthenticated record can therefore never widen a
  restriction.

### 8. The first vertical working slice

```
canonical market-event tape ──> qip-edge-node (QIP_VENUE_FEED=simulated only)
   seeded SimulatedGateway ─> Cell: book, features, strategy predicate,
   deterministic local risk/feasibility gate, simulated fill
      └─ try_send ─> ring ─> segment spool ─> drain ──TCP──> qip-fabricd
                                                  P2 journal, P1 outcomes
qip-ledgerd <── consumes P1 ── balanced double-entry postings, once in effect
   └─ read endpoint ──> qip-api relays /api/v1/ledger/... (decimal text)
control down: `qip event-fabric grant` (a labelled fixture, signed with the dev
   key) ──P0──> qip-edge-node, through the SAME verify code as the mesh downlink
every process: /metrics      replay: re-drive run_pass from recorded inputs
```

- **Reuse:** the tape pattern from `qip-market-ingestion` (known-at ordering
  refused, not sorted); `SimulatedGateway` seeded from the tape; the existing
  `Cell`, `Mirror` seam and `CapitalDownlink::absorb` verify path; `DurableStore`
  `WriteBatch`; `qip replay`'s verdict discipline.
- **Fix before it can run:**
  - nothing in the node seeds the venue today, so the tape does;
  - strategies compile against an empty feature catalogue, so it gets
    populated;
  - `Decision::Filled` lacks the side, quote unit and fee a posting needs, so
    they are added;
  - the mesh uplink drops reports under load, so **the slice runs with the
    mesh uplink off**, with control on P0 and outcomes on P1/P2.
- **Replay** records only exogenous inputs: tape events, control frames and
  clock ticks. It rebuilds the seeded simulated venue and re-drives `run_pass`
  verbatim. It does not wrap the venue in a recording layer, which would
  widen paper layer 3's typing. Deterministic comparisons run on a
  `ManualClock`: the same tape and seed are run healthy and stalled, never
  two wall-clock runs.
- **Proving tests** spawn real processes on localhost:
  1. the happy path;
  2. a fabric outage, with identical decisions and the ledger catching up
     after restart;
  3. a stalled fabric, with no change in pass latency;
  4. duplicate delivery, posted once;
  5. producer fencing;
  6. a ledger outage;
  7. replay, with the digest chain equal byte for byte;
  8. spool bytes returning to baseline after archive.

### 9. Paper trading stays intact

The three layers are untouched, and this build adds a fourth fence without
relaxing any of them:
- the tape feed is accepted only with `QIP_VENUE_FEED=simulated`;
- the replay venue can answer only from records and has no send path;
- **`qip-ledgerd` refuses any fill whose `simulated` flag is not true**,
  which is a fourth, independent paper fence.

## What it costs

- **Two more binaries to build, attest, image and operate.**
  `infrastructure.rs`'s binary lists, the image matrix and ADR 0010 each gain
  two entries.
- **RF1.** Until C2's consensus record, a regional fabric is one disk. The
  producer-retained spool and the ledger's own store narrow the loss window,
  but they do not close it.
- **A socket-owning lib.** `qip-transport` becomes explicitly what it already
  was in half. The rule it bends is stated in 00-boundaries rather than left
  as precedent.
- **The mesh uplink is off for the slice.** Until outcomes move onto the fabric
  for the central plane too, fills have two possible routes. The slice runs
  with one of them off, rather than letting two claims about one fact
  disagree.
- **The grant is a fixture.** The first slice proves P0 control over the
  fabric with an operator-signed fixture, not the central plane's own issue
  path. Wiring the central plane is a named follow-on packet.

## Alternatives rejected

- **Eleven new crates in the blueprint's component shape** (fidelity-first).
  It gave the most fidelity and the least reuse: the largest review surface,
  and ownership gaps in the acceptance suites. Its swap-seam idea is kept:
  `FabricTransport`, `PayloadCodec` (with an encoding byte in every batch
  header), and a `ContentHash { Sha256, Blake3 }` tag in seals.
- **The ledger writer inside `qip-api`** (migration-first). See §2.
- **In-tree replication with in-sync replicas, high watermark and automatic
  failover.** See §3.
- **Wrapping the venue in recording and replay placers** (safety-first). It
  widens the pass's venue typing, which is paper layer 3. Recording exogenous
  inputs gives the same replay without touching it.
- **Keeping capital and policy on the mesh for the slice.** It leaves
  FABRIC-007 unproven and depends on a central-plane path that is itself
  unproven end to end.
- **Waiting for C2 before building anything.** That would stall the critical
  path on a dependency decision, when the semantics can be built and proven
  now and the transport swapped later behind a named seam.

## What would make this wrong

- **Any path by which the decision thread waits on the fabric.** That includes
  a blocking send, a file read or a lock the drain holds. FABRIC-003 is the
  requirement the whole shape exists to keep.
- **The ledger posting a fill twice, or silently skipping one,** under
  duplicate delivery or after broker loss and resend.
- **The spool not returning to baseline in a healthy run.** Then producer
  retention has no release signal, and every healthy cell narrows and halts.
- **Replication appearing in-tree before C2's record.**
- **A second lib opening a socket** on the strength of this record's
  exception for `qip-transport`.
- **The fixture grant path surviving past the follow-on packet**, and becoming
  how grants are issued.
