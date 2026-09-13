# ADR 0062: A venue is withdrawn on feasibility evidence and reinstated only by two signatures

- **Status**: Proposed
- **Date**: 2026-09-13
- **Supersedes**: nothing
- **Related**: ADR 0003 (paper trading by default), ADR 0008 (cells decide alone), ADR 0055 (a counterfactual result narrows sizing and only narrows it), ADR 0061 (rule regret is a proposal, a defence or a dormancy finding)

## Context

Blueprint §12.3's table of what a counterfactual result changes has six rows.
ADR 0061 built the three about rules; ADR 0055 built the declined-path half
of the sizing row. The fourth row reads:

| Finding | Consequence the blueprint names |
|---|---|
| Feasibility rejections cluster on one venue | the venue is withdrawn |

It was `ABSENT`, and the reason was not that nothing consumed the evidence
but that the evidence had no key. A feasibility refusal was counted on both
planes by the *gate* that made it — `qip_orders_refused_total{control}` on
the desk, `qip_edge_refusals_total{gate}` at the cells — and by the venue on
neither. A window of such refusals could say how often the lot gate fired
and not that every firing was at one venue. The desk's `DeclinedPath`
carried no venue; a cell's refusal reached the centre as a count on the
delta sink and nothing else, because `CellReport` had no refusals field at
all.

Three further facts, each found by inspection rather than assumed, shape the
decision:

1. **`is_available()` can never fire on this platform.** `OrderManager::
   submit` consults it only inside `if !broker.is_simulated()`, and the only
   broker the kernel constructs is `SimulatedBroker`, whose `is_available()`
   is hard-coded `true`. A withdrawal read through that path would be a
   control that reads as protection and is none — this repository's
   standing example of what not to ship, `MaxExpectedShortfall`.
2. **The centre does not intersect `QIP_VENUES`.** The cell-side check the
   design brief attributed to `central/plane.rs` is a test-module mirror.
   The production enforcement is at three seams — the node's
   `graph_from_whitelist`, the cell's `install_arbitrage`, and the centre's
   `whitelist_for` under `permits_venue` — and every one of them reads a
   configured or signed list. A conversion must survive all three.
