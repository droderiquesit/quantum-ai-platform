# ADR 0609: A world/market mismatch has one paper-only consumer per kind, each charged to a budget and journaled

- **Status**: **Proposed**, 2026-11-16, by lane 04 of wave b52, for the owner
  ruling of 2026-10-31 on R12-TICK-2084 (ADR_THEN_IMPLEMENT). Not accepted
  until a person accepts it. Reworked after review: the first draft handed
  scout jobs to discovery directly and kept proposals in a side list.
- **Date**: 2026-11-16
- **Extends**: ADR 0151 decision 1 (an input has a named consumer or is not
  built), ADR 0158 A.1(iv) (every scout job is charged to the acquisition
  budget).
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no dependency).

## Context

`qip_kernel::fusion::fuse` names three mismatches between the world model and
the market. The tick desk counted them and dispatched none, so a disagreement
raised nothing.

## Decision

1. An investigation (`Mismatch::Investigation`) becomes an acquisition
   **candidate** on the research loop (`ResearchLoop::admit_mismatch_candidates`),
   ranked with every other candidate and charged to the same acquisition budget
   when `launch_ranked` launches it. Its significance and uncertainty are the
   prior the market contradicted. It is never handed to the discovery desk
   directly.
2. An evidence-quality challenge is a candidate the same way, valued at the
   quality the evidence lacks (`1 - quality`). Its only effect on a weight is
   `EvidenceChallenge::lowered`, which is `min`: it may lower, never raise.
3. A relative-value hypothesis enters `falsification::Book::propose` as
   `HypothesisStatus::Proposed`, **charged to the trial budget of one family**,
   `world_market_mismatch/relative_value`, through
   `TrialLedger::charge_proposal`. `ProposedHypothesis` has no quantity, price
   or side, so nothing can size from it. A duplicate (same event and
   instrument) is detected before the charge and charges nothing; a family at
   its budget refuses the proposal and counts it.
4. Every change to the proposal working set is **journaled**
   (`MismatchProposalJournal` under `Topic::HypothesisCreated`, key prefix
   `mismatch-proposal:`): an admission, an eviction, a refusal on budget.
   `mismatch::replay` rebuilds the working set and the family's spent count
   from the journal alone.
5. The working set holds `PROPOSED_CAPACITY = 32` proposals. **Provisional**:
   no figure was given, so it is chosen in the restrictive direction, half the
   family budget of 64, which also makes eviction reachable. The oldest is
   evicted when full; evictions and budget refusals are counted
   (`proposals_evicted`, `proposals_refused_on_budget`) and journaled.
6. `mismatch::route` returns every mismatch somewhere: the routed total equals
   `FusedState::mismatches().len()`.

## What it costs

- One trial of the family's cumulative budget (64) per distinct proposal, spent
  whether or not the proposal is ever evaluated. After 64 distinct proposals in
  a process the family refuses more until the ledger is rebuilt; that is the
  intended bound on a noisy feed, and it also consumes budget a later
  evaluation of the same family would have had (no evaluation of this family
  exists yet).
- Acquisition budget: each launched mismatch scout job spends what any scout
  job spends, and displaces a lower-ranked candidate when the budget is short.
- One log record per admission, eviction and refusal.
- A `ResearchLoop` is needed to offer candidates; without one the
  investigations and challenges are not offered anywhere.

## What would make this wrong

- A consumer that derives a size from `ProposedHypothesis`. The type has no
  field to size from; adding one needs this record revisited.
- A single family being too coarse: a flood of one event class's proposals
  would exhaust the budget for every other class. If proposals acquire a
  natural class, the family key should follow it, within the ledger's family
  bound.
- A budget refusal that is not noticed: nothing alerts on
  `proposals_refused_on_budget` yet.
- The tick desk being off by default (`QIP_DEEPBRAIN_TICK_EVERY` unset), so the
  whole path is dormant in a deployed binary until it is enabled and entitled.
- `Topic::HypothesisCreated` carrying another producer's body later: the
  idempotency prefix is what distinguishes these records.
