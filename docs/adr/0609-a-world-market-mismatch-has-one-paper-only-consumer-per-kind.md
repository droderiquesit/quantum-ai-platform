# ADR 0609: A world/market mismatch has one paper-only consumer per kind

- **Status**: **Proposed**, 2026-11-15, by lane 04 of wave b52, for the owner
  ruling of 2026-10-31 on R12-TICK-2084 (ADR_THEN_IMPLEMENT). Not accepted
  until a person accepts it.
- **Date**: 2026-11-15
- **Extends**: ADR 0151 decision 1 (an input has a named consumer or is not
  built).
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no dependency).

## Context

`qip_kernel::fusion::fuse` names three mismatches between the world model and
the market. The tick desk counted them and dispatched none, so a disagreement
raised nothing.

## Decision

1. An investigation opens a `ResearchQuestion`, handed to the discovery desk's
   scout queue at the rank of the prior it contradicted.
2. An evidence-quality challenge asks discovery for a better source, ranked at
   the quality the evidence lacks. Its only effect on a weight is
   `EvidenceChallenge::lowered`, which is `min`: it may lower, never raise.
3. A relative-value hypothesis enters `falsification::Book` as
   `HypothesisStatus::Proposed`. `ProposedHypothesis` has no quantity, price or
   side, so nothing can size from it; the book dedups on event and instrument.
4. `mismatch::route` returns every mismatch somewhere: the routed total equals
   `FusedState::mismatches().len()`.

## Consequences

The tick desk stays off by default (`QIP_DEEPBRAIN_TICK_EVERY` unset), so no
deployed binary routes a mismatch until it is enabled and entitled. Proposed
hypotheses are not yet evaluated by `Book::review`. Nothing applies
`EvidenceChallenge::lowered` to a live evidence weight yet.

## What would make this wrong

A consumer that derives a size from `ProposedHypothesis`. The type has no field
to size from; adding one needs this record revisited.
