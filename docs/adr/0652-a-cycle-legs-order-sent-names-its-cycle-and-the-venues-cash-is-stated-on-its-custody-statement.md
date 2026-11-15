# ADR 0652: A cycle leg's `OrderSent` names its cycle, and the venue's cash is stated on its custody statement (two ledger decisions of lane 26)

- **Status**: **Accepted**, 2026-11-15, wave b52 lane 26, under the owner's
  2026-10-31 delegation of in-repository design decisions. Paper only: nothing
  here sends an order or moves a balance.
- **Date**: 2026-11-15
- **Implements**: part of R12-LEDGER-10114 (cycle identity on P1; replay that
  evaluates a failed cycle).
- **Keeps**: ADR 0003; ADR 0153 decision 6 (`CycleCommitted` stays on P2, no
  cycle-completion record on P1); ADR 0490 (the register moves no balance).

## Context

A multi-region cycle's legs reached P1 as unrelated `OrderSent` and `Filled`
records. The register (ADR 0490) could say an order filled, not that its
sibling leg did not, so a cycle that filled one leg and lost the other, which
leaves the books holding a position whose offset never arrived, raised nothing.
ADR 0153 decision 6 declined a completion record on P1 because "every leg
filled" is derivable from booked fills. It is derivable only if the legs are
known to be one cycle.

## Decision

1. `Decision::OrderSent` gains `cycle_id: Option<String>`, serde-defaulted and
   skipped when absent, so chains sealed before it still verify. The cell sets
   it on cycle legs (`send_cycle_leg`) and leaves it absent on net orders.
   This is the leg's identity, not a completion record; decision 6 of ADR 0153
   stands.
2. The ledger's `OrderRow` carries it, copied from `OrderSent` in
   `register_move`. Hedges carry none: a hedge is not a cycle leg.
3. `qip_ledgerd::cycles::evaluate` groups rows by `(cell, cycle_id)` and
   judges each: `Open` if any leg is open; else `Complete` if every leg
   filled; else `Partial` if any quantity filled; else `Failed`.
   `LedgerStore::cycles()` is the read. A verdict is a report, never a posting.

## What it costs

- A cycle whose other legs were never sent shows only the sent legs, so a lone
  filled leg reads `Complete`. Judging against the intended leg count needs
  `CycleCommitted`'s order list on the ledger, which means putting it on P1 or
  carrying a leg count on `OrderSent`; neither is decided here.
- A *missed* cycle (admitted, nothing sent) leaves no register row at all.
- The verdicts are not yet raised as a break, counted, or carried to the
  centre's halt signer.
- No node, broker, ledgerd test of a two-region cycle with a hedge exists; the
  acceptance test reaches the store directly, as `cycle_ledger.rs` always has.

The record carries one more optional field on every cycle-leg `OrderSent`,
and `evaluate` walks every register row on each `LedgerStore::cycles()` read;
that scan is unmeasured and grows with the register. The verdict is a report
that nothing yet acts on, so the work of noticing a `Partial` cycle still falls
to whoever reads it.

## What would make this wrong

- A `Partial` or `Complete` verdict that disagrees with the cycle's real
  outcome in practice, for instance because a lone filled leg reads `Complete`,
  would show the sent-legs-only judgement is too weak and the intended legs
  must reach the ledger.
- A cycle whose legs are split across cells or carry differing `cycle_id`
  values would show `(cell, cycle_id)` is the wrong grouping key.
- A register large enough that the full scan in `cycles()` is slow would show
  that judging every cycle from every row on each read is the wrong read.
- A reader needing the verdict to halt or be counted, rather than only
  reported, would show that leaving the verdicts unraised was the wrong place
  to stop.

---

# Second decision: the venue's cash on its custody statement (R12-LEDGER-10185)

This record holds both of lane 26's ledger decisions because a lane owns two ADR numbers and has four rows.

## Context

ADR 0494 rebuilt positions from the venue's custody statement and left every
cash unit out of the comparison on both sides. A venue whose cash drifted from
the booked fills raised nothing.

### Decision

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

### Still open

- **Venue costs.** Until `ExecutionReport` carries the fee into
  `Decision::Filled { fee }`, costs are neither booked nor compared. This is a
  figure the venue states and the ledger does not hold, not a tolerance.
- A `custody_break` still reaches no centre halt signer (R12-LEDGER-8139).
- `CustodyMovement` postings to `custody:<venue>`, and a deployed
  authoritative store (Spanner absent).
