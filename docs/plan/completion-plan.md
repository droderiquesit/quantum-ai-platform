# Completion plan — how far from done, what is left, in what order, blocked on whom

**Living document.** First scored on branch
`claude/algorik-architecture-refactor-pmp0zy` at `de5d042`; re-scored at
`296e187`, fifty-seven commits later (`git log --oneline de5d042..296e187 |
wc -l`). Five more landed while the re-scoring was in progress — `88eb1e2`
(desk fills fed into the risk aggregate), `81dd1cd` (the three deployment
suites retargeted at the runtime that exists), `ecfb0a6` (documents and two
rule files corrected for the deleted runtime), `2b7e502` (ADR 0024) and
`fca98cc` — and are cited where they change a row; the checkout at `fca98cc`
is clean apart from this plan and its four sibling documents.

Corrected at `851c0ed`, the merge of PR #6 (`baffcd8`..`b1e709c`, 89
commits): A2, the full gate, ran at `29ce828` and this plan still said it had
not. The rows that said so — §2.4, §3, §4(i), §5 D12, §6 and §7 — are
re-stated below with the figures, and §3 gains the merge.

Re-scored at `e04815e`, thirteen commits after `cfe11c1` (`git log --oneline
cfe11c1..e04815e | wc -l`), the head PR #7 merged at `a084860`. What moved:
the node runs passes (`6340610`), a fill is a venue fact (`cb79b46`), pricing
is stated (`383d4e7`), the whitelist is produced and shipped (`5396679`,
`91d20f5`, `73a1694`), the roots assemble from the committed catalogue
(`8224509`, `e40335d`), the third halt-release direction is tested
(`6a515bb`), the alert names the polled halt (`cd16f79`), and ADR 0027 is
proposed (`360cfd8`). One defect was open and in flight at that re-score,
filed in §4 as B22; it closed at `5290bb9`, six commits after `e04815e`
(`9e45dc0`, `ef4464a`, `96a49f1`, `3c2b789`, `d59505d`, `5290bb9`), and this
document is re-stated there. The checkout at `e04815e` was not clean — the
B22 fix was uncommitted in seven files and did not compile — so that
refresh's documentation suite ran against a clean export of `e04815e`; this
one ran in the tree at `5290bb9`.

Re-scored at `2e19a4c`, twenty-four commits after `5290bb9` (`git log
--format='%h' 5290bb9..2e19a4c | wc -l`). Two things moved, on different
axes: wave 5 (`da0789d`) appended a `UniverseAssembled` record as the log's
first link, so a cycle over an unrecorded universe is unrepresentable, and
deployed the arbitrage desk from the payload's own pricing policy at the
node and in the console — B12 (per-region reservation) and B14 (payload-slot
producers) were checked against the tree after it landed and neither moved:
`grep -n -i reserv backend/crates/edge/qip-edge/src/cell.rs
backend/crates/apps/qip-edge-node/src` still finds nothing but "preserved",
and `qip-api/src/mesh.rs`'s own comment still reads "today is three of the
twelve items." Separately, thirteen commits ran real Terraform plans and
applies against `algorik-dev`/`dev` through CI — not from this shell, which
still has no `terraform` binary — tearing down the GKE runtime, standing up
Cloud Run, and root-causing a startup-probe failure to an Envoy sidecar
health listener bound to loopback. §6 is rewritten around what that sequence
actually proved; it is not what "no project reachable" said in every
scoring before this one.

This document aggregates the repository's own scorecards; it does not replace
them. Where it disagrees with one of them it says so rather than picking. The
sources, and what each is authoritative for:

| Source | Authoritative for |
|---|---|
| [`../architecture/algorik-blueprint-traceability.md`](../architecture/algorik-blueprint-traceability.md) | Status of every blueprint requirement — the live scorecard (ADR 0022) |
| [`../architecture/integration-truth-pass.md`](../architecture/integration-truth-pass.md) | Whether the seven flows connect, and where each breaks |
| [`../architecture/blueprint-diagram-reconciliation.md`](../architecture/blueprint-diagram-reconciliation.md) | Where the two authoritative references disagree with each other |
| [`gap-matrix.md`](gap-matrix.md), [`current-state.md`](current-state.md) | The ordered work and the measured state against the earlier diagram |
| [`../architecture/deployed-vs-blueprint.md`](../architecture/deployed-vs-blueprint.md) | What the committed Terraform would produce if applied, resource by resource, against what the blueprint requires — written at `bcad2d3`, before `808ca32` removed the cluster it describes, so its "deploys today" column is history |
| [`../adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md`](../adr/0020-two-runtime-topologies-and-the-order-to-resolve-them.md), [`../adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md`](../adr/0022-the-algorik-blueprint-is-the-architecture-of-record.md), [`../adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md`](../adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md) | The migration sequence, the reference, and the opening sequence |

**Vocabulary.** MEASURED (runtime evidence exists) · TESTED (a named passing
test) · CONFIGURED (wired in a manifest or tfvars) · IMPLEMENTED-UNVERIFIED
(code exists, no deployable composes it, or no tool here could validate it) ·
PLANNED (backlogged with a phase) · MISSING. The matrix's ALIGNED bar is
implementation plus a passing named test; nothing below is called done on the
strength of a commit.

**One rule for reading this.** "Code exists" is never "done". The four gates in
§2 are empirical claims about real data and real venues, and no amount of code
passes one.

---

## 1. Definition of done, stated twice

These are two finish lines an order of magnitude apart, and every remaining
item in §4 names which one it belongs to.

### 1(a) Alignment-done — the original brief

The programme is aligned when all five hold, each with evidence a person can
check:

1. **The current phase is internally clean.** Every control that exists can
   fire, every test measures something, and no scored document denies a type
   the workspace defines. Checked by `documentation.rs`, `architecture.rs`,
   the gap-matrix risk register's "control that cannot fire" count, and the
   full gate (`make check`).
2. **Every layer and plane carries an evidence-backed disposition** — a
   status from the vocabulary above with a file, test or commit behind it.
   Checked by the traceability matrix having no UNVERIFIED row that a
   composition root could have resolved.
3. **Changed behaviour is tested and mutation-verified.** Checked by each
   slice's mutation report in its commit message or PR body.
4. **Boundaries are enforced structurally.** The three paper layers, the
   dependency direction, the LM and quantum authority boundaries. Checked by
   `security.rs`, `compliance_proof.rs`, `architecture.rs`.
5. **Future phases are gated, not scaffolded.** Nothing exists for a phase the
   roadmap has not reached unless it has a consumer today. Checked by the
   matrix's PLANNED-FUTURE rows being empty crates nowhere in the tree.

Alignment-done says nothing about whether the platform works on real data. A
fully aligned repository has passed zero gates.

### 1(b) Blueprint-done — all twenty phases and four gates

The platform is blueprint-done when Phases 0–19 of blueprint §51 have met
their exit criteria and the four gates at the end of Phases 2, 3, 6 and 8 have
each been passed on real data or a real venue. Phase 19 is "ongoing" by the
blueprint's own table, so blueprint-done is more precisely "Phases 0–18 exited,
four gates passed, Phase 19 operating".

This finish line also requires the two direction decisions ADR 0022 settled to
be executed — no Kubernetes (ADR 0020 step 5) and Leptos (C3) — and the opening
sequence of ADR 0023 to reach step 9. None of those is authorised today.

---

## 2. Where we are

### 2.1 The four gates — zero of four have passed

| Gate | What it requires | Status | The specific blocker |
|---|---|---|---|
| End of Phase 2 | A family surviving holdout with honest significance after **cumulative** trial correction, on real data | **NOT PASSED** | No deployment has run on sustained real data (Phase 1 exit unmet — see below). The second blocker this row carried — that nothing counted trials across runs — closed in code at `9332bcb` and `94dd7e2`: `TrialBook` keeps one hash-chained journal per family and the holdout gate refuses an unknown lifetime count. The deployment-shaped half closed at `aa66c5d`: every composition root opens the book on the `trial-book` namespace of its own store through `Platform::open_trial_book` (`qip-api/src/main.rs:106`, `qip-fastbrain/src/main.rs:164`, `qip-deepbrain/src/main.rs:178`), a journal that does not verify refuses to start (`qip-kernel/tests/trial_book.rs::a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward`, `::a_journal_whose_count_was_lowered_by_hand_refuses_to_open_and_nothing_is_attached`), and since `e31aae4` the book budgets five hundred trials per family per calendar quarter under the same hash (`lifecycle.rs::the_five_hundredth_trial_of_a_quarter_charges_and_the_five_hundred_and_first_is_refused`). What remains is real data. ADR 0023's "What could not be specified" still describes the old state and is the owner's to amend |
| End of Phase 3 | Thirty days live, inside the holdout band, no unexplained break | **CANNOT PASS as the tree stands** | Structurally unreachable while ADR 0003 and ADR 0021 stand — three paper layers refuse it. ADR 0023 sequences the opening at steps 5–8; none is approved. The band now exists (`d0558b4`: `HoldoutBand::from_deflated` at `gates.rs:260`, carried on the admission, two-sided at the demotion monitor), so this row no longer also lacks its measuring stick |
| End of Phase 6 | Calibrated probability beating the market's implied on prediction contracts, Brier-scored | **NOT PASSED** | `qip-prediction` has `market.rs`, `oracle.rs`, `pricing.rs`, `resolution.rs`; no Brier comparison against a live venue exists (matrix, gates table) |
| End of Phase 8 | Regime-conditional allocation beating unconditional, out of sample | **NOT PASSED** | Regime detection exists (`qip-cost-router/src/context.rs`, `qip-simulation-engine/src/conditions.rs`); no out-of-sample comparison is computed (matrix, gates table) |

The Phase 1 exit — "7 days stable streaming, statistics converged, no raw
stream retained" — is not a gate but it precedes the first one, and it is
unmet: one real tick was fetched in-session through a TLS-terminating bridge
(`gap-matrix.md` item 6), and no deployment has streamed for any duration,
because no egress proxy runs. At `296e187` one exists as Terraform — a
co-located Envoy sidecar (`c924191`, wired at `808ca32`) — and has never been
planned, applied or pointed at a vendor (§6 below).

### 2.2 Per plane — derived from the traceability matrix

The matrix's plane table has seven rows with one status each. Counting them:

| Status | Rows |
|---|---|
| PARTIAL | 6 — Ingestion, Cognition, Intelligence, Optimisation, Execution, Ledger/wallet/treasury |
| MISSING-CURRENT | 1 — Valuation |
| ALIGNED | 0 |

So **0 of 7 planes are ALIGNED, 6 of 7 are PARTIAL, 1 of 7 is MISSING.** A
fraction per plane below is therefore not a matrix number; the matrix gives
one status per plane. What follows is this plan's own derivation, counting the
*named capabilities* in each plane's matrix row and flow evidence, with the
arithmetic shown so it can be disputed. A capability counts as present only at
the TESTED bar.

