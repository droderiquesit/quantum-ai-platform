# ADR 0691: An order on an unpriced instrument is refused behind a one-way switch, until fixtures price their instruments

- **Status**: **Proposed 2026-11-15**, lane 45 of wave b52. An in-repository
  design decision: one `Platform` field, one switch, one refusal. Restrictive
  only, no dependency, no paper-trading layer touched. **Written without
  compiling or running any test** (owner instruction of 2026-11-15).
- **Date**: 2026-11-15
- **Rows**: R12-RISK-4096 (stays PARTIAL), R12-RISK-9049 (not built; see below).

## Context

The owner ruled on 2026-10-31 that an order on an instrument with no quote in
the platform's snapshot is refused and journaled. Built default-on, the gate
refused 92 kernel tests across 16 targets, because their fixtures submit on
instruments nothing priced; it was withdrawn.

## Decision

1. `Platform::require_observed_quote()` turns the gate on. When on,
   `submit_order` refuses an order whose instrument has no quote, trade or bar
   in `MarketView::snapshot`, journaled as `Action::Rejected` under control
   `quote-availability` and counted on `qip_orders_refused_total`.
2. **There is no method that turns it off.** A switch that can relax a safety
   default is what principle 3 (fail closed) refuses.
3. **Provisionally default off** (the figure here is the default, not a bound):
   the restrictive default is blocked only by fixtures. The follow-up slice
   seeds an observed price in every order-submitting fixture, flips the default
   to on, and deletes the field. Until then no composition root turns it on;
   that is a one-line change in `qip-api`, `qip-fastbrain` and `qip-deepbrain`
   for a session that can run their suites.
4. Borrow stays fail-closed and unmeasured: an admitted licensed borrow source
   is an owner vendor decision under ADR 0157.

## R12-RISK-9049, not built

Routing hedge selection through `ComputeRouter` means formulating the choice of
a hedge portfolio as a `PortfolioProblem` (a selection over candidates with the
survival floor as a constraint and cost as the objective), with the classical
`hedge_search::search` answer always computed (ADR 0006) and the router's
result accepted only if it is no costlier and still meets the floor. That
formulation touches `qip-optimization-engine`'s problem encoding, which this
lane could not read and test without a compiler. Nothing was written.

## What it costs

Until the follow-up slice lands, the gate is off in every composition root, so
an order on an unpriced instrument is still admitted there and the owner's
2026-10-31 ruling is only partly met; R12-RISK-4096 stays PARTIAL. Once a root
turns it on, an operator or feed must price every instrument before the first
order on it, or that order is refused and journaled. Each submission on that
path takes one read of the market snapshot. A session able to run the suites
must still seed prices in every order-submitting fixture, flip the default and
delete the field. The cost of the read is unmeasured, and R12-RISK-9049 is left
unbuilt.

## What would make this wrong

A refusal appears in a journal for an instrument that a feed had in fact
priced, which would mean the snapshot check misses a source of observation
other than quote, trade or bar. Seeding fixtures proves larger than expected,
or the field outlives the follow-up slice, so the unsafe default becomes
permanent. A legitimate flow needs to trade an instrument before any price
exists, such as a first order priced from a reference. Any caller turns out to
need the gate switched off again, which would show the one-way design too rigid.
