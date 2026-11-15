# ADR 0652: A cycle leg's `OrderSent` names its cycle, and the ledger's register judges each cycle from its legs

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

## Consequences and what stays open

- A cycle whose other legs were never sent shows only the sent legs, so a lone
  filled leg reads `Complete`. Judging against the intended leg count needs
  `CycleCommitted`'s order list on the ledger, which means putting it on P1 or
  carrying a leg count on `OrderSent`; neither is decided here.
- A *missed* cycle (admitted, nothing sent) leaves no register row at all.
- The verdicts are not yet raised as a break, counted, or carried to the
  centre's halt signer.
- No node, broker, ledgerd test of a two-region cycle with a hedge exists; the
  acceptance test reaches the store directly, as `cycle_ledger.rs` always has.
