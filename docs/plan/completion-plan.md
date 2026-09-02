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
| End of Phase 2 | A family surviving holdout with honest significance after **cumulative** trial correction, on real data | **NOT PASSED** | No deployment has run on sustained real data (Phase 1 exit unmet — see below). The second blocker this row carried — that nothing counted trials across runs — closed in code at `9332bcb` and `94dd7e2`: `TrialBook` keeps one hash-chained journal per family and the holdout gate refuses an unknown lifetime count. What remains of it is deployment-shaped: the kernel's factory holds `TrialBook::in_memory` (`central/factory.rs:243`) and no composition root supplies a durable book, so a count survives one process and not the next. ADR 0023's "What could not be specified" still describes the old state and is the owner's to amend |
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
| 4 Intelligence | statistical gate; champion/challenger; drift detection; training; corridor policy; cumulative trial accounting across runs; holdout band as an output of validation | 5 of 7 | 5/7 | `lifecycle.rs`, `evolution.rs`, `training.rs`, `qip-deepbrain/src/learning.rs:279`; the band — `lifecycle.rs::a_holdout_admission_carries_the_band_its_validation_produced` (`d0558b4`), new since `de5d042`; corridor policy has no subject (Phase 12); cumulative trial accounting is TESTED at the crate (`::a_second_run_is_corrected_against_the_first_runs_trials_as_well`, `9332bcb`) and enrolled by the factory (`94dd7e2`) but counted here as absent, because the deployed book is in-memory and "across runs" is the property |
| 5 Optimisation | routing gate; classical baseline every time; authority boundary structural; family clustering; multi-horizon reconciliation | 3 of 5 | 3/5 | `optimization.rs`, `architecture.rs` solver tests, ADR 0006 |
| 6 Execution | paper-only cell; envelope admission; intent netting; internal crossing; contributor vector on the uplink; halt reaching a cell; §6.2 narrowing; feasibility gate; per-region reservation; crossing settled to books; leg producer for cycles | 10 of 11 | 10/11 | `qip-edge/tests/cell.rs`, `qip-edge-node/tests/gateway.rs`, `qip-api/tests/mesh.rs::a_cycle_ships_a_signed_payload_the_cell_verifies_and_a_trip_reaches_it`; three moved since `de5d042`: feasibility — `feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order` (`95a4932`); crosses settled at the centre's books — `qip-kernel/tests/attribution.rs::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact` (`7ef6063`); the leg producer — `arbitrage.rs::a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass` (`71f9465`). Reservation still CONTRADICTS (F6). The caveat that weighs on all ten: `qip-edge-node` calls `Cell::work` on no path, so TESTED here means the cell's tests and never a deployed pass |
| 7 Ledger, wallet, treasury | capital allocation; envelope; two-signature approval; reservation ledger in the kernel; per-user per-strategy ledger; §43.4 attribution chain at the centre (fill → contributor vector → strategy pro rata); wallet; corridor; transfer gate; destination registry; custody | 5 of 11 | 5/11 | `truth_loop.rs`, `compliance_proof.rs`, `platform.rs::a_second_proposal_is_sized_against_what_the_first_still_holds`; the chain — `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual` and `qip-api/tests/mesh.rs::the_orders_a_cell_reports_reach_the_centres_strategy_books` (`7ef6063`, `7d79161`), new since `de5d042`; the books are per strategy and not per user, so the §43.3 ledger stays absent; the rest are Phase 12 and bounded by ADR 0021 |