| Plane | Capabilities named in the matrix row / flows | Present (TESTED) | Fraction | Evidence |
|---|---|---|---|---|
| 1 Ingestion | absorb records; entity resolution; licensing before use; one live source sustained; deep-web tier | 3 of 5 | 3/5 | `absorption.rs`, `sense.rs`, `qip-fastbrain/src/licensing.rs`; live source is PARTIAL (one tick, no deployment); deep-web tier MISSING |
| 2 Cognition | world model; causal graph; episodic memory; hypotheses; belief stage in the cycle; counterfactuals with a production caller; self-model | 5 of 7 | 5/7 | `understanding.rs`, `reasoning.rs`, `world.rs:41`, `causal.rs:234`; belief still SUBSTITUTED (flow 2) though now graded — `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series` (`04738ee`); counterfactuals gained their caller — `learning.rs::a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate` (`b9e2242`), up from 4 of 7; no self-model |
| 3 Valuation | six engines (§16.1) | 0 of 6 | 0/6 | MISSING-CURRENT; deliberately not scaffolded. Corporate actions are *absorbed* (`platform.rs:1159-1180`) but no engine prices anything |
| 4 Intelligence | statistical gate; champion/challenger; drift detection; training; corridor policy; cumulative trial accounting across runs; holdout band as an output of validation | 6 of 7 | 6/7 | `lifecycle.rs`, `evolution.rs`, `training.rs`, `qip-deepbrain/src/learning.rs:279`; the band — `lifecycle.rs::a_holdout_admission_carries_the_band_its_validation_produced` (`d0558b4`), new since `de5d042`; corridor policy has no subject (Phase 12); cumulative trial accounting is TESTED at the crate (`::a_second_run_is_corrected_against_the_first_runs_trials_as_well`, `9332bcb`) and enrolled by the factory (`94dd7e2`) and, since `aa66c5d`, opened durable in every root (`qip-kernel/tests/trial_book.rs::a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward`) with the quarter budgeted at `e31aae4` — counted present at `584c96b`, up from absent at `296e187`, where the deployed book was in-memory |
| 5 Optimisation | routing gate; classical baseline every time; authority boundary structural; family clustering; multi-horizon reconciliation | 3 of 5 | 3/5 | `optimization.rs`, `architecture.rs` solver tests, ADR 0006 |
| 6 Execution | paper-only cell; envelope admission; intent netting; internal crossing; contributor vector on the uplink; halt reaching a cell; §6.2 narrowing; feasibility gate; per-region reservation; crossing settled to books; leg producer for cycles; the node running passes; a fill as a venue fact; a stated pricing policy; the cycle whitelist produced and shipped | 14 of 15 | 14/15 | `qip-edge/tests/cell.rs`, `qip-edge-node/tests/gateway.rs`, `qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it`; three moved since `de5d042`: feasibility — `feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order` (`95a4932`); crosses settled at the centre's books — `qip-kernel/tests/attribution.rs::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact` (`7ef6063`); the leg producer — `arbitrage.rs::a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass` (`71f9465`). Reservation still CONTRADICTS (F6). Four added at `e04815e`, all TESTED: the node running passes — `qip-edge-node/tests/pass.rs::a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move`, `::a_venue_feed_other_than_the_simulator_is_refused_at_start_naming_adr_0003` (`6340610`); a fill as a venue fact — `qip-edge/tests/fills.rs::an_order_the_venue_accepted_is_not_a_fill_until_the_order_entry_channel_reports_one` and `pass.rs::a_resting_order_the_venue_fills_on_a_later_pass_is_confirmed_and_the_node_keeps_trading` (`cb79b46`, `b8d18d3`); a stated pricing policy — `qip-edge/tests/pricing.rs::an_intent_with_no_stated_pricing_is_refused_and_nothing_reaches_the_venue`, `::a_resting_order_rests_at_the_mid_and_is_withdrawn_when_its_time_to_live_elapses` (`383d4e7`); the whitelist — `qip-kernel/tests/central.rs::issuing_a_whitelist_through_the_platform_journals_what_was_issued`, `qip-api/tests/mesh.rs::a_cycle_ships_the_desk_a_live_grant_funds_as_a_whitelist_the_cell_verifies` (`5396679`, `91d20f5`). The caveat that weighed on the first ten until `e04815e` — that `qip-edge-node` called `Cell::work` on no path — is gone; the one that remains is that no node is deployed, so TESTED never means a measured pass |
| 7 Ledger, wallet, treasury | capital allocation; envelope; two-signature approval; reservation ledger in the kernel; per-user per-strategy ledger; §43.4 attribution chain at the centre (fill → contributor vector → strategy pro rata); the centre's books settled from venue fills rather than placements; wallet; corridor; transfer gate; destination registry; custody | 6 of 12 | 6/12 | `truth_loop.rs`, `compliance_proof.rs`, `platform.rs::a_second_proposal_is_sized_against_what_the_first_still_holds`; the chain — `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual` and `qip-api/tests/mesh.rs::the_fills_a_cell_reports_reach_the_centres_strategy_books` (`7ef6063`, `7d79161`; the second renamed from `the_orders_...` at `d59505d`, when what it proves reaching the books became the fill), new since `de5d042`; the books are per strategy and not per user, so the §43.3 ledger stays absent; added absent at `e04815e` and TESTED at `5290bb9`: the books are settled from the delta's `fills` (`qip-contracts/src/wire.rs:93`; `qip-edge/src/mesh.rs:214`) and never from its `orders`, which `settle` registers as sent (`central/plane.rs:1161`, `:1187`) — `qip-kernel/tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate`, `::the_same_order_filled_in_the_next_report_charges_exactly_the_fill`, `qip-api/tests/mesh.rs::an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing` (B22, struck); the rest are Phase 12 and bounded by ADR 0021 |

Summed: **37 of 57 named capabilities at the TESTED bar** (3 + 5 + 0 + 6 +
3 + 14 + 6 over 5 + 7 + 6 + 7 + 5 + 15 + 12), up from 36 of 57 at `e04815e`,
32 of 52 at `584c96b`, 31 of 52 at `296e187` and 25 of 50 at `de5d042`. At
`5290bb9` one capability moved, the ledger plane's books settled from venue
fills (B22), from absent to TESTED. At `e04815e` five
capabilities were added because the blueprint names them and the tree now
has them — four to the execution plane, each TESTED, and one to the ledger
plane, then absent and in flight (B22) — so the numerator rose by four and the
denominator by five; earlier, two were added between `de5d042` and `296e187`
and six moved from absent to TESTED. That number is
this plan's, not the matrix's, and it double-counts nothing but weights every
capability equally, which flatters nothing and nothing in particular: a
missing valuation plane and a missing per-region reservation are both
"one".

### 2.3 Per layer — the matrix carries no status cells for layers

The matrix scores layers as *Current / Keep / Change / Remove / Defer /
Verification*, not with the status vocabulary, so there is nothing to count.
The derivation below maps each layer's matrix row, plus the flow links and
constraint rows that bear on it, to items at the TESTED bar. Same caveat as
§2.2: dispute the item list, not the arithmetic.

| Layer | Items | TESTED | Fraction | Evidence and what is missing |
|---|---|---|---|---|
| 1 Experience | sign-up surface; identity call; passkeys; customer mandate; product entitlements; per-user account; Leptos | 1 of 7 | 1/7 | Flow 1: page TESTED, identity IMPLEMENTED-UNVERIFIED, four MISSING; constraint row §2.1 CONTRADICTS (Next.js). Phase 13 |
| 2 Public edge and identity | one identity store (ADR 0019); sealed sessions; console as VPC viewer (ADR 0018); passkeys | 3 of 4 | 3/4 | `console_route.rs`, `security.rs`; passkeys MISSING (Phase 0 in §51) |
| 3 Application and API | documented endpoints exist; K3's narrower reach is what is built; per-user API; typed-intent surface (§40.9) | 2 of 4 | 2/4 | `documentation.rs::every_documented_endpoint_exists`; K3's reach is now a test rather than a reading — `api_boundary.rs::the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate`, `::the_api_uses_only_the_centre_half_of_the_mesh_and_none_of_its_service_clients` (`827a40e`); `qip-api` composes reads only, none per-user |
| 4 Domain contracts and control fabric | signed payload down; cell verification; atomic swap; §6.2 narrowing wired; outcome return; twelve producers; two independent halt wires; per-region reservation | 6 of 8 | 6/8 | Flow 3 verdict paragraph and `qip-api/tests/mesh.rs`; 3 of 12 payload slots have producers since slot 8's landed at `5396679`/`91d20f5` (PARTIAL); two halt wires that share no failure since `ff86473` — a flag polled on the node beside the signed broadcast, `qip-edge-node/tests/halt.rs::the_node_halts_the_cell_on_a_present_flag_and_releases_it_when_the_flag_is_removed`, flow 6 re-traced at `584c96b` — TESTED at the cell and the node and reached by no running node; F6 CONTRADICTS |
| 5 Data and state | source→facts; entity resolution; world event; bitemporal, bounded, hash-chained log; a `Ledger` per §43.3; central strategy books settled from cell reports; those books settled from venue fills and not placements; live source sustained; BigQuery derived series; content-hash manifests | 6 of 10 | 6/10 | Flow 2 links TESTED; `truth_loop.rs`; the books — `qip-kernel/tests/attribution.rs` (`7ef6063`), new since `de5d042`; ledger PARTIAL by naming; added absent at `e04815e` and TESTED at `5290bb9` — the books are settled from the delta's `fills` and a sent order settles nothing (`central/plane.rs:1161`; `qip-kernel/tests/attribution.rs::a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing`, `qip-api/tests/mesh.rs::an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing`; B22, struck); the last three deferred |
| 6 Cloud and network | `cloudrun` module wired; `execution-node` module wired; `trust-zones` module wired; `egress-proxy` module and sidecar wired; `terraform validate` run; a plan run; anything applied and observed | 0 of 7 | 0/7 | Re-scored at `296e187`: the item this row used to lead with — a GKE transitional runtime carrying traffic — is gone from the tree (`808ca32`, `67b3e92`, `7d79161`), and the three modules that were absent from `main.tf` are now four of its seventeen `module` blocks (`main.tf:274`, `:296`, `:467` and `catalogue.tf:234`; `808ca32`, `c924191`). Wired is CONFIGURED. Re-scored again after `5290bb9` (§6): a real plan and real applies ran against `algorik-dev`/`dev` through CI, not from this shell — GKE was destroyed for real (`8194b3b`), `qip-dev-fastbrain` is a real Cloud Run service serving the attested image (`06bedce`), and `qip-dev-api`/`qip-dev-deepbrain` are blocked on a startup-probe cause found and fixed (`32b344d`) but not yet redeployed. That is MEASURED — a fact about the real project read from CI run output the commits quote — and this document's TESTED bar is a named passing test in the repository, which none of this is or could be, so the fraction stays 0/7 by the document's own rule and not because nothing happened. `terraform validate` itself was never separately run and is not claimed; `infrastructure.rs` remains a text scanner over the HCL (`81dd1cd`: `test result: ok. 59 passed; 0 failed`) and does not change with the cloud. At `e04815e` the node the module boots is configured to run passes — `startup.sh.tftpl:174` writes `QIP_VENUE_FEED=simulated` (`6340610`) — which is CONFIGURED here and TESTED in the binary's own suite, and moves nothing in this row |
| 7 Security, observability, delivery, reliability | three paper layers; LM/quantum authority; WIF only; central telemetry recorded and served; edge telemetry recorded and served; belief calibration recorded; reconciliation break counted on both planes; secret-mount chain exercised live; scrape observed; OTel spans (§47); edge collector and alert; second halt wire; `qip_central_` alerts | 8 of 13 | 8/13 | `security.rs`, `compliance_proof.rs`, `architecture.rs`, `qip-edge/tests/telemetry.rs`; calibration — `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series` (`04738ee`); the break — `qip-kernel/tests/central.rs::a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause` (`de5d042`) and `qip-edge/tests/telemetry.rs`; both new since the first scoring; the second halt wire — `qip-edge-node/tests/halt.rs::the_node_halts_the_cell_on_a_present_flag_and_releases_it_when_the_flag_is_removed` (`ff86473`), new at `584c96b`, its third release direction asserted at `6a515bb` (`qip-edge/tests/telemetry.rs::clearing_the_kill_switch_while_the_polled_flag_is_present_leaves_the_cell_halted`) and the `edge_halted` alert's text naming the polled source at `cd16f79`. Never exercised live: the secret mount (CSI then, Cloud Run secret files now); `workload_metrics_exist=false` everywhere; the execution node declares an Ops Agent receiver and no node exists, and the Cloud Run services have no collector attached (`modules/observability/NOT-SCRAPED.md`); the rest MISSING |

Summed: **26 of 53 layer items at the TESTED bar** (1 + 3 + 2 + 6 + 6 + 0 +
8 over 7 + 4 + 4 + 8 + 10 + 7 + 13) — up from 25 of 53 at `e04815e`, where
layer 5 had gained the item B22 names, absent, and 25 of 52 at `584c96b`; the
one that moved at `5290bb9` is that item, now TESTED; up from 23 at `296e187` — which this paragraph then wrote
as "of 51" over the same seven denominators, whose sum is 52 — and 20 of 48
at `de5d042`. Layer 6 at
zero is still the number to notice — and it is a different zero: before, a
runtime existed in the tree that nothing had been shown to run on; now the
runtime in the tree is the blueprint's, and nothing has been shown to plan
it. §6 explains why nothing about it can be proven from this environment.

### 2.4 Where the scorecards disagree with each other, or with the tree

The table as first written is kept, with a resolution column added at
`296e187`; five of seven were closed by the matrix refresh this plan sits
beside, and two remain the owner's.

