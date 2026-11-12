# ADR 0440: A paper fill at a settlement-cycle venue books its cash in settlement, and only the venue's `Settled` record moves it to cash

- **Status**: **Accepted**, 2026-11-12, session t50-07 (wave580), under the
  owner's 2026-10-31 delegation of in-repository design decisions
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Paper only: no live
  variant is added, `Settlement` keeps its one variant, and every new record
  is refused as live by the same fence a fill meets.
- **Date**: 2026-11-12
- **Implements**: ADR 0130 decisions 1, 2 and 6, ledger side.
- **Keeps**: ADR 0003, ADR 0100 §9 (the fourth paper fence), owner
  decision 26 (no `Filled` shape change at fabric v1).

## Context

ADR 0130 decided that a simulated fill's cash leg posts to an in-settlement
subledger and moves to cash only when the simulated venue reports the
settlement. It left four things open, and R12-LEDGER-5011 cannot be built
without answering them:

1. How the ledger knows, at the fill, that a venue settles on a cycle. The
   cell holds `qip_edge::settlement::SettlementTerms` per venue, but
   `Decision::Filled` carries no terms, and adding a field to it changes the
   locked shape owner decision 26 holds for the fabric versioning ADR.
2. How "each ledger event carries its settlement state" is read, given that
   `Settlement` must keep exactly one variant
   (`settlement_has_no_variant_but_simulated`).
3. What one `Settled` record settles when an order filled in parts.
4. Whether a fee waits on settlement.

## Decision

1. **The stage is a separate type from the fence.** `SettlementState`
   (`Immediate`, `InSettlement`, `Settled { settlement_date, fills }`) is
   read off every event by `LedgerEvent::settlement_state`. `Settlement`
   stays `Simulated` only; the stage describes where simulated cash stands,
   never whether it was live. Only the two non-immediate stages are stored
   (`skip_serializing_if`), so every event written before this re-encodes
   byte for byte and reads as `Immediate`, which is what it was.
2. **An in-settlement posting must state its stage.** `LedgerEvent::new`
   refuses an `in_settlement:` posting; `LedgerEvent::staged` refuses a
   non-immediate stage with no such posting, and a `Settled` stage naming no
   fills or fills out of strict ascending order. The wire path routes a
   stored stage through `staged`, so a stage stripped from a stored event is
   refused on read rather than read as immediate.
3. **The ledger is told which venues settle on a cycle by its own
   composition root**, through `LedgerStore::with_settlement_cycle_venues`,
   and books every other venue's cash at the fill as before. This is a
   second statement of a fact the cell's `SettlementTerms` also states. It is
   accepted because the only alternative that keeps one statement, a field
   on `Filled`, is held by owner decision 26. When the fabric versioning ADR
   moves `Filled` to v2, the fill should carry its venue's terms and this
   setting should go.
4. **`Decision::Settled { order_id, venue, settlement_date, simulated }` is a
   new variant, appended last**, so no sealed entry's digest moves and the
   locked `Filled` sample is unchanged. It is a P1 outcome (ADR 0130
   decision 2): a settlement shed with P2 would leave cash pending for ever.
   A consumer built before this variant cannot decode it; it parks the cell,
   which fails closed.
5. **One `Settled` record settles every held fill of its order.** The
   ledger keeps the order's in-settlement fills in a `pending/<cell>/<order>`
   row written in the batch that books each fill and deleted in the batch
   that books the settlement. The settlement reverses each held fill's
   in-settlement legs into `trading:` and names their journal sequences, so
   the walk from settlement back to fill is a field join (ADR 0130
   decision 3). A settlement with nothing held, for another venue, or not
   marked simulated parks the cell; nothing is adjusted.
6. **A reported fee is charged to the book at the trade date.** The venue
   charged it when it reported the fill; only the proceeds wait.
7. **The setting is not wired into `qip-ledgerd`'s environment until the
   producer exists.** A venue named as settling on a cycle with no
   `Settled` producer would hold its cash in settlement for ever. Today the
   node's simulated feed installs `SettlementTerms::instant()` for its venue,
   so no deployed venue settles on a cycle and the default, an empty set, is
   the true one.

## Consequences

- The ledger's spendable cash agrees with the cell's settlement gate for any
  venue the composition root names, and every event says which stage its
  cash is at.
- R12-LEDGER-5011 stays PARTIAL: the producer (the simulated gateway's
  settlement clock in `qip-edge-node`, for a venue with cycle terms) and the
  composition-root setting remain, and must land together.

## What it costs

- A second statement of which venues settle on a cycle, in the ledger's
  composition root beside the cell's `SettlementTerms`, until `Filled` can
  carry the terms. The two can drift; a drift shows as cash held pending for
  ever or as a settlement that parks the cell, both visible and neither
  silent.
- A `pending/` row per order in settlement, rewritten on every partial fill
  and deleted at settlement. Bounded by the orders in settlement, not by
  history.
- A new failure mode ADR 0130 already named: a lost `Settled` record leaves
  cash in settlement for ever. It is visible as a non-zero `in_settlement:`
  balance, and it is why decision 7 keeps the setting off until the producer
  lands.
- A consumer built before `Decision::Settled` cannot decode it and parks the
  cell. That is fail-closed, and it is a reason to deploy the ledger before
  any node that emits the record.

## What would make this wrong

- If the fabric versioning ADR lets `Filled` carry terms, decision 3's
  second statement should be removed, not kept beside the field.
- If a settlement can fail or settle in parts (a fail-to-deliver), decision
  5's "one record settles every held fill" cannot express it.

## Rows

R12-LEDGER-5011.
