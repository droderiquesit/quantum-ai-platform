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
   only per-venue pass-time channel — **was** `Slot::unproduced()` at the
   centre when this record was written. The amendment below closes that
   half; the stale graph itself is unchanged. The past tense is deliberate:
   read on its own, a present-tense Context is the one part of a superseded
   record a reader takes for a current fact.

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

## The edge limit, stated — **superseded 2026-09-14, closed as named**

**This section is kept rather than struck.** It is the statement of a limit
this record shipped with, and the amendment that follows is only legible
beside it. Read the paragraph as history: the limit described below was real
between this record's date and 2026-09-14, and the closure took exactly the
shape the paragraph named, which is the useful part.

> Omission from the whitelist is necessary and not sufficient for a desk a
> cell has already installed (fact 3). A withdrawal takes effect for any desk
> installed *after* it; a desk installed before keeps its graph, including the
> withdrawn venue's conversions, until the node restarts. The closure is named
> rather than pretended: produce policy slot 11 (`FeasibilityConstraints`) at
> the centre, carry a `withdrawn_venues` set on it, and refuse an intent
> bound for a withdrawn venue in `qip_edge::feasibility::assess` under a new
> `GATE_*` literal — at which point the withdrawal reaches an installed desk
> on its next pass rather than its next install. Until then §12.3's fourth
> row is `PARTIAL`, and `docs/DELIVERY-STATUS.md` says so in the same words.

### Amendment: the edge limit is closed

Slot 11 has a producer. `CentralPlane::feasibility_constraints`, reached
through `Platform::feasibility_constraints` and assigned at the shipping seam
in `qip-api`'s `pending_policy`, ships `FeasibilityConstraints` on every
payload to every cell, carrying `withdrawn_venues` and **three empty grid
maps**. `qip_edge::feasibility::assess` refuses an intent whose venue is in
that set under `qip_contracts::feasibility::GATE_WITHDRAWN_VENUE`
(`feasibility_withdrawn_venue`), before every rule that asks a question about
the order, because this one asks nothing about the order. A desk a cell
installed before the withdrawal therefore stops reaching the venue on its
**next pass**;
`qip-edge/tests/arbitrage.rs::a_desk_installed_before_a_withdrawal_stops_trading_the_withdrawn_venue_on_its_next_pass`
drives exactly that sequence — three legs placed, a second install refused,
the withdrawal applied, no leg placed, and the desk's graph still holding its
three edges afterwards, so what stopped the legs is the pass-time gate and not
a teardown.

Four things about the shape are decisions rather than details.

- **The grids stay empty, and that is asserted rather than merely intended.**
  `central::whitelist`'s register refuses them because the centre's grids are
  keyed by instrument and the slot is keyed by venue, and
  `qip_edge::feasibility::effective` takes a slot grid in *preference* to the
  cell's own — so a re-keyed grid would not sit beside the right number, it
  would replace it for every instrument at that venue. The register's
  paragraph is amended in place, not deleted, for the same reason this
  section is. `the_slot_the_centre_ships_carries_the_withdrawn_set_it_applies_and_states_no_grid`
  fails if a producer ever begins filling one.
- **What ships is what is applied.** The producer reads `withdrawn_venues`,
  the one field `Platform::withdraw_venue` writes *after* the
  `venue.withdrawn` record is in the log, and the same field
  `cycle_whitelist_for` retains against. The set a cell refuses on and the set
  the whitelist omits on cannot disagree, and a withdrawal the log does not
  hold reaches neither. Nothing new is journaled here, because there is
  nothing new: a second record of one fact is the second source of truth
  ADR 0016 refuses.
- **The slot is produced even when nothing is withdrawn.** An empty set is a
  statement — the centre is applying no withdrawal — and a cell that could not
  tell that from a centre that had stopped speaking would have to guess.
  Producing it widens nothing: `PolicyItem::capability` maps slot 11 to no
  §6.2 capability, so it moves no sizing multiplier and lifts no pause, unlike
  the three slots `central::whitelist` refuses on precisely that ground.
