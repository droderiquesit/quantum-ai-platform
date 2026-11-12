# ADR 0407: A hedge plan reaches a cell as signed targets the centre derives pro rata from each cell's stated book, and only toward zero

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session cap-a-01. The delegation's limits hold:
  - it sets no figure: the quantity is the plan's, the deadline is the plan's
    own time to exit, and the share is proportional, so no cell order or
    weight is chosen by anyone;
  - it relaxes no limit, guard or refusal: every target only narrows, the
    cell keeps every refusal ADR 0126 gave it, and a cell may refuse the
    frame whole;
  - it touches no paper-trading layer, adds no dependency and no path to a
    venue: nothing here sizes a position or sends an order.
- **Amends**: nothing. It fills the producer ADR 0126 decision 4 left
  unbuilt.
- **Row**: R12-CAPITAL-10098.

## Context

ADR 0126 decision 4 sends a hedge plan to a cell as a signed narrowing: per
instrument and venue, a target net between what the cell holds and zero,
with a deadline. The frame, its signature, the cell's acceptance rule and its
refusal gate were built and tested; nothing at the centre made one, and
`PolicyCourier::send_hedge` had no caller. The reason was a design question,
not a missing part: `HedgePlan` states trades (buy or sell this quantity of
this instrument at this venue), a narrowing states a cell's target net, and
nothing said how the first becomes the second across several cells.

## Decision

1. **The plan's trades are netted per venue and instrument**, buys positive
   and sells negative, so each cell receives at most one target per
   instrument (`HedgeNarrowing::check` refuses two).
2. **A net sell reduces the cells long that instrument at that venue; a net
   buy, the cells short it.** A cell flat or on the trade's side is not
   touched. Binding it would ask it to open or widen a position, which a
   narrowing cannot express (ADR 0126, "What would make this wrong": that
   needs a new envelope).
3. **The quantity is shared pro rata by the size of each holding.** A cell
   holding `h` of a total `H` the cells hold against the trade keeps
   `h × (H − q) / H`, and zero once `q ≥ H`. The excess beyond `H`, and every
   trade nobody holds the opposite side of, is not sent and is named in a
   line. Proportional sharing was chosen because every other split needs an
   order or weight across cells that nobody gave.
4. **The holdings are each cell's stated book (ADR 0208), read only where it
   agrees with the centre's settled book for that instrument.** The stated
   book is the only per-venue record the centre has, and the cell re-checks
   every target against its own confirmed position anyway. The settled book
   stands (ADR 0208): an instrument on which the two disagree gets no target
   at that cell, and a cell with no stated book is not narrowed.
5. **A target is stated only at a venue one of the cell's live envelopes, as
   the centre issued them, grants**, because the cell refuses a whole frame
   naming an ungranted venue and one stray target would cost it every other.
6. **The narrowing carries the plan's instant as its issue instant and the
   plan's time to exit as its deadline.** A cell keeps the newest by issue
   instant and refuses one at or behind it, so a plan re-derived against a
   book its own targets already moved is refused rather than compounded.
   Nothing is derived at or past the deadline.
7. **The plane signs with its envelope issuer** (`EnvelopeIssuer::sign_hedge`),
   because a cell verifies a narrowing under its `envelope` key and a second
   signer of that class at the centre would be a second capital key.
8. **The platform holds the Hedge Brain's latest plan** (replaced on every
   run, `None` when it planned nothing) and `Platform::issue_hedge_narrowings`
   derives from it. The API's `pending_policy` calls it with the configured
   cells, and `MeshBackbone::dispatch_policy` sends each narrowing through
   `PolicyCourier::send_hedge`, before and independent of the policy signer,
   **once per cell per plan id and issue instant**: one already delivered is
   not resent, one not delivered is retried at the next dispatch.

## Consequences

- `PolicyCourier::send_hedge` has a production caller in `qip-api`, reached
  from the cycle route, and a cell that hears the centre receives each plan's
  targets once, signed under its envelope key.
- The cell remains the authority on what it does (ADR 0008): it may refuse a
  frame, and a cell that never hears one is exactly as bounded as its
  envelope leaves it.

## What it costs

- The derivation is not journaled as its own record. Its inputs are: the
  plan (`HedgePlanned`) and the cell reports the stated books came from.
  Replaying it needs both and the rule above.
- A cell whose position moved past its target between its statement and the
  frame's arrival refuses the whole frame, losing its other targets for that
  plan. That costs trades, not capital.
- A plan the brain stops making is not withdrawn at the cells: the last
  narrowing binds until its deadline. It only narrows, so this too costs
  trades.

## What would make this wrong

- A hedge that must add exposure in a cell. That is a new envelope, as
  ADR 0126 says, and not a widening of this rule.
- A desk that wants hedges concentrated in particular cells (by liquidity,
  cost or region). That is a weight somebody has to give, and an ADR that
  states where it comes from.
