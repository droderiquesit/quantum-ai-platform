# ADR 0408: A strategy candidate is stressed against the scenario library at the largest position its run held, and the result is measured, not gated

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). Session mod-a-01. The delegation's limits hold:
  - it sets no figure: the scenarios are `standard_library()`'s, the beta is
    `MarketFactor`'s, and the exit cost is the backtest's own `CostModel`;
  - it relaxes no limit, guard or refusal, and adds no promotion rule;
  - it touches no paper-trading layer and adds no path to a venue;
  - it adds no dependency and no crate edge: `qip-deepbrain` already
    depends on `qip-risk` and `qip-simulation-engine`.
- **Date**: 2026-11-12
- **Extends**: ADR 0059 (a measurement the ladder does not act on is
  reported on the round, not gated). It changes no clause of ADR 0059 or of
  ADR 0149.

## Context

R12-MODEL-1159 asks for a strategy factory that generates, backtests,
simulates, **stresses**, paper trades, promotes, decays and retires
strategies. The 2026-10-30 skeptic found "stress has no code": the deep
brain's `EvolutionEngine` generated candidates, backtested each, registered
the ones that produced evidence and put them before the holdout gate, and
nothing ever asked what a gap, a credit event or a liquidity drought would
have done to the book a candidate chose to carry. `StressTester` and
`standard_library()` existed and were reached from one place,
`Platform::stress_the_book`, which stresses the platform's open book and not
anything a search produced.

Three questions had to be decided:

1. **Which position is stressed.** A backtest ends at the window's last bar.
   A strategy that ends flat would be stressed at nothing however large the
   book it carried an hour earlier.
2. **Against which exposure model.** A beta written here would be a figure
   nobody gave.
3. **Whether the result judges.** No tolerance for a candidate's stressed
   loss exists anywhere in the tree or in an owner decision.

## Decision

1. **The largest signed notional the run held on its subject** is stressed,
   at the equity the run had at that instant (`peak_exposure` in
   `qip-deepbrain/src/evolution.rs`). Largest rather than last, because the
   question is what the strategy risks, not when the window closed.
2. **The beta is the platform's single-factor `MarketFactor`**, estimated
   from the closes the engine holds for every subject, filed under
   `EQUITY_SHOCK` exactly as `Platform::stress_the_book` files it. The exit
   is priced off the run's own `BacktestConfig::costs` (commission,
   half-spread and tax), so the stressed exit cannot disagree with the cost
   the holdout was charged. A subject the factor cannot measure is stressed
   by nothing and counted on `stress_unmodelled`, never reported as immune.
3. **Measured, not gated.** `RoundSummary` carries `stress_measured`,
   `stress_unmodelled`, `stress_worst` (scenario and loss fraction) and the
   bounded `stress_refusals`, and the round line says "measured, not gated:
   ADR 0408". Nothing refuses a promotion on the number. A tolerance is a
   person's to give; when one is given it may narrow the holdout rung, by a
   record that names it.

Stressed inside `evaluate`, from the run just scored, so the position the
library meets is one the candidate held on the window the holdout was scored
on, and the step replays from the same bars.

## What changes and what does not

- Every candidate the factory registers is stressed on the deep brain's
  path from its composition root.
- No gate, limit, refusal or promotion changes. The holdout gate reads the
  same evidence it read before.

## What it costs

Two hundred-odd lines in one file and one `MarketFactor` estimate per
registered candidate over the engine's bounded histories.

## What would make this wrong

- A desk tolerance for a candidate's stressed loss: then the number should
  narrow promotion, and this record's third clause is amended by one that
  names the tolerance and its source.
- A multi-factor model in the tree: then the single equity beta understates
  rates and credit scenarios, which today reach the position as
  `unmodelled_factors` on the tester's result.

## What remains of R12-MODEL-1159

Stated so the row's gap can say it exactly:

- **simulate** has no step of its own beyond the backtest's simulation clock;
- **paper trade** needs `PaperEvidence` with `against_live_data`, and the
  deep brain's only stream is the synthetic exchange; no deployed process has
  a live feed (outside the repository: a deployment and a licensed source);
- **decay and retire** fire only on a strategy with a pilot baseline, and
  `Pilot` requires a human approval (`GateStage::requires_human_approval`),
  so neither fires on the default path without a person;
- the thousand-strategy measure drives the ledger, not the factory from a
  root.

## Evidence

- `qip-deepbrain/src/evolution.rs`:
  `tests::every_registered_candidate_is_stressed_against_the_whole_scenario_library`,
  mutation: `record_stress` replaced by `let _ = scored.stress` failed it
  ("2 registered and 0 stressed").
- `tests::a_candidate_is_stressed_at_the_largest_position_its_run_held_and_not_the_one_it_ended_on`,
  mutation: `larger = true` (the last non-zero position) failed it with
  `Decimal(50)` against `Decimal(300)`.