- **The new gate is vocabulary the centre admits and never evidence** —
  **first half stands; second half superseded 2026-09-14 by Amendment C.**
  `GATE_WITHDRAWN_VENUE` joins `EDGE_GATES`, so `attribute_refusals`
  recognises it and a refusal under it is counted on
  `qip_feasibility_refusals_total{venue,constraint}` under its real venue and
  its real gate rather than under `other`, which is the label that means a
  cell used a name this build does not know. But it was **kept out of the
  window** — decided by the gate string alone, on a predicate over `gate`,
  and carried on a third return vector on `CellIngestion`. This is not
  tidiness. A desk
  installed before a withdrawal reports one such refusal per intent per pass
  for as long as it keeps offering cycles through the venue; admitted to a
  256-entry rate window those echoes evict every genuine refusal within a few
  passes and then hold the denominator every other venue's share is measured
  against, so no second venue could ever reach three in four and no second
  withdrawal could ever happen. That is a control that reads as protection and
  cannot fire — this repository's `MaxExpectedShortfall` template for what not
  to ship — reached by closing the edge limit carelessly. The desk seam has
  the same shape by a different route: a withdrawn venue refuses there under
  `RefusalReason::VenueUnavailable`, which is not a feasibility gate and never
  reached the window either. `a_refusal_a_withdrawal_itself_caused_is_counted_and_never_lands_in_the_window`
  withdrew one venue, sent twenty-four echoes, and then proved a second,
  genuine cluster still withdraws its venue.

  *(2026-09-14, Amendment C. The reasoning above — that an echo admitted
  whole evicts the window it came from and leaves a control that cannot fire
  — is right, and it stands. What it got wrong is the remedy. Total exclusion
  took the withdrawn venue out of the denominator its own runner-up is judged
  against, which is the opposite cascade in the same arithmetic, and on an
  edge-only fleet it took it out the instant the venue was withdrawn. An echo
  the centre's own set confirms now takes one window seat per venue per
  report and is weighed no higher than the genuine evidence that venue still
  holds, so it can sustain a denominator and can never evict one. And whether
  a refusal is an echo at all is no longer read off the gate string a cell
  sent — `is_withdrawal_echo(gate, venue, withdrawn)` asks the centre, so a
  cell citing a withdrawal the centre does not hold is ordinary evidence. The
  test named just above was replaced rather than relaxed, because its claim
  that an echo never reaches the window was the defect and not the guarantee;
  its closing half is kept and made harder to satisfy. Amendment C names the
  replacements.)*

The "no-cascade denominator" section above is unchanged and still governs:
a withdrawn venue's **genuine** later feasibility refusals stay in the window
and in the denominator. Only the echo of the withdrawal itself is excluded,
and the two are different facts — one is an observation about the venue, the
other is this platform's own decision arriving back at it.

### Why a withdrawn set on the wire is safe in the direction it travels

The policy payload crosses the cell↔centre mesh, and **that wire
authenticates nobody** — `qip-api/src/mesh.rs` and `qip-edge/src/mesh.rs` say
so in their own module docs, and the security review recorded in
"corroboration across cells" above found a single cell could force a
platform-wide withdrawal before `b92f2aa` required a plurality of distinct
cells. So the question has to be asked of every new field on that wire, and
it is answered here rather than left for a reader to derive.

`withdrawn_venues` travels *to* a cell and can only subtract. A forged,
replayed or corrupted payload naming venues in it costs the cell the ability
to trade somewhere it was configured to trade; it cannot make a venue
reachable, because there is no field on the slot and no branch in
`qip_edge::feasibility` by which a payload could add one. The direction
matters and the asymmetry is the whole argument: the *evidence* direction
(cell → centre, a refusal that could withdraw a venue for everyone) is the
dangerous one and is what corroboration guards; the *decision* direction
(centre → cell, a name to refuse) fails closed by construction.

Three structural guards make adding impossible and all three still hold after
this change, none of them touched by it:

1. `qip-edge-node`'s `graph_from_whitelist` checks every conversion against
   `QIP_VENUES`, the node's own configuration.
2. `Cell::install_arbitrage` checks every graph edge against
   `self.config.venues`, the cell's own configuration — asserted with a
   payload applied by
   `a_policy_payload_cannot_make_a_venue_this_cell_is_not_configured_for_reachable`,
   which was mutation-verified against removing that check.
