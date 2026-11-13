# ADR 0470: The counterparty member models the venue network as a dependency graph, carries refusals along it as contagion, and resolves on refusals recorded in the next cycle

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-22. The delegation's limits hold: no figure the owner has not
  stated, no requirement reworded, no dependency, no paper-trading layer
  touched. The two constants it introduces — a per-hop transmission of 0.5
  and a three-hop bound — are stated priors and a work bound, not limits;
  nothing sizes, vetoes or trades on the member's views.
- **Date**: 2026-11-12
- **Rows**: R12-WORLD-3049.

## Context

R12-WORLD-3049: "The world-model federation includes counterparty/network
models covering brokers, exchanges, bridges, custodians, settlement
dependencies and contagion paths." Its check: "A registered federation member
with this scope exists and produces state; absent member fails a population
check."

`network.counterparty.global` existed (domain `counterparty/network`, in
`REQUIRED_FAMILIES`) and forecast, per venue, whether the next pass records a
feasibility refusal there, from the venue's share of the window's Evidence
refusals. The 2026-10-31 skeptic graded it PARTIAL on two counts. First, it
was a per-venue rate prior with no relation between venues, so bridges,
custodians, settlement dependencies and contagion paths were not modelled at
all. Second, its resolver read the refusal window itself: an eviction from a
full window could remove an entry counted at the forecast and resolve the
forecast false; a cell report stamped before the forecast but ingested after
it was never counted; and the forecasting cycle's own DECIDE, stamped at the
forecast instant, resolved its own next-pass proposition.

## Decision

1. **A counterparty graph.** `CounterpartyView` carries a
   `CounterpartyRole` — `broker`, `exchange`, `custodian`, `bridge`,
   `settlement_agent`, the five parts the requirement names — and the
   counterparties it depends on, each edge a `DependencyKind`
   (`execution`, `custody`, `settlement`, `bridge`).
   `Platform::counterparty_views` builds it from facts the platform already
   holds and from operator declarations, never from a name:
   - the desk broker is the `broker`;
   - every holder a positive holdings statement names is a `custodian` the
     broker depends on for `custody`;
   - a venue with chain settlement terms (ADR 0396) depends on
     `settlement:<venue>` (a `settlement_agent`) and, where the terms declare
     a bridge, on `bridge:<venue>` (a `bridge`);
   - `Platform::declare_counterparty` and
     `Platform::declare_counterparty_dependency` add named roles and edges.
     They refuse an empty name, a counterparty depending on itself, and
     declarations past the member's own bounds (32 counterparties, 128
     edges). A declared role outranks a derived one.
   - Every other venue the window or the tally names is an `exchange`.
2. **Contagion paths.** `contagion_paths` walks each counterparty's
   dependencies breadth-first, at most three hops, with a visited set so a
   cycle ends a path rather than charging a counterparty's refusals to
   itself. For every provider holding refusals it records the shortest path
   and a pressure of `share × 0.5^hops`. A counterparty's forecast combines
   its own share and the pressure on it as independent causes:
   `p = 0.05 + 0.85 × (1 − (1 − share)(1 − pressure))`. With no path the
   forecast is exactly what it was before. The paths are journaled on the
   `FederationReview` (`contagion`), so the cause a forecast named is in the
   log beside the forecast.
3. **A per-part population check.** The review names every role no
   counterparty held that pass (`counterparty_unmodelled`). A deployment that
   models no bridge says so by name instead of reading as a network without
   bridges.
4. **Resolution on what was recorded, by cycle.** `record_feasibility_refusal`
   tallies each Evidence refusal under the cycle that recorded it, beside the
   window. The tally is not evidence: nothing that withdraws a venue reads it,
   and the window keeps its one writer. A forecast made in cycle *N*
   resolves, once cycle *N + 2* runs, on whether cycle *N + 1* recorded any
   Evidence refusal at the venue. This excludes the forecasting cycle's own
   DECIDE, counts a late-ingested report in the cycle that recorded it, and
   cannot be falsified by eviction. The tally keeps only the cycles a
   forecast can still wait on.

The `FederationReview` stays at schema version 1. Both new fields are
`#[serde(default)]` additions, as `information_requests` and
`contradictions` were, so a record written before them decodes unchanged.
No topic moves to a new schema version, so ADR 0429 is not engaged.

## What it costs

A pardon signed after a refusal was recorded does not un-tally it. The
forecast asks whether the venue refused next pass, and it did; the pardon
changes what the refusal may withdraw, not whether it happened. Echoes and
refusals recorded already pardoned are never tallied.

Derived settlement and bridge nodes (`settlement:<venue>`, `bridge:<venue>`)
are named by the venue they serve, because the chain terms name no chain and
no bridge operator. No refusal is ever recorded at them, so they carry no
pressure until an operator declares a named provider that orders do reach.
They model the dependency honestly and transmit nothing yet.

The forecast resolves one pass later than before. The proposition is now
scored on a complete cycle rather than a partial one.

## What would make this wrong

- A transmission prior that calibration shows is consistently wrong. The
  remedy is a fitted transmission per edge kind, measured against resolved
  forecasts, not a different chosen constant.
- A counterparty network larger than 32 nodes in a deployment. The member
  forecasts for the first 32 in name order. It would then need a priority
  ordering, and this record would need amending.
- Anything that comes to read the tally to decide a withdrawal. That would
  make it a second writer of the evidence a venue is withdrawn on, which is
  what the window's single-writer invariant forbids.
