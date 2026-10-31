# ADR 0232: The macro member forecasts each series' next surprise from the catalogued connectors, reads a surprise only where a source states consensus, labels the synthetic tape's surprises simulated, and may only narrow

- **Status**: **Accepted 2026-10-31 under the owner's delegation**, under
  decision 10 of `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`.
  The integration parent relayed it and assigned R12-WORLD-3043 to lane
  L2-INTEL. This record sets no risk number, adds no limit, and touches no
  paper-trading layer.
- **Date**: 2026-10-31
- **Amends**: the source clause of ADR 0151 decision 10 ("Federation members
  need a typed proposition, a source and a resolver"). ADR 0151 is not
  edited. The amendment is that a macro member has all three today, from
  sources the catalogue already holds.
- **Rows**: R12-WORLD-3043.

## Context

Macro releases reach the world model. `World::absorb_macro` records each
series' level and, only where the release states a consensus, its surprise
(`macro_surprise/<series>`). The NOW stage reads those surprises. The
federation had no member that read the economy, so a surprise in a
policy-relevant series changed no member's view. The ruling builds the
member on the on-chain member's pattern. It reads the macro surprises,
forecasts its own series, and is resolved by the series' next reading. It
speaks as soon as a catalogued connector is switched on at the deploy, and
says it is silent until then.

## Decision

1. **The member is `macro.surprise-persistence.global`**, declared on
   `sense:macro-connectors` (`MACRO_SOURCE`). It reads `FederationInputs.macros`.
   `Platform::macro_views` fills that field from
   `qip_world_model::now::macro_surprises`, which carries a surprise only
   where the release stated a consensus.
2. **One proposition per release.** For each series whose newest readable
   surprise is newer than the last one the member read, the member forecasts
   `macro:<series>:surprise-persists@<cycle>` at 0.6. That number is a
   declared persistence, not a fitted one: a surprise is read as likelier
   than not to be followed by one in the same direction. A release is fed
   once. A pass with no new release leaves the member silent, and the
   population check reports it so.
3. **Resolved by the series' next reading.** The forecast resolves true when
   the next release of the series surprises its consensus in the same
   direction, and false otherwise. A release of exactly consensus counts as
   false. A macro forecast waits `MACRO_PATIENCE` (60 days) of elapsed time
   before it is abandoned. It is measured in time, not passes, because a
   deployed cycle runs every 300 s on the deep brain and every 100 ms on the
   fast brain. The first draft counted 90 passes on the premise of daily
   cycles, so every monthly forecast would have been abandoned within hours.
   The in-lane skeptic caught it.
3a. **Currencies are read by their move.** A currency fixing
   (`FX.<base>.<quote>`, the ECB reference rates) states no consensus, so
   it has no surprise. `Platform::currency_views` reads each series'
   `macro_level` through `now::currency_levels`. The first fixing of a
   series seeds its level. Each later fixing that moved is forecast, at the
   same declared 0.6, to be followed by a move the same way
   (`macro:<series>:move-persists@<cycle>`). The next fixing answers it, and
   a fixing equal to the last states no direction.
4. **A synthetic tape's surprise is labelled simulated.** A release whose
   provenance is `LicensingClass::Synthetic` records its surprise with the
   source tag `simulated` (`now::MACRO_SIMULATED_SOURCE`). The tag reaches
   `WorldReading.sources`, so the member's proposition carries `:simulated`
   and its evidence fit is halved. The journal never records a synthetic
   surprise's forecast as one about a real economy.
5. **It may only narrow, and today it narrows nothing.** The federation's
   one power over DECIDE is a per-instrument narrowing, and a macro
   proposition is not an instrument's, so the member reaches no size at all.
   Conflict and sanctions have no licensed source, so the geopolitical half
   of the requirement is not built.

## Consequences

- `qip-kernel/tests/symbolic_federation.rs::a_cycle_with_a_macro_surprise_feeds_the_macro_member_and_a_cycle_without_one_reports_it_silent`
  drives one wire release and one synthetic release on the default cycle.
  The member is fed, the synthetic forecast is labelled simulated, a cycle
  with no new release reports the member silent, and the next same-direction
  release resolves the forecast true. Four mutations each fail it:
  1. building the inputs with no macros;
  2. feeding a release on every pass instead of once;
  3. dropping the synthetic tag in the world model;
  4. inverting the resolver.
- `WorldReading` gains `sources`, the feature value's own source list. Its
  four construction sites carry it. A synthetic release tags its level and
  its surprise alike.
- The test also drives two currency fixings on the default path, which give
  one move forecast. Dropping the `currencies` input fails it.
  `a_macro_forecast_waits_for_its_next_release_in_time_not_in_passes` runs
  200 passes inside a day and resolves the forecast on a release 30 days
  later. Counting patience in passes fails it.
  `a_currency_fixing_that_moves_is_forecast_to_keep_moving_and_the_next_fixing_answers_it`
  fails when the move is recorded inverted.
- **What a catalogued connector gives the member today.** All three
  catalogued macro connectors state no consensus: `ecb_key_rates`,
  `frankfurter_rates` and `nyfed_effr`. Switching one on therefore yields
  no surprise. The Frankfurter fixings reach the member through the
  currency path. The ECB and New York Fed rate series reach it only once a
  source states a consensus. Until then, surprises come from the narrative
  vendor adapter or the synthetic tape.

## What it costs

- The persistence figure is declared, not fitted, so until the member has
  a resolved record its view is a prior. The federation's arbitration
  requests more information on any proposition that carries an unscored
  member.
- A series released less often than the 7-day member lease leaves the
  member excluded from arbitration between releases. The next release
  renews it. The population check reports the gap, and that is accurate.
- The latest-read period per series lives in memory. After a restart, the
  first pass can feed a release the member already read once more.

## What would make this wrong

A macro feed that records a surprise against a consensus it did not state,
for example by imputing a consensus from the previous value. The member
would then read the platform's own guess as the economy's surprise, which
decision 1 relies on not happening. The guard belongs in
`MacroObservation::surprise`.