3. `ArbitragePolicy::whitelist_for` only ever emits conversions the grant's
   `envelope.permits_venue` admits.

The residual is availability, not capital: a cell that has never received
*any* payload naming the withdrawal keeps its graph, exactly as it keeps a
whitelist it never received. That is ADR 0008's "cells decide alone" and it
fails in the direction this platform chooses — a partitioned cell keeps
working inside an envelope, and the centre's ability to reach it is what is
missing, not a boundary.

### What is still not built

Reinstatement still has no `qip-api` route: the way back is two operator
signatures through `Platform::reinstate_venue`, and no HTTP surface exposes
it. That is unchanged by this amendment and remains follow-on work. It is
about the way back rather than the withdrawal, so it does not hold R4 short;
it does mean an operator cannot today undo a withdrawal without a process
that calls the kernel directly, which is worth an operator knowing before the
first cluster fires.

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

**Amendment C below reconciles this section with the edge seam.** The
parenthetical above is about the *desk*, and it stayed true. It was never
true of a cell once the amendment to "The edge limit" landed, and this
section did not notice.

## What it costs

- One more refusal step on every desk order, a set lookup on a string.
- One `Vec` on `WorkReport` at the cell, which crossed clippy's variant-size
  bar in `qip-edge-node`'s `PassOutcome`; the report is boxed there.
- A new shared module, `qip_contracts::feasibility`, holding the eight gate
  literals once — nine since the 2026-09-14 amendment, and the ninth was the
  only one excluded from the window outright. Amendment C ended that
  exclusion: it is now the only one whose weight in the window the centre
  decides rather than the cell, through `is_withdrawal_echo`.
  `qip_edge::feasibility` aliases them; the desk's
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
  *(2026-09-14: closed with the set, and the two assertions that would catch
  a regression to the hollow form are
  `the_slot_the_centre_ships_carries_the_withdrawn_set_it_applies_and_states_no_grid`
  at the centre and
  `a_desk_installed_before_a_withdrawal_stops_trading_the_withdrawn_venue_on_its_next_pass`
  at a cell. The second is the one that matters: a slot that exists and
  changes no pass is the failure this bullet names.)*
- **A tenth `EDGE_GATES` member that the window weighs differently.** When
  this bullet was written exactly one gate was excluded from the window, by a
  single comparison on the gate string, and the tripwire was a second
  exclusion. Amendment C ended the exclusion, so the tripwire has moved
  rather than gone: the ninth gate now enters the window as a seat capped by
  the venue's own surviving evidence when the centre's set confirms it, and
  as ordinary evidence when it does not, which restores the window's contents
  to "every feasibility refusal the centre could attribute" — the meaning the
  bars in this record are calibrated on. A second gate weighed as anything
  other than one refusal, whether excluded or capped, breaks that calibration
  again. Add one only with an amendment here saying what the window now
  measures.
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
- Follow-on work: ~~a `qip-api` route for reinstatement~~; ~~slot 11 with a
  withdrawn set, for the edge limit~~; and ~~a structured `Infeasible { venue,
  gate, detail }` refusal reason in place of the `Malformed` prefix, which
  this lane left alone because ADR 0061's lane was editing `RefusalReason`
  concurrently~~. **All three are done, 2026-09-14** — the edge limit in the
  amendment immediately below, the other two in Amendments A and B. They were
  built in three parallel lanes on one afternoon, which is why the strikings
  arrived separately; the bullet is struck once here rather than three times.
- **Amended 2026-09-14.** §12.3's fourth row moves `PARTIAL → REACHED` on
  the withdrawal mechanism: slot 11 is produced, the cells refuse a withdrawn
  venue at pass time, and an installed desk no longer outlives a withdrawal.
  `EDGE_GATES` is nine rather than eight, and the ninth is deliberately not
  admitted to the window. `docs/DELIVERY-STATUS.md` carries the same words.

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


## Amendment C — the denominator at the edge seam, and slot 11's wire shape (2026-09-14)

