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
  fill on one order cannot be charged to different venues
  (**superseded in part, 2026-09-14 — see Amendment A**: the value is
  unchanged and where it is read from is not. `capture_submission` read
  `self.broker.name()` at the capture site because the refusal carried no
  venue; it now reads `RefusalReason::Infeasible`'s own `venue` field, which
  `OrderManager::submit` filled from that same `Broker::name`) — and `None` for
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
- **Corroboration across cells.** A cluster whose window contains no desk
  refusal at the winning venue must also name at least
  `venue_review::VENUE_WITHDRAWAL_MIN_CELLS` (two) distinct cells before
  `assess` returns it. This closed a gap an independent security review
  found in the first cut of this record: `attribute_refusals` checked a
  refusal's gate and venue against configuration but never asked whether
  more than one cell agreed, so a window of ten refusals from **one** cell
  registration cleared the same sample-and-share bar the desk's own,
  trusted, single-source evidence clears — and the shipped test
  `ten_cell_refusals_at_the_only_policy_venue_withdraw_it_and_the_whitelist_
  says_so` demonstrated exactly that as intended behaviour. On a wire that
  authenticates nobody (`qip-api/src/mesh.rs`, `qip-edge/src/mesh.rs`), one
  compromised, buggy, or spoofed cell process could deny a venue to the desk
  and every other cell with no corroboration and no rate check against real
  order volume. `FeasibilityRefusal` now carries the reporting cell's
  self-asserted identity (`None` for the desk, `Some(report.cell)` for a
  cell), and `assess` requires either a desk refusal in the cluster or
  refusals naming at least two distinct cells. `ten_cell_refusals_at_the_
  only_policy_venue_withdraw_it_and_the_whitelist_says_so` was renamed
  `ten_refusals_from_one_cell_do_not_withdraw_the_only_policy_venue` and now
  asserts the opposite of what it did, and
  `ten_cell_refusals_from_two_distinct_cells_at_the_only_policy_venue_
  withdraw_it_and_the_whitelist_says_so` proves corroboration still admits a
  genuine cluster rather than merely refusing every edge-sourced one.
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
  is written before the venue is put back. ~~No HTTP route exposes it yet.~~
  **Superseded 2026-09-14 — see Amendment B.** `POST
  /venues/:venue/reinstatements` exposes it at the operator role, and `GET
  /venues/withdrawals` lists what is withdrawn at the viewer role.

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

