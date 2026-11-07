# ADR 0209: A pass that fails or halts after sending still ships the orders it sent, marked with its gate and pass number, exactly once

- **Status**: **Accepted 2026-11-07 under the owner's 2026-10-31 delegation**
  (decision 8 of `docs/blueprint/owner-decisions-2026-10-31.md`). The number
  was claimed on 2026-10-31 for lane L5b. The body is written by lane L5-EDGE,
  which took L5b's rows when L5b stopped on its weekly limit. The delegation's
  limits hold: this sets no risk number, changes no limit, threshold or limit
  set, touches no paper-trading layer, and adds no `Cell` constructor.
- **Date**: 2026-11-07
- **Keeps**: "Bill what ran, not what was planned" (principle 6); the mesh
  delta as the one route a cell's sent orders reach the centre; the halt that
  follows a broken cycle.
- **Rows**: R12-MESH-7143. It unblocks the broken-cycle half of R12-MESH-5068.

## Context

`Cell::work` builds its `WorkReport` as the pass runs. Every order it sends
is pushed onto `report.orders`. When a pass fails, the `Err` carries no
report, so the orders it had already sent were discarded with it. A cycle
that breaks between legs is the common case: `break_cycle` refuses under
`arbitrage_cycle_broken`, halts the cell and returns the error.

`qip-edge-node`'s scheduler held only the last successful report.
- `Err(_) => {}` kept nothing for a failed pass, so the delta offered that
  turn carried the previous pass's report again, or the default one.
- A halted pass reset the report to the default.

Either way, an order that reached a venue before the break never rode the
mesh delta. The centre never charged it into the aggregate the Hedge Brain
plans against. When the venue later reported its fill, the centre met a fill
on an order it had never seen sent, and filed it as `unsent_fill`.

## Decision

1. **The cell keeps what a failed pass sent.** `Cell::work` runs the pass
   into a report it owns. If the pass fails after sending at least one
   order, it keeps that report for its caller to take with
   `Cell::take_failed_report`. A failed pass that sent nothing keeps
   nothing, because there is nothing to bill.
2. **The cell marks it; the scheduler ships it once.**
   - The cell marks the kept report with the gate of the refusal the pass
     recorded last and with its pass number (`WorkReport::failed_pass`).
     The cell is the one that knows both, and the one that can journal a
     refusal (decision 4).
   - On a failed pass, the scheduler takes the report and offers it on
     this turn's delta.
   - It then clears it, so the next turn's delta cannot carry it again.
   - A failed pass that left nothing to take sets the scheduler's report to
     the default. Before this, it left the previous pass's report in place,
     which re-offered orders that had already been shipped.
3. **Nothing new after the failure.** The failed pass's orders are shipped
   as they were sent; nothing is added. The cell is halted by the break, so
   the next pass sends nothing until an operator releases it.
4. **Fail closed.** A failed pass's report that cannot be marked is refused
   whole and journaled under `failed_pass_unmarked`: one that recorded no
   refusal names no gate. It is never kept and never shipped unmarked.
5. **The centre settles those orders as it settles any other.** They arrive
   in `orders`, and the same delta carries the break's refusal. No new centre
   path is built: what was missing was the delivery, not the settlement.

## What changes and what does not

- **Changes.**
  - `Cell::work` delegates to `Cell::work_into`.
  - A failed pass that sent orders leaves its report for
    `Cell::take_failed_report`.
  - `WorkReport` gains `failed_pass`, and the cell gains the
    `failed_pass_unmarked` refusal gate, a `pub const`.
  - The scheduler ships the marked report once.
- **Does not change.**
  - The halt on a broken cycle.
  - The journal, which already held the refusal.
  - Every successful pass's report.
  - The delta's shape on the wire.

## What it costs

The failed pass's report is held in the cell between the pass and the
scheduler's take. That is one report at a time, replaced by the next failure
and cleared when taken.

## What would make this wrong

- If a failure could leave a report whose orders were not in fact sent, the
  centre would charge orders no venue holds. `report.orders` is pushed only
  after the placer accepted the order, which is what makes this record
  "sent".
- If a second failure arrived before the scheduler took the first, the first
  would be replaced. The scheduler takes the report on the same turn the pass
  failed, so that window is closed in the one caller this tree has.

## Evidence

- `backend/crates/edge/qip-edge/src/cell.rs` (`Cell::work`,
  `Cell::work_into`, `Cell::take_failed_report`).
- `backend/crates/apps/qip-edge-node/src/scheduler.rs` (`DecisionLoop::turn`'s
  pass arms).
- `backend/crates/edge/qip-edge/tests/arbitrage.rs::a_pass_that_breaks_after_sending_a_leg_keeps_its_report_marked_and_hands_it_over_once`.
  It holds decisions 1, 2's marking and the hand-over once.
- Not yet held by a test: the scheduler's ship-once and clear, and the
  centre-side settlement and hedge planning that the closure names for
  `qip-acceptance tests/cycle_unwind_hedge.rs`. R12-MESH-7143 says so.
