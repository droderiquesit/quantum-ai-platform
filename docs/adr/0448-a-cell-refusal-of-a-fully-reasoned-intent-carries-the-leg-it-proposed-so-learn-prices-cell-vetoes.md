# ADR 0448: A cell refusal of a fully reasoned intent carries the leg it proposed, so LEARN prices cell vetoes

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session t50-11 (wave580). The delegation's
  limits hold:
  - it sets no figure: no new bound, window or threshold is introduced; the
    centre's existing `DECLINED_HISTORY` window and per-cycle counterfactual
    cap bound what this adds;
  - it relaxes no limit, guard or refusal: every gate refuses exactly what it
    refused before, and the leg is attached after the refusal is recorded;
  - it touches no paper-trading layer and adds no path to a venue: a leg is
    what was proposed, never an order, and the centre never books it.
- **Date**: 2026-11-12
- **Extends**: the R12-MESH-5068 slice that gave `DeltaRefusal` its `legs`
  field and the centre its `queue_cell_refusals` / `score_cell_refusals`
  path. No wire field is added; no clause of an earlier ADR changes.

## Context

R12-MODEL-7210 asks that the platform learn from every actual, rejected and
counterfactual outcome, because learning only from what was executed biases
the loop toward its own gates. Its verification is that the LEARN stage
scores fills, declines and counterfactuals each cycle.

On 2026-11-12 the desk half of that was done: a desk veto is captured as
`Action::Rejected`, queued, and priced by `score_declined`; a desk fill is
queued in `filled` and priced by `score_filled`. The cell half was not. A
cell refusal reached the centre as `DeltaRefusal { gate, reason, venue,
legs }`, and only one kind of refusal filled `legs`: an arbitrage cycle
refused whole (`admit_cycle`, R12-MESH-5068). A directional signal a cell
refused at its capital envelope, drawdown stop, hedge target, exposure or
authority bounds, observation autonomy or net-edge gate shipped a gate and a
sentence. The centre could count it and could not price it, because pricing
needs the instrument, the venue, the side and the size.

The triage for this row proposed widening `DeltaRefusal` to carry the
refused intent as new optional fields. That is unnecessary: `legs` already
carries exactly those facts, the centre's queue already reads `legs` on any
refusal whatever its gate (`refused_cycles` filters on `!legs.is_empty()`,
not on the gate), and both ends already decode a delta written before the
field existed. A second field for the same fact would be a second source of
truth for it.

The triage also proposed a consumer of the desk outcome records
`DeskShipper` carries to the ledger stream, to feed them back into learning.
Reading the code shows that would score each desk fill twice: the record
`DeskLedgerFeed` queues and the `FilledPath` `score_filled` prices are both
written from the same booked fill in the same `Platform`
(`capture_submission`). The records reach the ledger, which is their job;
the learning already happens in process.

## Decision

1. **A cell refusal of a fully reasoned intent carries the leg it
   proposed.** `Cell::refuse_proposed` refuses through `Cell::refuse`
   exactly as before — same gate literal, same journal entry, same counter —
   and then writes one `RefusedLeg` into the report's `refused_legs` side
   table at that refusal's index, the table `admit_cycle` already writes.
   `Cell::state_delta`'s existing join attaches it to that refusal and no
   other.
2. **Which gates.** Those in `Cell::intent_for` at which instrument, venue,
   side, size and price are all known: `envelope_expiry`, `capital` (both
   the latched unpriced-fill stop and the envelope's refusal),
   `GATE_HEDGE_TARGET`, `GATE_ENVELOPE_DRAWDOWN`, the exposure verdict's
   gates, the authority verdict's gates, `autonomy`, and `GATE_NET_EDGE`.
   The size is the one the gate judged: the degradation-narrowed ask before
   the envelope, the admitted quantity after it, and the intent's own size
   at the net-edge gate.
3. **Which gates not, and why.** Routing gates before a venue or price is
   known (`signal_expiry`, the gates inside `route_for`, `signal_kind`)
   have no leg to carry. `degradation_sizing` and `capital`'s
   "rounded to nothing" refuse a size of zero, which has nothing to price.
   `deployment` refuses a signal from a strategy not deployed at this cell,
   a configuration fact rather than a judgement on the trade.
   `capital_reduced` is not a refusal of the intent — the reduced order still
   goes out — and pricing the full size would charge the fill and its
   reduction twice. `region_reservation` refuses inside
   `hold_region_capital` and is left for a later slice. Refusals of a *net*
   order after netting (`place_net`: feasibility, conduct, quote budget,
   broker backpressure) are not per-intent and are not covered here.
4. **The centre is unchanged.** It queues each leg under the refusal's gate,
   keyed by report and position so a replay queues nothing twice, counts an
   unobserved instrument or a full window rather than evicting, and charts
   every score under the one literal `gate="cell_cycle"` label, keeping the
   cell's gate string on the score and never minting it into a label.
5. **Desk outcome records get no second learner.** What is pinned instead
   is the join: every desk fill on the pending ledger stream is one
   `score_filled` prices
   (`every_desk_fill_shipped_to_the_ledger_is_one_the_learn_stage_prices`).

## What changes and what does not

- Changes: a `DeltaRefusal` at the gates in decision 2 now has one entry in
  `legs`; the centre queues and prices it.
- Does not change: which intents are refused, any gate's reason text, the
  refusal series, the journal, the wire schema, or anything the centre does
  with a refusal that carries no legs.

## What it costs

A cell refusing the same signal every pass — at observation autonomy, say —
now ships one leg per pass, and each takes a slot of the centre's
`DECLINED_HISTORY` window until its horizon passes. A full window is counted
`qip_counterfactuals_unscored_total{reason="cell_capacity"}` and named on
the capture problems, never evicted, so an arbitrage cycle arriving behind a
flood of repeated refusals waits or is counted rather than silently lost.
That is the existing discipline; no deduplication across passes is added,
because two passes refusing at different prices are two counterfactuals.

## What would make this wrong

- If a gate in decision 2 came to refuse something other than the intent
  `intent_for` built — a whole strategy, say — the leg would price a trade
  nobody proposed. Each gate is listed so that a change to one is visible.
- If the centre began labelling by the cell's gate string, the label would
  become unbounded through a wire that authenticates nobody.

## What remains (2026-11-12)

R12-MODEL-7210 stays PARTIAL: refusals of the gates decision 3 names carry no
leg; `arbitrage_cycle_broken` still ships no report (R12-MESH-7143); and no
cell, edge node or centre is deployed, so the integration is proven in test
only.

## Evidence

- `cargo test -p qip-edge --test reservation` —
  `a_signal_refused_at_observation_ships_the_leg_it_proposed_on_the_delta`.
- `cargo test -p qip-acceptance --test cell_refusal_learning` — the cell's
  refusal crosses the mesh codec, the centre queues it and LEARN prices it.
- `cargo test -p qip-kernel --test learning` —
  `every_desk_fill_shipped_to_the_ledger_is_one_the_learn_stage_prices`.
