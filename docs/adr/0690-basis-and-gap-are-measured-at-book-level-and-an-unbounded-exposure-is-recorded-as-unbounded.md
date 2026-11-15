# ADR 0690: Basis and gap are measured at book level, and an exposure with no stated bound is recorded as unbounded

- **Status**: **Proposed 2026-11-15**, lane 45 of wave b52, under the owner's
  instruction of 2026-11-15. An in-repository design decision: one new pure
  module in `qip-risk`, no dependency, no new default limit, no
  paper-trading layer touched. **Written and committed without compiling or
  running any test** (owner instruction of the same date); the pipeline's
  build after merge is the first time it is compiled.
- **Date**: 2026-11-15
- **Rows**: R12-RISK-4095 (stays PARTIAL). Also records the lane's finding on
  R12-RES-7104 and R12-RISK-3022 below.

## Context

R12-RISK-4095's check reads: each named exposure has a measurement and either
a deterministic limit or a recorded finding. ADR 0492 closed Greeks. Basis and
gap remained: `basis_volatility` is a per-candidate input to the hedge search,
and the stress library prices a tail-risk-event world, but no figure for either
reaches a reader of the book.

## Decision

1. **Basis** is measured as the plain sum, over held hedge pairs, of
   `notional × basis_volatility × sqrt(holding_days) × BASIS_QUANTILE`: the
   expression `hedge_search` already prices a planned hedge with, so the book
   figure and the plan figure are one formula. Pairs are summed, not netted:
   the restrictive reading, since basis moves between pairs are not assumed to
   diversify.
2. **Gap** is the worst `loss_fraction × gross_notional` over the stress worlds
   the caller names. An empty list is refused, because zero would read as
   protection.
3. **The decision is a finding, not yet a limit.** `assess` returns `Within`,
   `Breach` or `Unbounded`. With no bound stated the verdict is `Unbounded`,
   never `Within`: "nobody stated a bound" and "inside the bound" must not read
   alike. No `LimitKind` arm is added, because adding one without an owner's
   bound is the `MaxExpectedShortfall` defect, and an exhaustive `match` on
   `LimitKind` could not be edited safely without a compiler in this lane.
4. Malformed inputs (negative, non-finite, outside `[0, 1]`) are refused, never
   clamped.

## What this leaves open

* No production caller yet reads the position register for held hedge pairs or
  the stress library for the gap world, so the findings are not journaled.
  That wiring, and `MaxBasisExposure` / `MaxGapExposure` limit arms, need a
  compiling session and an owner-stated bound (**provisional: none chosen**).
* Theta, rho, vanna and vomma remain unaggregated.

## Findings on two other rows of this lane

* **R12-RES-7104**: the remaining step is that the capital-grant route is
  refused for every standing credential (ADR 0065) until a per-person
  credential exists (ADR 0076). `Platform::issue` documents that a cycle stage
  must not issue capital: it would forge control 4. The wiring is therefore
  not a code change this lane may make; it waits on ADR 0076.
* **R12-RISK-3022**: making `PreTradeChecker::check` demand a
  `DeterministicGate` is a signature change across about thirty call sites in
  other lanes' files. It is one atomic change for a session with a compiler.
