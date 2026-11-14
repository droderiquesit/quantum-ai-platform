# ADR 0492: Book gamma and vega are measured from the catalogue's Greeks and gated by limits that refuse unmeasured convexity

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**,
  session t50-33 (wave580). An in-repository design decision: it adds two
  `LimitKind` arms, two `RiskState` fields and one kernel derivation. It
  adds no dependency, ships no new limit in any default set, states no
  bound the owner did not state, and touches no paper-trading layer.
- **Date**: 2026-11-13
- **Rows**: R12-RISK-4095 (stays PARTIAL; see "What this leaves open").

## Context

R12-RISK-4095 asks that risk cover market exposure — price, volatility,
gap, basis, Greeks, factor and causal-driver — and its verification reads:
"Each named exposure in the row has a measurement and either a
deterministic limit or a recorded finding; a test drives each."

For Greeks the measurement was half there and the limit was absent.
`qip_kernel::hedge::BookGreeks` carried delta (the book's net signed
notional, already limited by `MaxNetExposure`) and reported gamma and vega
as `Some(0.0)` for a book held entirely in linear instruments and `None`
for anything else. That `None` was not a limitation of the data: the
catalogue's option, volatility and structured records already carry
per-unit `Greeks` on their extension (`Extension::greeks`), and the kernel
never read them. No `LimitKind` read gamma or vega, so a book of options
had no convexity control at the gate at all.

An earlier lane (L24, 2026-10-18) declined to add the limits first, and was
right to: a limit fed from a figure nobody computes is the
`MaxExpectedShortfall` defect. This decision supplies the figure and the
limit together, and proves both through the governed order path.

## Decision

1. **One derivation, read by the gate and the desk.**
   `qip_kernel::hedge::convexity_of(universe)` maps every catalogue record
   that can state its convexity to a `qip_risk::limits::Convexity` — gamma
   and vega per unit of signed notional. A linear instrument is a measured
   zero. Anything else is the record's extension Greeks, scaled to one
   contract by its contract multiplier and divided by its price. A record
   with no Greeks, non-finite Greeks, or a non-positive price or multiplier
   gets **no entry**. `Platform::risk_state_from` writes the map into
   `RiskState::convexity`, and the Hedge Brain's `ExposureSurvey` computes
   `BookGreeks` from the same map, so the figure a limit refused on and the
   figure the desk is shown cannot disagree.

2. **The divisor is the notional the state books.** The desk books an
   order's notional as quantity times price (`ProposedOrder::notional`) and
   a fill's as quantity times fill price — no contract multiplier. So the
   contracts a position holds are its notional over the price, and a
   contract's sensitivity is the per-unit Greek times the multiplier. The
   first form of this function divided by `FinancialObject::notional(1)`
   (price times multiplier), which cancels the multiplier and reads every
   option book a hundred times flatter than it is; the integration test's
   breaching order was admitted under it. Whether the desk *should* book
   the multiplier into notional is a separate question this decision does
   not answer — it records that the convexity figure follows the notional
   convention, and the test fails if the convention moves without it.

3. **`MaxBookGamma { limit }` and `MaxBookVega { limit }`.** Gamma is
   compared as an absolute figure in the catalogue's own units (change in
   underlying delta per unit move, summed over contracts). Vega is the
   value change for a one-point move in implied volatility, as a fraction
   of equity; a book with no positive equity refuses, as every
   equity-denominated limit does. Both read `RiskState::book_convexity`,
   which sums notional times sensitivity over held positions — so the
   pre-trade projection, which already moves `position_notionals`, charges
   the order under check with no second projection.

4. **Absent is unmeasured, and unmeasured refuses.** A held or ordered
   instrument with no `convexity` entry makes `book_convexity` return that
   instrument, and both limits record a `Critical` breach naming it. An
   option nobody priced is never read as flat.

5. **Two exemptions, so a book can always come back.**
   `PreTradeChecker::project` records `convexity_before_order`; an order
   that does not raise the absolute figure is never refused by the limit,
   so a book over its bound can sell down. On a book whose convexity cannot
   be measured at all, an order that does not raise gross is admitted, so a
   position whose Greeks are missing can still be closed. These mirror
   `MaxSurvivalGross` and `MaxMarginUtilisation`.

6. **Not in any default set.** No owner has stated a gamma or vega bound,
   and a figure nobody stated is not one this lane invents. The limits are
   available to a deployment's limits file and narrowed by the desk
   tolerance like every other maximum (`LimitKind::narrowed_by`).

## Evidence

- `qip-kernel/tests/market_risk_convexity.rs` drives orders through
  `Platform::submit_order`: a priced option is admitted inside the gamma
  bound and refused past it, refused past the vega bound, and an option
  whose record states NaN Greeks is refused naming the instrument while a
  stock beside it is admitted.
- `qip-risk/tests/limit_fixtures.rs` carries pass and veto fixtures for
  both arms (the exhaustive table test now covers 24 kinds), the
  unmeasured refusal, both exemptions, and the insolvent-equity refusal.
- `qip-risk-engine/tests/risk_engine.rs` shows the projection's "before"
  admits a sale that lowers gamma on a book already past its bound.
- Each test was mutation-verified; the mutations are named in the doc
  comment of each test.

## What it costs

- **Every risk state carries a convexity entry per catalogue record.**
  `Platform::risk_state_from` builds the map from the whole universe on each
  call, because the order under check is not known when the state is built.
  That is one `BTreeMap` of the catalogue's size per pre-trade check.
- **A desk that installs either limit trades no option the catalogue cannot
  price.** That is the intent, and it is a cost: a record arriving without
  Greeks blocks new orders in that instrument until its source states them.
- **The figure follows the notional convention.** Decision 2 couples the
  divisor to how the desk books notional; a change to one is a change to
  both, held only by the integration test.
- **Two more arms on an exhaustive enum.** Every match over `LimitKind` —
  the edge gate's `evaluable`, tolerance narrowing, the fixture table — had
  to answer for them, and will for the next.

## What this leaves open

R12-RISK-4095 stays **PARTIAL** (gap dated 2026-11-13):

- **Basis.** No book-level basis figure exists. `basis_volatility` is an
  input on a hedge candidate (`qip-risk/src/hedge_search.rs`), not a
  measurement of the held book, and `Universe::basis` prices one future
  against its underlying. A book basis needs hedge pairs carried in the
  aggregate; until then there is no measurement to limit.
- **Gap.** A gapped fill is charged at its fill price, and the stress
  library's worlds price discontinuous moves, but no named "gap exposure"
  figure reaches the gate or a recorded finding.
- **Greeks beyond gamma and vega** (theta, rho, vanna, vomma) are carried
  on the catalogue record and not aggregated.
- **Deployability.** No deployed process loads a catalogue with options,
  and no shipped limits file names either limit.

## What would make this wrong

- If the catalogue's Greeks were stated per contract rather than per unit
  of the underlying, the multiplier would be applied twice. The fixture
  convention (`qip-financial/tests/object_model.rs`: delta 0.45 on a
  multiplier-100 call) is per unit, which is the market convention.
- If the desk began booking notional with the multiplier, decision 2's
  divisor would have to move in the same change.
