# ADR 0622: An observed world record carries the fabric's belief weight into the detectors, and the release gate does not wait for a second origin

- **Status**: **Proposed 2026-11-15**, lane 11 of wave b52. It needs a person's
  acceptance before any seam below is built. It adds no dependency, no key and
  no figure, and touches no paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-EVID-2030 (stays PARTIAL; see "What this leaves open").
- **Related**: ADR 0152 (the world fabric and its journal), R12-EVID-1200.

## Context

R12-EVID-2030 requires that no single source, and no scraped text, is
converted directly into a fact. The fabric's gate admits or withholds a claim
(R12-EVID-1200). An admitted record then reaches `Platform::observe` and the
detectors as an observation on one source's word, because admission requires
no corroboration. Two arms were named on 2026-11-12:

- **(A) Carry the weight.** The record travels with the fabric's per-claim
  `verdict.belief_weight`, and a detector's confidence is bounded by it.
- **(B) Hold for corroboration.** The gate holds a world claim until
  `independent_origins >= 2`.

## Decision

1. **Arm (A).** An observed world record carries its claim's
   `verdict.belief_weight`, and an anomaly it produces has
   `confidence <= belief_weight`. An uncorroborated origin earns no
   `CORROBORATION_CREDIT`, so its bound is the unsigned floor plus its own
   pinned-hash and TLS-host signals; a corroborated twin's bound is higher.
2. **Arm (B) is declined.** Every connector committed today has one origin.
   Holding until two exist withholds every record on all three roots and so
   turns a weighting question into an outage, which is the failure mode
   R12-EVID-1200 exists to avoid in the other direction.
3. **The weight is data, not a clamp.** A record whose weight cannot be
   found is refused by name, not given a default weight. A record with no
   claim stays as R12-EVID-1200 leaves it; closing that is that row's work.

## The four seams this decision requires

1. The record-to-claim link: `SensedRecord` carries no claim id and
   `Platform::admit_world_records` is generic over `T`, so the weight must
   travel beside each released record into the roots' observe calls
   (`qip-api` `observe_fresh`, `qip-fastbrain` `SensePath::feed`,
   `qip-deepbrain` `ConnectorArm::poll`'s caller), or live in a kernel-held
   map keyed by a record digest bounded by `DEFAULT_EVIDENCE_CAPACITY`.
2. `MarketEvent::from_news`, `from_fundamental` and `from_macro` take their
   weight from the record's own quality; `CatalystLink` must carry the
   fabric weight and `Anomaly::confidence` must multiply by it.
3. `Platform::observation_history` is a bare `f64` series per subject, so a
   single uncorroborated surprise moves it with no weight; it needs a weight
   per point.
4. The test: one fabric-admitted single-origin record driven through
   `observe` to an anomaly has confidence at most that claim's belief weight,
   and its corroborated twin rises above it.

## What this leaves open

Nothing is built by this record. R12-EVID-2030 stays PARTIAL until the four
seams and the test exist. Seams 2 and 3 cross `qip-opportunity-engine` and
`qip-kernel` and are being edited by other lanes, so they should land as a
sequence of small commits, not one.
