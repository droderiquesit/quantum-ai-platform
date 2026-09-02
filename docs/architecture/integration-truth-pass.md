# Integration and truth pass — seven flows traced through the code

Phase 4 of the Algorik architecture programme. Every claim below was derived by
reading the path named in it. **Where a link could not be traced it is recorded
as untraceable rather than assumed**, and where a flow breaks the seam is named.

Scored against the blueprint (ADR 0022) on branch
`claude/algorik-architecture-refactor-pmp0zy`. Flows 2, 3, 6 and 7 were
re-traced at `296e187` where their seams changed — the feasibility gate, the
arbitrage scanner, central attribution, belief calibration and the trial
book — and every `path:line` in a re-traced row is at that commit. Rows and
sections the document marks "as originally found" are history and keep
their original line numbers. Flow 6 and one row of flow 7 were re-traced
again at `584c96b`, where the second halt wire (`ff86473`), the crossing
interval (`153e429`) and the desk's installation in the node (`584c96b`)
landed; every `path:line` in a row marked `584c96b` is at that commit.
Flows 2, 3, 6 and 7 were re-traced once more at `e04815e`, where the node's
pass loop (`6340610`), fills as venue facts (`cb79b46`), the stated pricing
policy (`383d4e7`), the produced whitelist (`5396679`, `91d20f5`) and the
third halt-release direction (`6a515bb`) landed; every `path:line` in a row
marked `e04815e` is at that commit.

**Status vocabulary.** MEASURED (runtime evidence exists) · TESTED (a named
passing test) · CONFIGURED (wired in a manifest or tfvars) ·
IMPLEMENTED-UNVERIFIED (code exists, no deployable composes it) · PLANNED
(backlogged with a phase) · MISSING.

---

## Flow 1 — public signup/passkey → mandate and entitlements → empty account

**Verdict: BREAKS at the first seam after authentication. The platform has no
concept of a customer account.**

| Link | Status | Evidence |
|---|---|---|
| Sign-up page | TESTED | `frontend/portal/src/app/(auth)/sign-up/page.tsx`; siblings for sign-in, reset, agreements, account-locked |
| Browser → identity | IMPLEMENTED-UNVERIFIED | `(auth)/_lib/api.ts` posts to Next.js `/api/auth/*` with CSRF priming. ADR 0019 makes Identity Platform the only identity store |
| Passkey | MISSING | `grep -rn passkey backend/crates` returns nothing. Blueprint Phase 0 names passkeys; nothing implements them |
| → mandate | MISSING | No customer mandate type. `Mandate` in `runtime/qip-kernel/src/config.rs` and `services/qip-portfolio-engine/src/construction.rs` is a *portfolio* mandate — risk limits for the desk's own book, not a customer agreement |
| → entitlements | MISSING (name collision) | `qip_contracts::governance::Entitlement` (`governance.rs:83-94`) is `Granted{dataset, usage, expires_at}` / `Denied{...}` — **dataset licensing**, not a customer's product rights. Blueprint §40.13 means the latter. Same word, different concept; do not conflate |
| → empty account | MISSING | No account creation anywhere. `qip-api` exposes 30 endpoints (`routes.rs:73-299`) and not one is per-user: `/portfolio`, `/orders`, `/capital`, `/risk` are all desk-wide singletons |

**The seam.** Authentication terminates at Identity Platform and nothing carries
a subject into the Rust platform as an account holder. `qip-api`'s `auth.rs`
resolves a `Principal` with a `Role` (`auth.rs:48-81`) — operator RBAC, not
customer identity.

