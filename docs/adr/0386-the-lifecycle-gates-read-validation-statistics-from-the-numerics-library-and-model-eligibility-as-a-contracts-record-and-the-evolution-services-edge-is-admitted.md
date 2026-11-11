# ADR 0386: The lifecycle's gates read validation statistics from the numerics library and model eligibility as a contracts record, and the evolution service's edge is admitted

- **Status**: **Accepted 2026-11-11 under the owner's 2026-11-11 instruction
  to the orchestrator** that every in-repository design decision be made so
  that no register row waits on one. Batch l1-d1-01 writes it on the
  orchestrator's ruling that `qip-lifecycle` stays an asset-brain crate. The
  limits of that instruction hold:
  - it sets no figure and changes no gate threshold;
  - it touches no paper-trading layer;
  - it adds no dependency and no crate edge; it removes two and admits one
    that already existed;
  - it moves no behaviour into `qip-contracts`.
- **Date**: 2026-11-11
- **Applies**: ADR 0127 decision 5 to the Asset Brain, and reads its second
  clause as written ("no other *brain's* crate depends on it"). ADR 0127's
  file is not edited.
- **Rows**: R12-CAPITAL-1095.

## Context

ADR 0127 decision 2 names the Asset Brain as `qip-portfolio-engine` and
`qip-lifecycle`. Batch l1-c1-01's closure test (2026-11-11) found the first
closed and the second open in three places, and pinned them:

1. `qip-lifecycle` depended on `qip-ai`. `demotion.rs` took a
   `ModelRegistry` and called `require_for_decision` on it. `qip-ai` holds
   models; it is on the alpha list.
2. `qip-lifecycle` depended on `qip-simulation-engine`, for
   `validation::{deflated_sharpe, sharpe_standard_error, PurgedSplit,
   assess_overfitting}`. Through that crate it reached `qip-strategy` (alpha)
   and the risk brain's `qip-risk`.
3. `qip-evolution`, an alpha service, depends on `qip-lifecycle`: its
   `promotion.rs` calls `attempt_promotion`.

The row asked whether the validation gates and the model-eligibility check
stay in the asset brain with those edges moved behind `qip-contracts` types,
or whether `qip-lifecycle` is not an asset-brain crate.

Two facts about the code decide how.

- The validation module named nothing but `qip-core` and `qip-numerics`. The
  simulation engine was its address, not its dependency.
- The holdout gate recomputes those statistics on purpose. Its doc says a
  gate that read a submitted Sharpe ratio would be trusting the researcher
  who submitted it. A `qip-contracts` record of validation *results* would
  be exactly that submitted number, with whoever built the record deciding
  what the gate sees. And `qip-evolution` builds the evidence.

## Decision

1. **`qip-lifecycle` stays an asset-brain crate.**

2. **The validation statistics move to `qip_numerics::validation`,
   unchanged.** They are arithmetic, not a contract, so they do not go to
   `qip-contracts`, and they are not replaced by a record of their results,
   because the gate must keep computing them itself.
   `qip_simulation_engine::validation` re-exports the module, so there is
   one implementation and the engine's callers keep their path.
   `qip-lifecycle` drops `qip-simulation-engine`.

3. **Model eligibility reaches the demotion monitor as a `qip-contracts`
   record, `qip_contracts::model_eligibility::ModelEligibility`.** It holds
   the registry's verdict on each named model and the instant the registry
   was asked. `DemotionMonitor::{triggers, enforce}` take it in place of the
   registry. The kernel's `StrategyFactory::review` builds it: one
   `require_for_decision` call per model that an observed strategy's baseline
   names, the clearance or the refusal's own message recorded.
   `qip-lifecycle` drops `qip-ai` from what it ships.

4. **A record is weaker evidence than the registry, so every reading that
   is not a clearance taken at the instant of the review is a trigger.**
   The registry could not be silent about a model or out of date; a record
   can be both. A model the record does not name, and a record taken at any
   other instant, each raise `ModelReviewOverdue` with a reason that says
   which. This narrows: nothing that was demoted before is now kept, and a
   composition that forgets to ask is demoted, not passed.

5. **`qip-evolution`'s dependence on `qip-lifecycle` is admitted, and it is
   the only dependent below the kernel.** ADR 0127 decision 5's clause is
   about other *brains'* crates, and the evolution service is not one. The
   direction is alpha depending on the gate. The gate's closure cannot see
   its caller, and `AuthorisedPromotion` has no constructor that skips a
   gate, so the caller can ask and cannot grant. Moving `promotion.rs` into
   the kernel would add nothing to that and would move a service's logic
   into the composition. `brain_closure.rs` pins the dependent set to
   exactly `{qip-evolution}`.

## What holds it

- `brain_closure.rs`,
  `the_asset_brain_reaches_no_alpha_and_no_other_brain_and_its_one_dependent_is_the_evolution_service`:
  the alpha set and the other-brain set are asserted empty (they were
  `{qip-ai, qip-strategy}` and `{qip-risk}`), and the dependent set exact.
- `qip-lifecycle/tests/lifecycle.rs`,
  `a_record_silent_about_the_model_or_taken_at_another_instant_is_not_a_clearance`,
  beside the existing `an_overdue_model_review_demotes_the_strategy_it_drives`.
- `qip-contracts/tests/model_eligibility.rs`: silence is `Unrecorded`, a
  second verdict on one model is refused, a refusal needs its reason.
- `qip-kernel/tests/central.rs`,
  `a_model_the_registry_refuses_demotes_the_strategy_it_drives_through_the_composed_review`.

## What it costs

- The model check is now two steps in two crates where it was one call. The
  failure that introduces, a record that is stale or incomplete, is closed
  by decision 4, at the price of a demotion where a composition bug would
  previously have been impossible.
- The protected guard suite `architecture.rs` lists the lifecycle's edge to
  `qip-simulation-engine` in `SERVICE_TO_SERVICE_EDGES` and fails while it
  names an edge that no longer exists. A lane may not edit that file; the
  removal is in `docs/blueprint/lanes/l1-d1-01-R12-CAPITAL-1098-architecture.patch`,
  one cumulative patch with ADR 0385's, because both touch the same array.
- `qip-numerics` gains a module about backtest overfitting, which is
  statistics but is more domain-specific than the rest of that crate.
- `qip-ai` stays a dev-dependency of `qip-lifecycle`, so its tests build
  the record from a real registry. A reader of the manifest sees the name
  and must read the comment.
- The kernel's `StrategyFactory::review` keeps taking the registry, so the
  record exists only between the kernel and the lifecycle. It is not
  journalled. A replay recomputes it from the registry's state.

## What would make this wrong

- If the lifecycle comes to need more of a model than a yes or a no at an
  instant (for example the regime a card was validated in), the record
  grows a field per fact. If it grows a method that decides, it has become
  the registry again and belongs behind a port the kernel implements.
- If a second service below the kernel needs `attempt_promotion`, the
  pinned set fails and that is a new decision, not an edit to the list.
- If decision 4 demotes in production because the kernel asks the registry
  at one clock reading and the monitor reads at another, the instants must
  be made the same value at the call site. The comparison must not be
  loosened to a tolerance: a tolerance is a figure, and nobody gave one.
