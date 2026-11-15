# ADR 0681: A measured funding rate raises the stated borrow and never lowers it

Status: Proposed (lane 40, wave b52, 2026-11-15). Provisional.
Nothing was compiled when this was written (owner instruction 2026-11-15).

## Context

R12-MESH-5055: an edge's net value includes borrow cost. The centre ships a
stated `borrow_rate_annual`; the cell's `FundingState` holds the measured
rate. The two were never combined.

## Decision

1. The cell charges the higher of the stated and the measured rate
   (`qip_edge_node::measured_borrow::with_measured_borrow`). A measurement
   never relaxes a signed cost.
2. A measured rate is read as an annual fraction. One at or above 1 is refused
   as a probable percentage. Provisional: the unit of `FundingRate` is not
   fixed by its type, and 1 is a restrictive guess, not a figure anyone gave.
3. A negative measured rate leaves the stated figure; a rebate is not
   netted against a borrow, because `EdgeCosts` refuses a negative rate.
4. The path-wide 5% carry stays a constant assumption.

## Not decided here

Calling `with_measured_borrow` from the desk refresh. The graph is built from
the whitelist without the cell's funding state, and which instrument of a
conversion is the borrowed one is a design choice for the wiring change.

## What it costs

Once the function is called, a measured rate above the stated one raises the borrow rate charged on the edge costs it is applied to, which lowers that edge's net value; a path the stated figure admitted may then no longer clear. A measured rate of 1 or more makes `with_measured_borrow` return an error rather than a figure, so a rate reported as a percentage must be re-reported as a fraction before the function yields costs for that instrument. The ceiling of 1 is a guess; the unit of `FundingRate` is not fixed by its type, and a real borrow cost at or above 100% a year is refused with it. A rebate (negative rate) is never netted, so a cheaper-than-stated borrow is left unused. Nothing calls the function yet, so no desk path is affected; the desk-refresh wiring is a row left unbuilt, and its cost is unmeasured.

## What would make this wrong

A source that reports funding as a percentage would be refused whenever its figure is 1 or more, and silently accepted as a fraction whenever it falls below 1; the refusal is not a unit check, and the unit would have to be fixed in the `FundingRate` type or where the rate is reported. Observed refusals of paths whose true borrow cost was below the stated figure would show the stated floor is stale, and the rule should be revisited together with how the stated rate is set. If measured rebates are persistent and material, ignoring them leaves value unclaimed and `EdgeCosts` would need to accept a negative rate through its own ADR. If a conversion's borrowed leg cannot be identified at the call site, the rule has no instrument to look up and the wiring decision must come first.