Corroboration is deliberately not this same shape applied to cells: it does
not require two *venues*, and it does not exempt a cell's only venue either.
It requires two distinct *cells* before edge-only evidence counts, because
the vector this closes is one untrusted reporter, not one venue. A single
cell that is the *only* cell configured for a region can still never clear
the edge-only bar alone — which is correct: nothing distinguishes "this
region's one cell is right" from "this region's one cell is compromised" on
an unauthenticated wire, and the desk's parallel path (fact-checked against
the platform's own broker) is exactly where that ambiguity does not exist.

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
- **Every subsequent order to a withdrawn venue is a fresh refusal, and a
  code review found it was landing on the wrong ledger.** `OrderManager::
  submit`'s step 5 refuses each one under `RefusalReason::VenueUnavailable`,
  and `Platform::capture_submission` used to queue every refusal for the
  twin's counterfactual pricing regardless of kind — feeding ADR 0055's
  declined-path sizing discount with evidence that is administrative (the
  venue's own reachability), not a judgment about whether the order was
  well sized. Ten such refusals to one instrument, easily reached within a
  cycle or two of a withdrawal, would have halved that instrument's sizing
  confidence for a reason no rule found. Fixed at `RefusalReason::
  is_sizing_evidence` (`qip-execution-engine`): only `Malformed` and
  `RiskRejected` — judgments about the order — reach the declined queue;
  `VenueUnavailable` and the platform's other posture refusals are still
  recorded (an operator can see every rejection) but do not narrow sizing
  confidence.

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
- **The corroboration key is a self-asserted string, not a verified
  identity.** `report.cell` is whatever the sender of a mesh frame wrote; the
  uplink authenticates nobody. Requiring two distinct values raises the cost
  of the attack this record's "corroboration across cells" section closes —
  a single unauthenticated sender must now name two apparently-distinct
  cells across separate reports rather than one — but it does not make the
  identity trustworthy, and a sender able to forge two distinct `cell`
  strings defeats it exactly as it would have defeated a bare count of
  reports. Closing this fully needs the uplink to authenticate its senders,
  which is out of this record's scope and is not claimed here. Until then,
  this is a structural raise of the bar, not a proof the bar cannot be
  cleared by one attacker.
- **The signing subject is durable by convention, and role-scoped in this
  deployment** (added 2026-09-14 with Amendment B, which argues it at
  length). `OperatorIdentity::subject()` is a `String`; nothing in the type
  system distinguishes a durable per-human identifier from a session token,
  and the reinstatement route's "two distinct people" check compares
  subjects. The composition root mints one credential per *role*, so today
  two humans holding the operator token are one subject and a
  countersignature is refused — closed rather than open, and identical for
  the two signature routes that shipped before this one. It becomes wrong in
  the dangerous direction only if something ever populates `subject` from a
  session or request value; an acceptance test holds every call site to
  `principal.subject` for exactly that reason.

## Consequences

- §12.3's fourth row moves `ABSENT → PARTIAL`: withdrawn at the centre in
  full, at the edge by omission with the installed-desk limit named.
- §12.4's "fill error tracked" moves to held, three of four, on the shared
  foundation; the trial-accounting gap stays.
- Two topics on the backbone, `venue.withdrawn` and `venue.reinstated`, both
  Decide and retained permanently; `Topic::ALL` is 75.
- Two central series, documented with grep commands in
  `.claude/rules/domains/observability.md` and `docs/ops/observability/README.md`.
- Follow-on work: ~~a `qip-api` route for reinstatement~~; slot 11 with a
  withdrawn set, for the edge limit; and ~~a structured `Infeasible { venue,
  gate, detail }` refusal reason in place of the `Malformed` prefix, which
  this lane left alone because ADR 0061's lane was editing `RefusalReason`
  concurrently~~. **Two of the three are done, 2026-09-14 — Amendments A and
  B below.** Slot 11 is not, so the edge limit stands exactly as "The edge
  limit, stated" describes it and §12.3's fourth row stays `PARTIAL`.

## Amendment A — the structured refusal reason (2026-09-14)

`RefusalReason::Infeasible { venue, gate, detail }` replaces the `Malformed`
variant with an `infeasible (<gate>):` prefix on its detail string.
`feasibility_gate()` reads the `gate` field instead of parsing that prefix.

The prefix was matched exactly against the four `GATE_*` constants and was
never wrong. What was wrong was the shape: a key recovered from a sentence
written for a person. ADR 0061 §1 had already found the same shape one level
up and said why a rule tally must never be keyed on a refusal's wording; here
the failure would have been quieter, because a reworded detail string would
have charted every off-lot order under `order-validation` again, stopped
§12.3's fourth row seeing the venue's refusals, and failed no test.

Four things are deliberately unchanged, and each was checked rather than
assumed:

- **The gate literals.** `feasibility_gate()` still *resolves* its answer
  against the four constants rather than returning the field as found, so
  the value remains a bounded metric label and attribution key; a gate the
  four do not name charts as `order-validation` and attributes to no rule,
  which is what the prefix match did with a sentence it did not recognise.
  `gate_of` maps to the same literals and no metric label value moved.
- **The sentence.** `describe()` produces the same string byte for byte.
  It is what reaches the `Action::Rejected` record, and the hash-chained log
  holds refusals written on both sides of the change; an operator reading it
  for a venue's vetoes must find one vocabulary there. Nothing parses it.
- **The venue.** The value on `DeclinedPath` and `DeclinedScore` is the same
  string as before — only where it is read from moved, from
  `self.broker.name()` at the capture site to the field the refusing code
  filled. The two cannot differ while the desk has one broker, which is the
  point: a second broker would have made them differ silently.
- **Sizing evidence.** `is_sizing_evidence` is a `matches!` and not an
  exhaustive match, so the split could have dropped every feasibility veto
  out of ADR 0055's declined queue without a compile error. The new variant
  is listed by hand and the enumerating test holds every variant to its side.

**Nothing in the workspace serialises or deserialises a `SubmissionResult`.**
It is not an `EventBody`, no route or store encodes one, and the API renders
refusals through `describe()` and `is_safety_control()`; the only durable
record of a refusal is `Action::Rejected`'s two strings. The change is
additive on the wire regardless — a record tagged `malformed` still decodes
as `Malformed` — but the compatibility question has no production instance.

## Amendment B — the reinstatement route (2026-09-14)

`POST /venues/:venue/reinstatements` at the operator role, cloned from the
promotion approval, with `VenueReinstatementRequest::parse` cloned from
`PromotionApprovalRequest::parse`: a rationale and nothing else, every other
key refused by position without being echoed, the same `MAX_RATIONALE`. The
approver is the authenticated session's subject and never the body;
`OperatorIdentity::verified` takes `principal.issued_at` and not `now`,
because `now` would make the kernel's freshness window compute an age of zero
and become a control that cannot fire. Beside it, `GET /venues/withdrawals`
at the viewer role lists what is withdrawn, the cluster each was withdrawn on
and whether a first signature stands — a boolean and not a name, because who
acted is on the event log at the authority that reads the log.

Why a route at all, given the record above argues the withdrawal should stop
the desk: because a fail-closed control whose *audited* recovery path does not
exist gets recovered from by one nobody audited. Before this, putting a venue
back needed direct access to the kernel. The runbook is
`docs/operations/reinstating-a-venue.md`.

The route can only subtract from a subtractive set, and that remains
structural: the withdrawn set is read in exactly two places and both read it
to refuse, so "Why evidence can never add a venue" above is unchanged and a
signature restores only what `QIP_VENUES`, the policy's venue map and the
grant already permitted. There is deliberately no route that *withdraws* a
venue.

**The durable-subject residual, stated honestly.** A security review flagged,
as latent and unreachable precisely because no route existed, that the "two
distinct people" guarantee rests on `OperatorIdentity::subject()` being a
durable per-human identifier by convention rather than by type. This route
makes it reachable, so the source was traced rather than assumed:
`principal.subject` is `Credential::subject`, fixed when the composition root
mints the credential and copied unchanged on every authentication. **It is
not session-scoped and not request-scoped**, and
`every_operatoridentity_is_built_from_the_principals_durable_subject_not_a_session_value`
now holds that directly — one credential yields one subject across two
authentications fifteen minutes apart, two credentials yield two, and the
route reaches the kernel's comparison.

What remains true, and is the residual: in the shipped composition root every
credential is minted as `<role>@env`, one per role, from one
`QIP_TOKEN_OPERATOR`. So two humans holding that token present **one** subject
and the countersignature is refused — the control fails closed rather than
open, and a reinstatement is unobtainable in that deployment until per-human
operator credentials exist. That is identical for the promotion and
recalibration signatures already shipped, it is a deployment change rather
than a code one, and it is written here rather than fixed so that nobody reads
the route's existence as a claim that two people can sign today. The type-level
fix — a `Subject` newtype a session value cannot inhabit — is still not built,
and the convention is still a convention.