Summed: **31 of 52 named capabilities at the TESTED bar** (3 + 5 + 0 + 5 +
3 + 10 + 5 over 5 + 7 + 6 + 7 + 5 + 11 + 11), up from 25 of 50 at `de5d042` —
two capabilities were added to the list because the blueprint names them and
the tree now has them, and six moved from absent to TESTED. That number is
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
| 4 Domain contracts and control fabric | signed payload down; cell verification; atomic swap; §6.2 narrowing wired; outcome return; twelve producers; two independent halt wires; per-region reservation | 5 of 8 | 5/8 | Flow 3 verdict paragraph and `qip-api/tests/mesh.rs`; 2 of 12 payload slots have producers (PARTIAL); halt is mechanism-independent not wire-independent (flow 6); F6 CONTRADICTS |
| 5 Data and state | source→facts; entity resolution; world event; bitemporal, bounded, hash-chained log; a `Ledger` per §43.3; central strategy books settled from cell reports; live source sustained; BigQuery derived series; content-hash manifests | 5 of 9 | 5/9 | Flow 2 links TESTED; `truth_loop.rs`; the books — `qip-kernel/tests/attribution.rs` (`7ef6063`), new since `de5d042`; ledger PARTIAL by naming; the last three deferred |
| 6 Cloud and network | `cloudrun` module wired; `execution-node` module wired; `trust-zones` module wired; `egress-proxy` module and sidecar wired; `terraform validate` run; a plan run; anything applied and observed | 0 of 7 | 0/7 | Re-scored at `296e187`: the item this row used to lead with — a GKE transitional runtime carrying traffic — is gone from the tree (`808ca32`, `67b3e92`, `7d79161`), and the three modules that were absent from `main.tf` are now four of its seventeen `module` blocks (`main.tf:274`, `:296`, `:467` and `catalogue.tf:234`; `808ca32`, `c924191`). Wired is CONFIGURED; none of it has been seen by a `terraform` binary, so every item is IMPLEMENTED-UNVERIFIED and none reaches the TESTED bar — `infrastructure.rs` is a text scanner, retargeted at the runtime that exists at `81dd1cd` (its message: `test result: ok. 59 passed; 0 failed`). Nothing was applied, and no process has ever been shown to run on the old runtime or the new one (§6) |
| 7 Security, observability, delivery, reliability | three paper layers; LM/quantum authority; WIF only; central telemetry recorded and served; edge telemetry recorded and served; belief calibration recorded; reconciliation break counted on both planes; secret-mount chain exercised live; scrape observed; OTel spans (§47); edge collector and alert; second halt wire; `qip_central_` alerts | 7 of 13 | 7/13 | `security.rs`, `compliance_proof.rs`, `architecture.rs`, `qip-edge/tests/telemetry.rs`; calibration — `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series` (`04738ee`); the break — `qip-kernel/tests/central.rs::a_reconciliation_break_is_recorded_by_direction_and_the_halt_by_cause` (`de5d042`) and `qip-edge/tests/telemetry.rs`; both new since the first scoring. Never exercised live: the secret mount (CSI then, Cloud Run secret files now); `workload_metrics_exist=false` everywhere; the execution node declares an Ops Agent receiver and no node exists, and the Cloud Run services have no collector attached (`modules/observability/NOT-SCRAPED.md`); the rest MISSING |

Summed: **23 of 51 layer items at the TESTED bar** (1 + 3 + 2 + 5 + 5 + 0 +
7 over 7 + 4 + 4 + 8 + 9 + 7 + 13), up from 20 of 48 at `de5d042`. Layer 6 at
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
| "Tests: 3,308 passing at `fef0c97`" | `current-state.md` | Thirteen commits later; not re-measured | **Superseded, still stale** — the row now cites 3,355 across 280 binaries from `a4f673c`'s message; thirty-odd commits have landed since. At `fca98cc` the checkout is clean, so the run is possible; it was not made in this series (A2, D12) |
| `NumericFact::observed` has no production caller | `gap-matrix.md` risk register | `480644d` and `125a7de` add an observed-fact constructor; the register's open count of three is likely two | **Resolved** — the register was recounted at `d4dcd44`, `67a584d` and again in this refresh: twenty-four found, twenty-three closed, one open |
| ADR 0023 step 3 "buildable today" | ADR 0023 | The record is in tension with itself; §5 lists it for the owner | **Overtaken in practice** — the feasibility gate (`95a4932`) and the attribution join (`7ef6063`) were built; the ADR text is unchanged (D10) |
| Blueprint §48 and rule 77: OpenTofu, Cloud Build, Cloud Deploy | Not scored anywhere | The matrix has no row; the matrix owner's call | **Still open** (D11). `deploy.yml` now moves Cloud Run services by digest itself (`b85684f`), which makes the row easier to write and no less the owner's |

---

## 3. What landed this session

Grouped by merge. Merge bodies could not be read from this environment (no
shell; git objects are not readable as text), so PR numbers are inferred from
the reflog's five fast-forwards of the target branch and from
`current-state.md`'s citation of "the PR #5 body for `fef0c97`". Commit
subjects are from `.git/logs/HEAD`.

