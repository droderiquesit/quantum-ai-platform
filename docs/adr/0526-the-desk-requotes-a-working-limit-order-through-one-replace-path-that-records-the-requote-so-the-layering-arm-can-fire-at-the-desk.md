# ADR 0526: The desk requotes a working limit order through one replace path that records the withdrawal, the requote judged against the touch the caller saw, and the replacement as a fresh submission, so the layering arm can fire at the desk

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**
  (`docs/blueprint/owner-decisions-2026-10-31.md`: a session decides the
  in-repository design question it meets, by ADR), session t5-02 (wave580).
  It sets no figure: the layering arm's bound is ADR 0285's, which is the
  cancel-before-fill gate's own 60 s, four, three quarters. It adds no
  dependency and touches no paper-trading layer.
- **Date**: 2026-11-13
- **Rows**: R12-EXEC-1068.
- **Amends**: ADR 0285 decision 5, by completing it. That clause said the
  desk had no replace path and that one added later must call
  `ConductWatch::requoted`; this record adds the path and its call. ADR 0285
  is otherwise unchanged and its file is not edited.

## Context

ADR 0285 refuses requote layering beside the cancel-before-fill gate, at the
cell and at the desk. The cell half fires: `Cell::record_replacement` records
each unfilled requote on `ConductWatch::requoted`, judged against the cell's
own book. The desk half could not fire. `OrderManager` recorded the desk's
fills (`ConductWatch::filled`) but had no way to reprice a working order, so
no desk requote was ever recorded and the arm's requote count at the desk was
always zero. An arm that cannot fire reads as protection and is not; the
owner's ruling ("refused at the cell and at the desk") stayed open on that
clause.

## Decision

1. **One path.** `OrderManager::replace(order_id, replacement, touch, ...)`
   is the desk's only way to reprice a working order. It is the withdrawal of
   the original and the submission of the replacement, recorded as both.
2. **What is recorded, in order.**
   - The withdrawal goes through the same private `withdraw` that
     `OrderManager::cancel` uses, as the desk's own choice, so it counts as
     an unfilled withdrawal when nothing of the order traded (R12-GOV-6126,
     unchanged).
   - When nothing of the original traded, the requote is recorded on
     `ConductWatch::requoted`. It is away from the touch only when it moved
     the limit further behind `touch`: a buy lower than before and below the
     best bid, a sale higher than before and above the best ask. That is
     `Cell::requote_moved_away`'s rule, restated for the desk's `Side`.
   - The replacement goes through `OrderManager::submit` whole. It meets the
     kill switch, autonomy, the conduct gate (which now holds this requote),
     feasibility and pre-trade risk like any new order, and its acceptance
     records a placement there.
3. **The touch is the caller's.** The order manager holds no book. The
   caller passes the best price on the side the order rests at, as its book
   shows it. `None` is a desk that cannot see the touch, and its requote is
   not away, which is ADR 0285 decision 4's rule for a cell holding no book.
4. **Not a requote, refused first.** A replacement for another instrument or
   side, or a pair where either order is not a limit order, is refused with
   `Error::invalid` before anything is withdrawn. The original keeps resting.
5. **A refused replacement leaves nothing resting.** Once the original is
   withdrawn, a gate that refuses the replacement leaves the desk with
   neither order. That is the restrictive outcome, and it is the one the gate
   chose. Checking the gates before withdrawing would need a second copy of
   `submit`'s gate order, and two copies drift.

## What changes and what does not

- **Changes.** One public method on `OrderManager` and one private helper,
  `requote_moved_away`. A comment in `submit`'s fill path that said the desk
  has no replace path now names this one.
- **Does not change.** `ConductPolicy` and its values; the cancel-before-fill
  arithmetic; the layering arm's arithmetic; `submit`'s gate order; any limit
  or limit set. Nothing in the kernel calls `replace` today, because the
  desk's cycle never reprices a working order. The arm is now live on the
  only path a desk requote can take.

## What it costs

- A desk that requotes away from the touch four times in a minute while its
  other side fills is refused the fourth replacement and any further order on
  that side for up to a minute. Its original is already withdrawn, so it
  rests nothing on that side until the window clears. That cost is
  deliberate: it is the pattern's shape.
- A caller that passes no touch gets no layering protection from its
  requotes. The cancel-before-fill gate still counts each one.

## What would make this wrong

- If a second desk path repriced a working order without going through
  `replace` (an in-place limit edit, a broker-side amend), decision 1 would
  be a silent gap again. Any such path must call `ConductWatch::requoted`.
- If a desk caller passed a touch taken from a different book (another
  venue's, or a stale one), the away judgement would read the wrong market.
  The caller must pass the touch at the venue the order rests at.
- If a venue's quoting obligations required requoting away from the touch,
  ADR 0285's "What would make this wrong" applies here too. It does not
  arise on the simulator.

## Evidence

- `backend/crates/services/qip-execution-engine/src/oms.rs`
  (`OrderManager::replace`, `requote_moved_away`).
- `backend/crates/services/qip-execution-engine/tests/execution.rs`:
  - `a_desk_that_requotes_its_bids_away_from_the_touch_while_its_offer_fills_is_refused_the_layering_bid`
  - `the_same_desk_requotes_with_no_opposite_fill_are_admitted_because_the_cancel_ratio_alone_does_not_see_them`
  - `a_desk_requote_judged_against_no_touch_is_not_counted_as_away`
  - `a_desk_requote_for_another_side_is_refused_before_the_original_is_withdrawn`
- Mutations, each restored byte-for-byte (`cmp`): `replace` recording no
  requote fails the first test; `requote_moved_away` counting a missing
  touch as away fails the third; the side check removed fails the fourth.
