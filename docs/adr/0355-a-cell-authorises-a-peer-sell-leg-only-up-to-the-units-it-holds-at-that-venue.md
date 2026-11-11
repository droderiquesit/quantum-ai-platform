# ADR 0355: A cell authorises a peer sell leg only up to the units it holds at that venue, and none when it holds none; buy legs stay bounded by the region's free capital

- **Status**: **Accepted 2026-11-11 under the owner's 2026-10-31 delegation**.
  Lane L5-EDGE writes it from lane L5b's block (decision 15) while L5b's rows
  are with it. The delegation's limits hold:
  - it sets no risk number and changes no limit, threshold or limit set;
  - it touches no paper-trading layer;
  - it adds no `Cell` constructor;
  - it only narrows what the cell hands Lane 1.
- **Date**: 2026-11-11
- **Extends**: ADR 0116 (the cell reaches the peer link only through
  `ReflexPort`, offering leg authorities it computed itself).
- **Rows**: R12-MESH-5052.

## Context

R12-MESH-5052 asks that a cross-region cycle trades against capital and
inventory positioned in advance.
`qip-edge-node`'s `edge_quotes::leg_authorities` sized both sides of every
book from one input, the share of the region's free capital. It never read
what the cell held.

A sell authority was therefore as large as the cash would buy at the bid.
The peer saga could then reserve and send a sell leg for units the cell did
not hold. That is a short nobody reasoned about, at a venue that would have
to lend it. The cell's own pass refuses such a sale under `GATE_BORROW`
when borrow is required, but the peer leg never passes through that gate
before it is authorised.

## Decision

1. **A sell authority is bounded by the units held at that venue.** Its
   `max_size` is the lesser of the capital bound and
   `Cell::position(venue, instrument)`.
2. **A cell holding none authorises no sell.** A bound that is not positive
   authorises nothing. The authority is omitted, not offered at zero.
3. **A buy authority is unchanged.** It stays bounded by the free share at the
   ask: buying needs cash, not inventory.

## What changes and what does not

- **Changes.** `leg_authorities` reads the cell's position for every sell.
  An unfunded-inventory cell offers its peers buys only.
- **Does not change.** The free-capital bound, the halt and the live-ceiling
  refusals, the touch, the spread and the authority's time to live, and
  everything the session does with an authority once offered.

## What it costs

A cell that holds nothing cannot be the selling side of a cross-region
cycle until it holds the units. That is the point: the inventory has to be
positioned first, which is what the requirement's "pre-positioned" names.
`RebalanceTask`, the saga's request to reposition inventory, is still only
counted. A consumer for it is separate work.

## What would make this wrong

If a venue ever lent units against the cell's account as a matter of course,
so that a peer sell were covered without a holding, this bound would refuse
cycles the venue would have filled. On the simulator no lending happens
outside the stated borrow, so the bound is the safe side.

## Evidence

- `backend/crates/apps/qip-edge-node/src/edge_quotes.rs::leg_authorities`.
- `backend/crates/apps/qip-edge-node/tests/decision_loop.rs::a_cell_authorises_a_sell_leg_only_up_to_the_units_it_holds_at_that_venue_and_none_when_it_holds_none`.
- `backend/crates/apps/qip-edge-node/tests/decision_loop.rs::a_running_funded_cell_authorises_legs_within_its_free_capital_and_a_halted_one_none`,
  which now asserts a buy alone from a cell holding nothing.