An independent security review of the merged edge closure found that
"The no-cascade denominator" above had become half true, and that slot 11 had
become a breaking wire change that documented itself as a careful one. Both
are the same kind of defect: a guarantee stated in prose and held by the code
on only one of the paths the prose claims it for.

**The denominator.** That section reasons that a withdrawn venue's later
refusals keep landing in the window, "the desk's feasibility gate runs before
the withdrawal check, so the window's denominator stays honest". At the desk
that is still exactly what happens — `OrderManager::submit` runs feasibility
at step 2 and the withdrawn-venue check at step 5. At a cell there is no such
ordering: the amendment above put the withdrawn-venue refusal at the *top* of
`qip_edge::feasibility::assess`, so after a withdrawal every refusal at that
venue arrives under `feasibility_withdrawn_venue`, and the centre routed all
of them to a vector that never reached the window. On an edge-only fleet —
the designed state, since `execution_nodes = {}` and the desk contributes
nothing — a withdrawn venue therefore left the denominator the instant it was
withdrawn, the runner-up became a cluster of what remained, and the platform
could withdraw its way down to no venue at all. The unit test that names the
property kept passing throughout, because it is arithmetic over a synthetic
window and what changed was upstream of it.

Two pressures point opposite ways here and the fix has to satisfy both. An
echo must not **evict** genuine refusals from a 256-entry window: a stale desk
reports one per intent per pass, and admitted whole they would clear the
window in a few passes and then hold the denominator, so no second venue could
ever reach three in four — a control that reads as protection and cannot fire.
And a withdrawn venue must not **vanish** from the denominator, which is the
finding above. So:

* the centre seats **one** echo per venue per report — a report asserts one
  fact, "this cell is still routing to a venue you withdrew", and repeating it
  once per intent measures how many cycles a stale desk enumerated, not
  anything about the venue. Every repeat is still counted on
  `qip_feasibility_refusals_total{venue,constraint}`, so the series says how
  hard the withdrawal is biting;
* `venue_review::VenueTally::weight` caps what those seats are worth: an echo
  may **sustain** a withdrawn venue's weight up to the genuine evidence that
  venue still holds in the window, and never beyond. The platform's own
  decision can keep a venue in the denominator for as long as the platform is
  still attempting it, and can never amplify it past what the venue earned.
  The cap is a `min` against the venue's own surviving refusals rather than a
  constant, so it decays with the evidence it is anchored to instead of being
  a floor nobody could check; when the venue's own refusals have aged out, its
  echoes weigh nothing.

A withdrawn venue is never a *candidate* under any of this — the filter is
the withdrawn set — so nothing an echo does can withdraw a venue twice or feed
a decision back into its own evidence. **That last clause was false when it
was written; see Amendment D.** The filter holds only while the venue is
withdrawn, and the classification it depends on was re-derived at read time,
so a reinstatement turned every echo into evidence against the venue in the
same step that made it a candidate again. The guarantee is real now and is
held by where the classification is written, not by this paragraph.

**Whose decision an echo is, is the centre's.** The classification took the
gate string off the report, and the cell→centre wire authenticates nobody. No
attacker is needed to exploit that: `Cell::feasibility_constraints` reads slot
11 whatever its freshness and `self.policy` is replaced only when a new policy
is applied, so a cell holding a stale slot after a reinstatement — or after
the centre simply stopped shipping policy — refuses every intent at a venue
*currently in use* under `feasibility_withdrawn_venue`, indefinitely. The
centre charted a withdrawal that no longer existed, and none of those refusals
reached the window, so the venue could not be withdrawn a second time on edge
evidence for as long as the slot stayed stale. `is_withdrawal_echo(gate,
venue, withdrawn)` replaces `is_withdrawal_evidence(gate)`: the centre's own
`withdrawn_venues` decides, and a cell citing a withdrawal the centre does not
hold is an ordinary refusal at a venue in use, evidence like any other.

The cardinality bound on `qip_feasibility_refusals_total` is unchanged in both
directions: `venue` is still bounded by the desk broker's name, the configured
and granted venue list and `unknown`, and `constraint` by the nine gate
literals and `other`. The recording sites are unchanged in number and place.
What moved is which of `Platform::ingest_cell_report`'s two loops a given
refusal arrives on, and both loops count the same labels.

