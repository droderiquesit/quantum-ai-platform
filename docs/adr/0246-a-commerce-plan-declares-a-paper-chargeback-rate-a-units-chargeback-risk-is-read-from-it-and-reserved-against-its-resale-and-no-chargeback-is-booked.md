# ADR 0246: A commerce plan declares a paper chargeback rate, a unit's chargeback risk is read from it and reserved against its resale, and no chargeback is booked

- **Status**: **Accepted 2026-10-31 under the owner's delegation** (decision
  14 of `docs/blueprint/owner-decisions-2026-10-31.md`, the ruling on
  R12-COMMERCE-5090). The limits of that delegation hold here: no risk
  number, limit, threshold or limit set changes, nothing new is sized, no
  billed infrastructure, and no paper-trading layer is touched.
  Tighten-only by construction: the reserve is subtracted from expected
  resale and can only lower a margin.
- **Date**: 2026-10-31
- **Rows**: R12-COMMERCE-5090.
- **Amends**: ADR 0153 decision 8, its chargeback sentence only ("Chargeback
  risk stays `Absent` with its reason: the plan terms carry no settlement
  term to derive a rate from"). Decision 8's ledger clause, that commerce
  inventory reconciles against the marketplace and a break holds the unit,
  stands unchanged. ADR 0153's own file is a protected path and is not
  edited; this record is the amendment.

## Context

R12-COMMERCE-5090 asks that a unit's attributes include its payment
chargeback risk. ADR 0153 decision 8 kept the attribute `Absent` because the
plan carried no figure to derive a rate from, and inventing one in code
would be a number nobody stated. The research plane's simulated merchant
takes no payment network, so no chargeback can be raised on paper, and the
ledger books only facts a producer observed.

The gap is the input, not the arithmetic. A plan already declares the
other figures the plane prices with (fees, duty, purchase tax, delivery
risk). A chargeback rate is the same kind of declared figure.

## Decision

1. **The plan declares it.** `CommercePlan` gains a required
   `chargeback_rate`, a fraction in `[0, 1)`. A plan without one is refused
   at load, and a rate outside the range is refused when the plane opens.
   There is no default: a default rate would be the number ADR 0153 refused
   to invent.
2. **A unit's chargeback risk is read from it.** The unit attribute
   `payment_chargeback_risk` is a value: the declared rate applied to the
   unit's expected sale value, the candidate's reference price, with the
   rate named. Attributes are set at receipt, before any sale, so the
   expected resale and not a realised price is the base.
3. **Feasibility reserves it.** `FeasibilityInput` carries the rate, refused
   when missing like every other input, and `assess` subtracts the reserve,
   the rate on the expected resale, from expected resale. It can only lower
   a margin, so a candidate that was not viable never becomes viable for it.
4. **Nothing is booked.** No chargeback is raised, journalled as an
   operation or posted to the ledger. The simulated merchant takes no
   payment network; the reserve is a figure in an assessment, not an event.

## Consequences

- Every assessed candidate journals its chargeback reserve beside the six
  figures, and every received unit names its chargeback risk rather than
  "absent".
- Every committed plan must state the rate; the dev plan states a paper
  rate set by the desk.

## What it costs

- One more required field in every plan and fixture.
- A reader must know the reserve lowers expected resale and is not a
  booked loss.

## What would make this wrong

- A merchant that takes a payment network. Then chargebacks are observed
  facts and are booked from the network's own record, not reserved from a
  declared rate.