| Merge | Commits | What landed, one line |
|---|---|---|
| PR #1 — `d8b3597` | before this branch | The GitOps cut-over (Argo CD, Kargo) — ADR 0020 names it; this branch was cut from it |
| PR #2 — `19241d8` | `4541923`..`8df1658`, 12 | Solver authority held to the LM rule; the two blueprint conflicts recorded; the traceability matrix; the §6.2 degradation type; ADR 0022; ADR 0023; manifest parser hardened then replaced with Cargo's own; the two credential windows bound; the seven-flow truth pass; the blueprint-vs-diagram reconciliation |
| PR #3 — `acfece3` | `9b8df9b`..`0c91cfa`, 9 | The twelve-item payload's wire shape; a cell that verifies, applies, narrows and halts on it; the reservation ledger, then wired into the kernel; the halt as a signed command; the centre shipping policy; the Deep Brain's reference universe and its own exchange; injective signing strings |
| PR #4 — `7f508cc` | `3be9855`..`db8ce8b`, 21 | One live market source behind the licensing gate, then selectable; `Intent` and netting in the cell, self-trade prevention; the `cloudrun`, `trust-zones` and `execution-node` modules, all unwired; the network module's blueprint notes; GitOps job identity; the console's order ticket deleted; the venue credential refused where the ceiling cannot use it; four unbounded collections bounded; money out of `f64` in risk and execution; the brute-force lockout made able to fire; the safest rung no longer reported live-capable |
| PR #5 — `baffcd8` | `64b765a`..`fef0c97`, 7 | `egress.rs` able to tell a deployed proxy from a described one; contributor attribution and the uplink schema bump; internal crossing at the mid with the forty-percent cap; the rounding remainder returned; the uplink proven; two scored documents corrected |
| Unmerged — this branch | `68b7da6`..`de5d042`, 13 | Edge telemetry parked, then finished and proven site by site; the node hands its cell the scraped registry; the observability rule corrected (C4); three Argo CD Applications that could never sync removed; five documents corrected; the reservation shortfall counted under its registered name; observed-fact constructors for agents; a cycle-leg type that cannot be nettable; the arbitrage scanner's leg emitter; a central reconciliation break and its scoped halt recorded and counted |
| Unmerged — this branch, since the plan was first scored | `de5d042`..`296e187`, 57 | **Risk:** tail figures derived from each limit's own confidence in the risk lib and the kernel (`d94b156`, `990032a`); a pass-and-veto fixture for every `LimitKind` arm (`160c4e8`); the aggregates-never-strategy-lists rule made structural and probed at two counts (`b9e9e7d`). **Lifecycle:** cumulative trials per family (`9332bcb`), one Sharpe arithmetic (`436e1fa`), the holdout band and its demotion (`d0558b4`), factory enrolment (`94dd7e2`). **Sealed seams:** the registered outcome behind the legality assessment (`47e9b81`), a proposal's status private (`6e3aad0`), the synthetic path refusing overflow rather than restarting (`cc92d66`). **The cell:** feasibility ahead of netting (`95a4932`), the arbitrage desk scanning the cell's own books and halting a broken cycle (`71f9465`), buy signed positive where the intent is made (`54d32fd`). **The centre:** LEARN grades every resolved thesis (`04738ee`) and prices every refused order (`b9e2242`); the cycle recorded where the console reads (`cf20457`); netted fills attributed and crosses settled to strategy books, the report carrying the interval (`7ef6063`, `7d79161`); a reorganisation failing the bridged transfer riding on it (`67b3e92`); the not-decision-grade instruments counted at assembly (`78026e2`); fourteen newer series spelled beside the rest (`296e187`). **The sweep:** 99 orphan public functions removed (`a4f673c`), four resolved by removal (`ed69a52`, `68ff891`, `b7d3edc`, `b8a8acd`), 110 then 32 dead dependency edges dropped (`2a74706`, `2753911`), the bench profile and a frontend config reader gone (`a95f702`, `ad9a937`). **Infrastructure:** the first egress proxy that exists, as a co-located sidecar (`c924191`); the blueprint runtime wired into the root module and the cluster's Terraform, chart, manifests and Argo CD stack removed (`808ca32`, `67b3e92`, `7d79161`); `deploy.yml` moving Cloud Run services by digest with the serving revision proven (`b85684f`); the resource-by-resource record (`bcad2d3`) — nothing applied, no `terraform` binary. **Proof of boundaries:** the application layer's reach made executable (`827a40e`); three premise-first test repairs (`08c52a0`, `4916217`, `6dd761b`). **Documents:** the truth pass, current state, gap matrix, traceability matrix and this plan re-scored (this series) |
| Unmerged — landed while this plan was being re-scored | `296e187`..`fca98cc`, 5 | Every desk fill fed into the risk aggregate and limits read from it, so the O(1) rule is held in production and not only by the lib's test (`88eb1e2`); the three deployment suites retargeted at the runtime that exists, every property kept (`81dd1cd`); the operations documents and two rule files corrected for the deleted runtime (`ecfb0a6`); ADR 0024 (`2b7e502`); the scaling runbook pointed at the catalogue (`fca98cc`) |

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
| A2 | **Re-measure the full gate at HEAD** — `current-state.md` now cites 3,355 across 280 binaries from `a4f673c`, itself thirty-odd commits old | — | A | None | None at `fca98cc`: `81dd1cd` retargeted the three deployment suites and the checkout is clean. Not run in this series, which ran the documentation suite alone | `test result:` lines quoted for every binary under `--no-fail-fast`; clippy zero warnings; `all permitted`; `nothing found` | 1 |
| A3 | ~~**Controls that cannot fire, remaining:** `Platform::learn_from`, `Platform::evaluate_alternatives`, `Web::record_cycle`~~ | §47, §12 | A | — | — | **Done, all three wired:** `learn_from` from LEARN (`04738ee`, `learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`); `evaluate_alternatives` from LEARN (`b9e2242`, `::a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate`); `record_cycle` from the cycle route (`cf20457`, `api.rs::a_cycle_run_through_the_router_reaches_the_operator_interfaces_stage_overview`). Two more found and wired since: `BridgeLedger::on_reorg` (`67b3e92`) and `Universe::not_decision_grade` (`78026e2`) | 3, spent |
| A4 | ~~**Assert the O(1)-in-strategy-count property of risk**~~ | §2.2, rule 11 | A | — | — | **Done** at `b9e9e7d`: `qip-risk/tests/aggregate.rs::the_aggregate_check_reads_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve`, mutation by rewriting the gross figure as a per-strategy sum | 1, spent |
| A5 | **Settle `qip-arbitrage` and `qip-normalization`** — half done. `qip-arbitrage` is constructed by the cell (`71f9465`, `Cell::with_arbitrage`), so its edge is live; `qip-edge-node` installs no desk (`grep -rn ArbitrageDesk apps/qip-edge-node/src` is empty), so no deployed process holds one. `qip-normalization`'s dead edge from the kernel was dropped at `2a74706`; the crate is now named by no manifest but the acceptance crate's and is constructed by nothing — research-only in fact, recorded as such by nobody | §30, §7.3 | A | D6 for the normaliser's disposition; a venue for the desk | D6 | For the desk: the node constructing it and a gateway test seeing a leg; for the normaliser: an owner's sentence recording it research-only, or a composition root constructing it | 1 each |
| A6 | **A collector for every emitter, and an alert for `qip_central_` and `qip_belief_` descriptors** — re-stated for the runtime the tree now describes. The execution node's startup template declares an Ops Agent Prometheus receiver on its health port (`808ca32`); no node exists. The Cloud Run services have no collector: the managed-Prometheus sidecar needs a digest mirrored and attested first (`modules/observability/NOT-SCRAPED.md`), and nobody has pinned it. The old `PodMonitoring` left with the cluster | §47 | A | Someone mirroring the sidecar image by digest through `vendored-images.txt` and `vendor.yml` | Binary Authorization admits only attested images, so an unattested sidecar reads as a broken deploy | The sidecar attached in `modules/cloudrun` by digest; an alert policy naming a recorded `qip_central_` or `qip_belief_` descriptor; the names test extended — and, separately, an observed scrape before `workload_metrics_exist` flips | 1 |
| A7 | **Correct the rules files** — half taken: `ecfb0a6` re-stated `.claude/rules/domains/data-and-streaming.md` against `modules/egress-proxy`. Still open: `.claude/rules/domains/observability.md:83-85` says `Platform::learn_from` and `Platform::evaluate_alternatives` have no production caller; both have had one since `04738ee` and `b9e2242` (C4) | — | A | None | Owner — it is a rules file | The two sentences re-stated against the LEARN callers, with paths | 1 (owner-approved) |

