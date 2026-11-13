# ADR 0409: A solver policy is judged by the classical verification its every answer faces, and a predecessor a rollback returns is judged in the same round

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). Session mod-a-01. The delegation's limits hold:
  - it sets no figure: the routing gates and the skill bar are the policies
    already in the tree;
  - it relaxes no limit, guard or refusal; its one behaviour change judges a
    model sooner, never later;
  - it touches no paper-trading layer and adds no dependency or crate edge.
- **Date**: 2026-11-12
- **Extends**: ADR 0006 (a classical baseline every time), ADR 0148 (models
  re-verified and demoted through the existing rollback path). It amends no
  clause of either.

## Context

R12-MODEL-10096: "Verifier emits VerificationResults that promote, reweight
or retire models and solver policies in the tournament." Since 2026-11-11 the
deep brain re-verifies a production model after its subject crosses a regime
boundary and rolls a refuted one back. The row's gap named four remainders.
This record decides two of them:

1. **The society verifies models and never a solver policy.** No type in the
   tree is called a solver policy. What chooses a solver is
   `qip-optimization-engine`'s `ComputeRouter`: per substrate and problem
   size it holds a `SubstrateRecord` that `RoutingLearning::absorb` folds
   from each run's `SolverMeasurement`, and every answer is classically
   verified before it is used (`MeasurementOutcome::Refused` when it fails).
   The verifier society's attack reads a model card's dossier — evaluations,
   datasets, regime crossings — and a substrate has no card.
2. **A predecessor a rollback reactivates returned on its pre-crossing
   evaluation and was judged only on the following round**, so between the
   two rounds every cycle read a model nobody had evaluated in the regime it
   was serving.

## Decision

1. **A solver policy is the router's substrate routing, and its verification
   result is the classical verification every answer faces.** The fold
   reweights it (`win_probability`, `repeatability`, calibration) and stops
   it: `ComputeRouter::measured_shortfall` refuses a substrate whose answers
   verify below the policy's `minimum_repeatability`.
   That is promote, reweight and retire, on
   a verification, for the thing that chooses a solver. No solver-policy card
   is invented for the society to attack: decision 28's fifth preference
   ("an arm with no producer in the tree is declared absent with its reason,
   and is not built on an invented input") applies, and the dossier a card
   carries has no meaning for a substrate.
2. **A predecessor is judged in the round it returns.**
   `LearningDesk::reverify_after_crossing` runs a worklist: when a refuted
   model is rolled back, the model reactivated in its place is pushed onto
   the same round's worklist if it is due a renewed evaluation, and is
   evaluated on the same post-crossing rows, put to the society, and rolled
   back in turn if refuted. Each reference is judged at most once a round, so
   the loop terminates on the set, not on the registry's history.

## What changes and what does not

- The deep brain's learning round may now emit `RolledBack` followed by
  `Refuted` (or a second `RolledBack`) for one subject in one call.
- The router is unchanged; this record names its fold as the arm the
  requirement asks for.

## What it costs

One worklist in one function, and on a round whose successor is refuted, a
second scoring of the same held-out rows and a second society attack for the
predecessor. Nothing on the router.

## What would make this wrong

- A solver policy given a card of its own (a registry of solver
  configurations with evaluations): then the society should attack it, and
  clause 1 is amended by the record that introduces the card.
- A rollback chain long enough that judging it in one round costs a cycle's
  budget: the chain is bounded by the promoted set's displaced lists, which
  are short; a measurement would be the evidence to revisit.

## What remains of R12-MODEL-10096

- A refuted model with no predecessor the registry will reactivate stays in
  the promoted set, withheld from decisions by its failed evaluation and not
  retired: withdrawing it needs a log record the promoted set and the Model
  Pack lineage both replay, which neither has.
- Only the round's own subject is examined; a production model on another
  subject is reached on the round that subject is next fitted.

## Evidence

- `qip-deepbrain/src/learning.rs`
  `tests::a_production_model_refuted_after_its_subject_crosses_is_rolled_back_and_its_predecessor_judged_in_the_same_round`;
  mutation: the worklist push of the reactivated model replaced by a
  discard — the call returned only `RolledBack`, and the test failed.
- `qip-optimization-engine/tests/learning.rs`
  `a_substrate_that_wins_but_answers_rarely_late_or_against_its_estimate_stops_being_tried`;
  mutation: `repeatability < 0.0` in `measured_shortfall` — "the
  repeatability gate did not stop a job".
