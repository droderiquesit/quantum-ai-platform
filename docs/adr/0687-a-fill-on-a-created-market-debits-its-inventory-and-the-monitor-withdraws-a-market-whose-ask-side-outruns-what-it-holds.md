# ADR 0687: A fill on a created market debits its inventory, and the monitor withdraws a market whose ask side outruns what it holds

- **Status**: Proposed 2026-11-15 by wave b52 lane 43. Written without a
  compiler; nothing here has been built or run.
- **Date**: 2026-11-15
- **Rows**: R12-EXEC-5080.
- **Extends**: ADR 0003, ADR 0021 (simulated venue only).
- **Paper boundary**: untouched. The factory still refuses any venue that is
  not simulated, and this change rests no order and sends nothing: the
  Terraform refusal, `AutonomyLevel::deployable` and the `Cell` constructor
  are not edited.

## Context

`MarketFactory::seed` commits inventory once and rests a ladder whose ask
side the inventory covers. Nothing afterwards changed the inventory, so a
market could sell the same units indefinitely while the validity monitor
believed it covered its ladder.

## Decisions

1. **`settle_fill` debits inventory.** A seeded market's inventory falls by
   the quantity a buyer took from its ask side; a fill above the inventory is
   refused, not cut to it, and every settled fill is journaled as `Filled`.
2. **No admission and no resting.** `settle_fill` rests nothing, so it needs
   no admission and cannot widen anything. Re-listing a trimmed ladder would
   rest orders and needs a fresh seed admission; it is not built.
3. **The monitor reads inventory.** `review` withdraws a seeded market whose
   venue-resting ask depth exceeds its inventory (`InventoryUncovered`),
   through the same withdraw-and-journal path as every other failure. That is
   the restrictive reading of an unbacked quote: withdrawn, not trimmed.
4. **Not built, and named.** `SimulatedBroker::fill_price` still sweeps the
   listed book without consuming resting depth, and no kernel path routes a
   fill on a created product back to `settle_fill`. Until both exist,
   `settle_fill` has no production caller and the row stays PARTIAL.

## What would make this wrong

A bid-side fill acquires inventory; this ADR does not model it, so a market
that buys is not credited. Crediting needs a bound on inventory first.
