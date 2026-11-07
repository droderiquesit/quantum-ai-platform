# ADR 0346: A price/belief discrepancy is a federation forecast the market keeps resolving against, measured in probability, and no belief price level is built (amends ADR 0151 decision 12)

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation
  (`docs/blueprint/owner-decisions-2026-10-31.md`, decision 10), on the
  ruling for R12-EXPAND-6067
  (`docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`, lane "L4b").
  The number is from L4b's block (decision 15). The delegation does not
  lift four limits:
  - no live-order path;
  - no real capital or money movement;
  - no weakened test or limit;
  - no new dependency.

  This decision needs none of them. The detector it adds raises research
  and moves nothing.
- **Date**: 2026-10-31
- **Amends**: ADR 0151 decision 12, its belief-price clause only.
- **Keeps**: ADR 0151 decision 12's refusal of a belief "price level";
  ADR 0345 is the record for its coverage clause.

## Context

R12-EXPAND-6067 asks for a persistence detector over "price versus belief".
ADR 0151 decision 12 did not build it, because the kernel exposes no belief
price level: beliefs are probabilities, and a price level computed from
them would be a number with no unit anybody measured.

A discrepancy between price and belief can be measured without one. The
federation arbitrates a probability for each proposition it forms, and the
market later resolves the proposition. Each `Resolution` in a journaled
`FederationReview` carries both: `ensemble`, the arbitrated probability at
its latest arbitration, and `outcome`. A federation whose forecast for a
subject keeps landing on the wrong side of what the price then did holds a
belief the market disagrees with. That is the discrepancy, in probability.

## Decision

1. **No belief price level is built.** The detector reads the arbitrated
   probability and the outcome, and nothing converts either into a price.
2. **A resolution is against the federation** when the arbitrated
   probability is above one half and the proposition resolved false, or
   below one half and it resolved true. A resolution with no arbitrated
   number, or one at exactly one half, took no side and is not counted.
3. **The subject is the proposition without its `@<cycle>` suffix**
   (`proposition_subject`). The same question formed afresh each cycle is
   one subject.
4. **A subject is flagged in a sweep** when a resolution new since the last
   sweep went against the federation and none after it, in that sweep,
   went its way (`belief_discrepancies`). "After" is by the proposition's
   cycle, not by list order: a review lists its resolutions in proposition
   order, so `@10` would otherwise come before `@9`.
5. **The second `PersistenceTracker`.** `BeliefWatch` feeds the flagged
   subjects to a tracker of its own, with `PERSIST_SWEEPS`. A subject
   flagged on that many consecutive sweeps raises one specialist research
   item. Its evidence names the detector (`belief-discrepancy`), the
   subject and the resolved propositions of the run (the newest sixteen).
   A subject that stays flagged raises nothing more until it drops out and
   returns, as the price-detector tracker already behaves.
6. **The reviews are read off the log each sweep** through
   `federation_stage::replay`, which refuses a review whose arbitrations do
   not follow from its own recorded views. A refused read is reported on
   the round (`belief_reviews_refused`) and the watch is not fed, so a bad
   read does not reset a run. The watch counts the reviews it has read, in
   log order, and never a cycle number: a restarted platform numbers its
   cycles from zero again over the same journal.
7. **It moves nothing.** The item asks whether the federation is wrong about
   the subject. Testing that with a specialist or a simulator is
   R12-EXPAND-6049's executor work, not this decision's.

## Consequences

- `tests/research.rs`
  `a_persistent_belief_discrepancy_queues_exactly_one_item_naming_its_origin`
  drives `BeliefWatch` over three cycles of resolutions and asserts one
  item, its evidence, and that a subject called right or left unsided
  raises nothing.
- The sweep runs the watch over the replayed reviews. No test yet drives a
  federation whose resolutions go against it through `ResearchLoop::sweep`,
  so that wiring is proven by its compile and by every existing sweep test
  still passing, not by a test of its own.

## What it costs

- A second read of the federation's reviews per sweep, through the replay
  that re-derives every arbitration. It grows with the log, as the
  knowledge-delta read already does.
- Persistence is counted in sweeps, not in the resolutions the ruling
  names. A flag needs a fresh resolution against the federation in every
  sweep of the run, and several in one sweep count once. A subject the
  federation resolves less often than the deep brain sweeps never
  persists, however wrong it is. That is the price of reusing the sweep
  tracker rather than counting each resolution, and it never counts a
  sweep that merely saw an old resolution.
- The replay is all or nothing. One review anywhere in the log that does
  not follow from its own views refuses every later read, and the detector
  stays off, reported only on the round.
- "Price" is wider than the name. The federation also resolves
  propositions about the platform's own halts, funding draws,
  counterparties and on-chain flows, and the watch reads those too. A
  discrepancy there is still a belief the world kept contradicting.

## What would make this wrong

- A measured case where the federation's subjects resolve less often than
  the sweep runs, so that real discrepancies never reach the count. Then
  the run should be counted in resolutions per subject, not in sweeps.
- A source that does price a belief directly (a prediction market on the
  same subject). Then the comparison should be made against that price,
  and this record superseded.
