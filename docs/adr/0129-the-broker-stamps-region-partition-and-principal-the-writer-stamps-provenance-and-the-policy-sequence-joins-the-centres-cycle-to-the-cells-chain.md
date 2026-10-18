# ADR 0129: The broker stamps region, partition and principal; the writer stamps provenance; and the policy sequence joins the centre's cycle to the cell's chain

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Additive changes to two contracts
  under the schema lock; no dependency, no paid resource, no paper-boundary
  change.
- **Date**: 2026-10-18
- **Extends**: ADR 0100 §1 (one batch format, every field stamped by exactly
  one stage) and §4 (producer epochs and sequences), ADR 0104 (the
  HMAC-sealed batch chain), ADR 0112 (mTLS gives the broker an authenticated
  peer), ADR 0007 (exact attribution).
- **Keeps**: `qip_contracts::reflex::chain_correlation`'s preimage unchanged,
  ADR 0008, ADR 0128 (the spine carries policy).

## Context

**The fabric frame.** `qip-events/src/event_fabric/codec.rs`'s module doc is
a table assigning every header field to the writer, the drain or the broker.
Region, partition, the producer's authenticated principal and provenance are
not in it. They exist only on the reader side: `FabricFacts`
(`qip-events/src/event_fabric/envelope.rs`) has `region`, `partition`,
`auth_context` and `provenance`, all supplied by whoever opens the envelope.
`qip-ledgerd/src/consumer.rs::fabric_context` fills them with
`READ_REGION = "central"` ("where they were read, not where they were
produced ... the producing region is an open item rather than something this
reader may guess"), `consumer-group:<group>` and `"qip-ledgerd"`. So
R12-FABRIC-8022 (every event carries region and provenance on the wire),
R12-FABRIC-8084 (the frame header carries the partition) and
R12-CONTRACT-7089 (region and auth context on the wire, not fabricated by a
consumer) are PARTIAL on one gap: the producing facts are not in the bytes.

**The decision trace.** The centre's lineage ids (`qip_core::lineage::Lineage`:
`correlation_id`, `causation_id`, `trace_id`) and the cell's chain ids are
independent id spaces. `chain_correlation(cell, session, order_id)` is
derived, never drawn, so a replay yields the same id; `Decision::ChainOpened`
joins an order to the `SignalRaised` journal sequences that caused it.
Nothing joins either to the central cycle whose policy the cell was acting
under, and nothing joins the API's correlation id to the cell. R12-OBS-7018
(one trace across brain, model, risk, node, venue, ledger, learning) and
R12-OBS-9154 (an id set at the API appears in the cell's event metadata) are
PARTIAL on that.

Locate them with
`grep -n 'pub struct FabricFacts' backend/crates/libs/qip-events/src/event_fabric/envelope.rs`,
`grep -n 'READ_REGION\|fn fabric_context' backend/crates/apps/qip-ledgerd/src/consumer.rs`,
`grep -n 'pub fn chain_correlation\|ChainOpened {' backend/crates/libs/qip-contracts/src/reflex.rs` and
`grep -n 'pub struct PolicyPayload' backend/crates/libs/qip-contracts/src/policy.rs`.

## Decision

1. **Region, partition and principal are broker-stamped.** They are facts the
   broker knows and the producer can only assert: the region is the
   broker's own, the partition is the one it appends to, and the principal
   is the identity the mTLS session (ADR 0112) or the local deployment's
   configured identity authenticated. A producer-supplied value would be an
   unauthenticated claim on a wire that authenticates nobody below the
   session. They join `stamp_broker`'s row of the codec table, under the
   batch CRC and the ADR 0104 chain hash, so a later custodian cannot alter
   them without breaking the chain.
2. **Provenance is writer-stamped**, as a bounded label naming the
   producing component (`qip-edge-node`, `desk`, `qip-api`, …) chosen from a
   closed set in `qip-events`, never a free string. The producer id already
   names the instance; provenance names the kind, so a reader can tell a desk
   record from a cell record without parsing ids.
3. **Readers stop fabricating.** `FabricFacts` takes these four from the
   batch. A reader opening a batch that predates the fields (format version
   below the new one) gets them as explicitly absent, never defaulted to its
   own region or group, and `fabric_context`'s `READ_REGION` is deleted.
4. **The format version is bumped once for all four** under the schema lock
   (CONTRACT-048), with a decode test proving an old batch still decodes and
   reads as absent, and a new batch round-trips each field.
5. **The policy sequence is the join between the centre and a cell**
   (R12-OBS-7018, 9154). `PolicyPayload` gains a `cycle` field, the central
   cycle number that produced it, and a `correlation` field, that cycle's
   `Lineage.correlation_id` (the id set at the API when the cycle was
   requested from it). `Decision::ChainOpened` gains `policy_sequence`, the
   sequence of the payload the cell had applied when it opened the chain.
   The walk is then: API correlation id → central cycle (the log) →
   `PolicyPayload.sequence` → `ChainOpened.policy_sequence` → chain
   correlation → the order's later hops → the ledger's `LedgerEvent` (its
   `qip_contracts::ledger::SourceEvent` names cell, session and order id,
   from which the chain correlation is derived) → the learning records keyed
   on the fill.
   `chain_correlation`'s preimage does not change, so every existing id and
   replay stays valid.
6. **Both payload fields are additive and signed.** They are inside the
   signed bytes, skipped when absent so a payload signed before them
   verifies unchanged, and a cell on an old build refuses a payload with
   unknown fields rather than ignoring them.

## Consequences

- A record on the fabric says where it was produced and by whom without the
  reader's opinion, and the ledger's envelopes stop claiming every record was
  produced in "central".
- One query joins an API request to a fill: the rows' "one trace" holds as a
  chain of exact joins, each on a field, none on a timestamp.
- Code slices left: the codec fields, the broker and writer stamps, the
  `FabricFacts` change and the old-batch decode test (FABRIC-8022, 8084,
  CONTRACT-7089, in `event_fabric_codec.rs` and a ledgerd test that a
  consumer-fabricated region is refused); the two payload fields, the
  `ChainOpened` field and an acceptance walk API → cell → ledger in
  `qip-acceptance/tests/vertical_slice_correlation.rs` (OBS-7018, 9154).

## What it costs

- Four fields on every batch header: tens of bytes per batch, not per
  record, so small against payloads, but the format version moves and every
  reader must handle both.
- The join in decision 5 answers "which central decision state was the cell
  acting under", not "which central decision caused this order": a cell's
  orders come from its own signals (ADR 0008), and the record does not
  pretend otherwise. A chain opened between two payloads is attributed to
  the one applied, which is exact but may surprise a reader expecting the
  newest.
- Broker-stamped region assumes one broker serves one region; a broker that
  accepted batches for two regions would stamp the wrong one. ADR 0111
  places brokers per region, so this holds today.

## What would make this wrong

- If producers in one region ever had to write through a broker in another
  (a failover path), the broker's region would no longer be the producing
  region, and the field must become the producer's, authenticated by its
  session certificate's SAN rather than asserted.
- If the cell ever acted on a central decision directly (for example a
  per-order instruction), the policy-sequence join would be too coarse and
  the order itself would need the central id.

## Rows

R12-FABRIC-8022, R12-FABRIC-8084, R12-CONTRACT-7089, R12-OBS-7018,
R12-OBS-9154. Each stays PARTIAL until its slice lands with a mutated test.
