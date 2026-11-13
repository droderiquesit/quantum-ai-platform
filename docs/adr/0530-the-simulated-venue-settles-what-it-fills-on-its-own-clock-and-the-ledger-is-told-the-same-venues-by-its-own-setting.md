# ADR 0530: The simulated venue settles what it fills on its own clock, and the ledger is told the same venues by its own setting

- **Status**: **Accepted**, 2026-11-13, session t5-03 (wave580), under the
  owner's 2026-10-31 delegation of in-repository design decisions
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Paper only: the producer
  is the in-process simulated venue, the record it produces carries the same
  `simulated` fence a fill does, and nothing here reaches a live venue.
- **Date**: 2026-11-13
- **Implements**: ADR 0130 decision 2 (the producer), ADR 0440 decision 7
  (the producer and the setting land together).
- **Keeps**: ADR 0003, ADR 0440 decisions 1-6, owner decision 26 (no
  `Filled` shape change at fabric v1).

## Context

ADR 0440 built the ledger half of ADR 0130: a fill at a venue the ledger's
composition root names books its cash to `in_settlement:`, and only that
venue's `Decision::Settled` moves it to `trading:`. It left the producer
open, and kept the setting unwired on purpose (decision 7), because a venue
named as settling on a cycle with no `Settled` producer would hold its cash
in settlement for ever. R12-LEDGER-1197's bar is the end-to-end case: a T+2
simulated fill reaches the book's cash on the second settlement day.

Three things had to be decided:

1. Who dates the settlement — the venue, the cell or the ledger.
2. How a settlement reaches the journal, given that the cell drains a
   venue's reports through `Placer` and the node never hands the cell the
   gateway itself (it hands a `RequotingPlacer` over it).
3. How the node and the ledger are told a venue settles on a cycle.

## Decision

1. **The simulated venue dates settlement, from its own terms and its own
   pass clock.** `SimulatedGateway` holds a `SettlementClock`: the venue's
   `SettlementTerms` (instant unless stated) and, on a cycle, each order
   whose fills it has handed to the cell, keyed by order and dated from the
   order's latest fill. When `advance_to(now)` reaches
   `SettlementTerms::available_at(latest fill)` the venue reports the order
   settled, once, with that instant as `settlement_date`. A date the terms
   cannot give stops the pass rather than leaving the order unsettled in
   silence. Neither the cell nor the ledger computes a settlement date;
   ADR 0130's "the venue observes settlement" holds.
2. **One settlement per order, dated by its latest fill.** ADR 0440 decision
   5 books one `Settled` for every held fill of an order. An order that
   filled across days therefore settles its earlier fills with its last
   one: cash late, never early. A fill on the order after its settlement
   starts a new wait and a second `Settled`.
3. **The report reaches the journal through `Placer::settlements`**, a new
   trait method defaulting to nothing, which `Cell::confirm_execution_reports`
   drains after the venue's fills and journals as `Decision::Settled`, with
   `simulated` read from the gateway exactly as a fill's is. A fill is
   recorded on the clock only when it is handed to the cell, so a
   `Settled` always follows the `Filled` it settles in the chain. Every
   wrapper delegates: `NodeGateway`, and `RequotingPlacer`, which also
   re-keys the clock from a requoted child's id to the parent's id the cell
   and the ledger hold (`SimulatedGateway::relabel_unsettled`), because a
   settlement naming the child would settle nothing in the ledger and park
   the cell.
4. **The node states the venue's terms in `QIP_VENUE_SETTLEMENT`**
   (`instant`, `T+0`, `T+1`, `T+2` on the weekday calendar; unset is
   instant). The composition root installs them on the gateway before any
   order and states the same value to the cell's settlement gate through
   `SimulatedFeed::attach_settling`, so the cell's funding projection and the
   venue's clock read one value. A cycle with no feed to run passes is
   refused at start. Terms cannot change once the venue has accepted an
   order.
5. **The ledger is told the same venues in
   `QIP_LEDGER_SETTLEMENT_CYCLE_VENUES`** (comma-separated; a blank entry is
   refused), passed to `LedgerStore::with_settlement_cycle_venues` before the
   first delivery is booked. This is ADR 0440 decision 3's second statement
   of the fact, now reachable from the binary.
6. **Neither variable is set in any committed environment.** Every deployed
   node's venue is instant, so the default on both sides — instant, and no
   cycle venue — is the true one, and both are argued unset in
   `manifest_wiring.rs`. Setting one without the other is the drift ADR 0440
   "What it costs" names: cash held for ever (ledger names a venue its node
   settles instantly) or a cell parked on the first `Settled` (node settles on
   a cycle the ledger does not know).

## Consequences

- R12-LEDGER-1197's bar is met in process:
  `qip-acceptance/tests/vertical_slice_cell_ledger.rs::a_t_plus_two_simulated_fill_reaches_the_books_cash_on_the_second_settlement_day_and_not_before`
  drives the production cell, simulated gateway, spool writer and
  `LedgerStore` from a Thursday fill to the Monday 09:00 settlement.
- R12-LEDGER-5011's producer and setting exist; its remaining item is a
  deployment choosing a cycle venue, which is an environment change.

## What it costs

- Two settings that must agree, on two binaries, until the fabric versioning
  ADR lets `Filled` carry its venue's terms. A drift is visible (a non-zero
  `in_settlement:` balance that never clears, or a parked cell) and never
  silent, but it is not refused at start, because neither process can see the
  other's configuration.
- An order filled across settlement days settles all of its fills at the
  latest one's date. Conservative, and wrong by up to the order's fill span
  for the earlier fills' cash.
- The clock's working set is the orders filled inside one settlement cycle;
  it is bounded by fill rate times the cycle, not by a stated constant.
- The `RequotingPlacer` re-keying is proven at the gateway seam
  (`settlement_clock.rs::a_requoted_childs_fill_settles_under_the_order_id_the_cell_and_the_ledger_hold`);
  no test drives a requoted child to settlement through the requoter itself.

## What would make this wrong

- If the fabric versioning ADR lets `Filled` carry terms, decision 5's
  ledger setting should go and the ledger should read the fill's terms.
- If a simulated venue must model a failed or partial settlement, decision 2
  cannot express it, for the reason ADR 0440 already gives.
- If a real venue adapter ever reports settlement, it reports it on its own
  channel and this clock must not be used for it: a simulated calendar dating
  a real venue's settlement would be the ledger booking a fact nobody
  observed.

## Rows

R12-LEDGER-1197; R12-LEDGER-5011 (producer and setting).
