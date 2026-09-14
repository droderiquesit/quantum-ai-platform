# A venue was withdrawn, and putting it back

The platform withdrew a venue from its own evidence. Two people sign, and it
comes back. Nobody edits a file and nobody restarts a process (ADR 0062).

## What you are looking at

The LEARN stage keeps one window of the last 256 feasibility refusals from
both planes — the desk's own order manager and the cells' reports — each
naming the venue it was bound for and the gate that refused it. When that
window holds at least ten refusals and one venue accounts for at least three
in four of them, the platform journals a **withdrawal** under `venue.withdrawn`
and stops using that venue at three seams: `OrderManager::submit` refuses
every order bound for it under `venue-availability`, the centre omits its
conversions from the whitelist it issues to the cells, and the centre ships
the withdrawn set on policy slot 11 so a cell refuses against it on its own
next pass.

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
   dominated — one of step 2's table, and read its last row first if the
   answer is `feasibility_withdrawn_venue`, because that one names a
   different problem — `count` is this venue's refusals and `sample` is the
   denominator they were measured against: the whole window, with a
   *withdrawn* venue's echoes weighted rather than counted whole, so it is
   not simply the number of entries. `seams`
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
   | `feasibility_depth` | the size resting at the touch against the size the cell is sending — an edge-only gate: the order is larger than the book it is aimed at |
   | `feasibility_fee_floor` | the venue's fixed costs against the edge the cycle claims — the cycle does not pay for itself |
   | `feasibility_gas_floor` | the same for gas, where the venue charges it |
   | `feasibility_constraint` | the policy payload, not the venue and not the order: slot 11 stated a grid value that is not a grid — a non-positive tick, a negative figure — and the cell refused it rather than quietly substituting its own. A cluster here is a bug in what the centre shipped, and it will withdraw a venue that never did anything wrong |
   | `feasibility_withdrawn_venue` | **the policy the cells are applying, not the venue.** A cell refuses under this gate when *its* slot 11 says the venue is withdrawn. If the centre agrees, that is an echo and it cannot dominate anything — see below. If you are reading it here, the centre does *not* hold this venue withdrawn and the cells do: their policy is stale, or they never heard a reinstatement. Fix the policy path; the venue's grid is not the problem |

   The first four are the only gates the desk's own order manager can refuse
   under; all nine are the cells'. The window spans both planes, so a cluster
   may be entirely the cells', entirely the desk's, or mixed; `seams` in
   step 1 says which.

   **Why the last row is different, and what an "echo" is.** A refusal under
   that gate is an echo only when the centre's own withdrawn set agrees with
   the cell — `qip_contracts::feasibility::is_withdrawal_echo(gate, venue,
   withdrawn)` decides it, and it takes the venue and the centre's set on
   purpose. The gate string arrives on a report over a wire that
   authenticates nobody, so a cell with a stale slot 11 can refuse at a venue
   the desk is still trading and call it withdrawn. That is not an echo; it
   is an ordinary refusal, and it counts as evidence like any other. This is
   the case the table's last row tells you to investigate.

   A confirmed echo is the platform's own decision arriving back at it, and
   it is handled with more care than "ignore it", because ignoring it is what
   broke: dropping a withdrawn venue's refusals emptied the denominator every
   other venue's share is measured against, the runner-up became a cluster of
   whatever remained, and the desk lost venue after venue. So one echo per
   venue per report is seated in the window as a **denominator** entry, its
   weight is capped at the genuine evidence that venue still holds
   (`venue_review::VenueTally::weight`), and a withdrawn venue is never a
   candidate for withdrawal — so no echo can withdraw anything, and the
   echoes stop counting for anything once the venue's real refusals age out.
   Read `VenueTally::weight` and `venue_review::assess` if you need the
   arithmetic; what you need operationally is that an echo cannot cause a
   withdrawal and cannot hide one.

   Enumerate the cells' gates from the source rather than from this table:
   `sed -n '/^pub const EDGE_GATES/,/^];/p'
   backend/crates/libs/qip-contracts/src/feasibility.rs` printed nine
   literals on 2026-09-14, and this table listed only the desk's four until
   that day — an operator whose cluster named one of the other five found no
   row.

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

It removes a name from a **subtractive** set. Every place the withdrawn set is
read, it is read to refuse, and there are three of them:

- `OrderManager::submit`'s withdrawal step, which refuses the desk's own
  orders under `venue-availability`;
- `CentralPlane::cycle_whitelist_for`'s `retain`, which omits the venue's
  conversions from the whitelist the centre issues to the cells;
- `CentralPlane::feasibility_constraints`, which puts the set on policy
  slot 11 so a cell refuses against it directly — the closure described under
  "The edge limit" below.

Enumerate them yourself rather than trusting the number; this paragraph said
"exactly two places" until 2026-09-14, when the third shipped and the sentence
outlived it by a merge. `grep -rn 'withdrawn_venues'
backend/crates/runtime/qip-kernel/src backend/crates/services/qip-execution-engine/src
backend/crates/edge/qip-edge/src` printed 32 lines on 2026-09-14; the three
above are the ones that refuse, and the rest are the writers, the accessors,
the constructors and the replay that rebuilds the set from the log.

There is no path on which it admits anything. So the most a reinstatement can
restore is what `QIP_VENUES`, the arbitrage policy's venue map and the capital
grant's terms already permitted — a venue none of those name is not made
reachable by any number of signatures.

## The edge limit, and how it closed

**Do not restart the node.** This section told you to until 2026-09-14, and
by then the closure it said was unbuilt had shipped in the same merge — a
document prescribing a desk-stopping action for a problem that no longer
existed.

The limit was real and is worth understanding, because the stale half of it
still stands. Omission from the whitelist is necessary and not sufficient for
a cell whose arbitrage desk is already installed: `Cell::install_arbitrage`
refuses a second desk and nothing clears the first, so a withdrawal reaches a
desk installed *after* it, while a desk installed before keeps its graph,
including the withdrawn venue's conversions.

What closed is the consequence, not the graph. Policy slot 11 now carries the
withdrawn set — `CentralPlane::feasibility_constraints` fills it from the same
field the whitelist omits on — and `qip_edge::feasibility::assess` refuses an
intent bound for a venue in that set under `feasibility_withdrawn_venue`,
ahead of every question about the order's own size, grid or book. The stale
graph goes on offering the cycle and the cell goes on refusing it, on the next
pass rather than on the next process restart. It is refused and never
re-routed: sending the same intent to another venue would be a gate choosing a
venue.

Two things an operator should still check before concluding the edge is
covered:

- **The cell must hold a policy payload.** The refusal reads the slot the cell
  has applied, so a cell that has never applied one refuses nothing here — the
  whitelist omission is still doing the work. `qip_edge_policy_sequence`
  carries what the cell applied; correlate it against what the centre believes
  it published.
- **Staleness does not disarm it.** A slot past its TTL is still read, on
  purpose: the last thing the centre knew beats nothing, and the degradation
  table already narrows the cell's sizing on that staleness rather than
  arming a second control on the same fact.

## Why there is no route that withdraws a venue

Because a withdrawal is the platform's finding about its own evidence, and a
caller who could ask for one could deny the desk its venue with a single
request. The evidence path can only ever subtract; the signature path can
only ever restore what configuration already allowed. Neither direction is a
way to widen what this platform may trade.
