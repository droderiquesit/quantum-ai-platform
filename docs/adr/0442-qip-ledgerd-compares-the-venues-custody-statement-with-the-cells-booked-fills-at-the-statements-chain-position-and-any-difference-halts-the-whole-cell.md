# ADR 0442: qip-ledgerd compares the venue's custody statement with the cell's booked fills at the statement's own chain position, and any difference halts the whole cell

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). Session t50-08. The delegation's limits hold:
  - it sets no figure: there is no tolerance to choose, because the
    comparison is exact;
  - it relaxes no limit, guard or refusal; its one behaviour change stops a
    cell that would otherwise have kept trading;
  - it touches no paper-trading layer — it adds a fifth refusal of a record
    not marked simulated — and adds no dependency or crate edge.
- **Date**: 2026-11-12
- **Extends**: ADR 0130 decision 4 (custody truth is the simulated venue's
  position statement; a disagreement is a break, journaled and counted,
  never an adjustment). It decides what that record left open: what the
  break does.

## Context

R12-LEDGER-8139: "Venue/custody truth must be continuously compared with the
ledger; discrepancies must raise a P0/P1 Fabric event and can narrow or stop
local execution under deterministic policy." The cell halts on its own
drop-copy break, and ADR 0130 decision 4 had qip-ledgerd compare the venue's
statement with its trading balances. Nothing implemented the comparison, and
no record said whether a ledger break narrows, halts or only counts. Three
questions needed answers before any code: which ledger figure, at which
instant, and what policy.

## Decision

1. **The ledger side is the cell's booked fills at or before the statement's
   chain position**, summed per unit over every `trading:<cell>/<strategy>`
   leg. A statement carries the `(session, sequence)` of the last journal
   entry the cell had shipped when the venue took it. A fill after that
   position is not in the venue's figure and is not counted on the ledger
   side either. Operations (ADR 0123) are not counted: they move units
   between the platform's own accounts and not to or from the venue.
2. **The comparison waits for the ledger, and never reads lag as a break.**
   While the ledger's tail for the cell is behind the statement's position,
   the answer is `Pending` and nothing is journaled. A statement at or behind
   the last one compared for that cell and venue is `Stale` and journals
   nothing twice.
3. **Any difference halts the whole cell.** The comparison is exact per
   unit; cash units the statement names are left out on both sides. One
   break or more yields one unsigned `HaltCommand` for the cell, issued at
   the statement's `as_of`. Whole-cell, not narrowed: every gate that sizes
   against a position is sizing against a wrong number, and narrowing would
   keep trading on it. Issued at `as_of`, so the same statement always yields
   the same command, and a policy payload issued before the statement cannot
   release it (the halt's release barrier).
4. **The halt travels the existing P0 path.** The centre signs it under its
   halt key and publishes it on the halt topic, where the cell's
   `PolicyDownlink` verifies it and `Cell::apply_halt` engages it. Release is
   what it already is: a newer signed policy payload, which the centre
   issues once a later statement agrees. qip-ledgerd holds no signing key.
5. **A statement not marked simulated is refused** and compares nothing
   (ADR 0130 decision 6).

## What changes and what does not

- New: `qip_contracts::ledger::custody` (statement, `compare`, `halt_for`),
  `LedgerStore::reconcile_custody`, the counter
  `qip_ledger_custody_breaks_total{direction}`, and the `custody_break`
  journal note.
- Unchanged: the ledger's balances (a break never adjusts them), the cell's
  own drop-copy reconciliation, and every halt path the cell already has.

## What it costs

- A false break halts a cell. The chain-position rule removes the lag case;
  a venue with more than one gateway per cell would read as a break until
  the node sums its gateways into one statement. That error is in the safe
  direction.
- Cash custody is not compared. The venue's cash balance is a separate
  statement, and comparing it is a named gap, not a zero.

## What would make this wrong

- A live custodian (ADR 0108 accepted) makes the venue and the custodian two
  parties, and decision 1 would need the custodian's statement instead.
- If a real break rate made whole-cell halts unworkable, a narrowing policy
  would need its own record and its own figure, which this one does not set.

## What remains of R12-LEDGER-8139 (2026-11-12)

- A producer: the node publishing `CustodyStatement` from
  `SimulatedGateway::positions` on the P1 stream, stamped with the chain
  position it last shipped.
- A caller: qip-ledgerd's consumer calling `reconcile_custody` on each
  statement, and a carrier from its `Broken` answer to `qip-api`'s halt
  signer and `broadcast_halt`.

## Evidence

`cargo test -p qip-acceptance --test custody_reconciliation` and
`cargo test -p qip-contracts --lib custody`, each mutation-verified (see the
commit that landed them).
