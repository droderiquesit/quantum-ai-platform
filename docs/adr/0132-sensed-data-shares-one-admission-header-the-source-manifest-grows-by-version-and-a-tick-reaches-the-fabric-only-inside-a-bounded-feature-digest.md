# ADR 0132: Sensed data shares one admission header, the source manifest grows by version, and a tick reaches the fabric only inside a bounded feature digest

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Contract changes inside the tree; no
  dependency, no paid resource, no new data source, no licence decision.
- **Date**: 2026-10-18
- **Extends**: ADR 0034 (licensing is evaluated before a source is used),
  ADR 0100 §5 (the fabric's classes), the data domain's bitemporal rule
  (`.claude/rules/domains/data-and-streaming.md`).
- **Keeps**: ADR 0113 (the tick-lake codec waits on a committed volume
  measurement), bounded retention, the hash-chained event log as the record.

## Context

**Two canonical records.** The edge admits venue messages as
`qip_contracts::market_event::MarketEvent`: a `MarketMessage` payload with
`event_time`, `receive_time` and `normalized_time` kept as three fields, an
`uncertainty` that is never negative, a `source_hash` recomputed at
construction, and an `Entitlement` the one constructor refuses unless granted
at `event_time`. The centre admits everything it senses as
`qip_market_ingestion::adapter::SensedRecord` (`Tick`, `Quote`, `Trade`,
`Book`, `Bar`, `CorporateAction`, `News`, `Fundamental`, `Macro`,
`AlternativeData`, `ReferenceData`), whose members carry their own times
(`qip_market::Tick` has `at` and a required `capture_time`, refused when it
precedes `at`) and no common header. R12-TICK-2053 ("normalise into typed
canonical records — one schema") is PARTIAL because the two coexist.

**The manifest.** `qip_financial::manifest::SourceManifest` records source,
locator, byte length, SHA-256 and `retrieved_at`, and is persisted in the
hash-chained log. R12-DATA-8013 asks for version/etag, entitlement, cursor,
freshness, parser lineage and re-fetch instructions; a field added to a
persisted chained record cannot be added by editing old records.

**Ticks on the fabric.** The node's fabric writer counts ticks as
`Decision::Ingested` and does not ship them
(`qip-edge-node/src/event_fabric/writer.rs`, the arm returning `false`), and
no feature-snapshot type exists in the node. R12-FABRIC-10102 asks for tick
and feature-snapshot records on P1/P2. A record per tick is the volume ADR
0113 deferred behind a measurement, and the fabric's retention is bounded on
purpose.

Locate them with
`grep -n 'pub struct MarketEvent' backend/crates/libs/qip-contracts/src/market_event.rs`,
`grep -n 'pub enum SensedRecord' backend/crates/services/qip-market-ingestion/src/adapter.rs`,
`grep -n 'pub struct SourceManifest' backend/crates/libs/qip-financial/src/manifest.rs` and
`grep -n 'Decision::Ingested' backend/crates/apps/qip-edge-node/src/event_fabric/writer.rs`.

## Decision

1. **"One schema" is one admission header, not one payload type.** A venue's
   order-book message and a central bank's macro release are different facts
   and a single payload enum spanning both would be a type nobody can match
   exhaustively. What must be one is how a record is *admitted*: a new
   `qip_contracts::admission::Admission` holding the three times, the
   uncertainty, the source hash and the entitlement, with `MarketEvent`'s
   refusals moved into its one constructor. `MarketEvent` becomes
   `Admitted<MarketMessage>`; every `SensedRecord` is admitted as
   `Admitted<SensedRecord>`. A record whose own times disagree with its header
   (a `Tick` whose `at` is not the header's event time, or whose
   `capture_time` is not its receive time) is refused at admission, not
   reconciled, so the bitemporal facts have one authority.
2. **The edge's venue messages stay `MarketMessage`.** They are not folded
   into `SensedRecord`; the cell's closure must not grow the centre's record
   types, and lane placement forbids it. The shared header is what makes the
   two comparable, and it is a `qip-contracts` type both lanes already link.
3. **`SourceManifest` grows by version** (R12-DATA-8013). A `schema_version`
   field is added and version 2 carries the six fields: `etag` (the source's
   version tag, absent where the source has none, never invented),
   `entitlement` (the same type the admission header carries), `cursor` (the
   opaque position the feed resumes from), `fresh_until` (the instant after
   which the bytes must not be served without re-fetch), `parser` (name and
   version of the code that parsed them) and `refetch` (a typed instruction,
   not a URL template). A version-1 manifest in the log stays readable and
   reads those six as *not recorded*, distinct from empty; nothing rewrites a
   sealed record.
4. **A tick reaches the fabric only inside a feature digest**
   (R12-FABRIC-10102). The node does not produce one record per tick. Once
   per pass it produces a `FeatureSnapshot` P2 record: per instrument the
   pass read, the feature values the cell sized against, their freshness,
   and the count and time range of ticks they were computed from, with a
   digest over the ticks' source hashes. The record is bounded by the
   instruments in the cell's configuration, not by market activity. Raw ticks
   remain the tick lake's question, after ADR 0113's measurement.

## Consequences

- One admission path holds the point-in-time rules for every sensed record,
  so a leakage fix lands once, as `capture_time` had to land in several
  places on 2026-10-11.
- A replay can name what features a cell acted on, from the fabric, without
  storing every tick.
- Code slices left: `Admission` and the two `Admitted` wrappers with refusal
  tests for a disagreeing time (TICK-2053); manifest v2 with refusal tests and
  a round-trip test that fails when any of the six is dropped, plus a v1
  decode test (DATA-8013); `FeatureSnapshot` from the node's pass with a
  bound test and a test that ticks are counted but not shipped (FABRIC-10102).

## What it costs

- Every `SensedRecord` producer changes: each connector builds an admission
  header, and a connector that cannot state an entitlement can no longer
  build a record at all. That is ADR 0034's rule enforced at the type, and
  it will stop connectors that today admit data with no licence state.
- Times are stored twice for the variants that already carry them, checked
  equal rather than deduplicated; removing the duplicate is a later,
  wider change to `qip-market`.
- A feature digest cannot be replayed into a different feature computation:
  a question about a feature the cell did not compute needs the raw ticks,
  which the fabric does not hold.

## What would make this wrong

- If a consumer needed every tick on the fabric with ordering guarantees (for
  example a second region's reflex model trained on the first region's tape),
  decision 4 is too coarse and the measurement ADR 0113 waits on becomes
  urgent.
- If any sensed source has no meaningful event time (a static reference file),
  the header's three times would be forced; such a source would need its own
  admission arm rather than a fabricated event time.

## Rows

R12-TICK-2053, R12-DATA-8013, R12-FABRIC-10102. Each stays PARTIAL until its
slice lands with a mutated test.
