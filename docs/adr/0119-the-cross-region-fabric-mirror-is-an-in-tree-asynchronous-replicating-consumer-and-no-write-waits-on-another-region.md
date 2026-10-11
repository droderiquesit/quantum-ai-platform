# ADR 0119: The cross-region fabric mirror is an in-tree asynchronous replicating consumer, and no write waits on another region

- **Status**: **Accepted**, 2026-10-11, by the owner in the session, under the
  instruction: "You make the recommended decision and have full permission".
- **Date**: 2026-10-11
- **Resolves**: the mirror half of ADR 0099/0101 conflict C8, which ADR 0111
  §4 left open ("Cross-region mirroring is not decided here") and handed to
  this question and to ADR 0113's placement.
- **Keeps**: ADR 0035 (one execution node, one region, in `dev`), ADR 0008
  (cells decide alone), ADR 0100 §4 (per-partition order, epoch fencing),
  ADR 0104 (HMAC-sealed batch chain), ADR 0111 (P0/P1 durable in a
  **regional** Spanner instance), ADR 0112 (mTLS between Compute Engine
  processes, across regions included), ADR 0113 §1 (the ledger alone is
  multi-region) and §4 (the project split at the second region).

## Context

`StreamPolicy` refuses every mirroring policy other than `None`, naming C8
(`grep -n 'fabric-mirror is not built' backend/crates/libs/qip-events/src/event_fabric/policy.rs`).
The register scores four rows on it: R12-FABRIC-7038 (fabric-mirror,
selective async cross-region replication, "each region stays independently
writable"), R12-RES-7114 (mirror down: local streams writable, lag visible
and bounded, no synchronous cross-region quorum, stale marking, replay from
archive on return), R12-OBS-9148 (a mirror-lag metric) and, by dependency,
R12-CONTRACT-7092.

Three shapes were available, and one of them is ruled out by the
requirement's own words:

1. **Multi-region Spanner for the fabric's P0/P1 tables.** Every produce
   would commit through a cross-region quorum. That is the "global write
   quorum" R12-FABRIC-7038 forbids and the coupling R12-RES-7114 is written
   against: a remote outage would stop local P0/P1 acknowledgement, and with
   it — through the spool-pressure wire — new exposure in a region that is
   otherwise healthy. ADR 0113 already put the fabric's tables in **regional**
   instances for latency; this record confirms that for availability too.
2. **The archive as the only transport.** `qip-fabricd`'s archiver already
   writes sealed segments to Cloud Storage. Cheap and asynchronous, but its
   lag is a segment roll, which makes "lag visible and bounded" a statement
   about minutes, and a consumer elsewhere would learn of a halt in another
   region late.
3. **An in-tree mirror process**: a consumer of committed records in the
   source region that produces them into a region-qualified stream in the
   target region's fabric. This is not consensus. It is a consumer and a
   producer with a checkpoint, which is the shape the platform already
   writes and tests for every other consumer.

## Decision

### 1. `fabric-mirror` is a replicating consumer, written in-tree

- A mirror runs **in the target region** and pulls from the source region's
  `qip-fabricd` over mTLS (ADR 0112), reading only **committed** records —
  for P0/P1, records at or below the managed log's committed sequence
  (ADR 0111 §1); for a selected P2 stream, records below the broker's high
  watermark.
- It produces each record into a **region-qualified** stream in the target
  fabric (`<source-region>.<stream>`), carrying the source
  `(region, stream, partition, sequence)` and the source batch's sealed
  watermark (ADR 0104) unchanged. A mirrored record is a copy of a fact the
  source region holds, never a new fact: nothing in the target region may
  write to a region-qualified stream except the mirror, and the mirror may
  write nowhere else. That is the rule that keeps this from becoming a second
  source of truth (`00-boundaries.md`).
- **Idempotence by source position.** The mirror's checkpoint is the last
  source sequence per partition it has had acknowledged by the target. A
  restart resumes from the checkpoint; a duplicate is recognised by its source
  position and dropped, and a gap (a source sequence missing) is journaled as
  a `Gap` record in the target stream rather than skipped silently.
- **Selection is a stream policy, not a flag.** `StreamPolicy` gains one arm,
  `Mirroring::Async { targets }`, admitted for P0, P1 and named P2 streams,
  and keeps refusing any mirroring claim that names a synchronous mode. Which
  streams are mirrored is committed configuration, reviewed like any policy.
  The first set: halts, policy announcements and signed watermarks (P0), and
  fills, cancels and settlements (P1). P2 market journals are not mirrored
  until a consumer in another region needs one (ADR 0069).

### 2. No write in one region waits on another

- The source region's produce path never learns that a mirror exists. A
  mirror that is down, slow or partitioned delays only the copy.