Alignment-done after (i), at `fca98cc`: **A1, A3 and A4 are spent; A2 is
unblocked and unrun; A6 waits on a mirrored sidecar digest; A5 and the
remaining half of A7 wait on the owner.** Two to three slices of agent work
remain, none of them code in the cycle. Layer 6 stays at 0/7 regardless — see
§6.

### (ii) Phase 0–3 blueprint work

Phase 0 and Phase 1 are where the repository is genuinely behind, however far
ahead it is elsewhere. This is the critical path to the first gate.

| # | Item | Blueprint | Line | Depends on | Blocker | Evidence that closes it | Slices |
|---|---|---|---|---|---|---|---|
| B1 | ~~**Decide and record the egress path**~~ | §46.2 network, §45.1 | P1 (exit) | — | — | **Done**: taken in code as the co-located sidecar (`c924191`, D1) and recorded at `2b7e502` — ADR 0024, which quotes the owner's instruction as the authorisation for the code and states that nothing was applied. At `296e187` the record was absent and `b85684f` cited it ahead of its existence; it landed five commits on | 1, spent |
| B2 | **Plan, apply and observe the sidecar** — this row used to be "switch the GKE proxy on"; that proxy and its manifest are gone (`7d79161`) and D4 with them. Now: `terraform validate` and a plan of `modules/egress-proxy` and the `modules/cloudrun` sidecar, an apply with step-named approval, and a vendor request in the sidecar's access log | §46.2 | P1 (exit) | B1, B3 | A `terraform` binary and a project (§6); apply approval | The plan's preconditions refusing a host outside `egress_allowed_upstreams` and admitting one inside; a request through the allowlist observed. `egress.rs` was refitted from the deleted manifest to the sidecar at `81dd1cd` (14 passed, per its message), so the suite half is done and the observed half is not | 2 |
| B3 | **Name the market-data vendor host and record its licensing posture** — the bootstrap's five clusters are Google and IBM endpoints (`infrastructure/egress/envoy.yaml:392-492`); no vendor | §7, rule 40 | P1 | — | **D9** | A `qip-data-finder` posture record, a cluster in `envoy.yaml` and its host in `egress_allowed_upstreams`; nothing else changes | 1 |
| B4 | **Seven days of stable streaming with statistics converged and no raw stream retained** — the Phase 1 exit | §51 Phase 1, rule 32 | P1 (exit) | B2, B3 | A deployment (§6) | Seven days of a scrape series, the feature store's bound held, the licensing posture in the journal | 1 to observe; 0 to build |
| B5 | ~~**Count trials cumulatively across runs**~~ — done at the crate and the factory (`9332bcb`, `94dd7e2`: `lifecycle.rs::a_promotion_whose_lifetime_trial_count_is_unknown_is_refused_naming_what_to_do`, `::a_second_run_is_corrected_against_the_first_runs_trials_as_well`, `::a_trial_book_replays_its_journal_from_the_store_and_refuses_a_tampered_one`). **What remains is the durable book:** the factory's default is `TrialBook::in_memory` and `with_trial_book` has no caller in any composition root, so "across runs" holds across cycles of one process and not across processes | §20.1, rules 24–25 | P2 | None — buildable now | None | `qip-deepbrain`'s composition root supplying a store-backed book and a test that a restarted process refuses to forget the first run's trials | 1 |
| B6 | ~~**Define the holdout band as an output of validation**~~ | §20.1, §51 Phase 3 gate | P2 | — | — | **Done** at `d0558b4`: `HoldoutBand::from_deflated` at the gate, carried on the `Admission`, refused off it, two-sided at the demotion monitor — `lifecycle.rs::a_holdout_admission_carries_the_band_its_validation_produced`, `::live_performance_outside_the_holdout_band_is_demoted_and_counted`, `::judging_or_admitting_without_a_holdout_band_is_refused` | 1, spent |
| B7 | **Attempt the Phase 2 gate on real data** | §51.1 | P2 (gate) | B4, B5, B6 | Phase 1 evidence (§6) | A family surviving holdout after cumulative correction, recorded in `qip-lifecycle/src/evidence.rs`'s own artefact — or a recorded failure, which the blueprint says is the more likely and the more useful result | 1 to run |
| B8 | **Passkeys** — `grep -rln -i passkey backend/crates frontend/portal/src` is still empty at `296e187` | §51 Phase 0, §40.3 | P0 | None | None known | An authenticator registration and assertion through Identity Platform; the grep non-empty; Playwright for the browser half | 2 |
| B9 | **PQC keys and real signatures for the payload channel** — depends on the crypto decision | §46.2 keys | P0 | — | **D2** | An ADR admitting a vetted crate, or an ADR declining and amending ADR 0002's reversal clause; then KMS-backed signing in place of `hmac_sha256` (`qip-core/src/hash.rs:163`) on the policy and envelope channels | 1 (ADR) + 2 |
| B10 | **Feasibility gate ahead of the profitability filter** — done at the cell (`95a4932`: `admit_feasible` ahead of `net` in `Cell::work`, eight refusal literals, `feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order`), and slot 11 of the payload is its first consumer with no producer. **Open:** the central pre-trade path in `qip-execution-engine` has no feasibility step, and no deployed process runs a cell pass | §18.1, rule 23 | P3 | — | None in code; D10 in principle, overtaken in practice | The same fixtures beside the central pre-trade path; a producer for slot 11 | 1 |
| B11 | ~~**Join the edge contributor vector to central attribution** and settle a cross to the books~~ | §27.1, §43.4, rule 12 | P3 | — | — | **Done** at `7ef6063` and `7d79161`: `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual`, `::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact`, `::a_cross_naming_two_buyers_is_refused_rather_than_split_evenly`; `qip-api/tests/mesh.rs::the_orders_a_cell_reports_reach_the_centres_strategy_books` | 2, spent |
| B12 | **Per-region reservation table** (F6) — still absent; `grep -n -i reserv edge/qip-edge/src/cell.rs` finds nothing but the word "preserved" | §4.2, §26, §33, rule 21 | P3 | — | None in code | A disconnected cell refusing its own second proposal against one envelope; the central ledger unchanged; `apps/qip-edge-node/tests/mesh.rs` extended | 2 |
| B13 | **Set the internal-crossing interval** so a full cancellation can cross — `cell.rs:1773` still records that the blueprint never defines it | §27.1 | P3 | — | **D3** | The cap evaluated per instrument per the chosen interval; a test where two strategies cancelling completely are both filled at the mid | 1 |
| B14 | **Twelve producers for the twelve payload slots** — ten still ship unproduced and narrow the cell; slot 11 gained its first consumer (`95a4932`) and `grep -rn feasibility_constraints apps/qip-api/src runtime/qip-kernel/src` is still empty | §41.5 | P3 for items 2, 9, 10, 11; later phases for 1, 3–6, 8, 12 | — | Most slots have no producing plane yet (belief, episodic, causal digest, self-model are P7–P9) | Per slot: a producer, the cell consuming it, and the §6.2 row it un-narrows | 1 per slot; 4 in P3 |
| B15 | **Second, independent halt wire** — a polled flag beside the broadcast | §46.2 kill switches | P3 | None | None in code; a managed store in deployment (§6) | Flow 6 re-traced with two wires that do not share `qip-transport`'s failure | 1–2 |
| B16 | ~~**ADR 0020 step 1 — establish which GKE workloads have ever run**~~ | §41.4 | — | — | — | **Moot.** The evidence was never gathered, and at `808ca32` there is no cluster in the tree to gather it from; the owner's instruction to devour the old runtime replaced the step. Recorded so nobody reads the step as passed: no process has ever been shown to run on either runtime | 0 |
| B17 | **Validate the wired modules** — the wiring half is done (`808ca32`: `cloudrun`, `execution-node`, `trust-zones`, `egress-proxy` in the root module; D5 taken for the code); the validating half is not: no `terraform` binary has read any of it | §41.4, §45.1, §46.1 | P3 (node), P16 (regions) | — | A `terraform` binary (§6); apply approval per step | `terraform fmt -check` and `validate` output; a plan that refuses a bad value (an undeclared zone, a missing digest, a host outside the allowlist) and admits a good one; nothing applied without step-named approval | 1 to validate; apply not estimated because it is not authorised |