| Claim | Where | What the tree said at `de5d042` | At `296e187` |
|---|---|---|---|
| "C4 — still open; the rule file says nothing writes to `Telemetry`" | Matrix, C4 | Closed. `.claude/rules/domains/observability.md` was corrected at `232bc16` and now says both planes emit and names the edge series. The rule file is right; the matrix row is stale | **Resolved** — the matrix's C4 now records the closure, and flags the rule file's next stale sentence (that `learn_from` and `evaluate_alternatives` have no caller; both do since `04738ee` and `b9e2242`) as the owner's |
| "The kernel does not consume cell deltas at all" | Matrix, F7 | `Platform::ingest_cell_report` is called from `qip-api/src/mesh.rs`, and `learn_from_cells` feeds outcomes back. Whether the *contributor vector* joins central attribution is a separate question this plan could not settle by reading | **Resolved by code** — the join landed at `7ef6063` and the sink carries the interval at `7d79161`; F7 records both |
| F8's footgun — a leg that forgets `as_cycle_leg` nets silently | Matrix, F8 | `3632932` and `6053935` landed after the matrix was scored; IMPLEMENTED-UNVERIFIED here | **Resolved** — F8 records `CycleLeg` and the producer (`71f9465`), and the plan's author ran nothing for it: the tests are named in the matrix and were run by their own commits |
| "Tests: 3,308 passing at `fef0c97`" | `current-state.md` | Thirteen commits later; not re-measured | **Resolved at `29ce828`** — the full gate ran on a clean checkout: 302 binaries, 3,485 passed, 1 failed, the failure repaired at `397c144` (A2). `current-state.md` carries the figures, the rest of the gate, and a not-clean re-run at `851c0ed` |
| `NumericFact::observed` has no production caller | `gap-matrix.md` risk register | `480644d` and `125a7de` add an observed-fact constructor; the register's open count of three is likely two | **Resolved** — the register was recounted at `d4dcd44`, `67a584d` and again in this refresh: twenty-four found, twenty-three closed, one open |
| ADR 0023 step 3 "buildable today" | ADR 0023 | The record is in tension with itself; §5 lists it for the owner | **Overtaken in practice** — the feasibility gate (`95a4932`) and the attribution join (`7ef6063`) were built; the ADR text is unchanged (D10) |
| Blueprint §48 and rule 77: OpenTofu, Cloud Build, Cloud Deploy | Not scored anywhere | The matrix has no row; the matrix owner's call | **Still open** (D11). `deploy.yml` now moves Cloud Run services by digest itself (`b85684f`), which makes the row easier to write and no less the owner's |

---

## 3. What landed this session

Grouped by merge. When first written, merge bodies could not be read from
this environment (no shell; git objects are not readable as text), so PR
numbers 1–5 were inferred from the reflog's five fast-forwards of the target
branch and from `current-state.md`'s citation of "the PR #5 body for
`fef0c97`", with commit subjects from `.git/logs/HEAD`. PR #6 was read with
`git show 851c0ed`: its parents are `baffcd8` and `b1e709c`, so the three
rows below that were labelled unmerged are what it merged, together with
the fourteen commits after `fca98cc`.

