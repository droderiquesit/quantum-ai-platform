# ADR 0653: The venue's cash is stated on its custody statement, before its own costs, and the ledger compares it with trading and in-settlement cash

- **Status**: **Accepted**, 2026-11-15, wave b52 lane 26, under the owner's
  2026-10-31 delegation. Paper only; a statement moves no balance.
- **Date**: 2026-11-15
- **Implements**: the cash half of R12-LEDGER-10185.
- **Keeps**: ADR 0494 (statement on the cell's chain, compared as applied,
  a difference is a counted break, never an adjustment), ADR 0442.

## Context

ADR 0494 rebuilt positions from the venue's custody statement and left every
cash unit out of the comparison on both sides. A venue whose cash drifted from
the booked fills raised nothing.

## Decision

1. `Decision::CustodyStatement` gains `cash: Vec<(unit, change)>`, serde
   defaulted and skipped when empty, so sealed chains verify. Each unit must be
   one of `cash_units`. A cash unit with a stated balance is compared; one
   without stays left out as before.
2. The stated figure is the **change** in the venue's clearing cash since it
   opened (its endowment is no booked fill), **before the venue's costs**:
   `settled - opening + costs`. The cell journals fills with no fee
   (`ExecutionReport` carries none), so the ledger books the trade's cash and
   nothing for what the venue charged. A statement net of costs read every
   charged fill as a break: the first run of the node-pass acceptance test
   found `USD: the ledger books -378.75 and XLON states -378.82575`.
3. The ledger side of a stated cash unit is the cell's `trading:` plus
   `in_settlement:` postings in that unit. The venue's clearing cash moves at
   the fill; the ledger parks a T+n sale's proceeds in settlement.
4. A cash-only change is a changed statement and is journaled
   (`Cell::record_custody_statement_with_cash`); the old entry point states no
   cash.

## Still open

- **Venue costs.** Until `ExecutionReport` carries the fee into
  `Decision::Filled { fee }`, costs are neither booked nor compared. This is a
  figure the venue states and the ledger does not hold, not a tolerance.
- A `custody_break` still reaches no centre halt signer (R12-LEDGER-8139).
- `CustodyMovement` postings to `custody:<venue>`, and a deployed
  authoritative store (Spanner absent).