This is consistent with `CLAUDE.md` ("intended users are the research and risk
desk — not external customers") and **contradicts the blueprint**, which
specifies an investor portal over a per-user ledger. Now that the blueprint is
the architecture of record, this is a real gap rather than a scoping choice.
Blueprint Phase 13. Backlogged; nothing built here.

---

## Flow 2 — the spine

filing/source → facts → entity resolution → world event → belief → strategy
intent → regional execution → fill → ledger → explanation

**Verdict: traceable end to end with two substitutions and one genuine break.**
The loop runs and closes; it is not the blueprint's loop at two points.

| Link | Status | Evidence |
|---|---|---|
| source → facts | TESTED | `platform.rs:1380 observe()`; news at `:1129-1138` → `WorldModel::absorb_news` + `MarketEvent::from_news`; fundamentals `:1140-1153`; macro `:1154-1158`; corporate actions `:1159-1180`; alt `:1181-1203`; reference `:1204-1243`. `absorption.rs` covers most arms |
| live source | PARTIAL — exercised once, not yet sustained | Two paths now: the vendor path (`feed.rs` `Live` arm, needs a keyed aggregator) and the connector path (`Feed::Connector` → `connector_feed.rs`), the latter **exercised against the real Coinbase endpoint in-session** through the full runtime with the licensing catalogue evaluated first. Sustained deployment streaming still unobserved |
| → entity resolution | TESTED | `stage_understand` (`platform.rs:2430`), `qip-entity-resolution` |
| → world event | TESTED | `qip-world-model/src/world.rs`; causal graph is real — `world.rs:41 causal: CausalGraph`, `:192 claim_causal`, `:495` shock propagation, `causal.rs:234` |
| → **belief** | **SUBSTITUTED — and now graded** | **No belief stage exists.** What runs is `stage_reason` (`platform.rs:2808`) producing *theses*, with Bayesian machinery in `qip-reasoning-engine/src/bayes.rs`. Confidence-weighted sizing per blueprint §11.2 is not the mechanism here. What changed at `04738ee`: the belief *calibration* the blueprint §47 calls its single most important metric is now a number. `stage_learn` settles each resolved claim against the platform's own series and grades it through `Platform::learn_from` (`platform.rs:3931` → `:4052`; `learn_from` at `:1680`), recomputing a Brier score over a bounded window and writing `qip_belief_brier_score` (`:1708`). `qip-kernel/tests/learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`; mutation fired when the LEARN call was severed. The row stays SUBSTITUTED because grading theses after the fact is not a belief stage before the trade |
| → **strategy intent** | **SPLIT — built at the edge, absent at the centre** | `Intent` exists (`libs/qip-contracts/src/intent.rs`), and with it netting, internal crossing (§27.1) and the contributor vector. `Cell::work` builds one intent per firing strategy, nets them on instrument/venue/representation, crosses the offsetting part at the book mid and places what survives. **This row said all four of those were absent; that was true when it was written and false from the netting slice onward.** What has not changed is the *central* path: `stage_decide` (`platform.rs:3450`) still sizes theses into `Proposal`s with legs and raises no intent, so §27's mechanism operates in one of the two planes. See the seam below, which is the same seam. Re-traced at `296e187`, the cell's intent path has two more seams, both ahead of `net()` in `Cell::work` (`cell.rs:750`): the **arbitrage desk** re-quotes its graph from the cell's own books and scans it (`:851 scan_cycles`), and the **feasibility gate** judges every intent in place (`:860 admit_feasible`) before `net` (`:869`) — minimum quantity, minimum notional, lot, tick, depth at the touch, fee floor, gas floor and a malformed policy constraint, eight gate literals in `feasibility.rs:76-83`, each a refusal and never a rounding. `qip-edge/tests/feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order`, `::a_sell_is_judged_against_the_bid_side_and_a_buy_against_the_ask`; `qip-edge/tests/arbitrage.rs::a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass`, `::an_infeasible_leg_vetoes_the_whole_cycle_and_no_leg_goes_out` (`95a4932`, `71f9465`). Direction is pinned too, after two compensating sign inversions were found cancelling at the gateway and nowhere else: `qip-edge/tests/direction.rs::an_enter_signal_leaves_the_cell_taking_the_ask_which_is_a_buy` and `qip-edge-node/tests/gateway.rs::an_enter_signal_is_a_buy_at_the_matching_engine_filling_against_offers_and_resting_against_bids` (`54d32fd`). The honest limit, as it read until `e04815e`: `qip-edge-node` called `Cell::work` on no path and installed no desk, so every seam above was proven by the cell's tests and reached by no deployed process. Re-traced at `e04815e`: that grep is no longer empty — `run_pass` (`qip-edge-node/src/pass.rs:84`) calls `cell.work` (`:118`) from the loop (`main.rs:586`) whenever `QIP_VENUE_FEED=simulated` (`feed.rs:79-82`; any other value refused at start naming ADR 0003, `:118-131`; `6340610`), and the installer's whitelist has a producer (flow 3). Every seam above is now in the deployed binary's loop: TESTED by `qip-edge-node/tests/pass.rs::a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move` and `::a_pass_with_nothing_listed_at_the_venue_refuses_under_the_venue_selection_gate`, MEASURED by nothing, because no node is deployed |
| → **regional execution** | **BREAKS** | `stage_act` (`platform.rs:3545`) runs the central risk monitor and places through the **central** OMS. It does not reach a cell. See the seam below |
| → fill | TESTED | Orders reach the simulated broker via `qip-execution-engine/src/oms.rs`; multi-leg in `multileg.rs`. Re-traced at `e04815e`, the cell's half: a fill is a venue fact (`cb79b46`) — what `Placer::execution_reports` returns (`cell.rs:3594`) is the only thing `Cell::confirm_execution_reports` (`:2073`) books as `Decision::Filled` (`:2160`), counted on `qip_edge_fills_confirmed_total`; an accepted order is an open order until the venue says otherwise, and the node confirms on its next pass what the venue filled between passes (`qip-edge/tests/fills.rs::an_order_the_venue_accepted_is_not_a_fill_until_the_order_entry_channel_reports_one`; `qip-edge-node/tests/pass.rs::a_resting_order_the_venue_fills_on_a_later_pass_is_confirmed_and_the_node_keeps_trading`, `b8d18d3`). How the order is priced is the strategy's stated `PricingPolicy` (`383d4e7`; `cell.rs:347-365`): `Marketable` takes the touch; `RestAtMid` rests and is withdrawn through `Placer::cancel` (`:3609`) when its time to live elapses, on `qip_edge_orders_expired_total`; an intent whose strategy stated no policy is refused under `pricing` before anything is placed (`:1442-1450`; `pricing.rs::an_intent_with_no_stated_pricing_is_refused_and_nothing_reaches_the_venue`). Re-traced at `5290bb9`, the centre's half: the fill the cell confirmed crosses the wire as its own record — `CellStateDelta::fills` (`qip-edge/src/mesh.rs:214`, built from `WorkReport::fills` at `cell.rs:3253`, never from its order list; `FillRecord` at `qip-contracts/src/wire.rs:93`, schema version 4 at `:148`) — decoded as its own half of the interval (`qip-mesh/src/delta.rs:195`), carried onto the report by the API sink (`qip-api/src/mesh.rs:1209`) and booked by `settle` from `report.fills` (`central/plane.rs:1191`). The round trip is `qip-edge/tests/mesh.rs::a_state_delta_a_cell_produced_arrives_at_the_centre_unchanged` (`:420`) and `acceptance.rs::the_centre_decodes_a_contributor_vector_out_of_bytes_the_edge_crate_produced` (`:648`) |
| → ledger | PARTIAL / naming | **There is no `Ledger` type.** `grep "pub struct Ledger\b"` finds nothing — the ledgers that exist (`PrivacyLedger`, `BudgetLedger`, `ReservationLedger`, `BridgeLedger`, `LifecycleLedger`) are each something narrower than money state. Money state is `qip-capital` (allocation, envelope, exposure) plus the hash-chained event log. Blueprint §43.3's per-user, per-strategy authoritative ledger does not exist. What arrived at `7ef6063`/`7d79161` is the per-cell, per-strategy, per-instrument *book* the centre keeps from a cell's report: `CentralPlane::ingest` (`central/plane.rs:855`) settles the interval's orders and crosses (`:976 settle`) before the halt step, a fill pro rata to the contributors on its own side through `split_pro_rata` (`:1021`; `qip-learning-engine/src/attribution.rs:266`, largest-remainder, asserts the shares sum), a cross moving buyer up and seller down at the recorded mid, and a decomposition that must close to the last unit or count `qip_central_attribution_failures_total` (`:1126`). `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual`, `::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact`, `::a_cross_naming_two_buyers_is_refused_rather_than_split_evenly`; and the API sink now carries the interval (`qip-api/src/mesh.rs:1148-1149`), `qip-api/tests/mesh.rs::the_orders_a_cell_reports_reach_the_centres_strategy_books` (since `d59505d` named `the_fills_a_cell_reports_reach_the_centres_strategy_books`, `:712`), mutation fired with the two builder calls removed. **Re-traced at `e04815e` — a break in what the books are settled from.** `CellInterval::orders` was filled from the pass report's *placed* orders, and `settle` booked each as a fill — so an order the cell rested and the venue never filled, which the cell itself books as nothing (the fill row above), was a position at the centre. Two claims about one fact, and the louder one was wrong. **Closed at `5290bb9`.** `settle` (`central/plane.rs:1161`) walks `report.orders` first and registers each as *sent* in a bounded per-cell register (`SentOrders`, 4,096 per cell, oldest evicted; `:1526`), counted under `qip_central_orders_sent_total` (`:1187`); then walks `report.fills` (`:1191`) and books each as the cell's own shares, refused and counted if they do not sum to the fill (`:1226`), pushed into `Settlement::absorbed` for B20's aggregate charge, and counted under `qip_central_fills_attributed_total{basis="contributor_vector"}` (`:1255`). A fill naming an order the centre never saw sent, or beyond what remains unfilled on one it did, is a `ReconciliationBreak` of origin `BreakOrigin::UnsentFill` (`:1214`), direction `unsent_fill`, merged with the report's own breaks (`:1066`) and halting the cell through the same path — the venue's channel and the platform's record disagree, which is what the cell's own drop copy halts on. A report carrying orders and no fills settles nothing and is not a break, which is also how a delta from before the field existed replays (`qip-mesh/src/delta.rs::a_delta_written_before_fills_existed_decodes_as_having_confirmed_none`, `:478`). `qip-kernel/tests/attribution.rs::a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing` (`:441`), `::a_fill_on_an_order_the_centre_never_saw_sent_halts_the_cell_and_books_nothing` (`:499`), `::a_fill_beyond_the_quantity_sent_is_the_same_break` (`:567`), `::a_fill_whose_shares_do_not_sum_to_it_is_refused_rather_than_booked_short` (`:635`); `tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate` (`:506`), `::the_same_order_filled_in_the_next_report_charges_exactly_the_fill` (`:562`); `qip-api/tests/mesh.rs::an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing` (`:760`). One remainder, in flight: `qip-acceptance/tests/e2e_live.rs::report_from` (`:638`), a test-only helper, still builds positions from `delta.orders`; a test engineer is correcting it as this row is written |
| → explanation | PARTIAL | Attribution is exact and reconciles (`qip-learning-engine/src/attribution.rs:152 reconciles()`, `:199 by_hypothesis()`), the cost router records a `rationale`, and since `b9e2242` every refused order is priced once the world has said what refusing it cost: `stage_learn` → `score_declined` (`platform.rs:3948`, `:4968`) → `Platform::evaluate_alternatives` (`:5028`, previously called only by tests), eight paths per cycle with the excess deferred and counted, scored to the gate that refused under `qip_counterfactuals_scored_total{gate}` (`:5040`). `learning.rs::a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate`, `::declined_paths_past_the_per_cycle_cap_are_counted_as_deferred_and_priced_next_cycle`. Blueprint §40.2 explanation — what the system believed and why — still has no surface; the cycle overview the console reads is recorded at `POST /cycle` (`qip-api/src/routes.rs:615`, `CycleOverview::record`; `qip-api/tests/api.rs::a_cycle_run_through_the_router_reaches_the_operator_interfaces_stage_overview`, `cf20457`), which is the stage table and not an explanation |

**The seam, precisely.** Central and regional execution are two disjoint paths,
not one flow:

- The central plane decides and executes through its own OMS (`stage_act`).
- A cell executes its own deployed strategies under a granted envelope
  (`edge/qip-edge/src/cell.rs work()`).
- What travels centre → cell is **policy, never a decision**: the signed
  capital envelope (`CapitalDownlink::absorb`), and since the payload slice the
  signed twelve-item payload and the halt command (flow 3, flow 6). What
  travels cell → centre is the report — standing, orders, crosses — which the
  centre attributes and settles (the ledger row above).

So a decision made centrally is never executed regionally, and that is still
the shape at `296e187`: the two planes share policy and books, not an order
path. When this seam was first traced the sentence here read "flow 3 is
missing"; flow 3 is now PARTIAL, and the break in flow 2 is the one the
blueprint intends — §4.2 puts execution decisions at the region.

---

## Flow 3 — policy generation → signed twelve-item payload → regional verification and atomic swap → outcome return

**Verdict at the original trace: MISSING, with one genuine precursor. Since
closed to PARTIAL by the payload slice** — `qip_contracts::policy` carries
typed slots for all twelve items, the centre builds, signs and ships one per
cell each cycle (`qip-api/src/mesh.rs::pending_policy`, `::dispatch_policy`),
the cell verifies into `VerifiedPolicy` and applies by one-assignment swap
with sequence discipline (`qip-edge/src/cell.rs::apply_policy`), and §6.2
narrowing runs through `DegradationState` at last. Two of twelve slots have
real producers (the grant manifest and the risk envelope as the enforced
`LimitSet`); the other ten ship unproduced and read as unavailable, which
narrows the cell — fail-closed, not omission. End-to-end over real sockets:
`qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it`.
The table below records the state as originally found.

| Link | Status | Evidence |
|---|---|---|
| Twelve-item payload | MISSING | No type carries the twelve items of §41.5. Nothing to grep for; the concept is absent |
| Signed policy down | **PARTIAL — one item of twelve** | Item 7, "family budgets and capital grants", exists and is genuinely signed: `qip_contracts::capital::CapitalEnvelope::signing_payload` (`capital.rs:128`), `signature()` (`:108`), verified cell-side into `VerifiedEnvelope` (`mesh.rs`) |
| Regional verification | TESTED (for that one item) | `CapitalDownlink::poll`/`absorb` (`mesh.rs:664+`) verifies against the cell's own key, refuses and de-duplicates by grant key |
| **Atomic swap** | MISSING | Envelopes are applied incrementally per grant. No pointer swap, no all-or-nothing application of a payload |
| **Stale-item narrowing (§6.2)** | PLANNED | `qip_contracts::degradation` types the narrowing (added this programme) but **has no production caller** and no payload to attach to |
| Outcome return | TESTED | `CellUplink::publish` (`mesh.rs:418`) sends `CellStateDelta` up — utilisation, halt flag, refusals, orders |

**Re-traced at `296e187`.** Two rows of the table above have moved since the
payload slice, and one slot has gained its first consumer:

| Link | Status now | Evidence |
|---|---|---|
| Stale-item narrowing (§6.2) | TESTED | `Cell::narrowing` (`cell.rs:400`) derives the state from the applied payload every pass and `work()` sizes by it; a payload-less cell sits at the floor. `qip-edge/tests/telemetry.rs::a_policy_going_stale_narrows_the_cell_to_the_floor_and_the_gauges_move_with_it`; the table itself in `qip-contracts/tests/contracts.rs::an_ingestion_stall_pauses_the_strategies_that_need_the_world_and_no_others` and its four siblings; `::an_unproduced_slot_is_stale_from_birth_and_narrows_like_staleness` |
| Slot 11, feasibility constraints — consumed | TESTED at the cell | First consumer: the feasibility gate reads `PolicyPayload::feasibility_constraints` (`qip-contracts/src/policy.rs:361`) and overrides the venue model's tick, minimum and fee floor per venue when the slot is produced; a constraint that is not a grid is refused rather than replaced. `qip-edge/tests/feasibility.rs::the_policy_payloads_feasibility_slot_is_the_constraint_the_cell_judges_by` (`95a4932`) |
| Slot 11 — produced | MISSING | `grep -rn feasibility_constraints apps/qip-api/src runtime/qip-kernel/src` returns nothing: the centre ships the slot unproduced, so the cell judges by its installed `VenueModel` and depth alone. Still two of twelve slots with producers |

**Re-traced at `e04815e`.** A third slot has a producer:

| Link | Status now | Evidence |
|---|---|---|
| Slot 8, cycle whitelist — produced | TESTED | `CentralPlane::cycle_whitelist_for` (`central/plane.rs:612`) derives the whitelist from `CentralConfig::arbitrage` (`:124`) and the desk's live grant — `ArbitragePolicy::whitelist_for` (`central/whitelist.rs:267`), empty with its reason when the policy is unset or the strategy holds no grant at that cell — and `Platform::issue_cycle_whitelist` (`platform.rs:1572`) journals what it issued (`5396679`; `qip-kernel/tests/central.rs::an_unset_arbitrage_policy_emits_an_empty_whitelist_that_says_why`, `::a_signed_payload_carrying_the_whitelist_round_trips_and_verifies`, `::issuing_a_whitelist_through_the_platform_journals_what_was_issued`) |
| Slot 8 — shipped | TESTED | `qip-api`'s `pending_policy` (`mesh.rs:636`) calls `issue_cycle_whitelist` per cell (`:663`) and ships it on the signed payload the cell verifies, from a policy read at `QIP_ARBITRAGE_POLICY_PATH` (`main.rs:374`; unset is no desk, and the cycle response says so) — `91d20f5`, registered as read by the API and unset on Cloud Run at `73a1694`; `qip-api/tests/mesh.rs::a_cycle_ships_the_desk_a_live_grant_funds_as_a_whitelist_the_cell_verifies`, `::without_a_policy_the_whitelist_ships_empty_and_the_cycle_says_the_policy_is_unset`; the operator's half is `docs/operations/arbitrage-policy.md` |
| Slot 8 — installed | TESTED at the node; reached by no deployed node | The installer of `584c96b` now has something to install: a desk exists in a node that runs with a policy set and a grant for its strategy, and `execution_nodes = {}` in every environment |

Three of twelve slots have producers; nine ship unproduced.

**Backlogged, not built.** Blueprint Phase 16 for the full payload. The capital
envelope is the pattern to generalise: it already proves the sign-verify-refuse
path works.

---

## Flow 4 — investment request → capital engine → grant → regional policy

**Verdict: the most complete flow in the platform.** Traceable end to end.

| Link | Status | Evidence |
|---|---|---|
| Request | TESTED | `qip_compliance::approval::CapitalRequest` with two-signature `ApprovalChain` (`approval.rs`), fresh-credential requirement (`:37`, `:402`) |
| Capital engine | TESTED | `qip-capital/src/allocation.rs` — `StrategyProposal`, `AllocationLimits` with per-cell (`:128`) and per-venue (`:134`) caps, `DrawdownSchedule` (`:169`) |
| Grant | TESTED | `CapitalEnvelope` / `CapitalGrant` in `qip-contracts/src/capital.rs`, signed |
| → regional policy | TESTED | `CapitalDownlink` verifies, refuses unsigned, de-duplicates; `RefusedGrant` records why |
| Cell cannot self-grant | TESTED | No edge crate reaches `qip-capital` or `qip-lifecycle` (`architecture.rs::no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy`) |

**Known gap, pre-existing:** capital reservation. A proposal that passes a
capital check does not hold the capital, so two concurrent proposals can pass
against the same free balance (`docs/plan/gap-matrix.md` item 10).

---

## Flow 5 — deposit/withdrawal/internal move → expected inflow or transfer intent → corridor → transfer gate → custody → reconciliation

**Verdict: MISSING almost entirely. Correctly so — Phase 12, and bounded by ADR 0021.**

| Link | Status | Evidence |
|---|---|---|
| Deposit / withdrawal | MISSING | `qip-portfolio/src/portfolio.rs:131` has "deposit or withdraw cash" — a simulated book operation, not a money movement |
| Transfer intent | PARTIAL | `qip-capital-fabric/src/transfer.rs`, `settlement.rs`, `plan.rs`, `location.rs` — internal capital placement, no external boundary |
| Corridor | MISSING | `grep -rn corridor --include=*.rs` returns **nothing** |
| Transfer gate | MISSING | `grep -rn TransferGate` returns **nothing** |
| Custody | MISSING | Only `VenueClass::Custodian` (`venue.rs:53-54`) as a venue kind |
| Destination registry | MISSING | Nothing |
| Reconciliation | PARTIAL, and real where it exists | The cell reconciles fills against the venue drop-copy and **self-halts on disagreement** (`cell.rs:774-786`) |

**Deliberately not built.** ADR 0021 permits the deterministic half and refuses
signing and withdrawal; ADR 0023 sequences this as step 10, separate from and
later than order submission. `security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
enforces the refusal.

---

## Flow 6 — halt/kill switch through both independent paths

**Verdict at the original trace: the halt worked where checked, there were
not two independent paths, and a central halt could not stop a regional cell.
Since partially closed:** `POST /api/v1/kill-switch` now also broadcasts a
signed engage-only `HaltCommand` to every cell inbox, a cell that hears it
halts, and release requires a strictly newer signed payload issued after the
halt's barrier — stopping easy, resuming a fresh decision. What remains open,
honestly: both paths share `qip-transport`, so this is mechanism independence
rather than the blueprint's two independent *wires*; the managed-store second
wire stays backlogged. **Closed at `ff86473`:** the second wire is a flag
polled on the node's own filesystem and shares nothing with the mesh; the
re-trace at `584c96b` below walks both wires, and what remains is the managed
store that would write the flag. ADR 0008 is intact — an unreachable cell keeps trading
its envelope, and the guarantee added is only that a reachable one obeys. The
sections below record the state as originally found.

### What works

| Property | Status | Evidence |
|---|---|---|
| Central halt enforced pre-trade | TESTED | `qip-execution-engine/src/oms.rs:248` — `if autonomy.kill_switch().is_halted(&order.scope)` refuses |
| Autonomy cannot be raised while halted | TESTED | `autonomy.rs:587` |
| Cell halt enforced | TESTED | `cell.rs:364-372` — `work()` refuses with `"kill_switch"` and keeps absorbing books, so a halted cell can still tell why |
| Cell self-halts on reconciliation break | TESTED | `cell.rs:774-786` — fills disagreeing with the venue's own account trip the cell's switch and journal a `HaltChanged` |
| Tripping needs no authority, clearing does | TESTED | `autonomy.rs:318-347` vs `:429`; mirrored in `qip-compliance/src/incident.rs` |
| The two credential windows agree | **TESTED — closed this pass** | `compliance_proof.rs::the_two_credential_windows_that_claim_to_be_the_same_window_agree_on_the_same_credential` |

Re-traced at `296e187` — the same properties at their current lines, plus
three halts that did not exist when the table was written:

| Property | Status | Evidence |
|---|---|---|
| Cell halt enforced | TESTED | `Cell::work` refuses under `"kill_switch"` at `cell.rs:768-769`; `is_halted` (`:388-389`) reads the local switch *or* the policy halt |
| Cell self-halts on reconciliation break | TESTED | `Cell::reconcile` (`cell.rs:2185`) trips the switch (`:2200`) and journals `HaltChanged` (`:2206-2208`) |
| Cell self-halts on a cycle broken between legs | TESTED | `place_cycle` (`cell.rs:1628`): a venue that breaks a cycle after some legs trips the switch as a reconciliation break does (`:1723`, `:1729`) and journals how far it got; `qip-edge/tests/arbitrage.rs::a_cycle_that_breaks_between_legs_halts_the_cell_and_records_the_break` (`71f9465`). This is what stands in for a `LegGroup`: the cell's `Placer` cannot cancel, so it halts rather than coordinating an unwind it cannot perform |
| The centre counts the cell's break and its own scoped halt | TESTED | `CentralPlane::record_halt` (`central/plane.rs:1398`, reached from `ingest` at `:1102` and so from `Platform::ingest_cell_report`, `platform.rs:1719`) records `qip_central_reconciliation_breaks_total` by direction (`:1404`) and `qip_central_cell_halts_total` by cause (`:1409`), on the outcome and not the report — an earlier version of this row cited the recording at `platform.rs:1368`, where at `5290bb9` there is only the descriptor. The directions are `cell_over_venue`, `venue_over_cell`, `detail_only` and, since `3c2b789`, `unsent_fill` — the centre's own finding, a fill on an order it never saw sent or beyond the remainder — counted through the same call because the settlement's breaks are merged with the report's before the halt (`:1066`); `qip-kernel/tests/central.rs::a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause`, `::a_reconciliation_break_halts_that_cell_and_only_that_cell`; over the wire, `qip-api/tests/mesh.rs::a_reconciliation_break_crossing_the_wire_halts_that_cell_and_only_that_cell` |
| The halted console still says PAPER TRADING | TESTED | `qip-web/tests/web.rs::a_halted_paper_platform_still_says_paper_trading_on_every_surface`, `console.rs::a_halted_console_still_states_that_it_is_paper_trading` (`03d5236`) — the halted banner used to replace the posture rather than add to it |

Re-traced at `584c96b` — **two wires, since `ff86473`.** §46.2 asks for two
independent paths, "Spanner flag polled and Pub/Sub broadcast", either of
which halts. Both now reach the cell, and the second shares nothing with the
first:

| Wire | Path, as read | Status | Evidence |
|---|---|---|---|
| 1 — broadcast | `POST /api/v1/kill-switch` (`qip-api/src/routes.rs:664`) trips the central switch and calls `broadcast_halt` (`:682`), which signs one engage-only `HaltCommand` per cell (`qip-api/src/mesh.rs:815`, `:822`) → the cell's inbox over `qip-transport` decodes and verifies it (`qip-edge/src/mesh.rs:1122`) → the node's mesh exchange applies every halt before any payload (`qip-edge-node/src/mesh.rs:348`) → `Cell::apply_halt` (`cell.rs:627`). Release: a strictly newer signed payload | TESTED | `qip-edge/tests/telemetry.rs::a_wired_cell_reports_not_halted_before_its_first_pass_and_a_central_halt_moves_the_gauge`; `qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it` |
| 2 — polled | An operator with root on the execution node creates `/run/qip/halt/engaged` — `startup.sh.tftpl:148-149` installs the directory root-owned and group-readable, so the service user can read the flag and cannot clear it, and `:172` sets `QIP_HALT_FLAG_PATH` in `node.env` → `qip-edge-node` polls it on every pass of its loop, before the flush and the mesh exchange (`main.rs:482-483`; `HaltFlag::poll` at `halt.rs:100`, one `read` and, when the file is absent, one `metadata` on its directory — two syscalls and nothing off-machine, `halt.rs:73-77`) → `Cell::apply_polled_halt` (`cell.rs:558`) → `is_halted` (`:529-533`) reads the polled halt beside the switch and the policy halt → `work` refuses under gate `polled_halt` (`:986`) | TESTED | `qip-edge-node/tests/halt.rs::the_node_halts_the_cell_on_a_present_flag_and_releases_it_when_the_flag_is_removed`, `::a_flag_that_cannot_be_read_halts_the_cell_rather_than_reading_as_absent`, `::a_flag_that_reads_released_does_not_halt_and_malformed_content_does`, `::an_empty_or_relative_flag_path_is_refused_at_configuration`, `::the_binary_reads_the_flag_variable_and_polls_the_flag_in_its_loop` |

What the second wire holds, each at its test:

- **The flag is the state, and it fails toward engaged.** The file present
  halts; removing it releases, and every poll re-applies what it read. A file
  that cannot be read, a directory that is gone, content that is not text,
  more bytes than a flag may hold, or any text that is neither `engaged` nor
  `released` — including a near miss of the release word — halts
  (`PolledHalt`, `cell.rs:2617`, `from_content` at `:2649`;
  `cell.rs::polled_halt_tests::every_content_the_flag_can_hold_reads_the_way_the_wire_needs`,
  `::an_unreadable_flag_halts_and_an_absent_one_releases_and_the_chain_says_which`).
- **Shared failure: none.** Wire 1 dies with the mesh — a wedged centre, a
  partition, a downlink with its circuit open. Wire 2 links no mesh:
  `HaltFlag::read` touches the local filesystem and nothing else, and the node
  polls it ahead of the exchange so the halt is in the journal the flush ships
  and in the delta the exchange publishes (`main.rs:477-483`).
- **Neither wire releases the other.** With both engaged, releasing the polled
  wire leaves the cell halted and its chain entry says so rather than reading
  as a cell that resumed
  (`cell.rs::polled_halt_tests::the_polled_wire_and_the_kill_switch_release_each_other_never`,
  `cell.rs:3424`); a fresh, verified policy payload — the thing that releases
  wire 1 — leaves the polled halt engaged and its gauge at one
  (`qip-edge/tests/telemetry.rs::a_polled_halt_moves_its_own_gauge_refuses_the_pass_under_its_own_gate_and_no_payload_releases_it`).
  The remaining direction — clearing the kill switch while the flag is
  present — held at `584c96b` because `is_halted` is a disjunction of three
  fields (`cell.rs:529-533`) and was asserted by no named test. Re-traced at
  `e04815e`: it is asserted —
  `qip-edge/tests/telemetry.rs::clearing_the_kill_switch_while_the_polled_flag_is_present_leaves_the_cell_halted`
  (`:620`, `6a515bb`) — so all three release directions are TESTED.
- **Series.** `qip_edge_halted{source="polled"}` beside `kill_switch` and
  `policy` (`qip-edge/src/telemetry.rs:172-193`), written from `record_halt`
  (`cell.rs:482`) wherever any halt can change, so a chart shows which path
  stopped the cell and which did not.

Honest limits, the same in kind as the rest of the edge plane:

- It reaches a deployed process only once a node runs — `execution_nodes = {}`
  in every environment's `terraform.tfvars` — so nothing has ever polled a real
  flag. A node started without the variable lists it among its production
  requirements (`main.rs:425-431`), because a node with one wire is unhaltable
  for as long as a partition lasts.
- §46.2's "Spanner flag polled" is still a file a person writes: the template
  says so (`startup.sh.tftpl:141-144`) — an operator with root, or a
  managed-store fetcher when one exists, and today nothing on the machine
  writes it but a person. `/run` is a tmpfs, so a reboot clears the flag; the
  broadcast is the halt that survives one.
- The alert's text, as originally traced, did not name the polled source:
  `edge_halted` in `modules/observability/main.tf:187` groups on `source`, so
  a polled halt would fire it once `workload_metrics_exist` were flipped and
  something had scraped a node, but its documentation text beneath the query
  named only the two disciplines that existed when it was written. Since
  `cd16f79` it names `polled` beside them (`main.tf:200`), so an operator
  paged on the third source is not reading text that says it does not exist.
  Nothing has been shown to scrape a node.

The "one mechanism, two user interfaces" finding below is now history twice
over: the broadcast closed the downward half, and the polled flag closed the
independence half.

### What does not work

**There is one mechanism, reached by two user interfaces — not two independent
paths.** Blueprint §46.2 requires "two independent paths — Spanner flag polled
and Pub/Sub broadcast. Either halts trading, quoting and transfers."

- `POST /api/v1/kill-switch` → `platform.autonomy_mut().kill_switch_mut().trip_global(...)` (`routes.rs:635-647`)
- Console `POST` → `platform.autonomy_mut().kill_switch_mut().trip_global(...)` (`console.rs:123`)

Both mutate the **same in-process object**. A process that is wedged, a
partition, or a lost API is one failure that takes both. Independence is the
entire point of the control and it is absent.

**And a central halt cannot reach a cell.** The cell's `is_halted()`
(`cell.rs:199-200`) reads its *own* `AutonomyController::new()` switch. The
downlink accepts only `CapitalGrantTopic` frames, so no halt command can arrive.
The mesh delta carries `halted` **upward** (`mesh.rs:173`, `delta.rs:100`) —
the cell reporting its own state, not receiving one.

Consequence, stated plainly: **an operator tripping the kill switch halts the
central plane and leaves every regional cell trading.** Each cell can only be
halted by its own local trip or by its own reconciliation self-trip.

**Why this is backlogged rather than fixed here.** The downward halt is a policy
item travelling centre → cell, which is exactly flow 3's twelve-item payload —
§41.5 item 9 is the risk envelope and §6.2 defines stale-item narrowing.
Building a bespoke halt topic now would be a second policy channel beside the
one the blueprint specifies, which is the process proliferation the programme's
own rules forbid. It belongs with the payload, and it is the strongest argument
for doing that work.

**Mitigating, and not a substitute:** a cell is bounded by a signed envelope
with an expiry, so an unhaltable cell can only spend what was already approved,
for as long as it has left (ADR 0008).

---

## Flow 7 — failure and degradation paths

| Dependency lost | Blueprint expects | Repository | Status |
|---|---|---|---|
| Central plane | Region narrows, never halts on that alone | `CellUplink`/`CapitalDownlink` circuit breakers (`mesh.rs:265,516`); cell keeps trading its envelope | TESTED — `apps/qip-edge-node/tests/mesh.rs:376` |
| Source / ingestion | World model ages; price-only strategies continue | Two levels now. Mechanism: a stale book supplies nothing (`edge/qip-edge/src/seam.rs`). Capability: `Cell::narrowing` (`cell.rs:400`) reads the payload's ingestion slot and `work()` pauses the strategy classes the §6.2 table names — `qip-contracts/tests/contracts.rs::an_ingestion_stall_pauses_the_strategies_that_need_the_world_and_no_others`, `qip-edge/tests/telemetry.rs::a_policy_going_stale_narrows_the_cell_to_the_floor_and_the_gauges_move_with_it`. The row this replaces said "no caller"; that was true at the original trace and false from the payload slice | TESTED |
| Belief | Fixed conservative multiplier; nothing halts | The narrowing consumer exists (row above) and the belief slot ships unproduced, so it reads stale from birth and the fixed multiplier applies — `contracts.rs::a_belief_state_stale_beyond_its_ttl_falls_back_to_a_fixed_multiplier_and_halts_nothing`, `::an_unproduced_slot_is_stale_from_birth_and_narrows_like_staleness`. There is still no belief state to *go* stale; what exists is its calibration (flow 2) | PARTIAL |
| Trial count unknown | Honest significance cannot be claimed, so it is not | `qip_lifecycle::trials::TrialBook` — one hash-chained journal per family, replayed from the store with the chain verified; a gate handed no account fails `lifetime_trial_count_known` (`gates.rs:242`), a ledger with no book refuses outright, and the kernel's `StrategyFactory` enrols each candidate in its family at registration (`central/factory.rs:281-299`). `qip-lifecycle/tests/lifecycle.rs::a_promotion_whose_lifetime_trial_count_is_unknown_is_refused_naming_what_to_do`, `::a_second_run_is_corrected_against_the_first_runs_trials_as_well`, `::a_trial_book_replays_its_journal_from_the_store_and_refuses_a_tampered_one` (`9332bcb`, `94dd7e2`). Limit: the factory's default book is `TrialBook::in_memory` (`factory.rs:243`) and `with_trial_book` (`:251`) has no caller in any composition root, so a deployed count is per-process — the accounting the blueprint forbids the moment a second process runs | TESTED, durable book PLANNED |
| Live returns leave the holdout band | Demote; a gate with no value cannot be failed | `HoldoutBand::from_deflated` at the holdout gate (`gates.rs:260`), carried on the `Admission` and refused off it (`ledger.rs:129`, `:246`); the demotion monitor's `OutsideHoldoutBand` trigger (`demotion.rs:154`) is two-sided — far above is a different strategy, not good news — and the kernel's factory drives `DemotionMonitor::enforce` (`factory.rs:449`). `lifecycle.rs::a_holdout_admission_carries_the_band_its_validation_produced`, `::live_performance_outside_the_holdout_band_is_demoted_and_counted`, `::judging_or_admitting_without_a_holdout_band_is_refused` (`d0558b4`) | TESTED |
| Infeasible leg in a cycle | Veto the cycle whole | A cycle short a leg is a position, not a smaller cycle: `arbitrage.rs::an_infeasible_leg_vetoes_the_whole_cycle_and_no_leg_goes_out` | TESTED |
| Capability degraded while a desk is installed | The desk is price-only and pauses with that class; a narrowed size opens no cycle | Re-traced at `584c96b`. `scan_cycles` (`cell.rs:1488`): the §6.2 pause for `PriceOnly` refuses under `degradation_pause` (`:1504-1511`) and a sizing multiplier below one under `degradation_sizing` (`:1513-1522`), because a cycle re-priced at a narrower size is a different cycle whose legs no longer close on what was priced. At installation the same table applies: a degraded cell installs no desk (`584c96b`, `qip-edge-node/tests/arbitrage.rs::a_degraded_cell_and_an_empty_whitelist_install_no_desk`) | PARTIAL — the installation refusal is TESTED; the two pass-time gates are code that no test drives a desk through (`degradation_pause` is asserted only for a strategy, `qip-edge-node/tests/gateway.rs::a_strategy_that_recognises_situations_pauses_when_episodic_memory_goes_stale`) |
| Source chain reorganises a deposit block | Fail the bridged transfer | `observe_chain` hands each reorganisation to `BridgeLedger::on_reorg` (`platform.rs:4736`); `qip-kernel/tests/bridges.rs::a_reorganisation_that_withdraws_a_deposit_block_fails_the_transfer_riding_on_it` (`67b3e92`) | TESTED |
| Synthetic path overflows | Refuse, do not restart from the initial price | `market_conditions.rs::a_synthetic_path_that_overflows_is_refused_rather_than_restarted_from_the_initial_price` (`cc92d66`) — the reset the comment above it denied is gone | TESTED |
| Ledger | — | No ledger type; event log is append-only and hash-chained | PARTIAL |
| Node / cell | Failure isolation per region | Venue health and degradation in `edge/qip-routing/src/health.rs`; `resilience.rs`, `chaos.rs` | TESTED |
| Venue | Refuse, do not guess | `qip-brokers/src/connection.rs` degradation; `oms.rs` refusals recorded | TESTED |
| Venue never fills a rested order | Withdraw it when its time to live elapses; a position is only what the venue reports | Re-traced at `e04815e`. `PricingPolicy::RestAtMid { time_to_live }` (`cell.rs:365`): the order rests at the mid and is withdrawn through `Placer::cancel` (`:3609`) when its time to live elapses, counted on `qip_edge_orders_expired_total`; a cancel the venue refuses is a break and halts the cell; a gateway that cannot withdraw refuses the resting policy at deployment — `qip-edge/tests/pricing.rs::a_resting_order_rests_at_the_mid_and_is_withdrawn_when_its_time_to_live_elapses`, `::a_cancel_the_venue_refuses_is_a_break_and_halts_the_cell`, `::a_resting_policy_is_refused_on_a_gateway_that_cannot_withdraw` (`383d4e7`) | TESTED at the cell and, since `5290bb9`, at the centre: the rested order crosses the wire as sent and books nothing until a fill for it arrives (flow 2's ledger row; `qip-kernel/tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate`) |
| Node halted, or nothing listed at the venue | No pass; refuse rather than guess | Re-traced at `e04815e`. A halted node runs no pass, and a pass with nothing listed at the venue refuses under the venue-selection gate — `qip-edge-node/tests/pass.rs::a_halted_node_runs_no_pass`, `::a_pass_with_nothing_listed_at_the_venue_refuses_under_the_venue_selection_gate` (`6340610`) | TESTED |
| IBM / quantum | Classical baseline always | ADR 0006; `qip-optimization-engine/src/router.rs` computes the baseline every time, so a QPU outage narrows nothing | TESTED |

**The pattern, re-stated.** When first traced, degradation was thorough at the
*mechanism* level — one book, one venue, one peer — and absent at the
*capability* level §6.2 defines. The capability level now has its consumer at
the cell, and the two compose rather than replace each other. What is still
missing is the *producers*: ten of twelve slots ship unproduced, so most
capability rows narrow the cell by absence rather than by a measured stall.
Fail-closed, and not yet informative.

---

## What this pass changed

One defect found and fixed: two credential windows that each documented
agreement with the other, in crates that cannot see each other. Bound by a
behavioural test, two mutations fired.

Everything else found is either a missing capability whose phase has not
arrived (flows 1, 3, 5) or a structural gap that belongs with the twelve-item
payload (flow 6's downward halt, flow 7's capability degradation). None of it
was built, per the no-speculative-scaffolding rule.

**The single highest-value next slice was the twelve-item payload**, and it
landed: flow 3 is PARTIAL, flow 6's downward halt rides it, flow 7's narrowing
consumes it. At `296e187` the convergence point had moved: flow 2's cell seams
(feasibility, scanner, netting, crossing), flow 6's cycle halt and flow 7's
capability narrowing were all proven by `qip-edge`'s tests and reached by no
deployed process, because `qip-edge-node` drove no `Cell::work` pass — since
`584c96b` it installed a desk, from a whitelist nothing at the centre
produced — and the one piece of work four flows converged on was the node
running passes against a venue feed.

At `e04815e` it does. `qip-edge-node` runs `Cell::work` over the in-process
simulated venue (`6340610`), a fill is a venue fact (`cb79b46`), pricing is
stated by the strategy or refused (`383d4e7`), the whitelist the installer
waits for is produced and shipped (`5396679`, `91d20f5`), and every one of
those is TESTED in the node's own tests and MEASURED by nothing, because no
node is deployed. At `e04815e` two things converged on each other. The first
was the wire: the report a running node ships carried its placements as
`orders`, and the centre billed them as fills (flow 2's ledger row) — a
defect that was invisible while no node ran passes and would have been the
first thing a running one produced. That closed at `5290bb9`: the delta
carries the venue's fills as their own field, the centre registers what was
sent and bills only what filled, and a fill it cannot trace to a sent order
halts the cell; one test-only helper in `e2e_live.rs` still reads positions
from `orders` and is being corrected. The second is a deployed node —
`execution_nodes = {}` in every environment — which is still the first thing
between the edge plane's telemetry and any collector, and the only thing
between every control above and a measurement.