| Merge | Commits | What landed, one line |
|---|---|---|
| PR #1 — `d8b3597` | before this branch | The GitOps cut-over (Argo CD, Kargo) — ADR 0020 names it; this branch was cut from it |
| PR #2 — `19241d8` | `4541923`..`8df1658`, 12 | Solver authority held to the LM rule; the two blueprint conflicts recorded; the traceability matrix; the §6.2 degradation type; ADR 0022; ADR 0023; manifest parser hardened then replaced with Cargo's own; the two credential windows bound; the seven-flow truth pass; the blueprint-vs-diagram reconciliation |
| PR #3 — `acfece3` | `9b8df9b`..`0c91cfa`, 9 | The twelve-item payload's wire shape; a cell that verifies, applies, narrows and halts on it; the reservation ledger, then wired into the kernel; the halt as a signed command; the centre shipping policy; the Deep Brain's reference universe and its own exchange; injective signing strings |
| PR #4 — `7f508cc` | `3be9855`..`db8ce8b`, 21 | One live market source behind the licensing gate, then selectable; `Intent` and netting in the cell, self-trade prevention; the `cloudrun`, `trust-zones` and `execution-node` modules, all unwired; the network module's blueprint notes; GitOps job identity; the console's order ticket deleted; the venue credential refused where the ceiling cannot use it; four unbounded collections bounded; money out of `f64` in risk and execution; the brute-force lockout made able to fire; the safest rung no longer reported live-capable |
| PR #5 — `baffcd8` | `64b765a`..`fef0c97`, 7 | `egress.rs` able to tell a deployed proxy from a described one; contributor attribution and the uplink schema bump; internal crossing at the mid with the forty-percent cap; the rounding remainder returned; the uplink proven; two scored documents corrected |
| PR #6 — `851c0ed`, first part (was "unmerged — this branch") | `68b7da6`..`de5d042`, 13 | Edge telemetry parked, then finished and proven site by site; the node hands its cell the scraped registry; the observability rule corrected (C4); three Argo CD Applications that could never sync removed; five documents corrected; the reservation shortfall counted under its registered name; observed-fact constructors for agents; a cycle-leg type that cannot be nettable; the arbitrage scanner's leg emitter; a central reconciliation break and its scoped halt recorded and counted |
| PR #6 — `851c0ed`, second part (was "unmerged, since the plan was first scored") | `de5d042`..`296e187`, 57 | **Risk:** tail figures derived from each limit's own confidence in the risk lib and the kernel (`d94b156`, `990032a`); a pass-and-veto fixture for every `LimitKind` arm (`160c4e8`); the aggregates-never-strategy-lists rule made structural and probed at two counts (`b9e9e7d`). **Lifecycle:** cumulative trials per family (`9332bcb`), one Sharpe arithmetic (`436e1fa`), the holdout band and its demotion (`d0558b4`), factory enrolment (`94dd7e2`). **Sealed seams:** the registered outcome behind the legality assessment (`47e9b81`), a proposal's status private (`6e3aad0`), the synthetic path refusing overflow rather than restarting (`cc92d66`). **The cell:** feasibility ahead of netting (`95a4932`), the arbitrage desk scanning the cell's own books and halting a broken cycle (`71f9465`), buy signed positive where the intent is made (`54d32fd`). **The centre:** LEARN grades every resolved thesis (`04738ee`) and prices every refused order (`b9e2242`); the cycle recorded where the console reads (`cf20457`); netted fills attributed and crosses settled to strategy books, the report carrying the interval (`7ef6063`, `7d79161`); a reorganisation failing the bridged transfer riding on it (`67b3e92`); the not-decision-grade instruments counted at assembly (`78026e2`); fourteen newer series spelled beside the rest (`296e187`). **The sweep:** 99 orphan public functions removed (`a4f673c`), four resolved by removal (`ed69a52`, `68ff891`, `b7d3edc`, `b8a8acd`), 110 then 32 dead dependency edges dropped (`2a74706`, `2753911`), the bench profile and a frontend config reader gone (`a95f702`, `ad9a937`). **Infrastructure:** the first egress proxy that exists, as a co-located sidecar (`c924191`); the blueprint runtime wired into the root module and the cluster's Terraform, chart, manifests and Argo CD stack removed (`808ca32`, `67b3e92`, `7d79161`); `deploy.yml` moving Cloud Run services by digest with the serving revision proven (`b85684f`); the resource-by-resource record (`bcad2d3`) — nothing applied, no `terraform` binary. **Proof of boundaries:** the application layer's reach made executable (`827a40e`); three premise-first test repairs (`08c52a0`, `4916217`, `6dd761b`). **Documents:** the truth pass, current state, gap matrix, traceability matrix and this plan re-scored (this series) |
| PR #6 — `851c0ed`, third part (was "unmerged, landed while this plan was being re-scored") | `296e187`..`fca98cc`, 5 | Every desk fill fed into the risk aggregate and limits read from it, so the O(1) rule is held in production and not only by the lib's test (`88eb1e2`); the three deployment suites retargeted at the runtime that exists, every property kept (`81dd1cd`); the operations documents and two rule files corrected for the deleted runtime (`ecfb0a6`); ADR 0024 (`2b7e502`); the scaling runbook pointed at the catalogue (`fca98cc`) |
| PR #6 — `851c0ed`, last part | `fca98cc`..`b1e709c`, 14 | The four scored documents refreshed (`9aa3b27`, `ba05d1d`, `5734f4f`, `79dbb8b`) and this plan re-scored (`29ce828`); the full gate run at `29ce828` — fmt exit 0, clippy `-D warnings` exit 0, 302 test binaries with 3,485 passed and 1 failed, `all permitted`, `nothing found` — and its one failure, the `qip-api` scrape premise, repaired and mutation-verified (`397c144`); the four delivery-stack scripts retired (`25d066e`); the release-engineer agent file, `CLAUDE.md`, the secrets document, the observability rule file and five runbooks corrected for the runtime the tree holds (`e894198`, `eb0442e`, `033ee11`, `132c1b7`, `94480b4`); the two diagram audits corrected (`ffa3c7a`); the retired-stack allowlist expired to empty (`b1e709c`). Merged with the thirteen checks `ci.yml` declares reported green on `b1e709c` — reported, because there is no `gh` here to read them |
| PR #7 — `a084860` | `851c0ed`..`cfe11c1`, 27 (`git log --oneline 851c0ed..cfe11c1 \| wc -l`) | Merged into the default branch on 2026-09-02 with **13 check runs on `cfe11c1`, every one `completed` / `success`** — read from the GitHub API's check-runs listing for this refresh, not reported; the merge commit is not in this checkout's history, which is why it is cited from the API and not from `git show`. What it carried: the crossing interval (`153e429`), the second halt wire (`ff86473`), the desk installed in the node (`584c96b`), the durable, quarter-budgeted trial book (`aa66c5d`, `e31aae4`), the fed aggregate and the cell's fills charged into it (`588335a`, `98bc687`), ADR 0025 and ADR 0026 proposed (`02031f1`), the scorecards re-scored (`cfe11c1`) |
| Wave 4 — unmerged, this branch | `cfe11c1`..`e04815e`, 13 | The node's pass loop over the in-process simulated venue, any other feed refused at start naming ADR 0003 (`6340610`); the third halt-release direction proven (`6a515bb`); a fill recorded only when the venue reports one (`cb79b46`); pricing stated by the strategy or refused, a rested order withdrawn at its time to live (`383d4e7`); the node proven trading pass after pass (`b8d18d3`); the two acceptance fixtures given a policy (`e04815e`); the cycle whitelist produced from an operator policy and the desk's grant (`5396679`) and shipped from the API's policy seam (`91d20f5`, `73a1694`); every central root assembled from the committed catalogue (`8224509`) and the catalogue mounted on the three workloads (`e40335d`); the `edge_halted` alert naming the polled source (`cd16f79`); ADR 0027 proposed (`360cfd8`). No gate result is quoted for this wave: the full gate is running as this plan is written and its figure is not invented here |
| B22 close-out, `5290bb9`..`095144b`, 12 | 12 | The wire bug that billed placements as fills closed, six commits ending `5290bb9` (§2.3, B22 above); the register, the two flow rows and the honest paragraph re-stated to say the distance is an absence again (`e5acf4e`, `1bb6390`, `20f20d5`, `162c91a`); the observability rule's central-break paragraph corrected (`4a46431`, `1820068`); the last test-only reader of `orders` as positions corrected (`095144b`) |
| Wave 5, `da0789d` | 1 (three agents' work landed as one commit) | `Platform::new` appends a `UniverseAssembled` record as the log's first link — catalogue hash, instrument count, membership digest — so a cycle over an unrecorded universe is unrepresentable; the edge node deploys strategies from the payload with a pricing policy; the console renders sent against filled, breaks by direction, the halt wires and the universe. The commit itself repairs five test failures the three parallel agents' work produced when run together: two stale expectations, one new test that had never passed, one deployment-manifest gap, and a real bug — `qip-deepbrain`'s archive watermark derivation broke once assembly started writing to the log before the derivation read it, and the fix is `Platform::inherited_through`, captured before the write rather than derived after |
| Infrastructure-pipeline debugging, `da4b85e`..`2e19a4c`, 13 | 13 | Real `infra.yml`/`deploy.yml` dispatches against `algorik-dev`/`dev`, not from this shell: a `gcloud` flag-ordering bug that had failed every deploy since the sidecar landed (`da4b85e`); a deadlock where the bootstrap never passed its digests file and the pipeline only ever moved a service, never created one (`1271f2c`); a null-guard bug that failed every plan (`ceff962`); an unsupported `mount_options` argument (`a4811b8`); a GKE teardown that destroyed 58 resources and needed two IAM-ordering fixes plus a decision not to cascade-delete the journal's backups (`5e42347`, `b7a2c87`); a Binary Authorization attestation gap between the index GKE asked about and the per-platform manifest Cloud Run asks about, fixed in `vendor.yml` (`8194b3b`); an untaint step for a service whose first revision failed and could be neither destroyed nor recreated (`06bedce`); a CI shellcheck fix that had kept every deploy refused (`f8a9245`); a rollout diagnosis step and the log-read role it needed (`4336da4`, `6d52f8e`); the startup-probe cause found — the egress sidecar's health listener bound to loopback, invisible to Cloud Run's external probe — and fixed (`32b344d`); the same bootstrap's second reader, the execution-node module, given the same one-listener exception (`2e19a4c`). Result, precisely: `qip-dev-fastbrain` confirmed serving the attested image; `qip-dev-api`/`qip-dev-deepbrain` blocked on the probe fix, not yet redeployed. §6 carries the full account |

Velocity, for what it is worth: sixty-two commits in roughly thirteen hours of
reflog (`4541923` at 1788257119 to `de5d042` at 1788304174) when first
scored, of which roughly a third were documents and corrections to documents;
fifty-seven more by `296e187` (committer time 1788310157). That ratio is the
programme working as designed — a scorecard that lags the tree is the failure
this repository has already named twice — and it is also the reason §2's
numbers are trustworthy enough to plan on.

---

## 4. The remaining work, as a sequenced backlog

One **slice** = one agent, one review, one PR. Estimates are in slices because
that is the unit this programme runs in; a slice that turns out to need two is
reported as two, not stretched.

Sequencing within (i) and (ii) is by dependency first, consequence second.
Every item names: the blueprint section; the finish line (A = alignment,
P*n* = blueprint phase *n*); dependencies; the blocker if any; the evidence
that closes it; the size.

### (i) Alignment work still open

| # | Item | Blueprint | Line | Depends on | Blocker | Evidence that closes it | Slices |
|---|---|---|---|---|---|---|---|
| A1 | ~~**Refresh the matrix and the truth pass for what landed after `fef0c97`**~~ | ADR 0022 | A | — | — | **Done in this series**, at `296e187`: C4, F7, F8, the gates, the constraint and plane rows, Layers 3, 4, 6 and 7; the truth pass's flows 2, 3, 6 and 7 re-traced. Every changed row cites a commit or a test name. What is not done is the OpenTofu/Cloud Build row, which is D11's | 1, spent |
| A2 | ~~**Re-measure the full gate at HEAD**~~ | — | A | — | — | **Done at `29ce828`**, clean checkout: `cargo fmt --all --check` exit 0; `cargo clippy --workspace --all-targets -- -D warnings` exit 0; `cargo test --workspace --no-fail-fast` 302 binaries, 3,485 passed, 1 failed — the `qip-api` scrape premise, repaired and mutation-verified at `397c144`, after which `cargo test -p qip-api` gives `3 passed; 0 failed` for the module, so 3,486 is implied at `397c144` and was not re-run whole; `dependency policy: 11 third-party package(s), all permitted`; `secret scan: nothing found`; `git diff --check` clean. Terraform not run (no binary); frontend not run (no source changed). The evidence asked for was every binary's `test result:` line; what exists is the summed totals in `397c144`'s message. A second run at `851c0ed` on a not-clean tree is in `current-state.md` | 1, spent |
| A3 | ~~**Controls that cannot fire, remaining:** `Platform::learn_from`, `Platform::evaluate_alternatives`, `Web::record_cycle`~~ | §47, §12 | A | — | — | **Done, all three wired:** `learn_from` from LEARN (`04738ee`, `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`); `evaluate_alternatives` from LEARN (`b9e2242`, `::a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate`); `record_cycle` from the cycle route (`cf20457`, `api.rs::a_cycle_run_through_the_router_reaches_the_operator_interfaces_stage_overview`). Two more found and wired since: `BridgeLedger::on_reorg` (`67b3e92`) and `Universe::not_decision_grade` (`78026e2`) | 3, spent |
| A4 | ~~**Assert the O(1)-in-strategy-count property of risk**~~ | §2.2, rule 11 | A | — | — | **Done** at `b9e9e7d`: `qip-risk/tests/aggregate.rs::the_aggregate_check_reads_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve`, mutation by rewriting the gross figure as a per-strategy sum | 1, spent |
| A5 | **Settle `qip-arbitrage` and `qip-normalization`** — half done. `qip-arbitrage` is constructed by the cell (`71f9465`, `Cell::with_arbitrage`), so its edge is live; `qip-edge-node` installs one since `584c96b` (`ArbitrageInstaller`, `apps/qip-edge-node/src/arbitrage.rs:249`) from `CycleWhitelist::conversions`, which the kernel produces and the API ships since `5396679`/`91d20f5` (B18, spent), and since `6340610` the node runs passes; a desk is installed in a node that runs while the centre has `QIP_ARBITRAGE_POLICY_PATH` set, and no node is deployed, so no deployed process holds one. **The normaliser half is decided: ADR 0029 removes it.** Not research-only — the record found that the crate does not do what its own doc comment claims: `Normalizer::normalise` (`src/normalizer.rs:244`) never reads `self.symbols` or `self.drop_unmapped`, so the "drop records whose symbol has no mapping" guard the field's comment promises cannot fire, `MaxExpectedShortfall` again; and four documents cite it as a running control (`docs/security/threat-model.md:206-209`, `infrastructure/terraform/modules/data/NOT-PROVISIONED.md:31-35`, `docs/architecture/current-state-audit.md:86`, `docs/performance/budgets.md:50`, whose published 0.31 µs describes "symbol mapping, unit conversion, quality stamp" for a call that does neither the mapping nor the stamp). **What is applied is the record only** — `docs/adr/0029-the-normaliser-is-removed-rather-than-recorded-as-research-only.md`, its index entry, and this row. The deletion and the four corrections are one atomic change nobody has made: removing the directory while `backend/Cargo.toml:40` still names it a workspace member makes `cargo` refuse to load the workspace, so the delete travels with `backend/Cargo.toml`, `backend/Cargo.lock`, `qip-acceptance/Cargo.toml:32`, and `architecture.rs`, `truth_loop.rs` and `performance.rs`. Note for whoever lands it: `architecture.rs:795` asserts `services.len() >= 25` against exactly 25 service crates, so the removal fails that test on its premise; `>= 24` is lowering a bar to obtain a pass and is refused — replace the floor with an equality between `cargo metadata` and the services directory | §30, §7.3 | A | A venue for the desk | — | For the desk: the node constructing it and a gateway test seeing a leg; for the normaliser: the crate gone from the tree, the four documents corrected, and `cargo test --workspace --no-fail-fast` green | 1 each |
| A6 | **A collector for every emitter, and an alert for `qip_central_` and `qip_belief_` descriptors** — re-stated for the runtime the tree now describes. The execution node's startup template declares an Ops Agent Prometheus receiver on its health port (`808ca32`); no node exists. The Cloud Run services have no collector: the managed-Prometheus sidecar needs a digest mirrored and attested first (`modules/observability/NOT-SCRAPED.md`), and nobody has pinned it. The old `PodMonitoring` left with the cluster | §47 | A | Someone mirroring the sidecar image by digest through `vendored-images.txt` and `vendor.yml` | Binary Authorization admits only attested images, so an unattested sidecar reads as a broken deploy | The sidecar attached in `modules/cloudrun` by digest; an alert policy naming a recorded `qip_central_` or `qip_belief_` descriptor; the names test extended — and, separately, an observed scrape before `workload_metrics_exist` flips | 1 |
| A7 | ~~**Correct the rules files**~~ | — | A | — | — | **Done.** `ecfb0a6` re-stated `.claude/rules/domains/data-and-streaming.md` against `modules/egress-proxy`; `132c1b7` re-stated `.claude/rules/domains/observability.md` against the LEARN callers — at `851c0ed` its lines 83-89 name `Platform::learn_from` and `Platform::evaluate_alternatives` as called from LEARN with the `platform.rs` lines and the commits (C4) | 1, spent |

Alignment-done after (i), at `851c0ed`: **A1, A2, A3, A4 and A7 are spent;
A6 waits on a mirrored sidecar digest; A5 waits on the owner.** One to two
slices of agent work remain, none of them code in the cycle. Layer 6 stays at 0/7 regardless — see
§6.

### (ii) Phase 0–3 blueprint work

Phase 0 and Phase 1 are where the repository is genuinely behind, however far
ahead it is elsewhere. This is the critical path to the first gate.

| # | Item | Blueprint | Line | Depends on | Blocker | Evidence that closes it | Slices |
|---|---|---|---|---|---|---|---|
| B1 | ~~**Decide and record the egress path**~~ | §46.2 network, §45.1 | P1 (exit) | — | — | **Done**: taken in code as the co-located sidecar (`c924191`, D1) and recorded at `2b7e502` — ADR 0024, which quotes the owner's instruction as the authorisation for the code and states that nothing was applied. At `296e187` the record was absent and `b85684f` cited it ahead of its existence; it landed five commits on | 1, spent |
| B2 | **Plan, apply and observe the sidecar** — this row used to be "switch the GKE proxy on"; that proxy and its manifest are gone (`7d79161`) and D4 with them. **Partly done, after `5290bb9` (§6):** a real plan and real applies ran through CI against `algorik-dev`/`dev` — not `terraform validate` by name, but a plan that computed a full diff and an apply that built the sidecar and, for `qip-dev-fastbrain`, ran it — and the sidecar's health listener bind was corrected once Cloud Run's own probe caught it wrong (`32b344d`). **Still open:** the fix has not been redeployed and observed for `qip-dev-api`/`qip-dev-deepbrain`, and no vendor request through the sidecar's allowlist has been observed in a log, because the allowlist names no vendor host (D9) | §46.2 | P1 (exit) | B1, B3 | A redeployment observed for the two blocked services; a vendor host in the allowlist (D9) | The plan's preconditions refusing a host outside `egress_allowed_upstreams` and admitting one inside; a request through the allowlist observed. `egress.rs` was refitted from the deleted manifest to the sidecar at `81dd1cd` (14 passed, per its message), so the suite half is done; the observed-request half is still open | 1 |
| B3 | **Name the market-data vendor host and record its licensing posture** — the bootstrap's five clusters are Google and IBM endpoints (`infrastructure/egress/envoy.yaml:392-492`); no vendor | §7, rule 40 | P1 | — | **D9** | A `qip-data-finder` posture record, a cluster in `envoy.yaml` and its host in `egress_allowed_upstreams`; nothing else changes | 1 |
| B4 | **Seven days of stable streaming with statistics converged and no raw stream retained** — the Phase 1 exit | §51 Phase 1, rule 32 | P1 (exit) | B2, B3 | A deployment (§6) | Seven days of a scrape series, the feature store's bound held, the licensing posture in the journal | 1 to observe; 0 to build |
| B5 | ~~**Count trials cumulatively across runs**~~ — done at the crate and the factory (`9332bcb`, `94dd7e2`: `lifecycle.rs::a_promotion_whose_lifetime_trial_count_is_unknown_is_refused_naming_what_to_do`, `::a_second_run_is_corrected_against_the_first_runs_trials_as_well`, `::a_trial_book_replays_its_journal_from_the_store_and_refuses_a_tampered_one`). **The durable book landed at `aa66c5d`:** every root calls `Platform::open_trial_book` (`platform.rs:1624`) on the `trial-book` namespace of its own store (`qip-api/src/main.rs:106`, `qip-fastbrain/src/main.rs:164`, `qip-deepbrain/src/main.rs:178`), `set_central` carries the book across a plane swap, and a journal that does not verify is a refusal to start rather than an empty book — `qip-kernel/tests/trial_book.rs::a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward`, `::a_journal_whose_count_was_lowered_by_hand_refuses_to_open_and_nothing_is_attached`, `::a_plane_swapped_in_after_the_book_was_opened_keeps_the_durable_book` | §20.1, rules 24–25 | P2 | — | — | Done: the roots open the book, and the reopen test is the one this row asked for | 1, spent |
| B6 | ~~**Define the holdout band as an output of validation**~~ | §20.1, §51 Phase 3 gate | P2 | — | — | **Done** at `d0558b4`: `HoldoutBand::from_deflated` at the gate, carried on the `Admission`, refused off it, two-sided at the demotion monitor — `lifecycle.rs::a_holdout_admission_carries_the_band_its_validation_produced`, `::live_performance_outside_the_holdout_band_is_demoted_and_counted`, `::judging_or_admitting_without_a_holdout_band_is_refused` | 1, spent |
| B7 | **Attempt the Phase 2 gate on real data** | §51.1 | P2 (gate) | B4, B5, B6 | Phase 1 evidence (§6) | A family surviving holdout after cumulative correction, recorded in `qip-lifecycle/src/evidence.rs`'s own artefact — or a recorded failure, which the blueprint says is the more likely and the more useful result | 1 to run |
| B8 | **Passkeys** — `grep -rln -i passkey backend/crates frontend/portal/src` is still empty at `296e187` | §51 Phase 0, §40.3 | P0 | None | None known | An authenticator registration and assertion through Identity Platform; the grep non-empty; Playwright for the browser half | 2 |
| B9 | **PQC keys and real signatures for the payload channel** — depends on the crypto decision | §46.2 keys | P0 | — | **D2** | An ADR admitting a vetted crate, or an ADR declining and amending ADR 0002's reversal clause; then KMS-backed signing in place of `hmac_sha256` (`qip-core/src/hash.rs:163`) on the policy and envelope channels | 1 (ADR) + 2 |
| B10 | **Feasibility gate ahead of the profitability filter** — done at the cell (`95a4932`: `admit_feasible` ahead of `net` in `Cell::work`, eight refusal literals, `feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order`), and slot 11 of the payload is its first consumer with no producer. **Open:** the central pre-trade path in `qip-execution-engine` has no feasibility step. The other half this row carried — that no deployed process runs a cell pass — closed at `6340610`: the node drives `Cell::work` from `run_pass` (`qip-edge-node/src/pass.rs:118`, called at `main.rs:586`) when `QIP_VENUE_FEED=simulated`, so the gate is in the deployed binary's loop — `qip-edge-node/tests/pass.rs::a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move` — and MEASURED nowhere, because no node is deployed | §18.1, rule 23 | P3 | — | None in code; D10 in principle, overtaken in practice | The same fixtures beside the central pre-trade path; a producer for slot 11 | 1 |
| B11 | ~~**Join the edge contributor vector to central attribution** and settle a cross to the books~~ | §27.1, §43.4, rule 12 | P3 | — | — | **Done** at `7ef6063` and `7d79161`: `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual`, `::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact`, `::a_cross_naming_two_buyers_is_refused_rather_than_split_evenly`; `qip-api/tests/mesh.rs::the_orders_a_cell_reports_reach_the_centres_strategy_books` (renamed `the_fills_a_cell_reports_reach_the_centres_strategy_books` at `d59505d`, B22) | 2, spent |
| B12 | **Per-region reservation table** (F6) — still absent. **Re-verified after wave 5 (`da0789d`) and the infrastructure work through `2e19a4c`: unchanged.** `grep -n -i reserv backend/crates/edge/qip-edge/src/cell.rs` finds nothing but "preserved"; `grep -rn -i reserv backend/crates/apps/qip-edge-node/src` and `grep -rln -i "per.region\|per_region" backend/` (excluding the unrelated ADR-0008 comment and capital-fabric doc comment) find no per-region reservation mechanism anywhere in `backend/crates/edge` or `backend/crates/runtime/qip-kernel`. Wave 5's deployment work (strategies installed from the payload) did not touch this; it is a different mechanism | §4.2, §26, §33, rule 21 | P3 | — | None in code | A disconnected cell refusing its own second proposal against one envelope; the central ledger unchanged; `apps/qip-edge-node/tests/mesh.rs` extended | 2 |
| B13 | ~~**Set the internal-crossing interval**~~ — **spent as code at `153e429`, with D3 still open.** `CellConfig::crossing_interval` (`cell.rs:70`) is `Passes(n)` or `Span(d)` (`:80-85`), refused at zero, negative or longer than the 1,024-sample history (`with_crossing_interval`, `:118-144`), and `None` by default (`:107`) — byte for byte the per-net arithmetic, so a full cancellation still never crosses until the owner sets it. Set, the cap is one window per net key, this cross plus the window's crossed against this net plus the window's gross, refused whole above two fifths; a window missing its oldest sample refuses under `internal_cross_window` (`:2063`). `qip-edge/tests/crossing.rs::over_a_three_pass_interval_a_repeated_full_cancellation_crosses_on_the_second_pass_at_the_mid`, `::with_no_interval_the_same_two_passes_never_cross` | §27.1 | P3 | — | **D3** — the code takes any interval and `grep -rn with_crossing_interval backend/crates/apps` finds no root setting one | An interval chosen by the owner and set in the node's configuration; the cell's tests hold the rest | 1, spent as code; the owner's half open |
| B14 | **Twelve producers for the twelve payload slots** — three produced (`capital_grants`, `cycle_whitelist`, `risk_envelope` — slots 7, 8 and 9 in `PolicyPayload`'s own field order, `qip-contracts/src/policy.rs:388-421`), nine still ship unproduced and narrow the cell: `trained_models`(1), `compiled_plan`(2), `belief_priors`(3), `episodic_digest`(4), `causal_digest`(5), `regime_state`(6), `inventory_targets`(10), `feasibility_constraints`(11, first consumer at `95a4932`, `grep -rn feasibility_constraints backend/crates/apps/qip-api/src backend/crates/runtime/qip-kernel/src` still empty), `adversary_profiles`(12). **Re-verified after wave 5 and the infrastructure work through `2e19a4c`: the count is unchanged** — `qip-api/src/mesh.rs`'s own comment still reads "today is three of the twelve items," and wave 5 added a kernel-log record (`UniverseAssembled`) and node-side deployment, neither of which is a payload-slot producer. The phase mapping this row previously carried listed item 9 (`risk_envelope`) as still needing P3 work; that was already stale before this refresh — `risk_envelope`'s producer has existed since `61f9392`, before this document's first scoring — and is corrected below | §41.5 | P3 for items 2, 10, 11 (buildable now: a plan compiler, inventory targets from the risk lib's own state, a feasibility-constraints producer at the centre paralleling the cell's consumer); later phases for 1 (trained-model manifest depends on the lifecycle's promotion output reaching a manifest type), 3–5 (belief, episodic, causal are P7–P8 by name), 6 (regime detection exists in `qip-cost-router/src/context.rs` but nothing serialises it into `RegimeState`, so it is plausibly P3-adjacent and not scored as blocked here), 12 (adversary profiles wait on Phase 18's adaptive simulation agents, which do not exist) | — | Most slots have no producing plane yet (belief, episodic, causal digest, self-model are P7–P9); three are code-buildable today against planes that already exist | Per slot: a producer, the cell consuming it, and the §6.2 row it un-narrows | 1 per slot; 3–4 in P3 |
| B15 | ~~**Second, independent halt wire**~~ — **done at `ff86473`.** `qip-edge-node` reads `QIP_HALT_FLAG_PATH` on every pass of its loop, before the flush and the mesh exchange (`main.rs:482-483`; `HaltFlag::poll`, `halt.rs:100` — two syscalls, nothing off-machine) and hands the reading to `Cell::apply_polled_halt` (`cell.rs:558`); engaged, unreadable or malformed halts, absent releases, no payload and no other wire releases it; `work` refuses under `polled_halt`; `qip_edge_halted{source="polled"}`. `qip-edge-node/tests/halt.rs` (five tests), `cell.rs::polled_halt_tests::the_polled_wire_and_the_kill_switch_release_each_other_never`, `qip-edge/tests/telemetry.rs::a_polled_halt_moves_its_own_gauge_refuses_the_pass_under_its_own_gate_and_no_payload_releases_it`; the execution node's template installs the directory root-owned and sets the variable (`startup.sh.tftpl:148-149`, `:172`). The third release direction — clearing the kill switch while the flag is present — is asserted since `6a515bb` (`qip-edge/tests/telemetry.rs::clearing_the_kill_switch_while_the_polled_flag_is_present_leaves_the_cell_halted`), and the `edge_halted` alert's text names the polled source since `cd16f79`. Flow 6 re-traced at `584c96b` and `e04815e` | §46.2 kill switches | P3 | — | Still a file a person writes — the managed store that would write it is deployment work (§6) — and no node runs | Done: flow 6 walks two wires that do not share `qip-transport`'s failure | 1, spent |
| B16 | ~~**ADR 0020 step 1 — establish which GKE workloads have ever run**~~ | §41.4 | — | — | — | **Moot.** The evidence was never gathered, and at `808ca32` there is no cluster in the tree to gather it from; the owner's instruction to devour the old runtime replaced the step. Recorded so nobody reads the step as passed: no process has ever been shown to run on either runtime | 0 |
| B17 | **Validate the wired modules** — the wiring half is done (`808ca32`: `cloudrun`, `execution-node`, `trust-zones`, `egress-proxy` in the root module; D5 taken for the code). **Partly done after `5290bb9` (§6):** real plans ran against `algorik-dev`/`dev` through CI and refused two genuine bugs before admitting the modules — a null-guard in `variables.tf` that evaluated both branches (`ceff962`) and an unsupported `mount_options` argument on the GA provider (`a4811b8`) — both fixed and re-planned successfully; the `execution-node` module's own variable validation refused a bootstrap the `egress-proxy` module now admits, fixed at `2e19a4c` and not yet re-planned. `terraform fmt -check` and `terraform validate` by name are still not run and not claimed; what ran is `plan`, which is a stronger check on the same HCL but a different claim | §41.4, §45.1, §46.1 | P3 (node), P16 (regions) | — | A dispatch confirming the `execution-node` fix plans clean; `terraform fmt -check`/`validate` output quoted by name; apply approval per step for anything beyond `dev` | `terraform fmt -check` and `validate` output; a plan that refuses a bad value (an undeclared zone, a missing digest, a host outside the allowlist) and admits a good one; nothing applied without step-named approval | 1 to confirm the last module plans clean |
| B18 | ~~**A producer for the cycle whitelist's edges**~~ — the desk was installed in the node at `584c96b`: `ArbitrageInstaller` (`qip-edge-node/src/arbitrage.rs:249`) holds the grant for the desk's strategy (`QIP_ARBITRAGE_STRATEGY`) and builds a desk from `CycleWhitelist::conversions` and `start_sizes` (`qip-contracts/src/policy.rs:333`, `:338`; additive to the signed shape, `qip-edge/tests/whitelist.rs::a_payload_signed_before_the_structured_whitelist_existed_still_verifies`), every conversion checked against the cell's venue list, refused on a degraded cell, an empty or stale whitelist, a grant for another strategy or a second desk — `qip-edge-node/tests/arbitrage.rs::the_node_installs_a_desk_from_the_payloads_whitelist_once_capital_for_it_has_arrived`, `::a_whitelist_naming_a_venue_outside_the_configured_list_is_refused_and_installs_nothing`, `::a_degraded_cell_and_an_empty_whitelist_install_no_desk`, `::a_grant_for_another_strategy_is_refused_by_the_installer_rather_than_held`. **Done at `5396679` and `91d20f5`:** `CentralPlane::cycle_whitelist_for` (`central/plane.rs:612`) derives `conversions` and `start_sizes` from `CentralConfig::arbitrage` (`:124`) and the desk's live grant through `ArbitragePolicy::whitelist_for` (`central/whitelist.rs:267`), empty with its reason when the policy is unset or the strategy holds no grant at that cell; `Platform::issue_cycle_whitelist` (`platform.rs:1572`) journals the issue; `qip-api`'s `pending_policy` ships it per cell (`mesh.rs:663`) from a policy read at `QIP_ARBITRAGE_POLICY_PATH` (`main.rs:374`; `73a1694` registers it as read by the API and unset on Cloud Run) — `qip-kernel/tests/central.rs::an_unset_arbitrage_policy_emits_an_empty_whitelist_that_says_why`, `::a_signed_payload_carrying_the_whitelist_round_trips_and_verifies`, `::issuing_a_whitelist_through_the_platform_journals_what_was_issued`; `qip-api/tests/mesh.rs::a_cycle_ships_the_desk_a_live_grant_funds_as_a_whitelist_the_cell_verifies`, `::without_a_policy_the_whitelist_ships_empty_and_the_cycle_says_the_policy_is_unset`; the operator's half is `docs/operations/arbitrage-policy.md` | §30, §41.5 item 8 | P3 | B14 (slot 8) | — | Done for the producer and the shipping. The second half of what this row asked — a node test seeing a cycle's leg placed from a shipped whitelist — this refresh did not find: the node's pass tests drive a strategy (`qip-edge-node/tests/pass.rs::firing_strategy`), not a desk. Recorded so the strike-through is read as the producer and not as a desk proven trading | 1, spent; the desk-through-a-pass test open |
| B19 | ~~**Feed the exposure buckets the bucket limits read**~~ | §26, §33 | P3 | — | — | **Done at `588335a`**: each instrument's sector, country, asset-class and venue buckets are projected from the universe at assembly (`platform.rs:1144-1150`) and fed at both seams — the pre-trade projection (`submit_order`, `:4486-4492`) and the fill (`aggregate_fill`, `:4723`; `exposure_axes_for`, `:4760`) — `qip-kernel/tests/risk_aggregates.rs::a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused`, `::an_order_that_keeps_its_sector_bucket_under_the_cap_is_admitted`. Two limits that could never fire now can, and what they did first is **D13**: a share-of-gross cap refuses the first order into an empty book, so two kernel fixtures drop `MaxConcentration` and say why (`tests/capital.rs:57-69`, `tests/risk_aggregates.rs:136-144`); the caps and the default set are untouched. The last sentence this row carried — that the three roots assemble `Universe::new()`, so no deployed process feeds a bucket — closed at `8224509`: every central root loads `data/datasets/universe.json` from `QIP_UNIVERSE_PATH`, refused unset, the manifest recorded under its hash (`qip-api/src/main.rs:346`, `qip-fastbrain/src/main.rs:282`, `qip-deepbrain/src/main.rs:367`; `qip-financial/src/catalogue.rs::load` at `:154`, `record_manifest` at `:239`), and `e40335d` mounts the file at `/etc/qip/universe.json` on the three Cloud Run workloads; the deep brain's replay branch keeps the empty universe on purpose (`qip-deepbrain/src/main.rs:230`). So a deployed desk would feed every bucket and refuse its first order — pinned by `qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_refused_by_the_default_concentration_cap_until_adr_0027_is_decided` (`:491`) — which is B23 | 1, spent; D13 framed by ADR 0027 and open |
| B20 | ~~**Charge a cell's reported fills into the centre's risk aggregate**~~ | §26, §33, rule 11 | P3 | — | — | **Done at `98bc687`**: `CentralPlane::settle` records each venue fill it books on the `Settlement` (`central/plane.rs:1081`) and `Platform::ingest_cell_report` charges that list under the cell's id as the aggregate's strategy axis (`charge_cell_fills`, `platform.rs:1720`) — a counter per cell, bounded by the deployment's cell list, so the O(1) rule holds; crosses are not charged; a refused report is charged nothing. `qip-kernel/tests/risk_aggregates.rs::a_cells_fills_are_charged_into_the_aggregate_and_the_next_desk_order_is_refused_on_leverage` | 1, spent |
| B21 | ~~**Budget each family at five hundred trials a calendar quarter**~~ | §20.1, §54.1 | P2 | B5 | — | **Done at `e31aae4`**: every trial-book record carries the family's count for the UTC calendar quarter of its own instant, under the same hash as the lifetime (`qip-lifecycle/src/trials.rs:69`, `:199`), and `charge` refuses — recording nothing — when a charge would carry the quarter past the budget (`:674`); zero is refused as a budget. `qip-lifecycle/tests/lifecycle.rs::the_five_hundredth_trial_of_a_quarter_charges_and_the_five_hundred_and_first_is_refused`, `::a_new_quarter_resets_the_running_count_and_not_the_lifetime`, `::the_quarterly_count_replays_from_the_store_and_a_lowered_one_is_refused` | 1, spent |
| B22 | ~~**Bill the wire's fills, not its placements**~~ | §43.4, rule 12, principle 6 | P3 | B11, B20 | — | **Done at `5290bb9`**, in six commits (`9e45dc0`, `ef4464a`, `96a49f1`, `3c2b789`, `d59505d`, `5290bb9`). The defect: since `cb79b46` the cell booked a fill only when the venue reported one, but the uplink carried only the placements, as `orders`, and `CentralPlane::settle` booked every one as a fill into the strategy books and, through B20, into the risk aggregate. Now `FillRecord` and `FillShare` are declared once in `qip-contracts/src/wire.rs:68-110`, the delta carries `fills: Vec<FillRecord>` beside `orders` (`qip-edge/src/mesh.rs:214`; built from `WorkReport::fills` at `cell.rs:3253`, bounded by `MAX_FILLS_PER_DELTA` = 64 at `wire.rs:120` with `fills_omitted` counting the rest), `CELL_DELTA_SCHEMA_VERSION` is 4 (`wire.rs:148`) so an older centre refuses the delta rather than decoding the orders as fills again, and the centre's mirror decodes `fills` with a serde default so a v3 delta with no field replays as sent-and-nothing-confirmed (`qip-mesh/src/delta.rs::a_delta_written_before_fills_existed_decodes_as_having_confirmed_none`, `:478`, asserting first that the fixture lacks the field). `settle` (`central/plane.rs:1161`) registers each order as sent in `SentOrders` — 4,096 per cell, oldest evicted (`:1526`) — under `qip_central_orders_sent_total` (`:1187`; `qip-observability/src/metrics.rs:711`) and bills, attributes and charges the aggregate from fills only, each booked as the cell's own shares and refused if they do not sum (`:1226`); a fill naming an order the centre never saw sent, or beyond its unfilled remainder, is `BreakOrigin::UnsentFill` (`:1214`), direction `unsent_fill`, merged with the report's own breaks (`:1066`) and halting the cell through the same path. The API sink carries the fills onto the report and counts `fills_reported`/`fills_omitted` apart from the orders (`qip-api/src/mesh.rs:1209`, `:1219-1220`). Tests, each mutation-verified per its commit: `qip-kernel/tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate` (`:506`), `::the_same_order_filled_in_the_next_report_charges_exactly_the_fill` (`:562`); `tests/attribution.rs::a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing` (`:441`), `::a_fill_on_an_order_the_centre_never_saw_sent_halts_the_cell_and_books_nothing` (`:499`), `::a_fill_beyond_the_quantity_sent_is_the_same_break` (`:567`), `::a_fill_whose_shares_do_not_sum_to_it_is_refused_rather_than_booked_short` (`:635`); `qip-api/tests/mesh.rs::the_fills_a_cell_reports_reach_the_centres_strategy_books` (`:712`), `::an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing` (`:760`); `qip-edge/tests/mesh.rs::a_state_delta_a_cell_produced_arrives_at_the_centre_unchanged` (`:420`, a fill of one against an order of three); `acceptance.rs::the_centre_decodes_a_contributor_vector_out_of_bytes_the_edge_crate_produced` (`:648`, the edge serialiser against the centre's decoder). B20's test still passes and now charges fills. The last reader of `orders` as holdings — `qip-acceptance/tests/e2e_live.rs::report_from`, a test-only helper that had survived `3c2b789` because its fixture pass sent nothing — closed at `095144b`: positions from `delta.fills` (`:698`), both lists forwarded (`:751`), and the walk's pass sends a hundred and fills forty (`::the_platform_completes_a_cycle_observed_from_sockets_and_acted_on_over_one`, `:780`), its mutation back to `orders` failing on a gross of 10002 where 4000.8 was required | 1, spent |
| B23 | **Decide what a concentration cap is a share of, then change the default set** — the fed buckets refuse the first order into any catalogued universe, and since B19's closing sentence every deployed root assembles one, so the pinned test in B19 is a standing refusal until ADR 0027 is accepted in some direction | §26, §28.1, §33 | P3 | B19, D13 | **D13 — CLOSED.** ADR 0027 was accepted as option (a) and applied at `eca7ebb`; line 3 of the record reads `accepted — option (a)`. `LimitKind::MaxAxisWeight` replaced the two share-of-gross entries in `LimitSet::conservative_default` on the same axes at 0.35 and 0.60, so the first order into a fed book is admitted and a sector past the cap is still refused. The two kernel fixtures that dropped `MaxConcentration` had become no-ops — the default set no longer holds that kind — and were removed at `0882a02`, so both files now run the set that ships (`capital.rs` 5 passed, `risk_aggregates.rs` 7 passed). This row was still reading "proposed, not accepted" 165 commits after the decision | ADR 0027's status changed by the owner; `LimitSet::conservative_default` amended to the chosen denominator; the pinned test replaced by one asserting the first order into the catalogue is admitted and the first breach of the chosen cap is refused | 1 (ADR, the owner's) + 1 |

Phase 0–3 total at `296e187`, excluding what is not authorised: **roughly
fourteen slices remain of the twenty-five**, six having been spent (B5's crate
half, B6, B10's cell half, B11, and B16 and B17's wiring half rendered moot or
done by the owner's instruction). **About six are unblocked today** — B5's
durable book, B8, B10's central half, B12, B13 once D3 is answered, B15 —
and the rest wait on a `terraform` binary, a project, a vendor host, or an
ADR.

At `584c96b` the table has twenty-one rows, of which ten are struck through
(`grep -c '^| B[0-9]* | ~~' docs/plan/completion-plan.md`; rows counted with
`grep -c '^| B[0-9]* |'`). Since `296e187`: B5's durable book (`aa66c5d`), B13
as code (`153e429`) and B15 (`ff86473`) are spent, and three rows were added
already spent because the work landed before the plan named it — B19
(`588335a`), B20 (`98bc687`), B21 (`e31aae4`). One row was added open: B18, the
producer for the whitelist the node now consumes. **Unblocked today:** B8,
B10's central half, B12, B18, and B13's owner half once D3 is answered.

At `e04815e` the table has twenty-three rows, of which eleven are struck
through (same two greps). Since `584c96b`: B18 is spent (`5396679`,
`91d20f5`, `73a1694`) and B10's node half closed (`6340610`); two rows were
added open — B22, the wire billing placements as fills, in flight; B23, the
concentration decision, behind D13 and ADR 0027.

At `5290bb9` the table has twenty-three rows, of which twelve are struck
through (same two greps). Since `e04815e`: B22 is spent, in six commits
ending at `5290bb9`, with the one test-only helper in `e2e_live.rs` that
still read `orders` corrected at `095144b`. **Unblocked today:** B8, B10's
central half, B12; B13's owner half and B23 wait on the owner.

### (iii) Later phases, gated behind Phase 2 and Phase 3

Not sequenced item by item, because estimating a plane that does not exist is
invention. What exists ahead of phase is labelled so it is not read as the
phase being reached.

| Phase | Deliverable | What exists today | Status | Gate above it |
|---|---|---|---|---|
| 4 Counterfactual scoring | Every declined path scored daily | `Platform::evaluate_alternatives` called from LEARN for every refused order once its horizon passes, eight per cycle with the excess deferred and counted (`b9e2242`) | Ahead of phase; scored per cycle on synthetic or replayed data, never daily on real | Phase 2 gate |
| 5 Ingestion and world model | Entities above confidence; events linked | World model, causal graph, entity resolution all TESTED (flow 2); no deep-web tier; no source discovery | Ahead of phase in part | Phase 2 gate |
| 6 Prediction markets | Brier beating implied — **gate** | `qip-prediction` four modules; no venue, no Brier comparison | PLANNED | Phase 3 gate |
| 7 Episodic and belief | Calibration within tolerance; sizing responds | `qip-agents/src/memory.rs`; `bayes.rs`; the calibration is now a Brier score on `qip_belief_brier_score` (`04738ee`); no belief stage in the cycle, so sizing does not respond | PLANNED; payload slots 3, 4 empty | Phase 3 gate |
| 8 Causal inference | Regime-conditional beating unconditional — **gate** | Causal graph real (`world.rs:41`); regime detection; no out-of-sample comparison | PLANNED; slot 5 empty | Phase 3 gate |
| 9 Self-model and exploration | Value of information measured | Nothing (`grep -rln SelfModel` empty) | MISSING, deliberately | Phase 8 gate |
| 10 Multi-strategy | 500+ strategies; netting ratio above 1.5; attribution exact | Netting at the cell; `qip_edge_netting_ratio` histogram; attribution exact centrally | Ahead of phase; ratio never measured under contention | Phase 3 gate |
| 11 Arbitrage and market making | Path 2 above 93 percent; quoting net positive | `qip-arbitrage` constructed by the cell and scanning its own books (`71f9465`); a cycle short a leg vetoed whole; `qip-edge-node` installs one from the payload's whitelist once a grant and a whitelist arrive (`584c96b`) and nothing produces the whitelist (B18); `qip-orderbook` | Ahead of phase in code, reached by no deployed process | Phase 3 gate |
| 12 Wallet and treasury | Every holding reconciled; zero unauthorised attempts | Nothing beyond internal placement; refused by ADR 0021; ADR 0023 step 10 | MISSING, bounded | Separate owner decision |
| 13 Web and mobile | Every operational question answerable; kill switch from mobile | Next.js portal and PWA, transitional (C3); Leptos not begun | CONTRADICTS §2.1 | Direction settled, execution unauthorised |
| 14 Valuation plane | Fixed income and options live | Nothing | MISSING, deliberately | Phase 2 gate |
| 15 Optimisation at cadence | Solver delta measured | Routing gate, classical baseline, QAOA adapter | Ahead of phase; no delta measured on real capital | Phase 3 gate |
| 16 Multi-region | Three regions, mirrors live | `execution-node` wired per region from the root module (`808ca32`); `execution_nodes = {}` in every environment because no venue is recorded; the pods this row once counted are gone with the cluster | Wired, unplanned, empty | Phase 3 gate, a venue (D9) |
| 17 Illiquid and private | Positions marked with method | Nothing | MISSING | Phase 14 |
| 18 Adversarial and simulation | Simulator calibrated to fills | `qip-simulation-engine` resampling; no adaptive agents | PLANNED | Phase 3 gate |
| 19 Market creation | Per class, on evidence | Nothing | MISSING, and the blueprint says last | Phases 7, 8, 14 |

Order-of-magnitude size: on the (ii) rate of one to two slices per capability
and roughly a hundred named capabilities across the sixteen later phases,
**well over a hundred slices** — and every gate in the column is a place the
blueprint says to stop and possibly not continue.

---

## 5. Owner decisions outstanding

Each verified at `de5d042` and re-verified at `296e187`; D3, D6 and D13
re-verified at `e04815e`. Where a decision has
been taken by a commit or by the owner's instruction — create the new
infrastructure while devouring the old, which `808ca32` and `c924191` cite as
their authority for the code and not for an apply — that is said.

| # | Decision | What it blocks | Default if undecided | Verified how |
|---|---|---|---|---|
| D1 | ~~**The egress path**~~ | — | — | **Taken, in code and recorded:** option (b), the co-located sidecar — `modules/egress-proxy` and the sidecar in `modules/cloudrun` (`c924191`), wired at `808ca32`, with a systemd unit on the execution node; ADR 0024 at `2b7e502`. `qip-transport/src/http.rs` still refuses `https` by name, which is the design |
| D2 | **In-tree HMAC vs ADR 0009** (F3) — admit a vetted crate by ADR, or decline and amend ADR 0002's reversal clause | B9; §46.2's real signatures and PQC; every further use of `hmac_sha256` | The primitive stays; each new caller restates F3 in its diff | `qip-core/src/hash.rs:151,163` carry `sha256` and `hmac_sha256`; no crypto ADR after 0023 |
| D3 | **The internal-crossing cap interval** (§27.1 "per instrument per interval") — the code half is taken at `153e429`; the choice is not | B13's owner half — a full cancellation can never cross in any deployment until an interval is set (F7) | `CellConfig::crossing_interval = None` (`cell.rs:107`), the per-net arithmetic: the cap refuses every full cancellation; safe, and less than the blueprint asks | `grep -n crossing_interval backend/crates/edge/qip-edge/src/cell.rs` names the field, `with_crossing_interval` (`:118`) and the window; `grep -rn with_crossing_interval backend/crates/apps` returns nothing, so no root sets one — re-run at `e04815e`, still nothing, so D3 is unchanged by a node that now runs passes: every full cancellation in a running node's pass is refused rather than crossed |
| D4 | ~~**Switch the GKE egress proxy on**~~ | — | — | **Moot.** Both manifest copies were deleted at `7d79161` with the chart and the raw manifests. The image question survived in a different shape and was answered: the sidecar's digest is read from `infrastructure/egress/vendored-images.txt`, the same one the chart pinned (`c924191`). What replaces this row is B2 |
| D5 | **ADR 0020 steps 1–5** | B17's validate half, Phase 16, Layer 6 leaving 0/7 | Nothing is applied | **Taken for the code, and now applied in `dev`.** `808ca32` reads the owner's instruction as approval to wire the blueprint runtime into the root module and remove the cluster's Terraform; steps 1 and 2 (evidence and a warm comparison) were skipped rather than passed, step 5 (retire the chart) happened in the tree. After `5290bb9` (§6), real plans and applies ran through CI against `algorik-dev`/`dev`: GKE was destroyed, Cloud Run services were created, `qip-dev-fastbrain` is confirmed serving. That is an apply in the one environment authorised for one (§4 assumption 5, gap-matrix.md), not a claim about any other environment, and it does not retroactively supply the evidence steps 1–2 asked for — there was never a warm comparison run. ADR 0020's text is unchanged and still describes a sequence the tree did not follow in order; amending it is the owner's |
| D6 | **`qip-arbitrage` and `qip-normalization`** — partly taken | A5's normaliser half; Phase 1 normalisation in the runtime path | The normaliser stays a crate nothing constructs | **Arbitrage: taken by code** — the cell constructs the desk (`71f9465`, `Cell::with_arbitrage`), so the edge is live; the node installs one at `584c96b` from a whitelist the kernel produces and the API ships since `5396679`/`91d20f5` (B18, spent), and runs passes since `6340610`; no node is deployed. **Normalisation: taken as a decision, not yet as code** — the kernel's dead edge was dropped at `2a74706`, so it is no longer compiled into a binary that never calls it; `grep -rln qip-normalization backend/crates --include=Cargo.toml` names only the crate itself and the acceptance crate. ADR 0029 answers the open question: neither research-only nor the runtime path — the crate is removed, because it is not the complete component the "keep it, it is finished" argument assumed (the batch path reads neither the symbol table nor `drop_unmapped`) and because four documents were citing it as a data-quality control that never ran. The blueprint's *node-side* Normalizer (§41.2) is expressly not settled by that record and is still the owner's. The removal itself is not applied; see A5 for the files it travels with |
| D7 | **K3 — what the application zone may reach**: the DOCX's "raise intents only, never a node, venue, QPU or key" or the diagram's wider "reaches Intelligence" | The typed-intent API surface (§40.9) | The narrower reading, which is what is built — and since `827a40e` what is tested: `api_boundary.rs` refuses the edge or constructor that would widen it | `blueprint-diagram-reconciliation.md` K3 unchanged; `trust_zones` is now wired (`808ca32`) with the narrower reading in its default-deny |
| D8 | ~~C4 — correct the observability rule file~~ | — | — | **Taken.** `232bc16` "Stop telling every agent the edge plane cannot emit" corrected `.claude/rules/domains/observability.md`; the reflog does not record who approved a rules-file edit, and that should be confirmed. What remains is A1 (the matrix row) |
| D9 | **The market-data and chain-RPC hostnames and their licensing posture** | B3, B2, B4; a venue for `execution_nodes` | No listener; the adapters stay inert; the blueprint's Phase 1 cannot start | `infrastructure/egress/envoy.yaml:392-492` declares five clusters — storage, BigQuery, Vertex, two IBM Quantum — none a vendor; `execution_nodes = {}` in every environment because a node needs a venue nobody has recorded |
| D10 | **ADR 0023 step 3 versus the Phase 2 gate** | B12, B15 | — | **Overtaken in practice.** The feasibility gate (`95a4932`), the arbitrage desk (`71f9465`) and the attribution join (`7ef6063`) are execution-side work built before the Phase 2 gate passed, under the same instruction that wired the runtime. ADR 0023's text is unchanged and still lists that under what would make it wrong; reconciling the record with what was done is the owner's |
| D11 | **Whether the matrix gains rows for §48 / rule 77** (OpenTofu, Cloud Build, Cloud Deploy, third-party source control) and what status they carry | A1's completeness | Unscored; a reader of §48 finds no row and assumes either aligned or ignored | No such row in the matrix's constraint or layer sections |
| D12 | ~~**A2's shell**~~ | — | — | **Taken by circumstance.** This refresh had a shell and ran the documentation suite; at `fca98cc` the checkout is clean and the three deployment suites are retargeted (`81dd1cd`). What was left of D12 was A2 itself — the run — which happened at `29ce828` |
| D13 | **Concentration semantics — share of gross or share of equity** (§26/§33; found at `588335a`) | B19's last step: the two share-of-gross caps in every default set, `sector-concentration` (35%) and `country-concentration` (60%), refuse the first order into an empty book now that the buckets are fed, because the first position is the whole of gross — which is what `MaxConcentration` says (`qip-risk/src/limits.rs:58`, `:484`) and not what the kernel's fixtures assumed | The caps and `LimitSet::conservative_default` are untouched; two kernel fixtures retain every default limit except `MaxConcentration` and say why (`qip-kernel/tests/capital.rs:57-69`, `tests/risk_aggregates.rs:136-144`). A deployed desk whose universe fed a bucket would refuse its first order — and since `8224509` every central root's universe is the committed catalogue, so every deployed desk would, which `qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_refused_by_the_default_concentration_cap_until_adr_0027_is_decided` (`:491`) pins until the decision (B23). Whether concentration is a share of gross or of equity is the risk desk's, and it now has its record: ADR 0027 (`360cfd8`), proposed, the owner decides, a per-axis share-of-equity cap recommended and marked as a recommendation — a record and not yet an answer | `grep -n "retain" backend/crates/runtime/qip-kernel/tests/capital.rs backend/crates/runtime/qip-kernel/tests/risk_aggregates.rs`; `grep -n QIP_UNIVERSE_PATH backend/crates/apps/*/src/main.rs`; line 3 of `docs/adr/0027-concentration-limits-are-a-share-of-what.md` reads `proposed` |

---

## 6. Environmental blockers — what can and cannot be proven from here

Re-scored after `5290bb9`: the paragraph below that said "no project
reachable" was true of every prior scoring of this document and is no longer
true. Between `5290bb9` and `2e19a4c` (twenty-four commits, thirteen of them
infrastructure work against a real GCP project, `algorik-dev`, environment
`dev`), `infra.yml` and `deploy.yml` were dispatched repeatedly against that
project through GitHub Actions. This environment still has no `terraform`,
`helm`, `kubectl` or `gcloud` binary — `which terraform helm kubectl gcloud`
finds none of the four here, and every commit in the sequence says so in its
own verification section ("NOT RUN — no binary here"). The distinction that
matters is the one the commits themselves draw: nothing was planned, applied
or observed *from this shell*, but real plans and real applies did run, in
CI, against the real project, and their output — error text, resource counts,
run.app URLs — was read back into the sessions that fixed what each one
found. That is a different epistemic position than "no project reachable",
and this section now states precisely what that sequence proved, in the
order it happened (`git log --format='%ad %h %s' --date=iso 5290bb9..2e19a4c`
gives the timestamps below).

**A real plan ran and read the new modules.** `ceff962`'s plan refreshed 157
resources and computed a full diff — 25 to add, 4 to change, 68 to destroy —
before exiting on three errors, two of them IAM read denials and one a
genuine bug (`a == null || a.field` evaluates both operands in HCL, so the
guard did not protect the dereference; fixed in the same commit, with a test
pinning the class of the mistake rather than the one instance). `a4811b8`'s
plan reached `module.cloud_run` for the first time — thirty-one references —
and refused on an unsupported `mount_options` argument the GA provider at
6.50.0 does not carry; fixed by mounting the whole bucket and moving the hash
into the path instead of the mount options. Both are `terraform plan`
outcomes, not `terraform validate` ones, and neither this document nor any
commit in the sequence claims `validate` itself was run and passed — that
narrower claim is still open and is named below.

**The GKE runtime was torn down for real.** `5e42347` and `b7a2c87` record a
teardown that destroyed 58 resources of the GKE runtime, hit two more
IAM-read-order failures the same shape as the plan's own (a role a
declarative grant would add arrives too late for the refresh that needed it,
`container.operations.get` to poll a delete already issued, then
`container.clusters.delete` itself — a run that removed, partway through, its
own permission to finish) and a refusal to cascade-delete the journal's
backup plan, resolved by releasing the Terraform resource with `destroy =
false` rather than deleting the backups (`NOT-COVERED.md` records what that
leaves unrestored: the backup agent's encrypter/decrypter, destroyed earlier
in the same apply, before its grant can be replaced). `8194b3b`, dispatched
after the fix, confirms the result in its own words: "the migration reached
the create phase — sixty resources built, the GKE runtime gone." The cluster
this plan describes is not hypothetically gone; it was destroyed by an apply
this sequence ran and then rebuilt around.

**Cloud Run services were created, and Binary Authorization was exercised
against real digests.** The same apply that confirmed the GKE teardown
refused two of three new Cloud Run services on an attestor denial the
committed vendor list could not explain: the digest Cloud Run asked about was
the platform-specific child manifest under the index the platform had
signed, not the index itself, which is what GKE had always asked about and
the only thing `vendor.yml` attested. `8194b3b` fixes `vendor.yml` to sign
the index and every platform manifest beneath it, out of the mirrored
image's own manifest. `06bedce`, the next apply, confirms the fix worked for
one service by name: "the attestations landed and the apply got past Binary
Authorization — `qip-dev-fastbrain` is up and serving at its run.app URL,
updated in place in the same run." That is a real Cloud Run service, in the
real project, observed running the attested image. `qip-dev-api` and
`qip-dev-deepbrain` did not reach that state in the same apply — their first
revisions were refused for an unrelated reason (a startup-probe failure, see
below) and Terraform marked both tainted; a tainted `google_cloud_run_v2_service`
with `deletion_protection = true` (hardcoded, deliberately, so a tfvars edit
cannot reopen the hole an acceptance test closed) can be neither destroyed
nor recreated by a further apply. `06bedce` adds an untaint step that reads
`terraform show -json` — the only place a taint is visible — and restores a
service Cloud Run still holds to an in-place update instead of a
destroy-then-create Terraform cannot complete.

**The startup-probe failure was root-caused and fixed, and is not yet
re-verified.** `4336da4`'s deploy built, scanned, signed and attested four
images and moved `qip-dev-api` to a new digest — Binary Authorization
admitted it, so the child-manifest fix held against a revision Cloud Run
actually judged — and then failed: "The user-provided container failed the
configured startup probe checks," with no cause in the line, only a log URL
CI does not open. `4336da4` adds a step that reads the failed revision's own
log, labelled by container; `6d52f8e` grants the narrowest role that read
requires (`logging.viewer`, refusing the wider `privateLogViewer`) after the
first attempt was itself refused for lacking it. `32b344d` reads the result:
the workload container (`api`) never spoke; the sidecar, `qip-egress`, failed
its own startup probe fifteen times with `ERROR_CONNECTION_FAILED` on port
9900, while its own logs showed Envoy starting normally three lines above.
The cause: the health listener bound to `127.0.0.1`, correct for the
execution node's own systemd probe (issued from inside the instance) and
wrong for Cloud Run's, which is issued from outside the container's network
namespace against the same shared bootstrap file. `32b344d` binds the health
listener to `0.0.0.0` — and only that listener; every other listener that
carries traffic keeps its loopback bind, and a plan-time gate now names the
one exception rather than refusing every wide bind. `2e19a4c`, the last
commit in the sequence, found that the execution-node module reads the same
bootstrap file and its own variable validation refused the same wide bind
for a reason that was correct before the fix and is not now; it narrows that
validation to the one named exception, matching the sidecar module's gate.
**No apply since `32b344d` has confirmed the fix**: every commit in this
final stretch ends its verification section with "NOT RUN — no binary here;
the next dispatch is the evidence," and that dispatch has not landed in this
branch. So, precisely: `qip-dev-fastbrain` is proven running the attested
image, in the real project, as of `06bedce`. `qip-dev-api` and
`qip-dev-deepbrain` are not proven running today — their probe failure has a
named cause and a committed fix, and the fix has not been redeployed and
observed.

**What is still genuinely open, stated without inflation.** `terraform
validate` itself has not been quoted passing in any commit in this sequence
— what ran is `plan` and `apply`, which subsume what `validate` checks but
are not the same claim, and this document does not substitute one for the
other. `workload_metrics_exist` is still `false` in every environment and
has not been flipped; nothing in this sequence scraped a metric. The
secret-mount chain (Secret Manager volumes on the Cloud Run services that now
exist) has not been separately exercised and confirmed live — the services
that came up did so past Binary Authorization and the startup probe, which
says nothing about whether a mounted secret file was read correctly by the
binary inside. No egress request through the sidecar's allowlist has been
observed in a log, so item 4 of the gap matrix (a vendor request through the
allowlist) is still open even though the sidecar itself is now confirmed
bindable and, for the fast brain, running. ADR 0020 step 1's evidence — which
GKE workloads ever ran — still cannot be gathered from this tree, because the
cluster this sequence tore down is gone and no earlier evidence was
gathered before it left. The Phase 1 exit (seven days of stable streaming)
and the Phase 2 gate remain untouched by any of this: a service starting and
passing its probe is not a week of streaming, and no deployed process has
been shown to run `qip-market-ingestion`'s live path for any duration. One
real tick was fetched in an earlier session (`gap-matrix.md` item 6) through
a bridge that is not this egress path; that remains the SENSE half of one
cycle and nothing more.

**A shell, this time.** The session that first wrote this document had no
shell; the one that re-scored it ran the documentation suite and quotes it in
the commit. It did not run the full gate: while it worked, the checkout
carried other owners' uncommitted edits to three acceptance suites, the kernel
and the risk lib, and by the time those landed (`88eb1e2`, `81dd1cd`) the
session's remit — five documents — was what it had evidence for. The run
was then made at `29ce828`, on a clean checkout, and §4(i) A2 carries it.

What *can* be proven from here: everything the Rust workspace asserts about
itself — the paper layers, the authority boundaries, the flow links marked
TESTED, the application layer's reach — because those are tests over source
and text, and they run without a cloud. What can now additionally be proven,
not from this shell but from the commit record of a real sequence of CI
dispatches against a real project: that the blueprint's Cloud Run runtime
plans, that GKE is actually gone, that one of three central services runs the
attested image today, and that the other two are blocked on a fix already
made and not yet redeployed. None of that reaches the TESTED bar this
document uses elsewhere — a named passing test in the repository — so
§2.3's Layer 6 arithmetic is unchanged by it; MEASURED is not the same claim
as TESTED, and the table below states which of the row's seven items moved
to which vocabulary word.

| Item | Before this sequence | Now |
|---|---|---|
| `cloudrun` / `execution-node` / `trust-zones` / `egress-proxy` modules wired | CONFIGURED | CONFIGURED — unchanged, since being wired was never in question |
| `terraform validate` run | Not run | Still not run and not claimed; superseded in practice by a plan, which is a stronger check than `validate` but a different one |
| A plan run | Never | **MEASURED** — `ceff962`, `a4811b8` (real diffs, real errors, real fixes) |
| Anything applied and observed | Never | **MEASURED** — GKE torn down (`8194b3b`'s own words), `qip-dev-fastbrain` created and serving at its run.app URL (`06bedce`); `qip-dev-api`/`qip-dev-deepbrain` not yet, blocked on a fix not yet redeployed |

---

## 7. How far away are we — the honest paragraph, twice

**Alignment-done.** Closer than at `e04815e`, and the distance is an absence
again rather than a defect. At `e04815e` this paragraph could say, for the
first time, that the distance was a defect: the edge plane's controls had
reached the deployed binary's loop (`6340610`), and the first thing a running
node would ship — its placements, as `orders` — the centre billed as fills.
That was an alignment-done failure by §1(a)'s own first clause — two claims
about one fact, one of them wrong, and a control, B20's exposure charge, that
fired on something that did not happen. It closed at `5290bb9` (B22): the
delta carries the venue's fills as their own field, the centre registers what
was sent and bills only what filled, a fill the centre cannot trace to an
order it saw sent halts the cell rather than being believed, and a delta from
before the field existed replays as having confirmed nothing; the one
test-only helper that still read positions from `orders` followed at
`095144b`. Of the seven alignment
items, five are spent; A6 still waits on an attested *collector* digest
nobody has pinned — not to be confused with the *egress* sidecar's image,
which was attested for the first time this sequence (`8194b3b`; §6);
`collector_image_digest` is still `null` in every environment
(`a4811b8`'s own words), so A6 is unmoved; and A5's desk half has closed on
its own terms — the whitelist is produced and shipped, the node runs passes.
The normaliser's disposition is no longer the owner's open question: ADR 0029
removes the crate rather than recording it research-only, on the finding that
its unmapped-symbol guard cannot fire and that four documents cite it as a
control that never ran. What is left of A5 is the removal itself, which is
mechanical and is not applied. The boundaries are
still enforced structurally, the application
layer's reach is still a test rather than a reading, the paper-trading line
still has three layers and a test on each, and the venue feed is a fourth
refusal in the same register — `QIP_VENUE_FEED` accepts one value and names
ADR 0003 for any other. The cloud layer moved for the first time since
`296e187`: a real plan has run against the blueprint's runtime, GKE is torn
down for real, and `qip-dev-fastbrain` is a real Cloud Run service serving
the attested image (§6) — `qip-dev-api` and `qip-dev-deepbrain` are not yet,
blocked on a startup-probe fix that is committed and not yet redeployed. That
is MEASURED, not TESTED by this document's own bar (no named repository test
asserts it, and none could — it is a fact about a cloud project, not the
workspace), so §2.3's Layer 6 arithmetic still reads zero; what changed is
that the row is no longer describing code nobody has run, and Layer 6's own
text says which of its seven items moved. Call it one slice from aligned —
the normaliser removal ADR 0029 decides and nobody has yet applied — with every edge and wire control
TESTED and measured nowhere except the one service now MEASURED serving, and
no execution node deployed at all.

**Blueprint-done.** Far, by the blueprint's own reckoning, and the distance
is still not mainly code. The tree holds capability from Phase 1 to roughly
Phase 15 — netting, a feasibility gate, an arbitrage desk with a produced
whitelist, a node that runs passes and books only the fills its venue
reports, cumulative trials and a holdout band, a cost router, a quantum
adapter with its classical baseline, a per-region node module, a committed
instrument catalogue every root assembles from — and has passed none of the
four gates, because every gate is a question about real data or a real venue
and the platform has never streamed real data for a day. Per gate, at
`e04815e`: Phase 2 waits on real data, the correction and its durable,
quarter-budgeted book both in the tree; Phase 3 cannot pass while paper
trading is absolute, and has a band to be inside of; Phase 6 has no Brier
comparison against any venue's implied probability; Phase 8 has no
out-of-sample comparison against an unconditional baseline. The first thing
between here and the first gate is still an egress path — a sidecar now
proven plannable and, for one of three central services, actually serving
(§6), but still pointed at no vendor: the allowlist names five Google and
IBM endpoints and no market-data host, so no vendor request through it has
been observed regardless of what else deployed. After that, seven days of
streaming nobody can run from this environment or, so far, from the project
either; after that, the Phase 2 gate, which the blueprint expects to fail
more often than pass. One new thing sits ahead of any deployed desk trading
at all: the default concentration cap refuses the first order into the
catalogue every root now loads, until ADR 0027 is decided (B23). Phases 0
to 3 are twenty-three rows, eleven spent; of the twelve open, four are
unblocked today and the rest wait on a vendor host, an ADR, the crossing
interval, the owner, or the redeployment §6 records as still needed for
`qip-dev-api` and `qip-dev-deepbrain`. Phases 4 to 19 are well over a
hundred slices more, behind gates that may say stop. Of the two direction
decisions — no Kubernetes, Leptos — the first has been taken for the code and
for nothing that runs, and the second is a proposed record (ADR 0025) and no
code. The honest unit is not weeks; it is gates, and zero of four have
passed.

---

## Verification of this document

The gate for this file is
`cd backend && cargo test -p qip-acceptance --test documentation --no-fail-fast`,
which checks every internal link resolves and refuses the overclaims it names.
Run it on every edit and quote the `test result:` line in the commit. This
document does not claim the gate ran; the commit that lands it must.

## Re-score at `2fd254f` — corrections this plan was carrying

Appended, not rewritten. Five statements in the rows above were false against
the tree when checked at HEAD; each is corrected here with the evidence,
because a plan that misreports what is done sends the next wave at work
already finished.

**B12/F6 — per-region reservation is implemented, not absent.** The rows above
say "still absent" and quote a grep they report as empty. It landed at
`0ca4b92`: `qip-edge/src/reservation.rs` holds `RegionAllocation::reserve`;
`Cell::with_region_allocation` takes it (`cell.rs:598`); `hold_region_capital`
(`cell.rs:3422`) is consulted on both capital-committing paths (`:1723`,
`:2668`); and `qip-edge/tests/reservation.rs` carries the property tests,
including
`a_second_strategy_is_refused_once_the_region_allocation_is_spent_even_though_its_own_envelope_would_admit_it`.
**Still open, and the reason this is not closed outright:** no composition
root constructs it —
`grep -rn "with_region_allocation\|RegionAllocation" backend/crates/apps/`
returns nothing — so a cell built by `qip-edge-node` today has none. Written
and tested; not installed.

**B23/D13 — ADR 0027 is accepted, not proposed.** Corrected in the row itself
above. It was accepted as option (a) and applied at `eca7ebb`, 165 commits
before this plan was last read.

**The `no terraform binary` premise is withdrawn.** It appears at line 45 and
in the verification notes at lines 424 and 512. A binary exists at
`/usr/local/bin/terraform`; `terraform fmt -check -recursive` exits 0 and
`terraform validate` returns "Success! The configuration is valid." The wave-7
backlog item asking for "`terraform validate` quoted by name" is satisfied.
What stays true: `validate` is not a plan, so every plan-time precondition —
ADR 0030's pairing rules, ADR 0031's `secret_env` refusal — remains asserted
and unexercised.

**The egress allowlist names a market-data vendor.** Rows B3, D9 and §7 say it
names none. `infrastructure/terraform/variables.tf:294-305` lists six
upstreams and the sixth is `api.frankfurter.app`, described in-tree as "a
market-data vendor… the first that is neither [Google nor IBM]", with a real
Envoy cluster on 9105 and `FrankfurterRatesConnector` registered in the
connector bridge. No scoring document mentioned it. **What is still open:** no
request through that allowlist has been observed in a deployment log, and
Frankfurter is FX reference rates — not the equities feed the Phase 2 gate
needs.

**All three central services are deployed and proven serving.** Lines 514-518,
571 and 608-610 say `qip-dev-api` and `qip-dev-deepbrain` are not proven
running. `infrastructure/environments/dev/images.tfvars` records all three
digests from run 33780092495, each written by the pipeline itself as having
moved onto its Cloud Run service and been proven serving before the line was
written. `qip-dev-openobserve` was added since, and answers 200 at `/web/` and
401 unauthenticated on its API.

### Not done, and deliberately: ADR 0029's removal

`qip-normalization` is still a workspace member. The decision to delete it is
taken and unchanged; executing it was judged the wrong use of this session and
that judgement is recorded rather than left as a silent omission.

The removal is not mechanical. The crate's only consumers are two acceptance
suites that use it as a *fixture*, and ADR 0029's own cost section says what
that means: `truth_loop.rs`'s fourth stage becomes an explicitly test-owned
value rewrite, and two published performance figures are withdrawn with the
budget check made bidirectional in the same change. `truth_loop.rs` asserts on
`NormalizationReport`'s `processed`, `venues_canonicalised`,
`timestamps_corrected` and `scale_warnings` (`:479-485`) and on
`SymbolMapping::canonical_symbol` (`:487`), and `normalised` is threaded
through eleven later assertions. That is a careful edit to a flagship
seven-stage suite for zero functional gain, and a hurried one risks exactly
the weakened test this repository forbids.

The next owner should take it as its own slice, with `architecture.rs:762`'s
`NO_MONEY_AUTHORITY` literal and the `services.len() >= 25` assertion handled
deliberately — that count is asserted against exactly 25 crates, so the
deletion fails it on its premise, and lowering the number to make it pass
would replace a guard with a tautology.
