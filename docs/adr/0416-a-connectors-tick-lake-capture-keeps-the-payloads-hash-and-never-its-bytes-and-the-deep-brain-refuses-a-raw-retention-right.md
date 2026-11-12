# ADR 0416: A connector's tick-lake capture keeps the payload's hash and never its bytes, and the deep brain refuses a raw retention right

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session fix-c-01. The delegation's limits hold:
  it sets no figure, changes no requirement's wording, reverses no accepted
  refusal, adds no dependency and touches no paper-trading layer. It only
  narrows what a durable store may keep.
- **Date**: 2026-11-12
- **Amends**: [ADR 0151](0151-what-enters-understand-each-tick-plane-and-federation-input-has-a-named-producer-and-consumer-or-is-not-built-and-an-outside-prior-may-only-narrow-reason.md)
  decision 8 ("Raw wire frames are captured by a separate writer, only where
  a licence permits raw retention"), for external sources: a licence that
  permits raw retention is no longer sufficient for a connector's payload to
  reach the durable tick lake. Leaves
  [ADR 0181](0181-the-simulated-depth-dataset-permits-raw-retention-raw-frames-reach-the-lake-off-the-decision-path-and-lake-segments-are-sized-at-the-seal.md)
  decisions 1 and 2 unchanged.
- **Rows**: R12-DATA-2014; R12-DATA-1039 (unchanged in grade, its connector
  path narrowed to hash only).

## Context

Two requirements read against one seam.

- **R12-DATA-2014** — "External source payloads exist only long enough to
  parse, validate, extract and reason; raw external content is then
  discarded and durable storage begins at the knowledge boundary." Its check
  fails "if any durable artefact holds the body".
- **R12-DATA-1039** — tick data and the platform's own records are separate
  retention classes, "retained only when venue rights, legal obligations and
  policy allow"; its check is that "raw venue bytes without retention
  entitlement are stored as hash only".

The seam is `qip_market_ingestion::connector::evidence::captured_of`, the
one place a connector's released record becomes a `CapturedMarketEvent` for
the tick lake. Since 2026-10-11 it carried the event's payload bytes and
their SHA-256, so that R12-DATA-1039's hash-only rule was exercised on the
production path. The deep brain's `QIP_DEEPBRAIN_TICK_LAKE_RETENTION`
admitted `class=canonical+raw`, and a lake given that right sealed each
payload in the segment's raw slot. Session fix-a-01 (`749599d4`) extended
the R12-DATA-2014 canary to the lake and showed it: under `canonical+raw`
the lake kept a fetched Coinbase ticker object whole, canary included.

Every deep-brain arm is a connector; no arm there reads a platform-own feed.
The only raw frames the platform legitimately keeps are its own: ADR 0181's
`qip-simulated-depth`, captured by the edge node's `LakeSink`, which no
third party wrote and which is not external content.

## Decision

1. **A connector's capture carries the payload's hash and never its bytes.**
   `captured_of` sets `raw_sha256` from the envelope's payload and `raw` to
   `None`, unconditionally. The narrowing sits at the only seam through
   which a connector payload can reach the lake, so it holds whatever right
   a lake writer is built with: the lake has nothing to put in a raw slot.
2. **The deep brain refuses `canonical+raw`.** The retention variable admits
   `canonical` only; `canonical+raw` is refused by name and stops the
   process, citing this record. A right that could keep nothing is not
   accepted silently, because an operator who granted it would believe raw
   bytes were retained when they were not (principle 3: a configuration is
   refused, not quietly lowered).
3. **ADR 0151 decision 8 is amended for external sources.** Its raw-frame
   writer is built only where a licence permits raw retention *and* the
   frames are the platform's own. For an external source, a licence that
   permits raw retention allows the transient handling decision 8 already
   names and nothing durable; the durable lake keeps the canonical record
   and the payload's hash.
4. **R12-DATA-1039 is met on the narrower reading its own text allows.** It
   retains raw bytes "only when venue rights, legal obligations and policy
   allow"; this record is the policy that does not allow it for external
   content. Its hash-only rule now holds on every connector capture, not
   only those without an entitlement, and its unit check (the lake's own
   partition and entitlement tests) is untouched. Raw retention stays
   available, and exercised, for the platform's own simulated depth
   (ADR 0181).

## What changes and what does not

- `connector/evidence.rs::captured_of` — `raw: None`.
- `qip-deepbrain/src/connectors.rs::TickLakeSettings::from_lookup` — refuses
  `canonical+raw`.
- `qip-storage`'s lake, `RetentionRight` and `TickLakeWriter` do not change:
  a raw right is still honoured for a writer handed raw bytes, which is what
  the edge node's simulated-depth path relies on.
- No deployment sets the retention variable, so no running configuration is
  refused by this change.

## What it costs

A licensed venue whose terms permit raw retention cannot have its frames
re-materialised byte for byte from the lake through a connector. Replay of
such a source is from the canonical record, checked against the stored
hash. No licensed venue source exists in the tree.

## What would make this wrong

- If a requirement were adopted asking that a licensed external venue's raw
  frames be replayable byte for byte, R12-DATA-2014's wording would have to
  change first; that is the owner's, and this record would be superseded
  then, not widened by a configuration value.
- If a deep-brain arm came to read a platform-own feed, it would need its
  own writer path rather than a raw right on the connector variable.

## Evidence

- `qip-market-ingestion` `connector_feed::tests::a_canary_in_a_market_body_reaches_no_tick_lake_evidence_or_knowledge_pack_the_poll_hands_on`
  runs the poll under `canonical` and under `canonical+raw`, asserts no
  canary in the lake, the evidence or the packs, no raw slot sealed, and the
  payload's hash on every read-back event.
- `qip-market-ingestion` `evidence_stage::a_released_tick_reaches_the_lake_with_its_raw_payload_hash_and_without_its_bytes`.
- `qip-deepbrain` `connectors::tests::a_raw_retention_right_is_refused_because_a_connector_payload_never_reaches_the_raw_slot`.
- `qip-acceptance` `data_infrastructure_guards::a_canary_in_a_fetched_body_is_found_in_no_durable_store_after_a_poll_through_the_platform`,
  its lake now granted `canonical`.
- Mutations are recorded in the commit that lands this record.
