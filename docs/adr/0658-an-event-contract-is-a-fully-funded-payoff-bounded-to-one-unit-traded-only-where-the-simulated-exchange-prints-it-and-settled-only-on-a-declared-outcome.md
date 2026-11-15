# ADR 0658: An event contract is a fully-funded payoff bounded to one unit, traded only where the simulated exchange prints it and settled only on a declared outcome

- **Status**: Proposed 2026-11-15, lane 29 of wave b52, carrying the owner
  ruling of 2026-10-31 (ADR_THEN_IMPLEMENT) for R12-COVERAGE-5104. Touches no
  paper-trading layer, adds no dependency and no figure.
- **Date**: 2026-11-15
- **Rows**: R12-COVERAGE-5104 (stays PARTIAL; this ADR and its first slice).

## Context

`AssetClass::Prediction` is registered in the kernel's asset-class registry,
but no `InstrumentType` maps to it, so no instrument of the class could be
built, assembled or traded. The registration was a capability claim with
nothing behind it. The owner ruled that event contracts trade on paper only:
a fully-funded payoff bounded to one unit, priced only where the simulated
exchange prints it, settled only on an operator-declared, dated outcome, and
a prediction-market venue stays unlicensed.

## Decision

1. `InstrumentType::EventContract` is the one instrument type of the
   Prediction class. It pays one unit if its declared outcome occurs and
   nothing otherwise.
2. Its price is a probability. A price above one is refused when the object
   is built, not clamped.
3. It is priced only from a print on the simulated exchange. No external
   prediction-market quote is a price source and no such venue is admitted.
4. It settles only on an outcome an operator declares with a date. Nothing
   infers an outcome from a feed.
5. It is fully funded: the position's maximum loss is the premium paid, so no
   margin or leverage applies to it.

## Consequences

Decisions 1 and 2 are built in this slice. Decisions 3 to 5 are not: the
settlement path (operator-declared outcome, payoff booking) and the
fully-funded sizing check remain, and the row stays PARTIAL until they land.

## Evidence

`qip-kernel/tests/asset_classes.rs::an_event_contract_assembles_in_the_prediction_class_trades_on_paper_and_is_bounded_to_one_unit`.
