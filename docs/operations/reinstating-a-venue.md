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
   dominated — one of step 2's table, and never `feasibility_withdrawn_venue`,
   for the reason that table's last row gives — `count` and `sample` are this
   venue's refusals and the whole window, and `seams`
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
   | `feasibility_withdrawn_venue` | **nothing, and you will not see it here.** It is the cell enforcing a withdrawal the centre already made. It is charted on `qip_feasibility_refusals_total` under its real venue and its real gate, and it is deliberately never admitted to the withdrawal window — see below |

   The first four are the only gates the desk's own order manager can refuse
   under; all nine are the cells'. The window spans both planes, so a cluster
   may be entirely the cells', entirely the desk's, or mixed; `seams` in
   step 1 says which.

   **Why the last row is different, and why you must not "fix" it.** Whether
   a gate's refusals may enter the window a venue is withdrawn on is decided
   in one place, `qip_contracts::feasibility::is_withdrawal_evidence`, and a
   withdrawal's own echo is excluded there. Every other gate asks a question
   about an order at a venue, so a cluster of them is evidence about the
   venue; the echo asks nothing — it reports the platform's earlier decision
   back to itself, once per intent per pass, for as long as a desk installed
   before the withdrawal keeps offering cycles through it. Admitted, those
   echoes would fill the window within a few passes and then own the
   denominator every other venue's share is measured against, and no second
   venue could ever be withdrawn. The exclusion is what keeps the control
   able to fire twice. Read the function rather than this paragraph if you
   need the current rule; the property is stable, the way it is decided is
   being worked on.

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