3. **An installed desk keeps its graph.** `Cell::install_arbitrage` refuses a
   second desk ("a second would reset the capital the first has
   committed"), nothing sets `self.desk = None`, and policy slot 11 — the
   only per-venue pass-time channel — is `Slot::unproduced()` at the centre.

## Decision

**A venue that accounts for at least three in four of a window of at least
ten recent feasibility refusals is withdrawn at both seams, by omission
only, after the withdrawal is on the record; and it comes back only when two
distinct, freshly authenticated operators sign.**

The mechanism, in the order it runs:

- **The key.** A feasibility refusal now names its venue at both seams. On
  the desk, `capture_submission`'s refusal arm sets `venue` for a refusal
  whose `feasibility_gate()` is `Some` — the venue is `Broker::name()`, the
  same string the accepted arm reads from `result.venue`, so a refusal and a
  fill on one order cannot be charged to different venues — and `None` for
  every posture refusal, which is about the platform and not about where the
  order was going. At the cell, `admit_feasible` records the intent's venue
  beside the refusal it just pushed, keyed by index; `state_delta` joins the
  two so `DeltaRefusal` carries `venue: Some(..)` for a feasibility refusal
  and nothing for any other gate, absent on the wire rather than `null`.
  `CellReport` carries the refusals, and the delta sink in `qip-api`
  forwards them.
- **The window.** `Platform::record_feasibility_refusal` is where every
  admitted refusal lands: it counts `qip_feasibility_refusals_total{venue,
  constraint}` and keeps the refusal in a window of `FEASIBILITY_WINDOW`
  (256). The window evicts its oldest entry, and that is right *here* and
  wrong for the declined and filled queues beside it: this is a sample —
  "what share of recent refusals name this venue" — and the oldest leaving
  is the sample staying current, where a queue of work that dropped its
  oldest would silently choose which veto goes unexamined.
- **Admission from a cell.** The centre admits a carried refusal only when
  its gate is one of the eight `qip_contracts::feasibility::EDGE_GATES`
  **and** its venue is one the arbitrage policy's venue map or a grant live
  at that instant names — the same sources the cell was configured from. What
  it cannot attribute is counted under the literals `unknown` and `other`
  and admitted to nothing. A cell that ships a venue nobody configured
  cannot mint a label, and a cluster that would have withdrawn such a venue
  is charted under a name an operator can search for rather than acted on.
- **The finding.** `venue_review::assess` is pure arithmetic over the
  window: the venue with the largest count whose share of the *whole*
  window is at least `VENUE_WITHDRAWAL_SHARE` on a window of at least
  `VENUE_WITHDRAWAL_MIN_SAMPLE`, excluding venues already withdrawn. The
  bars are ADR 0055's — ten and three in four, `COUNTERFACTUAL_SIZING_MIN_
  SAMPLE` and `COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION` by reference —
  because the platform has one answer to how much evidence makes a pattern a
  finding, and a second, differently-sized answer would be a number nobody
  could reconcile with the first.
- **The record, then the change.** `Platform::review_venues`, in LEARN after
  the rule review, journals a `VenueWithdrawal` under `venue.withdrawn`
  (Decide group, retained permanently, idempotent on venue and cycle) and
  only once the log holds it calls `Platform::withdraw_venue` — the single
  writer of the set — which forwards the name to the order manager and the
  central plane. A journal failure leaves the venue in use and reports a
  problem on the cycle, matching `apply_registration`'s "journal, then
  adopt": a withdrawal the log does not hold is one a restarted process
  would silently undo. The set is resumed from the log at assembly, walking
  withdrawals and `reinstated` records in log order, and re-forwarded to
  both seams; a plane swapped in through `set_central` receives it.
- **The desk seam.** `OrderManager::submit` gains step 5: a venue in the
  withdrawn set is refused under the existing `VenueUnavailable` reason,
  which `gate_of` already charts as `venue-availability`, with the refusal
  naming the way back. The step is independent of `is_simulated` (fact 1),
  sits after the kill switch and the autonomy gate so a halted platform
  still reports the halt, and after feasibility so a withdrawn venue's
  further infeasible orders keep landing in the window.
- **The edge seam.** `CentralPlane::cycle_whitelist_for` `retain`s the
  conversions `whitelist_for` produced, dropping those whose parsed venue —
  never a substring — is withdrawn. `WhitelistOutcome::Emitted` names what
  was omitted on the journaled issue, and when nothing survives the outcome
  is `AllWithdrawn`, said in words, so the cell's installer refuses "no
  conversion" and installs nothing. `CycleWhitelist.cycles` is not filtered:
  its only reader is a test asserting it is empty and `whitelist_for` never
  fills it.
- **Reinstatement.** `Platform::reinstate_venue` is cloned from
  `approve_promotion` where the two are about the same thing — a credential
  fresh within `PROMOTION_CREDENTIAL_AGE`, two distinct people, a first
  signature stale after `PROMOTION_APPROVAL_WINDOW` — and differs where they
  are not: the subject is a venue the platform withdrew, and one that is not
  withdrawn is refused as not found. Every signature is journaled under
  `venue.reinstated` before anything changes; the countersignature's record
  is written before the venue is put back. No HTTP route exposes it yet.

## Why evidence can never add a venue

The withdrawn set is read by exactly two things: the order manager, to
refuse, and `cycle_whitelist_for`, to `retain`. The only constructor of a
`WhitelistedConversion` is `whitelist_for` iterating the policy's markets
under `envelope.permits_venue`, and the node and the cell each re-check
`QIP_VENUES` (fact 2). Reinstatement removes a name from a subtractive set,
so the most it can restore is what configuration and the grant already
permitted; a venue `QIP_VENUES`, the policy's venue map and the grant's
terms do not name is not made reachable by any signature. The test
`a_window_dominated_by_a_venue_the_policy_does_not_name_changes_no_whitelist`
holds the property from the other side: the desk's broker is not a policy
venue, a cluster on it withdraws the desk's broker, and the cells' whitelist
is byte-for-byte what it was, with no name on the issue.

The fill error the shared foundation charts — `qip_venue_fill_error_bps`,
§12.4's "fill error tracked" — is read by nothing here.
`a_twin_that_is_wildly_wrong_about_fills_never_withdraws_a_venue` misprices
twelve fills by a thousand basis points and asserts no venue moves, no
withdrawal is journaled, and the window is empty because nothing was
infeasible. A venue is withdrawn on feasibility evidence alone.

## The sole-venue consequence, chosen

The desk has one broker. A cluster on it stops every desk order, loudly,
under `venue-availability`, until two operators reinstate it. That is
fail-closed by choice. The alternative — require at least two venues in the
window before any withdrawal, so the desk can never be withdrawn from its
only venue — was considered and rejected as an exception to fail-closed
dressed as prudence: ten off-grid orders in a row at the desk's only venue
is exactly the situation in which the desk should stop and a person should
look at the grid, and a rule that exempted the one venue that matters would
exempt the case the row exists for.

## The edge limit, stated

Omission from the whitelist is necessary and not sufficient for a desk a
cell has already installed (fact 3). A withdrawal takes effect for any desk
installed *after* it; a desk installed before keeps its graph, including the
withdrawn venue's conversions, until the node restarts. The closure is named
rather than pretended: produce policy slot 11 (`FeasibilityConstraints`) at
the centre, carry a `withdrawn_venues` set on it, and refuse an intent
bound for a withdrawn venue in `qip_edge::feasibility::assess` under a new
`GATE_*` literal — at which point the withdrawal reaches an installed desk
on its next pass rather than its next install. Until then §12.3's fourth
row is `PARTIAL`, and `docs/DELIVERY-STATUS.md` says so in the same words.

## The no-cascade denominator

The share is computed over the whole window **including a withdrawn
venue's entries**. A withdrawn venue's later infeasible orders keep landing
in the window — the desk's feasibility gate runs before its withdrawal
check, on purpose — and excluding them would make the runner-up a cluster of
whatever remained: ten refusals, eight at a venue just withdrawn, two at
another, and the other would be "two of two" and withdrawn next cycle, and
the desk's last venue after that. `withdrawing_one_venue_does_not_make_the_
runner_up_a_cluster_of_the_remainder` walks that cascade and refuses it;
`an_already_withdrawn_venue_is_not_withdrawn_twice` holds that the same
cluster reviewed again writes no second record and cannot undo a
reinstatement on the next pass.

## What it costs

- One more refusal step on every desk order, a set lookup on a string.
- One `Vec` on `WorkReport` at the cell, which crossed clippy's variant-size
  bar in `qip-edge-node`'s `PassOutcome`; the report is boxed there.
- A new shared module, `qip_contracts::feasibility`, holding the eight gate
  literals once. `qip_edge::feasibility` aliases them; the desk's
  `qip_execution_engine::feasibility` does not depend on `qip-contracts` and
  keeps its four declarations, pinned equal by a kernel unit test rather than
  by an internal dependency added for the purpose.
- The desk's window and the cells' share one sample, so a busy desk can
  drown a quiet cell's cluster and a busy cell the desk's. That is the
  choice: the row asks whether refusals cluster on a venue, not on a plane.

## What would make this wrong

- **The bars.** Ten and three in four are ADR 0055's, adopted by reference.
  If a desk's feasibility refusals turn out to be routinely bursty — a venue
  that rejects a whole batch of orders on one bad quote — the window will
  withdraw it on a single episode. The window is 256 and the bars are named
  constants; the response to that finding is a longer window or a per-venue
  episode count, recorded as an amendment here, not a quieter test.
- **The single window.** If the desk's broker and a cell's venue ever share a
  name, their refusals merge. Today they cannot: the broker is
  `simulated-venue` and no policy names it.
- **The edge limit closing silently.** If slot 11 is produced without the
  `withdrawn_venues` set, an installed desk still keeps its graph and the
  status row must not move to `REACHED` on the slot's existence alone.
- **Any code path that reads the withdrawn set to admit.** There is none;
  the day one appears, the "why evidence can never add a venue" section
  above is false and this record is void.

## Consequences

- §12.3's fourth row moves `ABSENT → PARTIAL`: withdrawn at the centre in
  full, at the edge by omission with the installed-desk limit named.
- §12.4's "fill error tracked" moves to held, three of four, on the shared
  foundation; the trial-accounting gap stays.
- Two topics on the backbone, `venue.withdrawn` and `venue.reinstated`, both
  Decide and retained permanently; `Topic::ALL` is 75.
- Two central series, documented with grep commands in
  `.claude/rules/domains/observability.md` and `docs/ops/observability/README.md`.
- Follow-on work: a `qip-api` route for reinstatement; slot 11 with a
  withdrawn set, for the edge limit; and a structured `Infeasible { venue,
  gate, detail }` refusal reason in place of the `Malformed` prefix, which
  this lane left alone because ADR 0061's lane was editing `RefusalReason`
  concurrently.
