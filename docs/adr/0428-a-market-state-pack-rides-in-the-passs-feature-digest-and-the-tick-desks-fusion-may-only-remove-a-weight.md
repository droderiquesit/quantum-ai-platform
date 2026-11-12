# ADR 0428: A MarketStatePack rides in the pass's FeatureDigest, and the tick desk's fusion may only remove a weight

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session t50-01, implementing owner decision 14
  of 2026-10-31 (ADR_THEN_IMPLEMENT). The delegation's limits hold: it sets
  no figure, changes no requirement's wording, reverses no accepted refusal,
  adds no dependency and touches no paper-trading layer.
- **Date**: 2026-11-12
- **Amends**: [ADR 0151](0151-what-enters-understand-each-tick-plane-and-federation-input-has-a-named-producer-and-consumer-or-is-not-built-and-an-outside-prior-may-only-narrow-reason.md)
  decision 2, its carrier only ("the pack rides inside the pass's
  `FeatureSnapshot` P2 record"). The producer, the never-over-zero rule and
  the downgrade-only consumer stand.
- **Rows**: R12-CONTRACT-7072.

## Context

ADR 0151 decision 2 put the `MarketStatePack` inside a `FeatureSnapshot` P2
record. No `FeatureSnapshot` was ever built. What exists is ADR 0132
decision 4's `FeatureDigest`: one bounded record per pass, built in
`qip-edge-node`'s scheduler (`Scheduler::feature_digest`) from the
`MarketEvent`s the pass actually applied, and shipped on
`reflex.feature_digested`. It already counts the applied events, ranges
their event times and digests their source hashes; it is the one record that
holds, per pass, exactly the tape the pack is supposed to describe.

Separately, the contract itself had a hole: `MarketStatePack` derived a
plain `Deserialize`, so every refusal of `MarketStatePack::new` (inverted
interval, zero horizon, blank lineage) was bypassed by the decoder, and a
pack the producing brain could not construct could still arrive at the
consuming one. That is closed in the same lane (`MarketStatePackWire`).

## Decision

1. **The carrier is the pass's `FeatureDigest`.** The pack rides as an
   optional field of the digest on `reflex.feature_digested`; no
   `FeatureSnapshot` is built. Adding it changes the digest's locked shape,
   so `REFLEX_FEATURE_DIGESTED` moves to schema version 2 (CONTRACT-046: a
   shape change without a version bump fails CI), its lock row's fixture
   carries the pack `Some` so the lock sees the nested shape, and version 1
   is retired rather than kept beside it: its one surveyed consumer,
   `qip-edge-node`, decodes version 2's type, which reads a version 1 body
   with the pack absent, and no deployed process holds a version 1 line.
2. **The producer is the seam that builds the digest.** The owner's triage
   named `event_fabric/drain.rs`; the drain only ships what the scheduler
   recorded, and holds `MarketEvent`s only inside the lake writer. The pack
   is built in `Scheduler::feature_digest`, over the same `applied` slice
   the digest already counts, so the pack and the digest cannot describe
   different tapes. A pass that applied no event carries no pack (ADR 0151
   decision 2; `MarketStatePack::new` refuses zero events).
3. **Interval and horizon are the pass's.** `from` is the earlier of the
   previous pass's instant and the earliest applied event time; `to` is the
   later of this pass's instant and the latest applied event time; the
   horizon is `to − from`. Nothing here is a constant: every term is a time
   the pass already holds. Where that interval is empty (a first pass whose
   every event is stamped at the pass's own instant) the constructor refuses
   and the digest carries no pack.
4. **An absent pack is never a clean one.** A digest with `applied > 0` and
   no pack is *unattested*, and the consumer treats it exactly as a flagged
   one. This is what keeps decision 3's refusal fail-closed rather than a
   silent way to drop a flag.
5. **The consumer stands as ADR 0151 named it, and may only remove.** The
   tick desk's fusion removes a flagged or unattested instrument's input
   weight from the fused state, never raises one, and is not a regime input.
   It stays off while `QIP_DEEPBRAIN_TICK_EVERY` is unset.

## Consequences

- R12-CONTRACT-7072's verification field — a pack missing any required
  field is refused on construction and on decode, and a round trip keeps
  every field — is met by `qip-contracts/tests/market_state.rs`.
- Decisions 1–4 landed with this ADR (session t50-01): the carrier field,
  the version bump, `qip_edge_node::market_state::market_state_of`, and its
  call in the scheduler's digest seam. Decision 5, the consumer, did not;
  the row stays PARTIAL and the assessment names it.
- Measured on the first pass of `decision_loop`'s rig: the simulated feed
  stamps pass 1's events at the pass's own instant, so pass 1 is
  unattested on every node start (logged "ships no market state pack").
  Every later pass carries a pack.

## What it costs

- A shape change on a locked topic: the digest's schema lock row moves when
  the field lands, and every reader of `reflex.feature_digested` must decode
  the new optional field.
- Decision 4 trades availability for honesty: a pass whose interval is
  empty, or whose pack failed to build, removes its instruments from fusion
  for that pass rather than letting them through unattested.
- The decoder cannot recompute the integrity flags (the events do not
  travel), so a producer that drops a flag before encoding is caught only by
  the carrier's provenance, not by the contract.

## What would make this wrong

A second record carrying a pack for the same pass. If a `FeatureSnapshot`
is ever built, it supersedes this carrier by ADR, not beside it.
