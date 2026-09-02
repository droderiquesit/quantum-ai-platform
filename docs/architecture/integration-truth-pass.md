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
their original line numbers.

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
| → **strategy intent** | **SPLIT — built at the edge, absent at the centre** | `Intent` exists (`libs/qip-contracts/src/intent.rs`), and with it netting, internal crossing (§27.1) and the contributor vector. `Cell::work` builds one intent per firing strategy, nets them on instrument/venue/representation, crosses the offsetting part at the book mid and places what survives. **This row said all four of those were absent; that was true when it was written and false from the netting slice onward.** What has not changed is the *central* path: `stage_decide` (`platform.rs:3450`) still sizes theses into `Proposal`s with legs and raises no intent, so §27's mechanism operates in one of the two planes. See the seam below, which is the same seam. Re-traced at `296e187`, the cell's intent path has two more seams, both ahead of `net()` in `Cell::work` (`cell.rs:750`): the **arbitrage desk** re-quotes its graph from the cell's own books and scans it (`:851 scan_cycles`), and the **feasibility gate** judges every intent in place (`:860 admit_feasible`) before `net` (`:869`) — minimum quantity, minimum notional, lot, tick, depth at the touch, fee floor, gas floor and a malformed policy constraint, eight gate literals in `feasibility.rs:76-83`, each a refusal and never a rounding. `qip-edge/tests/feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order`, `::a_sell_is_judged_against_the_bid_side_and_a_buy_against_the_ask`; `qip-edge/tests/arbitrage.rs::a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass`, `::an_infeasible_leg_vetoes_the_whole_cycle_and_no_leg_goes_out` (`95a4932`, `71f9465`). Direction is pinned too, after two compensating sign inversions were found cancelling at the gateway and nowhere else: `qip-edge/tests/direction.rs::an_enter_signal_leaves_the_cell_taking_the_ask_which_is_a_buy` and `qip-edge-node/tests/gateway.rs::an_enter_signal_is_a_buy_at_the_matching_engine_filling_against_offers_and_resting_against_bids` (`54d32fd`). The honest limit: `qip-edge-node` calls `Cell::work` on no path and installs no desk (`grep -rn "\.work(\|ArbitrageDesk" apps/qip-edge-node/src` is empty), so every seam above is proven by the cell's tests and reached by no deployed process |
| → **regional execution** | **BREAKS** | `stage_act` (`platform.rs:3545`) runs the central risk monitor and places through the **central** OMS. It does not reach a cell. See the seam below |
| → fill | TESTED | Orders reach the simulated broker via `qip-execution-engine/src/oms.rs`; multi-leg in `multileg.rs` |
| → ledger | PARTIAL / naming | **There is no `Ledger` type.** `grep "pub struct Ledger\b"` finds nothing — the ledgers that exist (`PrivacyLedger`, `BudgetLedger`, `ReservationLedger`, `BridgeLedger`, `LifecycleLedger`) are each something narrower than money state. Money state is `qip-capital` (allocation, envelope, exposure) plus the hash-chained event log. Blueprint §43.3's per-user, per-strategy authoritative ledger does not exist. What arrived at `7ef6063`/`7d79161` is the per-cell, per-strategy, per-instrument *book* the centre keeps from a cell's report: `CentralPlane::ingest` (`central/plane.rs:855`) settles the interval's orders and crosses (`:976 settle`) before the halt step, a fill pro rata to the contributors on its own side through `split_pro_rata` (`:1021`; `qip-learning-engine/src/attribution.rs:266`, largest-remainder, asserts the shares sum), a cross moving buyer up and seller down at the recorded mid, and a decomposition that must close to the last unit or count `qip_central_attribution_failures_total` (`:1126`). `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual`, `::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact`, `::a_cross_naming_two_buyers_is_refused_rather_than_split_evenly`; and the API sink now carries the interval (`qip-api/src/mesh.rs:1148-1149`), `qip-api/tests/mesh.rs::the_orders_a_cell_reports_reach_the_centres_strategy_books`, mutation fired with the two builder calls removed |
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
wire stays backlogged. ADR 0008 is intact — an unreachable cell keeps trading
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
| The centre counts the cell's break and its own scoped halt | TESTED | `Platform::ingest_cell_report` records `qip_central_reconciliation_breaks_total` by direction (`platform.rs:1368`) and `qip_central_cell_halts_total` by cause (`:1372`), on the outcome and not the report; `qip-kernel/tests/central.rs::a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause`, `::a_reconciliation_break_halts_that_cell_and_only_that_cell`; over the wire, `qip-api/tests/mesh.rs::a_reconciliation_break_crossing_the_wire_halts_that_cell_and_only_that_cell` |
| The halted console still says PAPER TRADING | TESTED | `qip-web/tests/web.rs::a_halted_paper_platform_still_says_paper_trading_on_every_surface`, `console.rs::a_halted_console_still_states_that_it_is_paper_trading` (`03d5236`) — the halted banner used to replace the posture rather than add to it |

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
| Source chain reorganises a deposit block | Fail the bridged transfer | `observe_chain` hands each reorganisation to `BridgeLedger::on_reorg` (`platform.rs:4736`); `qip-kernel/tests/bridges.rs::a_reorganisation_that_withdraws_a_deposit_block_fails_the_transfer_riding_on_it` (`67b3e92`) | TESTED |
| Synthetic path overflows | Refuse, do not restart from the initial price | `market_conditions.rs::a_synthetic_path_that_overflows_is_refused_rather_than_restarted_from_the_initial_price` (`cc92d66`) — the reset the comment above it denied is gone | TESTED |
| Ledger | — | No ledger type; event log is append-only and hash-chained | PARTIAL |
| Node / cell | Failure isolation per region | Venue health and degradation in `edge/qip-routing/src/health.rs`; `resilience.rs`, `chaos.rs` | TESTED |
| Venue | Refuse, do not guess | `qip-brokers/src/connection.rs` degradation; `oms.rs` refusals recorded | TESTED |
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
consumes it. At `296e187` the convergence point has moved. Flow 2's cell seams
(feasibility, scanner, netting, crossing), flow 6's cycle halt and flow 7's
capability narrowing are all proven by `qip-edge`'s tests and reached by no
deployed process, because `qip-edge-node` drives no `Cell::work` pass and
installs no desk. The one piece of work four flows now converge on is the
node running passes against a venue feed — which is also the first thing
between the edge plane's telemetry and any collector.
