# ADR 0285: Requote layering is refused beside the cancel-before-fill gate, at the cell and the desk, under that gate's own 60-second, four-withdrawal, three-quarters bound, with an opposite-side fill at any venue of the instrument counted

- **Status**: **Accepted 2026-10-31 under the owner's 2026-10-31 delegation**,
  relayed by the wave parent to lane L5-EDGE (decision 14 of the 2026-10-31
  rulings, `docs/blueprint/lanes/wave1031-owner-rulings-2-2026-10-31.json`;
  the number is the first of lane L5's block under decision 15). The limits
  of that delegation hold. This record sets no new number: the bound is
  `ConductPolicy::default()`'s, the one the cancel-before-fill gate already
  enforces. It touches no paper-trading layer and adds no constructor to
  `Cell`.
- **Date**: 2026-10-31
- **Amends, by a new record**: ADR 0155 section 13, whose open values
  ("Not decided here": the requote-layering window, minimum and ratio) this
  record fixes. ADR 0155 is not edited.
- **Keeps**: R12-GOV-6126's count of an unfilled requote as a withdrawal and
  a placement, unchanged; the spoofing gate and its refusal, unchanged.
- **Rows**: R12-EXEC-1068. Momentum ignition is manipulation, not spoofing,
  and is graded under R12-RISK-4103's market-conduct domain, not here.

## Context

R12-EXEC-1068 asks that requote layering is refused. ADR 0155 section 13
said why counting requotes cannot be the measure: an honest market maker
requotes all day, and a count convicts it. Section 13 defined the measure on
different facts and then left its values open, so nothing was built.

The facts it named are these. The cell requotes an order **away from the
touch**, the order ends **withdrawn unfilled**, and meanwhile the same
instrument **fills on the opposite side**. That is the layering shape. Orders
rest on one side to move the price, and they are pulled before anything
trades against them while the other side takes the benefit. A requote at or
inside the touch is trying to trade. A requote that a venue or a halt forced
was not the cell's choice.

The cancel-before-fill gate (`qip_execution_engine::conduct`, at the cell
since R12-GOV-6126 and at the desk's order manager) cannot see this shape. A
layering cell's cancel ratio can sit under that gate's bound. Every
replacement is also a placement, so a requote adds one to each side of the
ratio, and the ratio approaches one without ever exceeding three in four.

## Decision

1. **The measure is section 13's, on `ConductWatch`.**
   - `ConductWatch::requoted(venue, object, side, away_from_touch, at)`
     records a requote whose replaced order ended withdrawn with nothing
     filled. The record is keyed by venue, instrument and side, as every
     other conduct event is.
   - `ConductWatch::filled(object, side, at)` records a fill keyed by
     instrument and side only. A fill on the opposite side **at any venue**
     of the instrument counts, because the benefit of layering one venue's
     book can be taken at another.
2. **The bound is the cancel-before-fill gate's own.** The same
   `ConductPolicy` values apply, so no new number is chosen. Within the
   window (60 s by default), the layering arm refuses a further order on one
   side of one book when all of these hold:
   - at least `min_cancellations` (four) of that side's requotes moved the
     order away from the touch;
   - those requotes are more than `max_cancel_ratio` (three quarters) of the
     side's requotes;
   - the instrument filled on the opposite side.
   Without the opposite-side fill, nothing is refused.
3. **Beside the cancel-before-fill gate, never in its place.**
   - `ConductWatch::refusal` checks the cancel ratio first and the layering
     arm second.
   - Both refuse under the same gate, `conduct_spoofing`, so the refusal
     label keeps its bound.
   - The existing count of an unfilled requote as a withdrawal and a
     placement is unchanged.
4. **The cell judges away-from-touch against its own book.**
   - `Cell::record_replacement` compares the replaced price and the new
     price with the touch on the order's resting side. A buy is compared
     with the best bid and a sell with the best ask.
   - A replacement that moved the order further from the touch is away.
     One that stayed or moved closer is not.
   - A cell that holds no book for the instrument cannot judge the requote
     and does not count it as away.
   - The cell's fill confirmation records every fill.
5. **The desk** records its fills on the same watch through
   `OrderManager::submit`'s fill path. The desk's order manager has no
   replace path today: a desk order is cancelled or left to rest, and never
   repriced in place. So the desk's layering arm has no requote to read
   until a replace path exists. That replace path must call `requoted`.
   This is stated so the arm is not read as protection the desk already has.

## What changes and what does not

- **Changes.** One more refusal at the cell's pass, at `admit_replacement`
  and at the desk's submit gate, on the arithmetic the cancel-before-fill
  gate already uses. Two recording calls: a requote in
  `record_replacement`, and a fill in the cell's and the desk's fill paths.
- **Does not change.** The cancel-before-fill bound and its values; the
  R12-GOV-6126 count; any limit, threshold or limit set; the label set of
  `qip_edge_refusals_total{gate}`.

## What it costs

- A market maker that requotes away from the touch four times in a minute
  while its other side trades is refused a further order on the requoted
  side for up to a minute. That is the pattern's shape, and the cost is
  deliberate.
- Fills are keyed by instrument across venues, so the working set grows by
  one bounded queue per instrument and side. It is pruned to the window and
  capped at `MAX_EVENTS_PER_KEY`, like every other conduct queue.

## What would make this wrong

- If a venue's own quoting obligations (a designated market maker's spread
  and size duties) required requoting away from the touch while the other
  side fills, this arm would refuse an obligation. That is a live-venue
  question and does not arise on the simulator.
- If the desk gained a replace path that did not call `requoted`, decision
  5's limit would become a silent gap. Any such path must be added with its
  call.

## Evidence

- `backend/crates/services/qip-execution-engine/src/conduct.rs`
  (`ConductWatch::requoted`, `ConductWatch::filled`, the layering arm in
  `ConductWatch::refusal`).
- `backend/crates/edge/qip-edge/src/cell.rs` (`Cell::record_replacement`, the
  fill confirmation).
- `backend/crates/edge/qip-edge/src/cell.rs` (`crossing_tests::a_cell_that_layers_its_bids_by_requoting_them_away_from_the_touch_while_its_asks_fill_is_refused_a_further_bid`).
  The test lives beside the cell's net and fill helpers, not in `tests/pricing.rs`: it drives a sell through `place_net` and the venue's fill through `confirm_execution_reports`, which the integration helpers cannot reach. Its twin with no opposite fill is admitted, which proves the cancel-before-fill gate does not refuse the pattern. Three mutations each fail it: the arm ignoring the opposite-side fill, no requote counted as away, and the cell's fill path recording no fill.
- There is no desk twin. The desk has no replace path, so nothing at the desk can requote (decision 5).