**Slot 11's wire shape.** `FeasibilityConstraints` carries
`deny_unknown_fields` and `PolicyPayload` carries no schema version, and
`withdrawn_venues` arrived required and always serialised. That is a breaking
change to a signed cross-process contract, and the dangerous direction is not
the obvious one: a *new* centre shipping the field to a cell built before it
fails the **entire** payload, all twelve slots, on one added key — and
`qip-api` (Cloud Run) and `qip-edge-node` (Compute Engine) deploy separately,
so a rolling upgrade has a window in which every cell degrades every
capability. Nothing is deployed, so this costs nothing today; it was a
guarantee weakened without saying so, which is the part that had to be fixed
whatever it costs.

The field is now `#[serde(default, skip_serializing_if = "BTreeSet::is_empty")]`,
which is the shape `CycleWhitelist::conversions` had already chosen in the
same file, for the same reason, and which the sibling change in this merge
(`RefusalReason::Infeasible`) documented and this one did not. What that buys,
and what it deliberately does not:

* an old centre's slot 11 decodes at a new cell with an empty withdrawn set,
  which is not a guess — a centre that predates this record could not have
  withdrawn a venue on feasibility evidence;
* a new centre with nothing withdrawn produces byte-for-byte the pre-field
  encoding, so an old cell accepts it and the slot digest, and the signature
  over it, are the ones a pre-field payload carried;
* once something **is** withdrawn the field is present and an old cell refuses
  the whole payload. That is left fail-closed rather than papered over: the
  cell applies no new policy, narrows on staleness per §6.2, and keeps the
  last payload it applied, which is safer than applying eleven slots and
  silently keeping a venue the centre stopped using. **Cells upgrade before
  the centre**, and that is the deploy-order constraint this slot imposes.

Defaulting to an empty set is not fail-open. It removes nothing and it cannot
add anything: there is no field on `FeasibilityConstraints` by which a payload
could make a venue reachable, the cell's venue list is its own configuration's
business, and a withdrawal still reaches a cell by the `CycleWhitelist` the
centre already omits the venue from — which is where this record started.

Evidence: `slot_elevens_withdrawn_set_is_additive_on_the_wire_in_both_directions`
(`qip-contracts/tests/contracts.rs`),
`a_venue_withdrawn_on_edge_evidence_stays_in_the_denominator_the_runner_up_is_judged_against`,
`a_repeated_echo_of_one_withdrawal_is_counted_in_full_and_seated_once` and
`a_cell_citing_a_withdrawal_the_centre_does_not_hold_is_evidence_and_not_an_echo`
(`qip-kernel/tests/central.rs`), and three unit tests beside `assess`.

## Amendment D — a reinstatement now survives the cycle after it (2026-09-14)

**Two operators sign a venue back into use and the next LEARN pass withdraws
it again.** That was true of every build this record has ever described, and
Amendment C made it worse rather than causing it. A control two people
exercise and the machine reverts by itself is protection in the reading and
nothing in the fact — the `MaxExpectedShortfall` shape this repository keeps
as its standing example of what not to ship — and this record shipped one.

It was reproduced before anything was changed, on a platform with twenty-four
corroborated lot refusals at one venue and eight at another:

```
PROBE while withdrawn:     None
PROBE after reinstatement: Some(VenueCluster { venue: "XNYS",
    constraint: "feasibility_lot", sample: 38, count: 30, share: 0.789 })
PROBE withdrawn after next cycle: {"XNYS"}
```

and, with thirty seated echoes rather than six, the record that second
withdrawal wrote:

```
PROBE withdrawal record: venue=XNYS constraint=feasibility_withdrawn_venue
    count=54 sample=62
```

An audit line saying the venue was withdrawn because it had been withdrawn.
Amendment C asserts that "nothing an echo does can withdraw a venue twice or
feed a decision back into its own evidence". **That sentence was false when it
was written**, and it is corrected below rather than deleted, because the
reasoning around it is right and only the place the guarantee was held was
wrong: it was held by prose about `assess`, and `assess` re-derived the
classification every time it ran.

### Two defects, and either alone leaves the control unable to do what it says

