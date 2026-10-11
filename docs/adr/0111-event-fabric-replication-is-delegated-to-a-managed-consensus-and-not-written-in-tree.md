# ADR 0111: Event-fabric replication is delegated to a managed consensus, and is not written in-tree

- **Status**: **Accepted**, 2026-10-11, by the owner in the session
  conversation, adopting the recommendation as drafted: "Update as needed
  remove my decisions make the recommended". Drafted as *Proposed* the same day. **No longer blocked on the dependency
  rule** (ADR 0109, accepted 2026-10-11, including its Amendment 2, which
  also permits an async runtime). Blocked on its own acceptance, on ADR 0112
  (TLS to Google APIs) and on ADR 0113 (Spanner).
- **Date**: 2026-10-11
- **Resolves**: owner decision D1 in `docs/blueprint/v12-completion-plan.md`;
  ADR 0099/0101 conflict C2 for the fabric's replication.
- **Supersedes, on acceptance**: ADR 0100 §3's "RF1 … Replication waits for
  C2's consensus record" — **for the P0 and P1 classes only**. This is that
  record. ADR 0100's refusal of *in-tree* replication ("hand-rolled
  distributed coordination") is kept, and kept deliberately even though
  ADR 0109's Amendment 2 no longer forbids it.
- **Keeps**: ADR 0100 §4 (per-partition order, drain-time sequences, epoch
  fencing), §6 (the hot path never waits, ADR 0103), ADR 0104 (HMAC-sealed
  batch chain), ADR 0105 (ledger dedup by chain continuity), ADR 0106.

## Context

`qip-fabricd` is one broker with one disk per region
(`qip-fabricd/src/lib.rs`), and `StreamPolicy::new` refuses any replication
factor other than 1 (`qip-events/src/event_fabric/policy.rs`, the C2
refusal). The register scores the consequence: FABRIC-7035 and FABRIC-7038
INCORRECT, FABRIC-8086 INCORRECT ("quorum" that is one broker), FABRIC-7037
MISSING (no controller), RES-7112 MISSING. ADR 0100 named the gap honestly —
"no acknowledged record lost to a broker, zone or device failure" is not met —
and deferred it to a consensus record.

At real-money scale the classes that matter are **P0 control** (grants,
policy, halts, package announcements, signed watermarks) and **P1 outcomes**
(fills, cancels, settlements, chain-continuity records): "never dropped"
under ADR 0100 §5. They are low volume — one record per decision that
changed capital or control, not per tick. **P2** (the market journal) is high
volume and already tolerates loss as an explicit `Gap`; P3 and P4 shed by
design.

The owner's draft named two shapes: an audited Raft (`openraft`) or Google
Cloud Managed Service for Apache Kafka.

## Decision

### 1. P0 and P1 are made durable in Spanner; consensus is Spanner's

- Each regional `qip-fabricd` acknowledges a P0 or P1 produce **only after a
  Spanner read-write transaction commits** the batch to a table keyed
  `(stream, partition, sequence)`, carrying the batch's sealed watermark (ADR
  0104) and payload hash. The high watermark for those classes is the last
  *committed* sequence. A Spanner commit is a majority-acknowledged Paxos
  write across three zones of a regional instance — the property
  FABRIC-7035/8086 ask for, bought rather than built.
- **Leadership is a lease row in the same database**: `(region, partition) →
  (leader_id, epoch, expires_at)`. Every produce transaction reads the lease
  and commits only if its epoch is still the holder's, so a deposed leader's
  write fails at commit — fencing by the database, not by a clock. Two or
  three `qip-fabricd` instances per region, in different zones, contend for
  the lease; the standby takes it after expiry and bumps the epoch, which
  ADR 0100 §4's epoch fencing and sequence carry-over already handle on the
  producer side.
- Spanner is reached by the in-tree REST adapter over `rustls` with
  metadata-server tokens (ADR 0113 §2), from the `qip-fabricd` composition
  root.

### 2. P2, P3 and P4 stay RF1, producer-retained, archived

Unchanged from ADR 0100 §3 and §5. A broker or disk loss on P2 surfaces as a
`Gap` record, which the design already accepts; P3 and P4 shed. **Escalation
rule:** if a committed measurement of P2 volume and loss shows the journal
itself must survive a zone loss, the next record admits Google Cloud Managed
Service for Apache Kafka for P2 alone, with its client chosen then.

### 3. `StreamPolicy::new` admits exactly what this delivers

It admits `replication = Managed` for P0 and P1 and keeps refusing any
replication factor it cannot honour for the rest. A policy that says RF3
while one disk holds the data is FABRIC-8086's INCORRECT verdict again, so the
refusal stays structural and only the true claim is admitted.

### 4. Cross-region mirroring is not decided here