Phase 0–3 total at `296e187`, excluding what is not authorised: **roughly
fourteen slices remain of the twenty-five**, six having been spent (B5's crate
half, B6, B10's cell half, B11, and B16 and B17's wiring half rendered moot or
done by the owner's instruction). **About six are unblocked today** — B5's
durable book, B8, B10's central half, B12, B13 once D3 is answered, B15 —
and the rest wait on a `terraform` binary, a project, a vendor host, or an
ADR.

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
| 11 Arbitrage and market making | Path 2 above 93 percent; quoting net positive | `qip-arbitrage` constructed by the cell and scanning its own books (`71f9465`); a cycle short a leg vetoed whole; `qip-edge-node` installs no desk; `qip-orderbook` | Ahead of phase in code, reached by no deployed process | Phase 3 gate |
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

Each verified at `de5d042` and re-verified at `296e187`. Where a decision has
been taken by a commit or by the owner's instruction — create the new
infrastructure while devouring the old, which `808ca32` and `c924191` cite as
their authority for the code and not for an apply — that is said.

| # | Decision | What it blocks | Default if undecided | Verified how |
|---|---|---|---|---|
| D1 | ~~**The egress path**~~ | — | — | **Taken, in code and recorded:** option (b), the co-located sidecar — `modules/egress-proxy` and the sidecar in `modules/cloudrun` (`c924191`), wired at `808ca32`, with a systemd unit on the execution node; ADR 0024 at `2b7e502`. `qip-transport/src/http.rs` still refuses `https` by name, which is the design |
| D2 | **In-tree HMAC vs ADR 0009** (F3) — admit a vetted crate by ADR, or decline and amend ADR 0002's reversal clause | B9; §46.2's real signatures and PQC; every further use of `hmac_sha256` | The primitive stays; each new caller restates F3 in its diff | `qip-core/src/hash.rs:151,163` carry `sha256` and `hmac_sha256`; no crypto ADR after 0023 |
| D3 | **The internal-crossing cap interval** (§27.1 "per instrument per interval") | B13 — a full cancellation can never cross today (F7) | The cap refuses every full cancellation; safe, and less than the blueprint asks | `grep -i interval` in `qip-contracts/src/intent.rs` returns nothing; `cell.rs:1000-1006` documents the refusal |
| D4 | ~~**Switch the GKE egress proxy on**~~ | — | — | **Moot.** Both manifest copies were deleted at `7d79161` with the chart and the raw manifests. The image question survived in a different shape and was answered: the sidecar's digest is read from `infrastructure/egress/vendored-images.txt`, the same one the chart pinned (`c924191`). What replaces this row is B2 |
| D5 | **ADR 0020 steps 1–5** | B17's validate half, Phase 16, Layer 6 leaving 0/7 | Nothing is applied | **Taken for the code, not for an apply.** `808ca32` reads the owner's instruction as approval to wire the blueprint runtime into the root module and remove the cluster's Terraform; steps 1 and 2 (evidence and a warm comparison) were skipped rather than passed, step 5 (retire the chart) happened in the tree. No plan has run — no `terraform` binary — and no apply is authorised. ADR 0020's text is unchanged and now describes a sequence the tree did not follow; amending it is the owner's |
| D6 | **`qip-arbitrage` and `qip-normalization`** — partly taken | A5's normaliser half; Phase 1 normalisation in the runtime path | The normaliser stays a crate nothing constructs | **Arbitrage: taken by code** — the cell constructs the desk (`71f9465`, `Cell::with_arbitrage`), so the edge is live; the node installs none. **Normalisation: half taken** — the kernel's dead edge was dropped at `2a74706`, so it is no longer compiled into a binary that never calls it; `grep -rln qip-normalization backend/crates --include=Cargo.toml` names only the crate itself and the acceptance crate. Whether it is research-only or belongs in the runtime path is still unsaid |
| D7 | **K3 — what the application zone may reach**: the DOCX's "raise intents only, never a node, venue, QPU or key" or the diagram's wider "reaches Intelligence" | The typed-intent API surface (§40.9) | The narrower reading, which is what is built — and since `827a40e` what is tested: `api_boundary.rs` refuses the edge or constructor that would widen it | `blueprint-diagram-reconciliation.md` K3 unchanged; `trust_zones` is now wired (`808ca32`) with the narrower reading in its default-deny |
| D8 | ~~C4 — correct the observability rule file~~ | — | — | **Taken.** `232bc16` "Stop telling every agent the edge plane cannot emit" corrected `.claude/rules/domains/observability.md`; the reflog does not record who approved a rules-file edit, and that should be confirmed. What remains is A1 (the matrix row) |
| D9 | **The market-data and chain-RPC hostnames and their licensing posture** | B3, B2, B4; a venue for `execution_nodes` | No listener; the adapters stay inert; the blueprint's Phase 1 cannot start | `infrastructure/egress/envoy.yaml:392-492` declares five clusters — storage, BigQuery, Vertex, two IBM Quantum — none a vendor; `execution_nodes = {}` in every environment because a node needs a venue nobody has recorded |
| D10 | **ADR 0023 step 3 versus the Phase 2 gate** | B12, B15 | — | **Overtaken in practice.** The feasibility gate (`95a4932`), the arbitrage desk (`71f9465`) and the attribution join (`7ef6063`) are execution-side work built before the Phase 2 gate passed, under the same instruction that wired the runtime. ADR 0023's text is unchanged and still lists that under what would make it wrong; reconciling the record with what was done is the owner's |
| D11 | **Whether the matrix gains rows for §48 / rule 77** (OpenTofu, Cloud Build, Cloud Deploy, third-party source control) and what status they carry | A1's completeness | Unscored; a reader of §48 finds no row and assumes either aligned or ignored | No such row in the matrix's constraint or layer sections |
| D12 | ~~**A2's shell**~~ | — | — | **Taken by circumstance.** This refresh had a shell and ran the documentation suite; at `fca98cc` the checkout is clean and the three deployment suites are retargeted (`81dd1cd`). What is left of D12 is A2 itself — the run — and nothing blocks it |

---

## 6. Environmental blockers — what can and cannot be proven from here

**No `terraform` binary** — `which terraform helm kubectl gcloud` finds none
of the four in this environment. The `cloudrun`, `execution-node`,
`trust-zones` and `egress-proxy` modules, the root module that now wires them
(`808ca32`, `c924191`), the catalogue, the reduced network and secrets modules
and every tfvars have never been run through `terraform fmt -check`,
`terraform validate` or a plan. What that means for confidence, stated
precisely: the HCL has been read by `infrastructure.rs`, which is a text
scanner, by a hand checker that confirmed every brace closes (`808ca32`'s own
words), and by people; it has not been read by the `hashicorp/google ~> 6.12`
provider's schema. A misspelt attribute, a wrong block type, or a variable
validation that never compiles would pass every check that has run. Every
precondition the new modules assert — the catalogue refusing an undeclared
zone or a missing digest, the bucket refusing a host outside the allowlist —
is asserted and unexercised. This plan scores all of it IMPLEMENTED-UNVERIFIED
and nothing about it may be promoted until a validate has been quoted.

**No `helm` binary, and no longer a chart.** The blocker this paragraph
recorded is moot: `infrastructure/helm/qip/` was deleted at `7d79161` without
ever having been rendered here. For five commits the gate carried the
consequence — `egress.rs`, `infrastructure.rs` and `manifest_wiring.rs` at
`296e187` read the chart and the raw manifests (`egress.rs:46`, `:1096`) and
could not pass — until `81dd1cd` retargeted all three at the runtime that
exists, keeping every property they held (its message: infrastructure 59,
egress 14, manifest_wiring 11 passed, 28 mutations fired).

**No project reachable.** Nothing can be planned, applied or observed.
Consequently: `workload_metrics_exist` cannot be flipped, the secret-mount
chain stays never-exercised-live, `infra.yml down` (now targeting execution
nodes, `b85684f`) stays never-run, and the sidecar cannot be observed serving
a request. ADR 0020 step 1's evidence can no longer be gathered at all, since
the cluster it asked about is not in the tree.

**No live-data deployment.** The Phase 1 exit (seven days streaming) and the
Phase 2 gate are impossible to attempt from here, whatever code lands. One
real tick was fetched in-session (`gap-matrix.md` item 6) through a bridge that
is not the platform's egress path; that is the SENSE half of one cycle and
nothing more.

**A shell, this time.** The session that first wrote this document had no
shell; the one that re-scored it ran the documentation suite and quotes it in
the commit. It did not run the full gate: while it worked, the checkout
carried other owners' uncommitted edits to three acceptance suites, the kernel
and the risk lib, and by the time those landed (`88eb1e2`, `81dd1cd`) the
session's remit — five documents — was what it had evidence for. At `fca98cc`
nothing blocks A2 but the hour it takes.

What *can* be proven from here: everything the Rust workspace asserts about
itself — the paper layers, the authority boundaries, the flow links marked
TESTED, the application layer's reach — because those are tests over source
and text, and they run without a cloud. That is the whole of the evidence
base above §2.3's Layer 6, and it is why Layer 6 is the one row at zero.

---

## 7. How far away are we — the honest paragraph, twice

**Alignment-done.** Closer than at `de5d042`, and what is left is not code
in the cycle. Of the seven alignment items, three are spent — the scorecards
are re-scored at `296e187`, the three controls nothing called are called, the
risk rule has its probe — and two more were found and wired on the way. What
remains: re-measuring the full gate, unblocked at `fca98cc` and not run in
this series; a collector for emitters that have none, which needs an attested
sidecar digest nobody has pinned; and the owner saying what becomes of a
crate nothing constructs and correcting two sentences in one rules file that
still say LEARN's two callers do not exist. The boundaries are
enforced structurally, the application layer's reach is a test rather than a
reading, and the paper-trading line has three layers and a test on each. What
alignment-done will *not* mean is that the cloud layer is proven — and that
sentence has changed shape: the runtime in the tree is now the blueprint's,
wired into the root module under the owner's instruction and recorded in ADR
0024, and it has never been seen by a Terraform binary, never planned, never
applied; no process has ever been shown to run on the old runtime or the new.
Call it two or three slices from aligned, with one layer that cannot be
scored above zero from this environment.

**Blueprint-done.** Far, by the blueprint's own reckoning, and the distance is
not mainly code. The tree holds capability from Phase 1 to roughly Phase 15 —
netting, a feasibility gate, an arbitrage desk, cumulative trials and a
holdout band, a cost router, a quantum adapter with its classical baseline, a
per-region node module — and has passed none of the four gates, because every
gate is a question about real data or a real venue and the platform has never
streamed real data for a day. Per gate, at `296e187`: Phase 2 waits on real
data and a durable trial book, the correction itself now being in the tree;
Phase 3 cannot pass while paper trading is absolute, and now at least has a
band to be inside of; Phase 6 has no Brier comparison against any venue's
implied probability; Phase 8 has no out-of-sample comparison against an
unconditional baseline. The first thing between here and the first gate is
still an egress path — no longer undecided, now a sidecar written in Terraform
that no binary here can plan, pointed at no vendor, in a project nothing can
reach; after that, seven days of streaming nobody can run from this
environment; after that, the Phase 2 gate, which the blueprint calls the most
important sentence in the document and expects to fail more often than pass.
Phases 0 to 3 are roughly fourteen slices now, six unblocked; the rest wait on
a Terraform binary, a vendor host, an ADR and a decision on the crossing
interval. Phases 4 to 19 are well over a hundred more, behind gates that may
say stop. Of the two direction decisions — no Kubernetes, Leptos — the first
has been taken for the code and for nothing that runs, and the second has not
begun. The honest unit is not weeks; it is gates, and zero of four have
passed.

---

## Verification of this document

The gate for this file is
`cd backend && cargo test -p qip-acceptance --test documentation --no-fail-fast`,
which checks every internal link resolves and refuses the overclaims it names.
Run it on every edit and quote the `test result:` line in the commit. This
document does not claim the gate ran; the commit that lands it must.