**Non-stationarity.** Whether a stored window entry was an echo was decided at
*read* time, by `is_withdrawal_echo(gate, venue, withdrawn)` against the
withdrawn set as it stood at that call. Amendment C introduced that function
for a good reason and put it in the wrong place. The moment a reinstatement
removed the venue from the set, every seat it had taken as an echo while
withdrawn was promoted — in one step, with no record changing — from a
weight-capped denominator entry into a full numerator refusal *against* it,
at the same instant the venue became a candidate again. The case against a
reinstated venue therefore grew with how long it had stayed out.

The classification is now taken once, where it is known, and stored:
`venue_review::RefusalStanding` is `Evidence`, `Echo` or `Pardoned`, written
by `CentralPlane::attribute_refusals` and by the desk's recording site, and
read by `assess` without re-derivation. One enum rather than a pair of flags,
because "an echo that is also pardoned" is not a thing this platform can mean.

**Reinstatement never reached the evidence.** `reinstate_venue_at_seams`
removed the venue from the withdrawn set and from the two seams and left the
feasibility window exactly as it found it, so the cluster that withdrew the
venue was still standing and re-fired immediately. This half is older than
Amendment C and independent of echoes: with no seated echoes at all the
baseline probe read `count=24 sample=32 share=0.75`, which clears
`VENUE_WITHDRAWAL_SHARE` on its own.

Two people deciding a venue should come back is precisely a decision about the
evidence that removed it, so the signatures now reach the window through
`venue_review::pardon`. **They mark, and do not delete, and the three
candidate designs are worth recording because each of the other two is wrong
in a way that reads as right.**

* *Delete this venue's entries.* Shrinks the denominator every **other**
  venue's share is measured against while leaving their numerators alone — the
  runner-up becomes a cluster of what remains. That is the cascade "The
  no-cascade denominator" exists to refuse, arriving by a new door.
* *Delete the whole window.* Unbiased, but over-reaching: two people signed
  for *this* venue, and evidence about venues they did not name is not theirs
  to clear. A reinstatement would become a way to delay another venue's
  withdrawal, which nobody signed for.
* *Mark, and keep weighing the marked entries on the venue's own share.* Turns
  a pardon into a shield: the ten refusals that withdrew a venue would sit in
  its own denominator, so thirty fresh ones would be needed before it could go
  again. The signatures would have raised the bar on a control rather than
  reset it.

So a pardoned refusal holds the denominator at full rate for every other
venue, and is excluded from the numerator, the corroboration and the modal
constraint of the venue it names — and from that venue's own denominator.
`assess` judges a candidate on every other venue's weight in full plus its own
binding refusals. A reinstated venue starts from the same bar any venue starts
from, and a venue nobody signed for sees no change at all.

### Two further holes, found while proving the fix

**One report could be the whole window.** `VENUE_WITHDRAWAL_MIN_CELLS`
requires two distinct cells before edge-only evidence withdraws anything, and
it counts distinct cell *names* rather than evidence per cell. Thirty refusals
in a single report plus one token refusal from a second name cleared it — a
probe withdrew a venue for the entire platform in two messages, on a wire
`qip-edge/src/mesh.rs` says authenticates nobody. Amendment C's one-seat-per-
report rule already existed and was applied to `GATE_WITHDRAWN_VENUE` alone;
its argument — "a report asserts one fact, and repeating it once per intent
measures how many cycles a stale desk enumerated" — was never particular to
that gate, and the eight gates it did not cover are the ones an attacker would
use. It is now one seat per venue **per gate** per report, so a report's seats
are bounded by the gate vocabulary times the configured venue list and by
nothing a sender chooses. Every repeat is still counted on
`qip_feasibility_refusals_total`.

**A reinstatement makes every cell stale about that venue by construction.** A
cell learns on its next policy frame, not on the signature, so until the frame
arrives every intent there comes back under `feasibility_withdrawn_venue`.
Those are not echoes — the centre no longer holds the withdrawal — and under
Amendment C's rule they are ordinary evidence, which withdrew the venue again
at once. A probe re-withdrew a just-reinstated venue on 256 window entries of
which not one was a genuine refusal. Left there, **no reinstatement could ever
have stuck while any two cells were behind**, and the withdrawal that followed
made the cells' stale belief true.