RES-7114 and CONTRACT-7092 (fabric-mirror, lag metric, stale marking) are
ADR 0101 C8 and ADR 0113's placement questions. A multi-region Spanner
configuration would carry P0/P1 across regions, at multi-region commit
latency; whether the fabric's P0/P1 tables live in the regional or the
multi-region instance is decided in 0113, not assumed here.

## Consequences

- An acknowledged P0/P1 record survives the loss of a broker, a disk or a
  zone. ADR 0100's named gap closes for the classes that move capital and
  control.
- P1 outcomes and the ledger (ADR 0113 §1) live in the same database. The
  ledger's posting can commit in the same transaction that marks the outcome
  consumed, which makes ADR 0105's chain-continuity dedup a check inside one
  transaction rather than across two stores. It is **not** a second source of
  truth: the outcome record and the posting derived from it are two facts.
- Commit latency (milliseconds to tens of milliseconds, unmeasured here) is
  on the fabric's produce path and never on the cell's decision thread (ADR
  0103's `try_send` holds). Spool pressure absorbs it.

## What it costs

- Spanner capacity per region (minimum processing units per instance;
  unpriced here, `dev` billing disabled) and a per-mutation cost that scales
  with P0/P1 volume.
- A dependency on Google's control plane for control and outcomes. A region
  whose fabricd cannot reach Spanner stops acknowledging P0/P1, so producers
  narrow and then halt new exposure through the spool-pressure wire (ADR 0100
  §6). Cells keep deciding inside their envelope (ADR 0008); they stop
  growing. That fails closed, and it is an availability coupling the in-tree
  RF1 design did not have.
- Two code paths in the broker: managed for P0/P1, local for P2–P4.

## Alternatives rejected

- **`openraft` across three fabricd instances.** Admissible since ADR 0109's
  Amendment 2 removed the async objection. Rejected as the first choice
  because the crate supplies the algorithm and this repository would write
  the log storage, snapshot install, network transport and membership
  changes — the parts where Raft deployments lose acknowledged writes — and
  then operate a consensus group per region. **It is the named fallback** if
  the owner rejects the Spanner coupling above; the record that takes it must
  carry a linearisability test under injected partitions.
- **Managed Kafka for every class now.** Real replication and a log model
  that maps onto partitions and producer epochs. Rejected for now: no
  measurement says P2 needs it, the mature client binds C `librdkafka` with
  its own OpenSSL — a second TLS stack beside `rustls` — and a pure-Rust
  async client is far less audited. Kept as section 2's escalation.
- **In-tree replication (ISR, high watermark, failover).** ADR 0100 rejected
  it; Amendment 2 makes it permitted, not wise. Consensus fails silently
  under a partition schedule nobody wrote a test for (ADR 0109 §5).
- **Cloud Pub/Sub.** At-least-once with redelivery and ordering keys; it
  does not give dense per-partition sequences or epoch fencing, so ADR
  0100 §4 would have to be rebuilt on top.

## What would make this wrong

- An acknowledged P0/P1 record found missing after a broker, disk or zone
  loss — the property this record exists for.
- Two fabricd instances committing for one partition in the same epoch: the
  lease fencing is not doing its job.
- Any `.await`, Spanner call or lease check reaching the cell's decision
  thread.
- `StreamPolicy` admitting a replication claim for P2–P4 that no mechanism
  honours.
- Spanner availability in a region proving worse than a single local disk in
  practice — then the coupling cost is larger than the loss it prevents, and
  the `openraft` fallback is the answer.

## Dependency direction

`StreamPolicy`'s new arm lives in `qip-events` (lib, I/O-free). The broker
core's managed-commit port is a trait in `qip-streaming` (service); its
Spanner implementation and the lease loop live in `apps/qip-fabricd`, the
composition root, which holds the token and the transport. `qip-edge` gains
nothing. No lib performs I/O; no service depends on the runtime; nothing
depends on an app. `rustls` is reached from `qip-transport` (the named
socket-owning lib) and the app; no decision-core crate gains a dependency.

## R12 ids it unblocks

Addressed in shape: **R12-FABRIC-7035**, **R12-FABRIC-7037** (the lease is
the controller for leadership), **R12-FABRIC-7038**, **R12-FABRIC-8085**,
**R12-FABRIC-8086**, **R12-FABRIC-8096** (multi-broker group across zones),
**R12-RES-7112**, **R12-RES-8103** (leadership transfer by lease; the upgrade
runbook automation is separate work). Not addressed and still open:
**R12-FABRIC-8075** and **R12-FABRIC-8102** (partition move tooling),
**R12-RES-7114** and **R12-CONTRACT-7092** (cross-region mirror; ADR 0113).
Whether "managed Paxos" satisfies requirements worded as "RF3 majority-ack"
is the register's call on evidence, not this record's.

## The paper-trading boundary

Untouched. Replication carries records; it opens no order path. ADR 0106's
ledger fence, the three original layers and `Determinism::Required` stand.

## Gates

None run: no shell in the session that wrote this record.
