# A venue was withdrawn, and putting it back

The platform withdrew a venue from its own evidence. Two people sign, and it
comes back. Nobody edits a file and nobody restarts a process (ADR 0062).

## What you are looking at

The LEARN stage keeps one window of the last 256 feasibility refusals from
both planes — the desk's own order manager and the cells' reports — each
naming the venue it was bound for and the gate that refused it. When that
window holds at least ten refusals and one venue accounts for at least three
in four of them, the platform journals a **withdrawal** under `venue.withdrawn`
and stops using that venue: `OrderManager::submit` refuses every order bound
for it under `venue-availability`, and the centre omits its conversions from
the whitelist it issues to the cells.

That is fail-closed by choice, and it is worth being clear about what it means
on a desk with one broker: **ten off-grid orders in a row stop the desk
entirely**, loudly, until two people look at the grid. The alternative — never
withdraw a desk's only venue — was considered and rejected in ADR 0062 as an
exception to fail-closed dressed as prudence.

So before you sign anything, the question is not "how do I turn trading back
on" but "why did ten orders in a row fail to fit the venue's grid". A lot
size, a tick, or a minimum has usually changed at the venue, or a strategy is
sizing in units the venue does not quote.

## Do this

1. Read what was withdrawn, and on what evidence:
   ```sh
   curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/venues/withdrawals
   ```
   Each row names the venue, whether a first signature is already standing,
   and the cluster the finding was made on: `constraint` is the gate that
   dominated (`feasibility_lot`, `feasibility_tick`,
   `feasibility_minimum_quantity`, `feasibility_minimum_notional`), `count`
   and `sample` are this venue's refusals and the whole window, and `seams`
   says whether the desk, the cells, or both saw it. `withdrawals_recorded`
   counts every withdrawal the log holds, including venues since put back —
   so a zero there means this platform has never withdrawn anything.

2. Fix the cause. A withdrawal is a finding about the venue's grid or about
   the desk's sizing against it, and reinstating without changing either
   buys ten more refusals and the same withdrawal on the next review. The
   gate the cluster names says which figure to check:

   | `constraint` | What to re-read |
   |---|---|
   | `feasibility_lot` | the instrument's board lot, and the sizes the strategy is producing |
   | `feasibility_tick` | the venue's tick, and the prices limit orders are stating |
   | `feasibility_minimum_quantity` | the venue's minimum order size |
   | `feasibility_minimum_notional` | the venue's minimum order value |

   The reference catalogue is where the platform reads a lot and a tick from,
   so a grid that has moved is corrected there and reaches the gate on the
   next assembly.

3. Sign, twice, as two different people, each within fifteen minutes of
   authenticating and the second within a day of the first:
   ```sh
   curl -X POST -H "Authorization: Bearer $QIP_TOKEN_OPERATOR" \
        -H "Content-Type: application/json" \
        -d '{"rationale": "<what was wrong and what was corrected>"}' \
        .../api/v1/venues/:venue/reinstatements
   ```
   Replace `:venue` with the venue's name as `GET /venues/withdrawals` lists
   it — `simulated-venue` for the desk's own broker. Pasted literally,
   `:venue` is not a venue this platform has ever withdrawn and the route
   answers 404, which is the same answer any venue the platform did not
   withdraw gets: a reinstatement signs a withdrawal the platform made, and
   there is no way to sign one into existence.

   The body takes `rationale` and nothing else. It cannot name the approver
   (that is your session) and it cannot name the venue (that is the path).
   The first answer is `awaiting_countersignature`; the second, **from a
   different subject**, is `reinstated`. A second session on the same
   credential is refused: it is one person, and the whole point of the
   control is that it is two.

4. Confirm. `GET /venues/withdrawals` no longer lists the venue, and
   `withdrawals_recorded` still counts the withdrawal — a venue put back is
   not a venue that was never withdrawn, and the log keeps both facts.

## What a signature can and cannot do

It removes a name from a **subtractive** set. The withdrawn set is read in
exactly two places and both of them read it to refuse: the order manager's
withdrawal step, and the centre's `retain` over the conversions it whitelists
for the cells. There is no path on which it admits anything. So the most a
reinstatement can restore is what `QIP_VENUES`, the arbitrage policy's venue
map and the capital grant's terms already permitted — a venue none of those
name is not made reachable by any number of signatures.

## The edge limit

Omission from the whitelist is necessary and not sufficient for a cell whose
arbitrage desk is already installed. `Cell::install_arbitrage` refuses a
second desk, so a withdrawal reaches a desk installed *after* it; a desk
installed before keeps its graph, including the withdrawn venue's
conversions, until the node restarts. ADR 0062 names the closure — policy
slot 11 carrying a withdrawn set, refused at the cell's own feasibility gate
— and it is not built. If the venue matters at the edge, restart the node.

## Why there is no route that withdraws a venue

Because a withdrawal is the platform's finding about its own evidence, and a
caller who could ask for one could deny the desk its venue with a single
request. The evidence path can only ever subtract; the signature path can
only ever restore what configuration already allowed. Neither direction is a
way to widen what this platform may trade.