The centre now remembers what it reinstated (`CentralPlane::reinstated_venues`,
cleared when the venue is withdrawn again) and seats such a refusal as
`Pardoned`. This does not touch Amendment C's security finding, which stands:
a cell asserting a withdrawal the centre has **never** made is still making an
ordinary refusal at a venue in use, and is still evidence. The distinction the
centre can draw, and the cell cannot, is between a cell that is behind on a
decision the centre made and a cell inventing one.

**And nothing that cannot withdraw a venue may displace something that can.**
An echo or a pardoned refusal arriving at a full window is counted on the
series and seated nowhere. Otherwise a cell reporting at a venue nobody is
judging pushes out the evidence every other venue's withdrawal rests on, until
fewer than `VENUE_WITHDRAWAL_MIN_SAMPLE` genuine entries survive and no venue
can be withdrawn at all — the same dead control reached by arithmetic instead
of by anybody's decision.

### What this does not change

The paper-trading boundary is untouched at all three layers. A reinstatement
still only removes a name from a subtractive set; `FeasibilityConstraints`
gains no permitted-venues field and no field by which a payload could make a
venue reachable; `pardon` writes to an in-process evidence window and to
nothing else. "Why evidence can never add a venue" stands unchanged, and so
does the cardinality bound on `qip_feasibility_refusals_total`: `venue` by the
desk broker's name, the configured and granted venue list and `unknown`, and
`constraint` by the nine gate literals and `other`.

The window remains per-process and is not resumed from the log, so the pardon
at assembly is a no-op — a restarted process has no window to pardon, which is
why the two callers of `reinstate_venue_at_seams` are kept on one path.

### What would make this wrong

If a venue's grid is genuinely broken and two operators reinstate it without
fixing it, the venue now needs a fresh cluster — ten refusals at three in four
— before it is withdrawn again, where before it was withdrawn on the next
pass. That is the intended cost and it is the runbook's existing advice made
true: "reinstating without changing either buys ten more refusals and the same
withdrawal on the next review" was a description of behaviour the platform did
not have until this amendment.

If a fleet is large enough that one cell's pass carries many venues, the
per-gate seat cap makes a genuine multi-venue problem slower to establish. The
bound is the vocabulary and not the sender, so it is slower and never blind.

### Evidence

`qip-kernel/tests/learning.rs::reinstatement_needs_two_different_fresh_operators_and_is_journaled_at_each_signature`
(its final assertion reversed, and the old one named as wrong in the commit
that reversed it), `qip-kernel/tests/central.rs::one_report_cannot_be_the_whole_window_however_many_refusals_it_carries`,
`::a_cell_that_has_not_heard_a_reinstatement_cannot_undo_it`,
`::a_refusal_that_cannot_withdraw_a_venue_never_evicts_one_that_can`, and four
unit tests beside `assess`:
`a_pardoned_refusal_holds_the_denominator_for_every_venue_but_the_one_it_names`,
`a_pardoned_refusal_never_names_the_constraint_a_withdrawal_record_cites`,
`a_pardoned_venues_cluster_still_needs_two_cells_to_corroborate_it` and
`an_echo_stays_an_echo_after_the_venue_leaves_the_withdrawn_set`. Every one was
mutation-verified and each mutation fired for its own reason.

### Three documents this amendment falsifies

Not edited here, because they are outside this lane's territory, and named so
that the next reader does not take them for current:

* `venue_review::assess`'s doc comment — corrected in this change.
* `.claude/rules/domains/observability.md`, whose third bullet on the ninth
  gate literal says a seated echo "can never withdraw anything". True for an
  entry stored as an echo, and it was the re-derivation that made it false;
  the bullet should name where the classification is taken.
* `docs/operations/reinstating-a-venue.md`, which tells an operator "an echo
  cannot cause a withdrawal and cannot hide one" — in the document they are
  reading *while performing the reinstatement that triggered it*. It should
  also gain the fact that a reinstatement now sets the venue's accumulated
  refusals aside, and that a cell which has not yet heard the reinstatement
  refuses at the venue without that counting against it.
