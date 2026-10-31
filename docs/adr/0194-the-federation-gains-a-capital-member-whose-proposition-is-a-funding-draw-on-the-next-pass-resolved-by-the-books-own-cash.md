# ADR 0194: The federation gains a capital member whose proposition is a funding draw on the next pass, resolved by the book's own cash

- **Status**: **Accepted**, 2026-10-31. Decided by lane L4-quant under the
  parent's 2026-10-31 delegation (`docs/blueprint/owner-decisions-2026-10-31.md`,
  decision 2), which lets a lane decide a row that waited only on a product
  decision. The number was claimed on 2026-10-24 under that day's delegation.
  The 2026-10-31 delegation does not lift four limits: no live-order path, no
  real capital or money movement, no weakened test or limit, and no new
  dependency. This decision needs none of them.
- **Date**: 2026-10-31
- **Keeps**: ADR 0003 (paper trading), ADR 0005 (confidence as arithmetic),
  and the federation's population check (WORLD-3037).

## Context

R12-WORLD-3048 asks for a world-model federation that includes portfolio,
capital and liquidity models. The requirement names six things: exposures,
collateral, funding, leverage, optionality and capital mobility. Its
acceptance is an inspection: a registered member with this scope exists and
produces state, and a missing member fails the population check.

The row stayed `PARTIAL` with no code for one reason. Every federation member
forecasts a proposition that reality later answers. The capital-side data the
platform has is per currency and per funding need:
- `DemandForecaster` output;
- the `pre_positioning` exposure.

Neither of these is a yes-or-no question that one later pass settles. So the
member needed a proposition and a resolver. That was new federation
vocabulary, and nobody had chosen it.

## Decision

1. **The proposition is "will the next pass find the book's cash lower than
   this pass did?"** It is named `capital:funding-draw-next-pass@<cycle>`. A
   funding draw is the event a desk funds:
   - cash spent on a fill;
   - costs charged.

   The tracked capital already holds cash after every fill and charge, so
   the resolver needs no new record. It compares the next pass's cash with
   the cash this pass recorded.
2. **The member is `portfolio.capital.global`**:
   - domain `portfolio/capital`;
   - horizon one pass;
   - observational.

   It reads a `CapitalView`, built from the paper book in
   `understand_federation`:
   - cash as a share of equity;
   - gross exposure over equity (the leverage);
   - net exposure over equity;
   - the number of approved theses still waiting for construction. These
     would be the book's open claims on capital for the next pass, but on
     the shipped cycle the count always reads zero. UNDERSTAND runs before
     REASON queues theses, and DECIDE drains the queue in the same cycle.
     The term in decision 3 is therefore inert on the shipped path. It is
     kept because a caller that holds theses across passes would read it,
     and it is recorded as a gap rather than presented as working.

   Money crosses from `Decimal` to `f64` at that one place, as statistics.
   Nothing comes back out as an amount.
3. **The probability is a stated formula.** It is an arithmetic prior, not
   a fitted model:
   - start at 0.15;
   - add 0.5 when a thesis waits for capital;
   - add 0.15 times the cash share, taken up to one.

   The federation's calibration charges the member for every miss.
   Calibration is the measure; the formula is only where it starts.
4. **The member is silent when equity is not positive.** A share of nothing
   is not a number, so the member writes no view at all. A view of zero
   would read as "no draw expected". A member that is fed nothing lets its
   lease lapse, and the population check then names `portfolio/capital` as
   uncovered.
5. **`portfolio/capital` joins the required families.** Its absence is now a
   finding in `FederationReview::uncovered`, as the acceptance asks.
6. **Not decided here, and still unmet**:
   - collateral and capital mobility. The paper book posts no collateral and
     moves no capital between regions. ADR 0039's region shares are policy,
     not a forecast this member could be scored on.
   - optionality. No option position exists in the book.
   - liquidity, which the requirement names in its title. This member does
     not model it; the liquidity read that sign-off uses is a separate
     control and is not a federation member.
   - leverage and net exposure as modelled state. The view carries both
     figures, but nothing reads them yet, so they are recorded and not
     forecast.

   These clauses of the requirement stay open, and the row says so.

## Consequences

- The funding member is scored like every other member:
  - by resolution, on the log;
  - with no path to a decision. ADR 0125's disagreement rule narrows sizing
    only for instrument propositions (`narrowings_of`), so a funding view
    cannot narrow anything.
- The member cannot move money and cannot place an order. It reads the
  paper book and forecasts that book's own cash.

## What it costs

- One more member in every pass: one view, one pending proposition and one
  resolution per cycle, bounded by the same `FEDERATION_PENDING_CYCLES`
  window as every other proposition. No new store, service or dependency.
- One more required family. A composition that does not hand the stage a
  book view now reports `portfolio/capital` as uncovered once the member's
  lease lapses. That report is the point, but it is a new finding an
  operator will see.
- A forecast that looks like capital modelling and is narrower than the
  requirement. The ADR names what is unmet so that nobody reads the member
  as covering collateral, optionality, capital mobility or liquidity.

## What would make this wrong

- A capital-mobility or collateral record, once the platform has one. That
  needs a proposition of its own, not a widening of this one.
- A measured calibration showing the prior is worse than a constant. Then
  the formula should change, and this ADR should be amended to say so.
- Theses that persist across passes. The waiting-thesis term would then
  start to act, and its weight should be argued from a measurement rather
  than kept from this prior.
