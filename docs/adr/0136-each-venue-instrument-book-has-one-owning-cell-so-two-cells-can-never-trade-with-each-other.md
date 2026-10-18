# ADR 0136: Each venue-instrument book has one owning cell, so two cells can never trade with each other

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It adds a refusal to the cell and a
  field to the signed policy channel; it widens nothing and admits nothing.
- **Date**: 2026-10-18
- **Extends**: ADR 0008 (cells decide alone, on authority granted in
  advance), ADR 0088 (versioned slots with their own TTL), ADR 0128 (policy
  travels the spine).
- **Keeps**: ADR 0003, ADR 0116 (cross-region arbitrage over the peer link
  trades different venues and is not touched), the cell's internal crossing
  (`Cell::cross_internally`).

## Context

R12-EXEC-1067 asks that "any order pattern constituting wash activity is
refused before it leaves the platform", across cells and venues. Inside a
cell it is: the cell nets intents before they leave
(`Cell::cross_internally`, keyed by `crossing_key` on venue, object and
representation), so one cell never sends a buy and a sell that could meet.
Across cells nothing prevents it. Two cells configured with the same venue
can each rest an order on the same instrument's book on opposite sides, and
the venue would match the platform against itself.

The triage named two remedies: a centre-side pre-trade detector, which puts a
central hop on every order and is exactly what ADR 0008 refuses (a cell that
cannot reach the centre would stop trading); or sharding, so that the
pattern cannot be formed.

Locate the cell's half with
`grep -n 'fn cross_internally\|fn crossing_key' backend/crates/edge/qip-edge/src/cell.rs`.

## Decision

1. **Ownership is per venue-instrument book, not per instrument.** Two orders
   can match each other only on the same venue's book for the same
   instrument, so that pair is the unit. One cell owning BTC at venue A and
   another owning BTC at venue B can trade both books, and their buying on
   one and selling on the other is arbitrage, not wash; the record does not
   refuse it.
2. **The centre assigns owners and signs the assignment.** The policy payload
   gains an additive `books: Slot<BookOwnership>` naming, for this cell, the
   venue-instrument books it may place on. The centre never assigns one book
   to two cells in payloads valid at the same instant; on a handover it
   publishes the old owner's narrower set first and the new owner's wider set
   only after the old owner's payload is applied or has expired (the slot's
   TTL bounds the wait). A handover therefore has a gap, never an overlap.
3. **The cell refuses an order on a book it does not own,** at the `Cell::work`
   pass gate, before an order object exists, journaled and counted under a
   new gate constant. A cell whose `books` slot is unproduced or stale
   places on no book it does not already hold a position in, and may only
   reduce such positions: fail closed, as every stale slot narrows today.
4. **A cell's resting orders on a book it loses are withdrawn** on the payload
   that removes the book, by the existing withdrawal seam, so a handover never
   leaves two cells resting on one book.
5. **The centre checks its own assignment.** The kernel refuses to publish a
   set of payloads in which one book has two owners within overlapping
   validity windows, and journals the refusal; this is a check on the centre's
   own arithmetic, not a pre-trade check on any order.

## Consequences

- Cross-cell wash is impossible by construction rather than detected after the
  fact, and no order waits on the centre.
- Regions that today share a venue must split its books between their cells;
  a book is traded by one cell at a time.
- Code slices left: `BookOwnership` and the slot; the cell's refusal gate and
  the stale-slot reduce-only rule with tests that an unowned book is refused
  and a stale slot reduces only; the withdrawal on loss; the centre's overlap
  check with a test that a two-owner assignment is refused before publish
  (EXEC-1067). Each needs its mutation.

## What it costs

- Liquidity on a book is reachable by one cell only; a strategy that wanted
  two regions to work the same venue's book loses that, and the centre must
  route the opportunity to the owner.
- A handover gap means a book can be untraded for up to one slot TTL; a
  venue moving between cells on a regional outage is unavailable for that
  interval.
- One more signed slot, one more refusal gate, and one more label value on
  `qip_edge_refusals_total{gate}`; the gate is a `pub const`, so the label's
  bound holds.

## What would make this wrong

- If the platform ever traded through two accounts at one venue that the venue
  treats as different beneficial owners, matching between them might be
  permitted and ownership per book would refuse legitimate trades; this is a
  live-venue question and does not arise on the simulator.
- If wash activity were defined to include patterns across venues (for
  example a regulator treating linked cross-venue trades as wash), decision 1
  is too narrow, and ownership would need to move to the instrument.

## Rows

R12-EXEC-1067. It stays PARTIAL until its slices land with a mutated test.
