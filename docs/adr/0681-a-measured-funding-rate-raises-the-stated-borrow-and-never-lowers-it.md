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
