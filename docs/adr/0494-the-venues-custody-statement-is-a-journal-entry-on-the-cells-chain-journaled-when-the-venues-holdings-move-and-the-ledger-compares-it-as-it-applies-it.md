# ADR 0494: The venue's custody statement is a journal entry on the cell's chain, journaled when the venue's holdings move, and the ledger compares it as it applies it

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). Session t50-34. The delegation's limits hold:
  - it sets no figure: the statement is journaled on a change, not on a
    period, so no period had to be chosen;
  - it relaxes no limit, guard or refusal; it adds a refusal (a live
    statement parks the cell, as a live fill does) and a comparison that
    did not run before;
  - it touches no paper-trading layer, and adds no dependency or crate edge.
- **Date**: 2026-11-13
- **Extends**: ADR 0130 decision 4 (custody truth is the simulated venue's
  position statement, published on P1) and ADR 0442 (what is compared, at
  which chain position, and what a break does). It decides what both left
  open: how the statement travels, when it is taken, and who calls the
  comparison.

## Context

R12-LEDGER-10185: "Reconciliation rebuilds and verifies positions and cash
from the authoritative ledger plus venue/custody statements without trusting
caches." ADR 0442 landed the comparison (`qip_contracts::ledger::custody`,
`LedgerStore::reconcile_custody`, `qip_ledger_custody_breaks_total{direction}`)
and named its own gap: no producer published a statement, and no binary
called the comparison. The acceptance suite built its statements by hand and
stamped each with a chain position it computed itself.

Three questions needed answers. How does the statement reach the ledger,
and with which chain position? When is one taken? Who compares it?

## Decision

1. **The statement is a journal entry, `Decision::CustodyStatement`, on the
   cell's own chain**, appended as the last variant so no sealed digest
   moves. Its chain position is the entry's own: every fill the cell
   journaled before it is in the venue's figure, and nothing journaled after
   it is. ADR 0442's `(session, sequence)` stamp is then not a claim the
   statement makes about itself but a fact of where it sits, and a
   statement cannot be stamped at a position it does not occupy. It is a P1
   outcome, beside the fill (`event_fabric::writer::is_outcome`), because
   the ledger reads P1 and a statement shed with P2 would leave a break
   unfound for as long as the stream was shedding.
2. **The cell journals it, and only when the venue's statement moved.**
   `Cell::record_custody_statement` journals the statement only when it
   differs from the last one journaled for that venue. A venue with no
   statement journaled yet that names no holding has not moved: holding
   nothing is where every chain starts, so that first statement is not
   journaled, and the first one that names a holding is. Every fill moves
   the venue's holdings, so every fill is followed by a statement that
   covers it, and a quiet cell does not grow its chain by a statement per
   pass. A venue that stated a holding and now states none has moved, and
   that statement is journaled.

   This was first written as "on the first call, and thereafter only when
   it differs". That put a P1 outcome on a pass the capital gate had refused
   outright, and `vertical_slice_cell_ledger` refuses exactly that: an
   unfunded cell, and an order past the grant's per-order limit, book
   nothing. The cost of the narrower rule is real and is this: a cell that
   restarts against a fresh simulated venue no longer journals "holds
   nothing" on its first pass, so a ledger still booking the earlier
   session's holdings finds the disagreement at the first statement that
   names a holding, and never if the cell does not fill again. The method has no `simulated` argument:
   a cell is paper by construction and the entry always says so. It refuses
   a venue the cell is not configured with, and a unit stated both as a
   holding and as cash. The node's pass calls it after the drop-copy
   reconciliation, from `SimulatedGateway::custody_holdings` — the venue's
   clearing ledger, never the cell's own record.
3. **The ledger compares the statement in the batch that applies it.**
   `LedgerStore::apply` reads the statement off the entry, and the chain
   checks it already runs prove every earlier entry of the session is
   applied, so the ledger side is complete without waiting; the comparison,
   its `custody_break` notes, the mark that makes a replay `Stale`, and the
   chain tail commit together or not at all. The answer is
   `Disposition::CustodyCompared { breaks }`. A break is counted by
   direction after the commit, so a counter never names a break the journal
   does not hold, and it never adjusts a balance.
4. **A statement not marked simulated parks the cell as `LiveFill`**, before
   the chain, and is counted with the live fills: ADR 0130 decision 6's
   fence, the same as for a fill and a settlement. A statement whose holding
   is not an exact decimal, or that names a unit twice, parks the cell as a
   `RefusedFill`: a venue account nobody can read is not an account that
   agrees.

## What changes and what does not

- New: `Decision::CustodyStatement`; `CustodyStatement::from_entry`;
  `Cell::record_custody_statement`; `SimulatedGateway::custody_holdings`;
  the pass's call; `Disposition::CustodyCompared` and the comparison inside
  `LedgerStore::apply`.
- Unchanged: ADR 0442's comparison and policy (`compare`, `halt_for`), the
  out-of-band `reconcile_custody`, the ledger's balances, and every halt
  path the cell already has.

## What it costs

- One more journal entry after every pass that moved the venue's holdings.
- The ledger side is read by scanning the cell's booked fill events at each
  statement — linear in the ledger's fill history. Bounded today by the
  paper ledger's size; an index per cell is the remedy when it is not.
- The ledger side is the cell's fills at every venue, and a statement is one
  venue's. The node holds one gateway, so the two coincide; a cell trading
  through two gateways would read as a break on each statement (ADR 0442
  names the same limit). The error is in the safe direction.
- Cash custody is still not compared (ADR 0442's named gap): the statement
  lists the clearing currency and each held instrument's listing currency
  as cash units, excluded on both sides.

## What would make this wrong

- A venue that books fills between the cell's confirmation of them and the
  statement — so that the clearing ledger runs ahead of the cell's journal
  within one pass — would read as a break. The simulated venue matches only
  inside calls the cell makes during its pass, so it cannot; a gateway that
  matched asynchronously would need the statement taken under the same
  confirmation the fills are.
- A live custodian (ADR 0108 accepted) — as ADR 0130 and 0442 already say.

## What remains (2026-11-13)

- The carrier from a `custody_break` to the centre's halt signer and
  `broadcast_halt` (ADR 0442's decision 4; R12-LEDGER-8139).
- Posting the statement's units to `custody:<venue>` as `CustodyMovement`
  operations (ADR 0130 decision 4's first half). Not done: the operations
  path (ADR 0123) books a book's deposit with a custodian, and on a paper
  platform the venue is both, so the posting would be a second record of the
  same units; it is left until a reader needs a custody balance.
- `CustodyWatch` (ADR 0242) fed from the journaled statements, so custody
  freshness is observed (R12-RES-7099).

## Evidence

`cargo test -p qip-ledgerd --test store` and
`cargo test -p qip-acceptance --test custody_reconciliation`, each new test
mutation-verified (named in the commit that landed them).