- **Lag is published, and bounded by a stated figure.** The mirror records
  `qip_fabric_mirror_lag_records{source_region,stream}` and
  `qip_fabric_mirror_lag_seconds{source_region,stream}` (R12-OBS-9148), with
  labels bounded by the configured region list and the committed stream set.
  When lag exceeds the stream's committed bound, every consumer of the
  region-qualified stream reads the remote knowledge as **stale** — a typed
  state, not an absent value — and the global consumers that size or report
  on it narrow, exactly as freshness narrows sizing at the cell today.
- **Replay on return.** When the mirror reconnects it resumes from its
  checkpoint against the source fabric while the source still retains the
  records, and from the source region's archive segments in Cloud Storage
  when it does not. Both routes verify the batch chain before producing.

### 3. Built now, proven locally now, deployed at the second region

ADR 0035 authorises one execution node in one region, and this record does
not authorise a second. So:

- The mirror, the policy arm, the lag series and the stale state are built
  and tested now, against two `qip-fabricd` processes on one host standing in
  for two regions, including a game day that kills the link, asserts the
  source keeps acknowledging, asserts lag grows and the remote stream reads
  stale, restores the link, and asserts the target converges with no
  duplicate and no unexplained gap.
- Deployment waits on the change that adds a second execution region, which
  is ADR 0113 §4's trigger for the project split and needs its own record
  superseding ADR 0035's "one". This record is not that record.

## What it costs

- **A process to operate per target region**, with a checkpoint that must
  itself be durable (it lives in the target region's regional Spanner
  instance beside the fabric's P0/P1 tables, so a lost mirror host loses no
  position).
- **Cross-region egress** for every mirrored record, unpriced here; P2 is
  excluded from the first set partly for this reason.
- **Remote knowledge is late by design.** A global consumer in region B sees
  region A's halt only after the mirror lag. Each cell's own halt does not
  depend on the mirror (ADR 0008), so this costs reporting and global
  sizing, not safety at the cell — and it is the price of refusing a global
  write quorum.
- **One more in-tree distributed component.** It is not consensus, but it is
  ordering and deduplication across a network, and it needs the same
  partition-schedule testing ADR 0109 §5 asks of anything that crosses one.

## Alternatives rejected

- **Multi-region Spanner for the fabric.** A global write quorum on the
  produce path; refused by the requirement and by ADR 0008.
- **Archive-only transport.** Lag measured in segment rolls; kept as the
  replay route, not as the mirror.
- **Google Cloud Managed Service for Apache Kafka with its replicator.**
  Admissible under ADR 0109 Amendment 2, and ADR 0111 §2 keeps it as the
  escalation for P2 volume; a second log technology for a low-volume P0/P1
  copy is a larger supply chain than the problem.
- **Pub/Sub as the bridge.** At-least-once with redelivery and no dense
  per-partition sequence; the deduplication and gap detection above would be
  rebuilt on top of it.

## What would make this wrong

- A source-region produce acknowledgement that waits for the mirror, or any
  synchronous cross-region step on a produce path.
- A record in a region-qualified stream that the mirror did not copy from the
  source, or a mirror writing to a stream that is not region-qualified.
- A mirrored copy accepted with a broken batch chain, or a missing source
  sequence not journaled as a `Gap`.
- The lag series absent, or a remote stream past its bound read as fresh.
- The mirror deployed to a second region before a record supersedes ADR
  0035's "one".

## Dependency direction

The `Mirroring::Async` arm lives in `qip-events` (lib, I/O-free). The
mirror's copy-and-checkpoint logic is a pure function over source batches and
a checkpoint, in `qip-streaming` (service), tested without a socket. The
sockets (the mTLS client to the source fabric, the producer to the target)
and the Spanner checkpoint store are composed in `apps/qip-fabricd` as a
second role of that binary, or a sibling app — never in a lib other than
`qip-transport`. `qip-edge` gains nothing and the cell's decision thread
never sees the mirror. No lib depends on a service; no service depends on
the runtime; nothing depends on an app. **No new dependency**: `rustls` and
`ring` are already admitted (ADR 0112, ADR 0110).

## R12 ids

Unblocked to build: **R12-FABRIC-7038**, **R12-RES-7114**, **R12-OBS-9148**;
R12-CONTRACT-7092 follows from them. Deployment of all three waits on a
second region (decision 3).

## The paper-trading boundary

Untouched. A mirror copies records; it opens no order path, and a mirrored
fill is the source region's fill, still refused by ADR 0106 if it says
`simulated=false`. The three layers — Terraform's refusal of the live rungs,
`AutonomyLevel::deployable`, `Cell::new` and `Determinism::Required` — and
the ledger's fourth fence stand as they are.

## Gates

None run. This change edits Markdown only; `cargo`, Terraform and the
frontend gates have nothing to judge, and the session that wrote it had no
shell. The documentation acceptance suite's checks this record must satisfy
(a cost section, a reversal section, an index row) hold by construction and
were not executed.
