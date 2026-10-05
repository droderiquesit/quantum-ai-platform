# SYSTEM_MAP: what Algorik is, what exists, and what must hold

Phase 0 output of `HERMES_MISSION.md` section 2.2. Written 2026-10-04 in the worktree
`.claude/worktrees/hermes-phase-0`. Nothing was committed, no cloud or Terraform state was touched.

## 0. How to read this, and what it does not claim

**Sources, in the mission's priority order (HERMES_MISSION.md s2.1).** (2) Master Blueprint v12.0,
`docs/blueprint/source/algorik-master-blueprint-v12.0.txt`, cited as `v12 pN sX` where N is the
`=== page N ===` marker. GCP Blueprint v3.0, `.../algorik-gcp-platform-blueprint-v3.0.txt`, cited as
`GCP3 pN sX`. (3) `docs/MASTER_ROADMAP.md`, cited as `RM-xxx`. (4) The code and infrastructure:
`backend/crates/**`, `frontend/**`, `infrastructure/**`, `.github/workflows/**`, cited by path. Also read:
`CLAUDE.md`, `.claude/rules/**`, `docs/blueprint/v12-delta-master.md`, `docs/blueprint/v12-delta-gcp.md`,
`docs/adr/` (0099, 0100 in full; others by title), `docs/ops/hermes-baseline-2026-10-04.md`.

**Standing constraint that outranks the blueprint (HERMES_MISSION.md s10.5, ADR 0003, ADR 0021, ADR 0099).**
This platform is paper-trading only and never submits a live order. v12 and GCP3 assume live capital in
several places (`GCP3 p27 s24` "prod: Live bounded execution"; `v12 p26 s13` treasury; `v12 p31 s20`
purchasing; `v12 p30 s18.2` market creation; `v12 p37 s24.4` publishing). Every such element is marked
`BLOCKED` below and is mapped only as far as its shadow, paper or simulated form. v12 is not yet the
architecture of record: ADR 0101 is claimed in `docs/adr/README.md` with no body, so on ADR 0099's terms
every standing decision a v12 item contradicts (conflicts C1 to C8) keeps its force.

**Method limits, stated because the repo's evidence rule requires it.**
- The Bash tool was unavailable to the agent that wrote this file. **No command was run.** No test was
  executed, so no pass/fail, test count or `test result:` line is claimed anywhere. Everything below comes from
  reading files and from content searches (Grep/Glob).
- "Test exists" means a function with that exact name was found by search in the tree. It does not mean the
  test passes, and it does not mean it was mutation-verified (repo rule, `.claude/rules/architecture/01-testing-strategy.md`).
- Measured 2026-10-04, after `cargo clean` freed 12 GiB: `cargo test --workspace --no-fail-fast` gave 492
  `test result:` lines, 6497 passed, 4 failed, 0 ignored. One failure (the pinned non-Rust tooling set) was
  fixed in `decdf675`; three are pre-existing on main (`infrastructure/kubernetes/base/egress.yaml` is tracked).
  The first baseline run in `docs/ops/hermes-baseline-2026-10-04.md` failed to link on a full disk and is superseded
  by this. `cargo fmt --check` and clippy exited 0 there.
- Line numbers into live files are deliberately not cited (repo convention, `.claude/rules/domains/observability.md`).
  Cite by path and function name; re-find with grep.
- Status vocabulary for tests and invariants. **Y** = a named test whose name states the property was found.
  **P** = a related test or a compiler/lint/CI-script enforces part of it; the gap is named. **N** = no test found.
- Owner column: "stream/role" follows `docs/MASTER_ROADMAP.md` (streams A to J; roles SA, BE, DAI, CPE, SEC, SRE,
  TE, TW, PM, FE). No owner assignment exists elsewhere in the repo, so these are proposals, not facts.

## 1. Product, users, journeys

### 1.1 Product

- **v12 identity.** "Algorik is not a trading bot and not merely an investment platform. The intended system is
  a global cognitive financial operating system" that senses the world, builds a causal world model, trains
  models, allocates capital, executes across regions, "learns from every action and non-action, and maintains
  one authoritative multi-asset ledger of truth" (`v12 p2 s1`). v12 adds a "financial superintelligence"
  target: hidden-state estimation, multi-horizon forecasts, synthetic futures, active sensing and CPU/GPU/TPU/QPU
  routing (`v12 p2 s1`). Its status paragraph says it is a target architecture and "not a claim that
  present-day AGI has been achieved" (`v12 p1`).
- **Two nervous systems plus a warm layer** (`v12 p2 s1`): slow cognitive (search, model, simulate, train);
  fast reflex (microseconds to milliseconds, local, no wait on the global brain); warm coordination
  (cross-region arbitrage, capital movement, settlement). Five time lanes, 0 to 4, are described in `v12 p9-10 s4`.
- **What the repo says it is.** "Multi-regional AI and quantum research platform for investment decisions.
  Strictly paper trading. It never submits a live order." Eight stages run in one cycle: SENSE, UNDERSTAND,
  DISCOVER, REASON, SIMULATE, DECIDE, ACT, LEARN (`CLAUDE.md`; the enum has exactly eight variants in
  `backend/crates/runtime/qip-kernel/src/cycle.rs`). Non-goals: live order submission, retail distribution,
  a trading venue, a general-purpose ML platform (`CLAUDE.md`).
- **Delivered state.** `execution_nodes = {}` in every environment; no process of the platform is running
  anywhere; the dev project has billing disabled (`docs/ops/hermes-baseline-2026-10-04.md` s3); the GitOps
  plane is suspended (ADR 0093); `docs/architecture/current-state.md` ("No process of this platform is running in any environment today").
- **The tension between the two, kept visible.** v12 describes live trading, market creation, purchasing and
  public communication as product capabilities; the repo's non-goals and ADR 0021 refuse them. The map treats
  the v12 product as the target and the paper-only form as the only buildable one (see section 7, T1).

### 1.2 Users

| User | Source | What they do |
|---|---|---|
| Research and risk desk (the only stated intended user) | `CLAUDE.md` "Intended users are the research and risk desk running it, not external customers" | Run the loop, read the book, halt it, sign recalibrations |
| Operator with a named identity (four API roles: Monitor, Viewer, Analyst, Operator) | `docs/architecture/current-state.md` (qip-api::auth); ADR 0038 (passkeys), ADR 0041 (venue registration is one operator's attributed click), ADR 0076 (per-person operator identity) | Registers venues, signs reinstatements and recalibrations, trips the kill switch |
| Customer, mandate owner, "user/mandate ownership" | `v12 p28 s15` (subledger), `GCP3 p14 s12` (Identity Platform for customer identities); portal has sign-up, marketing and legal pages (`frontend/portal/src/app/(auth)`, `(marketing)`) | **Unresolved**: see section 7, U22 |
| Engineering agents (Hermes agency, Claude agents) | `GCP3 p14-15 s13`, `HERMES_MISSION.md s4`, `docs/adr/` ADR 0098 | Change code and infrastructure through the normal pipeline only |
| Counterparties: venues, custodians, data vendors, IBM Quantum | `v12 p6-7 s3`; `infrastructure/egress/vendored-images.txt` | Not users; the platform's external dependencies |

### 1.3 Journeys that matter (these become the first E2E tests)

Order is the build priority. "Runs today" means it can be executed in-tree against simulated venues; none runs
in a deployment because nothing is deployed.

| ID | Journey | Source | Nearest existing evidence (found by search, not run) | Proposed E2E test name | Runs today |
|---|---|---|---|---|---|
| J1 | Tick to decision to journal to ledger to replay: a canonical tape drives one reflex cell, which decides under deterministic local risk, journals through the event fabric, the ledger posts the fill once, and replay reproduces the decision hash | `GCP3 p25 s21.1`; `v12 p28-29 s16`, `p47 s30` Phase 1; ADR 0100 s8 | `a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move` (`qip-edge-node/tests/pass.rs`); `a_clean_journal_replays_into_three_identical_registries_and_the_run_exits_zero` (`qip-cli/tests/replay.rs`); real-process harness only in `qip-acceptance/tests/event_fabric_harness.rs`. **ADR 0100 s8 lists eight proving tests; none was found by name.** | `the_first_vertical_slice_runs_tape_to_ledger_across_real_processes_and_replays_byte_for_byte` | Partly (per-crate), not as one test |
| J2 | Operator opens the console behind IAP, signs in with a passkey, sees the book labelled PAPER TRADING, and cannot find a control that submits an order | ADR 0038, ADR 0095, `.claude/rules/domains/frontend.md` | `frontend/portal/tests/{auth,boundary,iap,gate}.spec.ts`; `every_surface_that_renders_a_platform_fact_still_says_paper_trading` (`qip-web/tests/web.rs`) | `an_admitted_operator_reads_the_paper_book_and_no_page_offers_an_order_control` (Playwright, against a deployed dev URL) | Spec files exist; Playwright not installed locally (baseline s4) |
| J3 | Operator trips the kill switch; every entry point stops; nothing clears it from the console | `docs/operations/kill-switch.md`; `v12 p4 s1.3` | `a_halt_stops_the_assembled_platform_at_every_entry_point` (`acceptance.rs`); `the_kill_switchs_release_is_as_permanent_as_its_engagement` (`qip-events/tests/backbone.rs`) | `a_kill_switch_tripped_from_the_console_halts_every_cell_and_survives_a_restart` | Partly |
| J4 | Centre issues a signed capital grant; the cell spends within its share, is cut off, keeps working on its last valid envelope, then stops at expiry | `v12 p27 s13.2`, `p45 s28` row 1; ADR 0008, ADR 0039 | `a_cell_cut_off_from_the_centre_spends_only_its_grant_and_then_stops` (`stress.rs`); `with_nothing_delivered_the_cell_runs_on_its_last_valid_envelope_until_it_expires` (`qip-edge-node/tests/control.rs`); `region_share.rs` | `a_grant_issued_by_the_centre_funds_a_cell_and_the_cell_survives_losing_the_centre` | Yes in-process |
| J5 | World event to cognitive response: a discovered source passes its licence gate, becomes evidence with provenance, updates the world model, and ends as a learned lesson | `GCP3 p25 s21.2`; `v12 p12-13 s6.3`, `p13 s7` | `the_platform_walks_from_a_discovered_source_to_a_learned_lesson` (`e2e.rs`); `one_market_event_travels_all_seven_stages_of_the_truth_loop` (`truth_loop.rs`; see U16) | `a_source_licensed_for_research_never_reaches_a_trade_and_still_teaches_the_world_model` | Yes in-process |
| J6 | Operator registers a venue under their own identity, a venue is withdrawn on feasibility evidence, and reinstatement needs two signatures | ADR 0041, ADR 0062; `docs/operations/{registering,reinstating}-a-venue.md` | `qip-api/src/venue_views.rs`; `a_venue_the_twin_says_is_filling_the_desk_badly_is_measured_and_put_on_the_record` (`adversary.rs`) | `a_withdrawn_venue_is_reinstated_only_by_two_distinct_operator_signatures` | Partly |
| J7 | A risk limit is recalibrated: the LEARN stage proposes from counterfactual regret, two operators sign, the emitted file is committed | ADR 0061; `docs/operations/recalibrating-a-limit.md` | `no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration` (`security.rs`) | `a_limit_moves_only_after_two_signatures_and_a_committed_file` | Partly |
| J8 | Cross-region multi-leg arbitrage in simulation: discover, reserve, fire, partial-fill unwind, counterfactual score | `v12 p29-30 s17`; `GCP3 p25 s21.3` | `a_broker_outage_mid_arbitrage_stops_the_plan_and_names_the_exposure_it_stranded`, `an_arbitrage_leg_the_book_cannot_fill_is_refused_rather_than_part_executed` (`stress.rs`); `qip-edge/tests/arbitrage.rs` | `a_three_leg_cycle_across_two_simulated_regions_completes_or_unwinds_and_is_scored` | Single-cell only; no peer mesh (E03) |
| J9 | A quantum optimisation runs asynchronously with its classical baseline in the same record and ships the classical answer when quantum does not win | `v12 p23-25 s11`; ADR 0006 | `the_classical_baseline_is_always_computed`, `a_quantum_answer_that_ties_loses_to_the_classical_baseline` (`qip-optimization-engine/tests/optimization.rs`); `quantum_being_unavailable_produces_a_classical_answer_that_says_why` (`stress.rs`) | `a_cycle_that_sizes_a_proposal_journals_both_answers_and_the_classical_one_ships_when_quantum_loses` | Yes (local simulator) |
| J10 | An engineering change goes branch, PR, gates, attested image, promotion with no unauthorised path to prod | `GCP3 p16 s14`, `p25 s21.4`; `.github/workflows/*` | `nothing_deploys_that_has_not_passed_the_test_suite`, `the_infrastructure_workflow_cannot_touch_production` (`infrastructure.rs`); `prod_is_promoted_by_nobody_until_an_adr_says_otherwise` (`gitops.rs`) | `a_change_reaches_dev_only_through_ci_green_attestation_and_a_pinned_digest` | Gates exist; deploy blocked (billing) |
| J11 | Forecast to capital: forecasts fan out, are scored, and bounded `CapitalGrant`/`RiskEnvelope`/`HedgePlan` come back | `GCP3 p25-26 s21.5`; `v12 p19-20 s9.7-9.14` | None: no `ForecastState`, `ModelVote`, `HedgePlan` symbol exists | `a_forecast_with_high_disagreement_shrinks_a_grant_and_never_enlarges_one` | No |
| J12 | Causal agency, shadow only: ambient signal to goal to intervention plan with a no-action baseline to gates to "would have done" record | `v12 p38 s24.7`, `p47-48 s30` Phase 10 | None: no `GoalSpec`, `InterventionPlan`, `ActionIntent` symbol | `an_intervention_plan_in_shadow_records_what_it_would_have_done_and_sends_nothing` | No (and real actions BLOCKED) |

## 2. Subsystems and the contract on every edge

### 2.1 Subsystems

Status column: **built** (code reached from a composition root and tested), **partial**, **not built**,
**BLOCKED** (needs live capital or external action or an unrecorded dependency decision). "Reached from a
production process" is not claimed for anything: nothing is deployed. Component names are the blueprint's;
"real crate/path" is what was found in this worktree. The in-flight files in the main checkout
(`qip-causal`, `fusion.rs`, `lattice.rs`, `state_estimator.rs`, `saga.rs`) are not in this worktree and are
therefore not counted (see `docs/MASTER_ROADMAP.md` "In-flight work").

| ID | Blueprint component (cite) | Real crate/path, or "not built" | Status | Owner (proposed) |
|---|---|---|---|---|
| S01 | Global Scout Fabric: discovery, access, licensing, source scoring (`v12 p12-13 s6`; `p35 s23`) | `services/qip-data-finder` (catalogue, licensing, robots, freshness, registration); `apps/qip-deepbrain/src/{discovery,connectors,campaign}.rs` | partial | D / DAI |
| S02 | Evidence, Provenance and Truth Fabric (`v12 p13 s7`) | `services/qip-reasoning-engine/src/{evidence,hypothesis,redteam}.rs`; `services/qip-streaming/src/provenance.rs`; `libs/qip-contracts/src/governance.rs` (`Provenance`, `Entitlement`) | partial; no `EvidencePacket` type | D / DAI |
| S03 | Market Intelligence and Tick Learning Fabric (`v12 p13-15 s8`) | `services/qip-market-ingestion/src/{tape,replay,depth}.rs`; `services/{qip-learning-engine,qip-training}`; `edge/qip-feature-dag` | partial | D / DAI |
| S04 | Tick capture, clock/sequence normalisation, book reconstruction (`v12 p14 s8.2`) | `edge/{qip-protocols,qip-sequencing,qip-orderbook}`; `libs/qip-contracts/src/{message,market_event,time}.rs` | built (simulated feed only) | C / BE |
| S05 | Immutable Tick Lake (`v12 p14 s8.2`; `GCP3 p10 s9.2` tier T) | `apps/qip-fabricd/src/archiver.rs` and `libs/qip-storage/src/segment/archive.rs` exist; the archiver is a stub in this tree per `RM-P2-06` | not built | C / DAI, BE |
| S06 | Replay, execution digital twin, counterfactual execution (`v12 p14-15 s8.3`) | `services/{qip-simulation-engine,qip-twin}`; `edge/qip-edge/src/journal.rs`; `apps/qip-cli` replay subcommand | partial | C / TE, DAI |
| S07 | World + market intelligence fusion (`v12 p15 s8.4`) | not built in this tree (draft `fusion.rs` only in the main checkout) | not built | D / DAI |
| S08 | World Model Federation, arbitration (`v12 p17 s9.3`; `p8`) | `services/qip-world-model` (single model: state, graph, causal, granger, confounder, falsification) | partial; no federation, no branching | D / DAI, SA |
| S09 | Symbolic and neuro-symbolic reasoning (`v12 p17-18 s9.4`) | `services/qip-reasoning-engine/src/bayes.rs` is probabilistic only; no rule, SAT/SMT or temporal-logic engine | not built | D / DAI (C5 blocks solvers) |
| S10 | Proactive Ambient Model Mesh and Attention Router (`v12 p18 s9.5`; `p38 s24.7`) | none; `ambient` hits in the tree are unrelated (`RM-P8-05`) | not built | J / DAI |
| S11 | NOW Brain: global state estimation (`v12 p18-19 s9.6`; `GCP3 p12 s11.2`) | none in this tree | not built | D / DAI |
| S12 | Temporal Forecast Lattice (`v12 p19 s9.7`; `GCP3 p12 s11.3`) | none in this tree | not built | D / DAI |
| S13 | Model Tournament, Forecast Market, internal synthetic capital (`v12 p19 s9.8`, `p32 s22.1`) | none; `services/qip-prediction/src/scoring.rs` scores event-market probabilities, a different thing | not built | D / DAI |
| S14 | Future-state tree, synthetic worlds, rare-event factory (`v12 p19-20 s9.9`) | `services/qip-twin/src/counterfactual.rs` only | partial | D / DAI |
| S15 | Active Sensing, Curiosity, value of information (`v12 p20 s9.10`) | `apps/qip-deepbrain/src/campaign.rs` (bounded fetch campaign) only | partial | D / DAI |
| S16 | Adversarial verification society (`v12 p20 s9.11`) | `services/qip-reasoning-engine/src/redteam.rs`; `services/qip-world-model/src/falsification.rs` | partial | D / DAI |
| S17 | Meta-Intelligence Brain, Cognitive Attention Market (`v12 p20 s9.12`) | none; `services/qip-cost-router` prices a decision's cost, which is adjacent | not built | J / DAI |
| S18 | Cognitive Compiler / reflex distillation (`v12 p20 s9.13`; `p14 s8.2`) | `services/qip-training` (fit and shrink for the hot path) | partial | D / DAI |
| S19 | Specialist Brain Society (`v12 p16-17 s9.1`) | `agents/qip-investment-agents`, `libs/qip-agents` | partial | J / DAI |
| S20 | Memory and Self Model (`v12 p17 s9.2`) | `libs/qip-ai` (episodic, memory), `libs/qip-agents/src/memory.rs`; portal `cognition/self-model` | partial | D / DAI |
| S21 | Model and Strategy Foundry, Model Registry (`v12 p21-23 s10`) | `services/{qip-evolution,qip-training,qip-lifecycle}`, `edge/qip-strategy`, `quant/qip-quant`, `libs/qip-ai` | partial | D / DAI |
| S22 | Quantum Foundry (`v12 p23-25 s11`) | `libs/qip-quantum` (statevector, QAOA, benchmark, solver, provider), `services/qip-optimization-engine/src/router.rs` | partial. Problem Compiler, Compute Intelligence Router and Solver Registry: not built | G / DAI |
| S23 | Asset Brain (`v12 p26 s12`) | `services/qip-portfolio-engine`, `libs/{qip-portfolio,qip-financial}`; asset registry not found (`RM-P3-01`) | partial | E / BE |
| S24 | Capital Brain and Capital Bank (`v12 p26 s13`) | `services/{qip-capital,qip-capital-fabric}` (simulated); Capital Intelligence Society, Survival Kernel, Shadow Portfolio Universe not built | partial; real treasury BLOCKED | E / BE |
| S25 | Risk Brain and Deterministic Risk Gate (`v12 p27 s14`) | `libs/qip-risk` (limits, hedge engine), `services/qip-risk-engine`, `libs/qip-compliance/src/model_risk.rs`; Model-Risk Brain and Tail Universe partial | partial | E / BE, SEC review |
| S26 | Ledger, accounting, state of truth (`v12 p28 s15`) | `libs/qip-portfolio/src/ledger.rs` (pure double-entry), `apps/qip-ledgerd` (single writer), `libs/qip-contracts/src/ledger.rs`; Spanner not built | partial | B / BE |
| S27 | Regional Reflex Node (`v12 p28-29 s16`) | `edge/qip-edge` (Cell), `apps/qip-edge-node`; feed, books, features, strategy, risk, routing crates under `edge/` | built, simulated feed only | C / BE |
| S28 | Reflex Mesh, peer to peer: opportunity tokens, reservations, sagas (`v12 p29-30 s17`) | **cell-to-centre** mesh exists (`edge/qip-edge/src/mesh.rs`, `apps/qip-api/src/mesh.rs`, `libs/qip-transport/src/mesh.rs`); **cell-to-cell peer mesh and `OpportunityToken`: none**. `services/qip-mesh` is "point-in-time data mesh ports", not this (its own lib doc) | not built | F / BE |
| S29 | Arbitrage graph, 2 to 20 legs (`v12 p29 s17.1`) | `edge/qip-arbitrage`, `edge/qip-routing/src/pathcycle.rs`, `edge/qip-edge/src/arbitrage.rs` | partial (cross-region coordination absent) | F / BE |
| S30 | Execution, market making (`v12 p30 s18`) | `services/{qip-execution-engine,qip-brokers}`, `edge/qip-edge/src/quoting.rs`, `libs/qip-market` | built (simulated venues) | F / BE |
| S31 | Market creation / Market Factory (`v12 p30 s18.2`) | quote is a priced intent with no path to a venue, originated markets gated by a type (ADR 0067) | BLOCKED as a real act | H / BE |
| S32 | Prediction and event markets (`v12 p30 s19`) | `services/qip-prediction`, `services/qip-world-model/src/resolution_source.rs` | partial; wagering isolation unproven | H / BE |
| S33 | Physical commerce (`v12 p31 s20`) | none | BLOCKED | H / BE |
| S34 | Asset coverage registry (`v12 p31-32 s21`) | object model in `libs/qip-financial`; no registry | not built | E / SA, BE |
| S35 | Continuous Learning and Evaluation Brain (`v12 p32 s22`) | `services/qip-learning-engine`, `qip-twin`, `apps/qip-deepbrain/src/learning.rs`, kernel LEARN stage | partial | D / DAI |
| S36 | Intelligence Expansion Engine and eight registries (`v12 p32-35 s23`) | source catalogue only (`qip-data-finder`) | not built | J / SA, DAI |
| S37 | Causal Agency and Intervention Engine; Communications and Conduct Brain (`v12 p36-38 s24`) | none | not built; real actions BLOCKED | J / DAI, SEC |
| S38 | Governance, legal isolation, authority (`v12 p39 s25`) | `libs/qip-compliance` (licensing, approval, signing, incident, point-in-time), `services/qip-capital` eligibility | partial | E / SEC, BE |
| S39 | Native Rust Event and Control Fabric (`v12 p41-42 s26.1`; `GCP3 p7-9 s7`) | `libs/qip-events/src/event_fabric/`, `services/qip-streaming/src/event_fabric/`, `libs/qip-transport/src/event_fabric/`, `libs/qip-storage/src/segment/`, `apps/qip-fabricd`, `qip-cli` subcommands. Controller/Raft, mirror, bridge, schema registry on Protobuf: not built | partial: single node, RF1, plaintext TCP (ADR 0100 s3, s7) | B / BE |
| S40 | AIOps (`v12 p46 s29`) | `libs/qip-observability`; no remediation agent | not built | I / SRE |
| S41 | Portal, API, console (`GCP3 p14 s12`) | `apps/qip-api`, `apps/qip-web` (library), `frontend/portal`, `frontend/landing` | built, not deployed | I / FE, BE |
| S42 | Observability: OpenTelemetry + OpenObserve (`v12 p40 s26`; `GCP3 p17-18 s16`) | `libs/qip-observability`, `infrastructure/terraform/modules/{observability,openobserve}`; Prometheus exposition rather than OTel (ADR 0026) | partial; nothing proven scraped (`.claude/rules/domains/observability.md`) | I / SRE |
| S43 | Engineering plane: CI, supply chain, GitOps, autonomous development (`GCP3 p14-16 s13-14`) | `.github/workflows/{ci,deploy,image,infra,vendor}.yml`, `infrastructure/gitops/`, `.claude/` | partial; GitOps never bootstrapped (ADR 0093) | I / CPE, SRE |
| S44 | Security and trust (`GCP3 p17 s15`) | terraform modules `identity`, `secrets`, `binaryauthorization`, `trust-zones`, `egress-proxy`; Cloud HSM, CAS, VPC-SC, PAM: not found | partial | I / SEC, CPE |

Counts for this table (by reading the Status column, not by script): 44 rows. built 4 (S04, S27, S30, S41); partial 25;
not built 13 (S05, S07, S09, S10, S11, S12, S13, S17, S28, S34, S36, S37, S40); BLOCKED 2 (S31, S33). S24 and S37 carry a BLOCKED
qualifier for their real-money or external-action forms inside another status.

### 2.2 Edges and where each communication contract lives

"Contract" means a written, checkable agreement between the two ends: a shared type crate, a wire constant
held by a test, a catalogue file, or an ADR. "No contract yet" means none was found.

| ID | Edge (from to) | Blueprint cite | Where the contract lives | Gap |
|---|---|---|---|---|
| E01 | Venue feed to reflex node | `GCP3 p23 s20`; `v12 p29 s16` | `libs/qip-contracts/src/{message,market_event,venue}.rs`; decoders in `edge/qip-protocols`; tape rule: only with `QIP_VENUE_FEED=simulated` (`a_tape_is_refused_unless_the_feed_mode_is_simulated`) | No live feed exists or is allowed |
| E02 | Reflex node to venue (orders) | `GCP3 p23 s20` | `services/qip-execution-engine` `Broker` port, `edge/qip-routing` gateway, `VenueClass` in `qip-contracts::venue`; live class refused (`a_cell_handed_a_live_class_gateway_places_nothing_and_names_the_gate`) | Paper only, by design |
| E03 | Reflex node to peer reflex node (opportunity tokens, reservations, sagas) | `v12 p29-30 s17`, `p43 s27` `OpportunityToken`; `GCP3 p23 s20` "Direct QUIC" | **No contract yet.** No `OpportunityToken` symbol; QUIC refused (conflict C2) | `RM-P4-02`, `RM-P4-03`; roadmap cites `qip-mesh` as the peer mesh but it is not |
| E04 | Reflex node to event fabric (journal, outcomes) | `v12 p28 s16`, `p41-42 s26.1`; `GCP3 p23 s20` | ADR 0100 s3 to s8; `qip-events::event_fabric::{envelope,policy,codec,schema_id,hlc}`; `qip-contracts::reflex` (`Decision`); schema lock `qip-acceptance/tests/event_fabric_schema_lock.rs`; stream catalogue `infrastructure/event-fabric/streams.local.json` held by `event_fabric_catalogue.rs` | Plaintext TCP, RF1, bearer-token identity (ADR 0100 s7); QUIC/mTLS/Protobuf/BLAKE3 not allowed (C2) |
| E05 | Control down: grants, policy, halt, package announcements, to reflex node | `v12 p42 "Hot-path contract"`; `GCP3 p8 s7.2` class P0 | `qip-contracts::{capital,policy}` (`CapitalEnvelope`, `PolicyPayload`), signing in `libs/qip-compliance/src/signing.rs`, verify path `CapitalDownlink` in `edge/qip-edge/src/mesh.rs`; P0 stream in `streams.local.json` | **Two control paths** (cell-to-centre mesh from ADR 0011, and fabric P0 from ADR 0100); which is canonical is not stated (U20). `ControlPackAnnouncement` has no symbol |
| E06 | Cell to centre, state up | `v12 p5 s2` ("policy down, outcomes up only" is corrected); ADR 0008, 0011, 0039 | `qip-contracts::wire` (`CELL_DELTA_SCHEMA_VERSION`), held by `architecture.rs::neither_end_of_the_cell_uplink_declares_its_schema_version_as_a_literal`; `qip-edge/src/mesh.rs`, `qip-api/src/mesh.rs`, `qip-transport/src/mesh.rs` | No wired config in any environment (`docs/architecture/current-state.md` apps) |
| E07 | Event fabric to ledger writer | `GCP3 p10 s10`; `v12 p28 s15` | `apps/qip-ledgerd/src/consumer.rs`, `qip-contracts::ledger` (`LedgerEvent`), posting logic `qip-portfolio::ledger`; dedupe by chain continuity (ADR 0100 s4) | Spanner transactional commit: no contract (C4) |
| E08 | Ledger to API (read) | `v12 p40 s26` ("no browser access to ledger internals") | `qip-ledgerd/src/read_api.rs`, `qip-api/src/ledger_views.rs`; ADR 0100 s1 (decimal text); `qip-acceptance/tests/api_boundary.rs` | none for the in-tree form |
| E09 | Scout/sources to evidence to world model | `v12 p12-13 s6.3`, `p13 s7` | `qip-data-finder` catalogue and licensing; `qip-contracts::governance` (`Provenance`); ADR 0056, 0057, 0060 (egress routes) | `EvidencePacket` and `KnowledgeDelta` have no symbol |
| E10 | Evidence/world model to NOW Brain, forecast lattice, model society | `v12 p18-20`; `GCP3 p24 s20.1` | **No contract yet** (`ForecastState`, `WorldBranch`, `InformationRequest`, `ModelVote`, `WorldModelState`, `ModelDisagreement`: no symbol in `backend/crates`) | Whole v12 s24.9 set missing |
| E11 | Cognition to symbolic reasoning | `v12 p17-18 s9.4`; `p43 s27` `SymbolicDerivation` | **No contract yet** | |
| E12 | Cognition to Quantum Gateway to IBM Quantum | `GCP3 p24 s20`; `v12 p25 s11.2` | `qip-quantum` provider/solver/benchmark with a `ClassicalBaseline` in the same record (ADR 0006); egress through the proxy listeners (ADR 0060, `egress.rs`); trust zone rule `the_trust_zones_deny_by_default_and_only_optimisation_may_reach_ibm` | `ComputePlan` no symbol; no Qiskit version pinned anywhere readable (U24) |
| E13 | Capital/Risk/Hedge to regions (`CapitalGrant`, `RiskEnvelope`, `HedgePlan`) | `GCP3 p24 s20.1`; `v12 p39 s24.9` | `CapitalGrant`, `RiskEnvelope` in `qip-contracts::{capital,policy}`; `HedgePlan` none | `CapitalGrant` is defined twice with different fields in the blueprint (`v12 p39` vs `p42 s27`; U3) |
| E14 | Training to model registry to signed package to reflex node | `v12 p23 s10.1`; `GCP3 p11 model-package rule`, `p24 s20` | `qip-compliance/src/{artifacts,signing}.rs`; `qip-contracts::policy` (`Slot`, `PolicyItem`) | `ModelPack`, `StrategyPack`: no symbol; no package manager |
| E15 | Fabric to archive (sealed segments) | `v12 p14 s8.2`, `p41 fabric-archive` | `qip-storage::segment::archive`; `archived_through` ack field (ADR 0100 s3); retention classes ADR 0089 | Archiver is a stub here (`RM-P2-06`) |
| E16 | Browser to API (REST and SSE) | `GCP3 p14 s12`; `v12 p40 s26` | `qip-api/src/routes.rs` route table (auth role per route), `tests/api_boundary.rs`, `tests/security.rs`; frontend `packages/{api-client,shared-types}`; ADR 0042 (keyed assertion), ADR 0095 (IAP) | none |
| E17 | API to fast brain / deep brain | not in blueprint as an edge | **None.** `catalogue.tf` sets `invokers = []` for fastbrain and deepbrain; each composes its own `qip-kernel` `Platform` | The "central plane" is three independent processes sharing no network edge; how they are meant to exchange state is unstated (U21) |
| E18 | CI to cloud (WIF, attest, deploy) | `GCP3 p16 s14.2` | `.github/workflows/*`, `infrastructure/terraform/modules/{cicd,binaryauthorization,registry}`; held by `infrastructure.rs` tests | Cloud Build private pools absent |
| E19 | Reflex package to venue sessions, standby fencing | `GCP3 p6 s6` | **No contract yet** (no standby, no fencing token) | |
| E20 | Reflex Mesh vs fabric separation | `v12 p29 s17` | **No contract yet**: separation cannot be tested while the peer mesh does not exist | |

Edge counts (by reading): 20 edges. Contract found for 12 (E01, E02, E04, E05, E06, E07, E08, E09, E12, E15, E16, E18), several with a
named gap in the last column. Type exists for one side only, 2 (E13, E14). "No contract yet", 6 (E03, E10, E11, E17, E19, E20).

## 3. Deployment topology: actual Terraform beside the blueprint

Terraform reality is read from `infrastructure/terraform/main.tf`, `catalogue.tf`, `modules/*`,
`environments/*/terraform.tfvars`, `infrastructure/CLAUDE.md`; measured cloud state is from
`docs/ops/hermes-baseline-2026-10-04.md` (`billingEnabled: false` on `algorik-dev`, no `*.tfstate` in the
worktree). Nothing was re-queried by me.

| Concern | Blueprint (GCP3 / v12) | Actually in `infrastructure/` | Conflict record |
|---|---|---|---|
| Execution regions | Three: Americas, Europe, APAC, chosen by venue RTT (`GCP3 p4 s3`) | `dev`: `us-east4`. `test`, `stage`, `prod`: `europe-west2` and `project_id = "unprovisioned"`. No APAC. CLAUDE.md and ADR 0008 say seven cells; `qip-edge/src/mesh.rs` says nine (U14) | C8 |
| Reflex cell | C4D/C4 VMs under systemd, fenced warm standby (`GCP3 p6 s6`); `v12 p39` says C3/C3D | `modules/execution-node`: machine types limited to `c3-highcpu-8/22`, `c3d-highcpu-8/16`; no external address, no container runtime, shadow mode literal `true`; **`execution_nodes = {}` in all four environments**. ADR 0035 authorises one node (`newyork-1`, `us-east4`, shadow) with no boot image baked and no capital allocation chosen | C8, ADR 0035, 0045 |
| Reflex Mesh | Direct peer QUIC (`GCP3 p5 s5`) | Nothing | C2 |
| Event fabric brokers | Five Rust broker VMs per region across three zones, Raft controller (`GCP3 p8 s7.3`) | **Nothing in Terraform references the fabric**; `infrastructure/event-fabric/streams.local.json` is local runtime configuration (`infrastructure/CLAUDE.md`). Binaries `qip-fabricd`, `qip-ledgerd`, `qip-edge-node` exist but are not catalogue workloads | C2, C8, ADR 0100 |
| Warm services | Regional GKE Standard, Config Sync, Argo CD, Kargo, Rollouts, Cloud Service Mesh (`GCP3 p9 s8`) | **No GKE Standard.** Catalogue: three Cloud Run workloads, `qip-api`, `qip-fastbrain`, `qip-deepbrain` (`catalogue.tf`), plus OpenObserve and the portal (`gitops/envs/dev/*.yaml`). One GKE **Autopilot** control-plane cluster for Config Connector, Argo CD, Kargo (`modules/gitops-control-plane`) behind `gitops_enabled`; **suspended** by `infra.yml suspend` (ADR 0093); bootstrap has never succeeded (Argo CD image fails the Trivy CRITICAL gate) | C3, ADR 0024, 0036, 0093 |
| Ledger | Spanner Enterprise Plus, multi-region (`GCP3 p10 s10`) | `modules/data` has Spanner (regional config) behind `enable_spanner = false` in every environment. Actual ledger is the in-tree `qip-ledgerd` single writer | C4, ADR 0100 |
| Knowledge stores | Spanner Graph, Bigtable, BigQuery, GCS, Knowledge Catalog, AlloyDB, Memorystore (`GCP3 p10 s9.2`) | `modules/data` declares Bigtable, AlloyDB, Memorystore, BigQuery, Cloud Storage, each behind an `enable_*` flag that is `false`; no Spanner Graph, no Knowledge Catalog resource found | C4 |
| AI and accelerators | Vertex, Agent Engine, GPU pools, TPU7x (`GCP3 p11-13 s11`) | `modules/ai`: Vertex metadata store and endpoint behind `enable_vertex_ai = false`. No GPU, TPU or Agent Engine resource (`docs/blueprint/v12-delta-gcp.md` baseline) | C5, C8 |
| Quantum | IBM Quantum via an isolated gateway (`GCP3 p22`) | No gateway service. Egress proxy listeners for IBM exist in `infrastructure/egress/envoy.yaml`; only the optimisation trust zone may reach them | ADR 0060 |
| Network | NCC hubs, separate Reflex/Fabric/Service VPCs, Interconnect (`GCP3 p4-5`) | `modules/network`, `modules/trust-zones` (thirteen zones, default deny; dev declares four: application-identity, cognition, intelligence, management), `modules/connectivity` (one interconnect attachment). No NCC | C8 |
| Edge and identity | Global ALB, Cloud Armor, CDN, Identity Platform (`GCP3 p14`) | Console reached at its Google-issued `run.app` URL behind Cloud Run IAP (ADR 0095); `modules/public-edge` creates nothing (`hostnames` empty everywhere); Cloud Armor quota is 0 in this project; Identity Platform enabled in dev | ADR 0094, 0095 |
| Observability | OTel + OpenObserve, Managed Prometheus (`v12 p40`; `GCP3 p17`) | OpenObserve v0.92.2 pinned by digest (`vendored-images.txt`, `dev/terraform.tfvars`); nine alert policies, all gated on `workload_metrics_exist = false`; metrics collector sidecar refused on a CVE | ADR 0026, 0028, 0033 |
| Supply chain | Cloud Build private pools, Artifact Registry, Binary Authorization, KMS-signed manifests (`GCP3 p16`) | GitHub Actions + WIF, Artifact Registry (`modules/registry`; destroyed by the 2026-09-13 teardown), Binary Authorization (`modules/binaryauthorization`), KMS; **no Cloud Build private pool** | |
| Environments | dev, integration, replay, paper, staging, prod (`GCP3 p27 s24`) | `dev`, `test`, `stage`, `prod` only; no integration, replay or paper environment. `prod` is refused by `infra.yml` and by the deploy gate | C1, C8 |
| Org and projects | Org policy, folders, ten-odd projects (`GCP3 p4 s4`) | One project per environment, named in tfvars; no folder or org-policy module found in `modules/` | |
| Cost control | Budgets, billing export (`GCP3 p26 s23`) | `infrastructure/terraform/modules/observability/budget.tf` declares an opt-in `google_billing_budget` (off by default, not applied); a hand-made 750 USD/month budget also exists on the project (DECISIONS.md), so only one of the two may be enabled. Mission ceiling is 25 USD/day (HERMES_MISSION.md s9); budgets only alert | HM-03 |
| Serving state | n/a | `qip-dev-*` Cloud Run services were observed 2026-09-04, torn down 2026-09-13, infrastructure re-applied 2026-09-20 with **no Cloud Run service existing** (`infrastructure/CLAUDE.md`); billing now disabled | ADR 0040, 0093 |

## 4. Tech stack, existence-verified

Every row names the file it was read from. "Not pinned" means no version could be read; it is not a claim of absence.
Tools named in the blueprints that were not found in the tree or on this machine are listed after the table and
are **not** adopted by this map (HERMES_MISSION.md s2.4).

| Layer | Item | Version / pin | Read from |
|---|---|---|---|
| Language | Rust toolchain | `1.94.1` with rustfmt, clippy, profile minimal | `backend/rust-toolchain.toml` |
| Language | Edition, resolver, MSRV | edition `2024`, resolver `3`, `rust-version = "1.89"` | `backend/Cargo.toml` |
| Workspace | Crates | 60 members listed in `[workspace] members` (16 libs, 8 edge, 24 services, 1 agents, 1 quant, 1 runtime, 1 tests, 8 apps) | `backend/Cargo.toml` |
| Lints | `unsafe_code = "forbid"`; clippy `todo`, `unimplemented`, `panic_in_result_fn` = deny; `float_cmp` warn | | `backend/Cargo.toml` `[workspace.lints]` |
| Lints | **No `unwrap_used` or `expect_used` lint exists** anywhere in `backend/` (search found zero), and `backend/clippy.toml` sets only `too-many-arguments-threshold = 12` | | so the CLAUDE.md rule "no unwrap() outside tests" has no automated gate (RP-18) |
| Dependencies | `serde`, `serde_json` (feature `float_roundtrip`) | requirement `1`; locked `serde 1.0.229`, `serde_json 1.0.151` | `backend/Cargo.toml`, `backend/Cargo.lock` |
| Dependencies | The other nine locked third-party packages (proc-macro and formatting closure) | `itoa 1.0.18`, `memchr 2.8.3`, `proc-macro2 1.0.107`, `quote 1.0.47`, `serde_core 1.0.229`, `serde_derive 1.0.229`, `syn 3.0.3`, `unicode-ident 1.0.24`, `zmij 1.0.23` | `backend/Cargo.lock` (11 third-party packages; the baseline script printed "11 ... all permitted") |
| Release profile | opt-level 3, thin LTO, 1 codegen unit, `panic = "abort"` | | `backend/Cargo.toml` |
| Policy | `deny.toml` repeats the 11-name allowlist and licence set for `cargo deny` | | `backend/deny.toml`, `.github/workflows/ci.yml` comments |
| Backend runtime | Blocking `std` I/O, no async runtime; in-tree HTTP/1.1 client and server, RESP, SHA-256/HMAC, RNG | | `CLAUDE.md`, ADR 0001, 0002, 0009, 0011, 0043, 0100 |
| Frontend | Next.js | `16.3.5` (portal and landing) | `frontend/portal/package.json`, `frontend/landing/package.json` |
| Frontend | React, react-dom | `19.2.8` | same |
| Frontend | chart.js `^4.5.1`; tailwindcss `^4`; TypeScript `^5`; eslint `^9`; `@types/node ^20`; `@playwright/test 1.56.1` (portal), `^1.56.1` (landing); swiper `^12.1.2` (landing) | as written | same |
| Frontend | Eleven workspace packages `@algorik/{analytics,api-client,auth,brand,charts,design-tokens,feature-flags,shared-types,testing,ui,validation}` | `*` | `frontend/packages/*/package.json` |
| Frontend | Node | CI uses `22`; image `node:22-alpine` pinned by sha256 | `.github/workflows/ci.yml`, `infrastructure/docker/{portal,landing}.Dockerfile` |
| Container | Backend build image `rust:1.94-alpine` pinned by sha256; final stage `FROM scratch` | | `infrastructure/docker/Dockerfile` |
| IaC | Terraform | `required_version = ">= 1.9.0"`; CI pinned 1.9.8 per `infrastructure/CLAUDE.md` | `infrastructure/terraform/main.tf` |
| IaC | Providers | `hashicorp/google ~> 6.12`, `hashicorp/google-beta ~> 6.12` | `infrastructure/terraform/main.tf` |
| Vendored images (digest-pinned, mirrored, attested) | Envoy `distroless-v1.38.3`; OpenObserve `v0.92.2`; Argo CD `v3.5.2`; Kargo `v1.11.4`; cert-manager `v1.21.1` (controller, webhook, cainjector); Config Connector operator `1.156.0`; Redis `8.2.9-alpine`; google-cloud-cli `585.0.0-slim` | | `infrastructure/egress/vendored-images.txt` |
| CI | Workflows `ci`, `deploy`, `image`, `infra`, `vendor`; ci jobs: format, clippy, test, release build, dependency policy, security audit (`cargo audit --deny warnings`), cargo-deny, sbom (`cargo cyclonedx`), portal, landing, vulnerability scan (Trivy CRITICAL,HIGH), trunk meta-linter, infrastructure, secrets | actions `checkout@v4`, `setup-node@v4`, `rust-cache@v2`, `trunk-action@v1`, `upload-artifact@v4`; `cargo-audit`, `cargo-deny`, `cargo-cyclonedx` installed with `--locked` and **no version pin** | `.github/workflows/ci.yml` |
| Local machine at baseline | graphify `0.9.74`; ponytail plugin `4.10.1`; superpowers plugin `6.4.2`; terraform `v1.15.8` (differs from the 1.9.8 pin); node `v22.23.3`; `gh` logged in; **Playwright not installed**; root disk 100% full | | `docs/ops/hermes-baseline-2026-10-04.md` (not re-measured) |

**Named in a blueprint and not found or not verifiable here** (each needs an ADR and an install-and-version check before use):
Tokio, QUIC, prost/Protobuf, BLAKE3, a Raft crate (`GCP3 p7`, `v12 p41-42`; refused by C2); Z3, OR-Tools
(`GCP3 p8, p11`; C5); JAX, PyTorch (`v12 p5`; C5); TPU7x Ironwood, Vertex AI Agent Engine, Memory Bank, Cloud Batch,
"Gemini Enterprise Agent Platform", Knowledge Catalog rename, IBM "quantum-centric supercomputing reference
architecture (March 2026)" (`docs/blueprint/v12-delta-gcp.md` "Products and claims that may not exist");
Config Sync, Argo Rollouts, Cloud Service Mesh, NCC, CAS, VPC Service Controls, PAM (no Terraform resource found);
a Qiskit Runtime client (CLAUDE.md names it; `libs/qip-quantum` has no external dependency, so the integration is
the in-tree `hosted.rs` provider over the egress proxy and its API version is not pinned).

## 5. Invariants, each with a named test and a CI gate

**Gate names are the `ci.yml` job names**: `test` = `cargo test --workspace --all-features --no-fail-fast`
(includes `qip-acceptance`), `clippy`, `format`, `dependency policy` (`scripts/check-dependencies.sh`),
`cargo-deny`, `security audit`, `secrets` (`scripts/check-secrets.sh`), `infrastructure` (terraform fmt, validate,
`terraform test`), `portal`, `landing`, `vulnerability scan`. A test that needs a deployed environment is
gated `staging E2E (job to be created, Phase 3)`. Test locations are by file; function names are exact as found.
A name that does not exist yet is prefixed `PROPOSED`. A **P** row names what exists and what is missing.

Totals for this section (counted by reading each table, not by a script): **176 invariants: 59 Y, 61 P, 56 N.**
By block: V12 79 (17 Y, 33 P, 29 N); failure model 20 (5, 4, 11); GCP3 24 (5, 11, 8); repo 49 (32, 11, 6); mission 4 (0, 2, 2).

### 5.1 From Master Blueprint v12.0 (V12-NN), in page order within each group

Authority, conduct and agency

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-01 | Money movement, leverage, exposure, settlement and venue access stay bounded by deterministic, testable controls (`p4 s1.3`) | `a_proposal_cannot_reach_execution_without_both_controls` (`qip-acceptance/tests/acceptance.rs`) | test | Y |
| V12-02 | The Risk Brain forecasts; the Risk Gate enforces; every executable action passes deterministic limits on locally available state (`p27 s14`) | `a_decision_that_must_be_deterministic_cannot_be_routed_to_any_model_tier` (`qip-cost-router/tests/cost_router.rs`); `the_expected_shortfall_limit_can_actually_fire` (unit, `qip-kernel/src/platform.rs`) | test | Y |
| V12-03 | Ambient models' continuous operation does not increase their authority; outputs advisory until promoted (`p4`, `p18 s9.5`) | PROPOSED `an_ambient_output_cannot_reach_an_order_or_an_action_without_a_promotion_record` | test | N |
| V12-04 | No fabricated identities, sockpuppets, fake consensus, undisclosed promotion, wash trading, spoofing, rumor engineering, pump and dump; a plan that depends on deception is infeasible (`p4`, `p38 s24.5`) | PROPOSED `a_plan_that_needs_a_deceptive_method_is_refused_as_infeasible_before_simulation` | test | N |
| V12-05 | Truthfulness, provenance, identity, disclosure, conduct, channel permission and jurisdiction are checked before any external action (`p4`) | PROPOSED `an_outbound_message_without_a_conduct_gate_pass_is_never_emitted` (shadow only; `RM-P10-04`) | test | N |
| V12-06 | Wanting an outcome never implies permission to use every causal lever (`p37 s24.1`) | PROPOSED `a_goal_cannot_use_a_lever_its_authority_class_does_not_name` | test | N |
| V12-07 | When causal identification is weak the engine abstains or proposes an experiment, never pretends correlation is control (`p37 s24.2`) | exists in part: `an_unobserved_confounder_cannot_reach_a_regression_across_the_crate_boundary` (`cognition.rs`); PROPOSED `an_intervention_with_unidentified_effect_is_abstained_not_ranked` | test | P |
| V12-08 | The planner always carries a no-action baseline (`p37 s24.3`) | PROPOSED `every_candidate_plan_set_contains_the_no_action_baseline` | test | N |
| V12-09 | Algorik acts only through registered tools with identity, permission and audit (`p37 s24.4`) | exists in part: `an_agent_that_holds_a_language_model_cannot_touch_the_market` (`architecture.rs`); no tool registry | test | P |
| V12-10 | Correlation is never upgraded to causal certainty without evidence (`p38 s24.6`) | exists in part: `qip-kernel/tests/causal_precedence.rs` (ADR 0054, 0071), test names not read | test | P |
| V12-11 | An ambient signal reaches the planner only through the Attention Router and "does not directly cause action" (`p38 s24.7`, `p50`) | PROPOSED `an_ambient_signal_reaches_the_planner_only_through_the_attention_router` | test | N |
| V12-12 | Eligibility gate before strategy deployment and before every new venue or instrument becomes executable (`p39 s25`) | exists in part: `an_expired_eligibility_refuses_funding_from_the_instant_it_expires` (`qip-kernel/tests/ledger.rs`); venue registration refused (`registrations.rs`) | test | P |
| V12-13 | Separate identities and credentials for trading, custody/transfer, market creation, banking, commerce (`p39 s25`) | exists in part: `every_deployable_has_its_own_cloud_run_identity`, `the_venue_credential_is_bound_to_the_fast_brain_and_only_where_the_ceiling_permits` (`infrastructure.rs`); only trading exists | infrastructure, test | P |
| V12-14 | Human-signed or governance-signed authority envelopes (`p39 s25`) | `a_capital_envelope_granted_to_another_cell_is_refused_when_replayed`, `an_expired_envelope_cannot_be_replayed_after_the_window_closes` (`security.rs`) | test | Y |
| V12-15 | Immutable audit of model, data, policy, code and control versions per decision (`p39 s25`) | exists in part: `an_artifact_whose_inputs_lead_nowhere_is_not_fully_traced` (`security.rs`) | test | P |
| V12-16 | Research and simulation may be broader than executable scope; unsupported opportunities stay shadow-only (`p39 s25`) | exists in part: `a_venue_promoted_to_the_simulator_still_cannot_receive_an_order_at_a_live_class_broker` (`decentralised_venue.rs`) | test | P |
| V12-17 | Goal approval, plan approval and action execution are separately auditable (`p39 s25`) | PROPOSED `goal_plan_and_action_approvals_are_three_distinct_journal_records` | test | N |
| V12-18 | Public communications cannot be justified solely by expected profit or desired price impact (`p39 s25`) | PROPOSED `a_communication_whose_only_justification_is_price_impact_is_refused` | test | N |

Knowledge, evidence and data correctness

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-19 | Algorik must not become a warehouse of the world's raw external content; the payload is discarded after extraction (`p3-4 s1.3`; `GCP3 p2 s2.1`) | exists in part: `what_registration_keeps_of_a_sample_is_a_manifest_whose_hash_is_of_the_bytes_served` (`qip-data-finder/tests/registration_is_not_ingestion.rs`); PROPOSED `no_durable_store_port_accepts_a_raw_external_document_body` | test | P |
| V12-20 | Persist source reference, provider, retrieval time, etag, hash, entitlement, geography, parser lineage, freshness (`p3 s1.3`) | same file as V12-19 covers manifest and hash only | test | P |
| V12-21 | If the original cannot be re-fetched, keep derived knowledge and record that evidence is no longer re-fetchable (`p4`) | PROPOSED `a_belief_whose_source_can_no_longer_be_fetched_is_kept_and_flagged_unrefetchable` | test | N |
| V12-22 | Tick/order-book and Algorik's own records are separate retention classes (`p4`) | exists in part: `p1_and_p2_are_archive_required_and_p4_is_the_only_sheddable_class` (`event_fabric_catalogue.rs`); ADR 0089 classes in `qip-events/src/retention.rs` | test | P |
| V12-23 | The NOW Brain persists compact state and deltas, not raw payloads (`p19 s9.6`) | PROPOSED `the_state_estimator_persists_no_raw_payload` (subsystem not built) | test | N |
| V12-24 | Every claim is an Evidence object with provenance; scraped text is never converted directly into fact (`p13 s7`) | `a_record_that_fails_its_quality_gate_is_refused_by_name_and_never_reaches_the_world_model` (`truth_loop.rs`); `every_number_the_organisation_produces_carries_a_provenance` (`acceptance.rs`) | test | Y |
| V12-25 | Corroboration is weighted by common-source dependence: ten copies of one press release are not ten sources (`p13 s7`) | PROPOSED `ten_copies_of_one_release_corroborate_as_one_source` | test | N |
| V12-26 | The contradiction engine keeps competing hypotheses instead of deleting conflicts (`p13 s7`) | PROPOSED `a_contradicted_claim_is_retained_with_both_sides_and_the_resolving_evidence`; modules `hypothesis.rs`, `redteam.rs` exist, no named test read | test | N |
| V12-27 | Adversarial source detection: spam, coordinated manipulation, poisoned feeds (`p13 s7`) | PROPOSED `a_source_with_coordinated_duplicate_content_is_quarantined` | test | N |
| V12-28 | No look-ahead: training and replay see only what was observable by the decision timestamp (`p4`, `p15 s8.5`) | `nothing_the_loop_wrote_is_visible_before_the_moment_it_became_known` (`truth_loop.rs`); `a_reader_never_holds_a_fact_that_was_not_yet_knowable` (`qip-compliance/tests/point_in_time.rs`) | test | Y |
| V12-29 | No impossible fills in replay: depth, queue, rate limits, latency, partial fills, impact (`p15 s8.5`) | exists in part: `an_arbitrage_leg_the_book_cannot_fill_is_refused_rather_than_part_executed` (`stress.rs`); `qip-simulation-engine/tests/market_conditions.rs` | test | P |
| V12-30 | No timestamp laundering: source time, receive time, normalised time and uncertainty stay distinct (`p15 s8.5`) | exists in part: `both_timestamps_survive_every_stage_of_the_loop` (`truth_loop.rs`) covers two instants (valid, known), not four | test | P |
| V12-31 | No hidden survivorship: delisted, renamed, failed instruments stay in historical universes (`p15 s8.5`) | PROPOSED `a_delisted_instrument_stays_in_every_historical_universe_it_belonged_to` | test | N |
| V12-32 | Entitlements are part of lineage; artifacts record which licensed data trained them and where they may run (`p15 s8.5`) | exists in part: `a_dataset_licensed_for_research_cannot_be_spent_on_a_trade` (`truth_loop.rs`) | test | P |
| V12-33 | Raw tick volume is not intelligence; models must prove incremental value against simpler baselines (`p15 s8.5`) | exists in part: `an_established_precedent_that_does_not_beat_the_baseline_keeps_the_baseline` (unit, `qip-deepbrain/src/learning.rs`) | test | P |
| V12-34 | Lane 0 has no remote dependency and never reads the historical Tick Lake (`p9 s4`) | exists in part: `a_cell_cut_off_from_the_centre_spends_only_its_grant_and_then_stops`; `qip-edge/Cargo.toml` depends on no storage crate; PROPOSED `qip_edge_reaches_no_store_lake_or_model_client` (architecture suite) | test | P |
| V12-35 | Drift feeds retraining and curriculum, not silent adaptation of money-moving logic (`p14 s8.2`) | exists in part: `a_feature_that_drifts_past_its_bound_degrades_the_models_that_read_it_and_no_others` (`streaming_estimators.rs`) | test | P |
| V12-36 | A sequence gap or corrupt book marks the interval unreliable and keeps contaminated windows out of training and replay (`p14 s8.2`, `p44 s28`) | exists in part: `a_corrupt_sequence_invalidates_the_book_rather_than_being_absorbed` (`stress.rs`); training exclusion untested | test | P |

Fabric and hot path

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-37 | Google Pub/Sub and Kafka are not core internal dependencies (`p3 s1.1`, `p40`, `p50`; `GCP3 p2`, `p23 s19.1`, `p28 s27`) | exists in part: `qip-streaming/src/pubsub.rs` returns `Unavailable`; the two-dependency check makes a client impossible; PROPOSED `no_internal_stream_binds_to_a_pubsub_or_kafka_transport` | test, dependency policy | P |
| V12-38 | A fabric outage must not stall a live local decision; decision, inference, risk veto and venue send never wait on a broker acknowledgement (`p29 s17`, `p42`) | `ship_returns_without_waiting_when_the_writer_is_stalled_and_the_entries_stay_unshipped` (`qip-edge-node/tests/event_fabric_outbox.rs`); `journal_pressure_narrows_sizing_before_it_halts_new_exposure` (`qip-edge/tests/journal_pressure.rs`). ADR 0100 s8 test 3 (real processes, "a stalled fabric, no change in pass latency") not found | test | P |
| V12-39 | Strict per-partition order; no promise of a total global order (`p42`) | exists in part: `a_same_epoch_retry_with_a_different_payload_is_a_sequence_conflict_not_a_duplicate` (`qip-streaming/tests/event_fabric_producer.rs`) | test | P |
| V12-40 | At-most-once only for telemetry; at-least-once default with idempotent producer sequencing (`p42`) | `a_superseded_epoch_is_fenced_after_its_successors_first_append` (`event_fabric_producer.rs`); `a_duplicate_is_still_suppressed_across_separate_batches` (`qip-streaming/tests/processing.rs`) | test | Y |
| V12-41 | Critical control and outcomes are never silently dropped; only telemetry and low-value ambient signals are shed (`p42`) | `p1_and_p2_are_archive_required_and_p4_is_the_only_sheddable_class` (`event_fabric_catalogue.rs`) | test | Y |
| V12-42 | A regional cluster never blocks on a remote acknowledgement (`p42`) | PROPOSED `a_regional_append_is_acknowledged_with_the_mirror_link_cut` (mirror BLOCKED, C2/C8) | staging E2E | N |
| V12-43 | Model and policy packages are immutable and signed; a node keeps its last valid package until TTL (`p42`) | `with_nothing_delivered_the_cell_runs_on_its_last_valid_envelope_until_it_expires` (`qip-edge-node/tests/control.rs`) covers TTL; signed model packages not built | test | P |
| V12-44 | Every durable stream declares partitioning, ordering, retention, replication, overload and mirroring (`p50`) | `the_committed_catalogue_validates_and_declares_every_field_of_every_stream` (`event_fabric_catalogue.rs`) | test | Y |
| V12-45 | Reflex Mesh traffic stays separate from durable fabric traffic (`p29 s17`, `p50`) | PROPOSED `a_peer_mesh_outage_leaves_journaling_and_control_distribution_untouched` (peer mesh not built) | test | N |
| V12-46 | A cell decides alone; no synchronous global dependency on the hot path; it spends only its grant (`p5 s2`, `p28 s16`) | `a_cell_cut_off_from_the_centre_spends_only_its_grant_and_then_stops` (`stress.rs`); `no_edge_cell_can_issue_its_own_capital_or_promote_its_own_strategy` (`architecture.rs`) | test | Y |

Quantum, models and promotion

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-47 | QPU jobs are asynchronous and never on the market hot path; quantum cannot reach anything that vetoes, executes or moves money (`p10 s4`, `p23 s11`, `p25 s11.5`) | `nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver`, `no_edge_cell_can_reach_a_quantum_solver`, `a_quantum_solver_cannot_reach_anything_that_vetoes_executes_or_moves_money` (`architecture.rs`) | test | Y |
| V12-48 | Every quantum workload has a classical baseline, a common objective and a promotion threshold (`p23 s11`; ADR 0006) | `the_classical_baseline_is_always_computed` (`qip-optimization-engine/tests/optimization.rs`); `a_benchmark_whose_classical_baseline_fails_reports_nothing_at_all` (`qip-quantum/tests/solvers.rs`) | test | Y |
| V12-49 | If quantum loses, the classical result ships (`p24`) | `a_quantum_answer_that_ties_loses_to_the_classical_baseline`, `a_failed_quantum_attempt_leaves_the_classical_answer_standing` (`optimization.rs`) | test | Y |
| V12-50 | Returned quantum candidates are verified classically (`p25 s11.2`) | `a_solution_that_fails_classical_validation_is_refused_however_good_the_claim` (`qip-quantum/tests/solvers.rs`) | test | Y |
| V12-51 | Quantum outputs are ensemble features, never direct authority, and confer no authenticity or truth (`p13`, `p25 s11.4`) | exists in part: `a_hardware_result_is_still_a_candidate_that_needs_a_classical_baseline` (`qip-quantum/tests/hosted.rs`); ensemble layer not built | test | P |
| V12-52 | Training: classical baseline trained and calibrated first; models validated out of time and in simulation; signed, versioned, attested (`p23 s10.1`) | exists in part: `training_that_misses_its_bar_promotes_nothing_and_names_the_shortfall` (`stress.rs`); `a_tampered_artifact_fails_its_provenance_check` (`security.rs`) | test | P |
| V12-53 | Promotion is never direct from research into live money movement; production control plane is not mutated in place (`p35 s23.1`, `p36 s23.4`) | exists in part: `no_code_path_assigns_a_limit_set_after_boot_and_no_root_reads_one_from_anywhere_but_its_configuration` (`security.rs`); `every_generated_strategy_carries_an_expiry` (`qip-kernel/tests/foundry.rs`) | test | P |
| V12-54 | A new model must beat a named baseline on a declared benchmark and stay calibrated under stress (`p36 s23.4`) | exists in part: `performance_decay_against_the_pilot_baseline_demotes_without_a_human` (`qip-lifecycle/tests/lifecycle.rs`) | test | P |
| V12-55 | New tools default to read-only sandbox; new specialists declare competence and abstain outside it (`p36 s23.4`) | PROPOSED `a_created_tool_cannot_open_a_socket_or_read_the_environment` (`RM-P9-03`) | test | N |
| V12-56 | No frontier-model call from the order path (`p20 s9.13`; `GCP3 p11` model-package rule) | `no_edge_cell_can_reach_a_language_model`, `no_safety_critical_engine_can_reach_a_language_model`, `nothing_that_decides_or_executes_names_the_language_model_interface` (`architecture.rs`) | test | Y |
| V12-57 | A conclusion gains authority only by surviving independent challenge (`p20 s9.11`) | PROPOSED `a_leading_forecast_without_a_recorded_independent_challenge_carries_no_authority` | test | N |
| V12-58 | World models may disagree and disagreement is preserved (`p17 s9.3`, `p45 s28`) | PROPOSED `two_world_models_that_disagree_on_one_entity_are_both_kept_and_flagged` (`RM-P2-10`) | test | N |

Capital, risk, markets and ledger

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-59 | Every strategy receives an expiring capital grant (`p27 s13.2`) | `an_issued_envelope_is_sized_inside_the_allocators_budget_and_expires` (`qip-kernel/tests/central.rs`) | test | Y |
| V12-60 | Capital is withheld for hedge capacity and stressed liquidity before opportunity capital is released (`p27 s13.2`) | PROPOSED `hedge_and_stress_reserve_is_withheld_before_opportunity_capital_is_released` | test | N |
| V12-61 | Grants and size contract automatically on calibration decay, rising disagreement, stale evidence, OOD; the path can only restrict (`p28 s14.2`) | exists in part: `a_belief_state_stale_beyond_its_ttl_falls_back_to_a_fixed_multiplier_and_halts_nothing` (`qip-contracts/tests/contracts.rs`); `the_regime_reader_narrows_a_bound_in_every_regime_and_widens_one_in_none` (`regime_allocation.rs`) | test | P |
| V12-62 | Capital and hedge systems demonstrate survivability across the eight listed shock families (`p28 s14.3`) | exists in part: `the_standard_scenario_library_still_shocks_the_factor_the_risk_crate_names` (`architecture.rs`) | test | P |
| V12-63 | Capital, hedge, model-risk and survival systems are independent of alpha and can veto or reduce exposure, not enlarge it (`p50 s31.1`) | PROPOSED `no_hedge_or_survival_output_can_increase_an_exposure_limit` | test | N |
| V12-64 | Regulated functions (deposit-taking, lending, brokerage, dealing) only through an appropriate legal entity (`p26 s13`) | PROPOSED `no_regulated_function_adapter_can_be_constructed_without_an_entity_binding`; real forms BLOCKED (ADR 0021) | test | N |
| V12-65 | Market creation is a governed product workflow, not an unrestricted model action (`p30 s18.2`) | `an_origination_mandate_cannot_be_decoded_into_existence` (`quote_loop.rs`); ADR 0067 | test | Y |
| V12-66 | Wagering is isolated by eligibility and jurisdiction (`p30 s19`) | PROPOSED `a_wager_for_an_ineligible_jurisdiction_is_refused_before_pricing` | test | N |
| V12-67 | The ledger is append-only, event-sourced, double-entry, authoritative (`p28 s15`) | exists in part: `a_fill_not_marked_simulated_has_no_representation_in_the_ledger` (`qip-portfolio/tests/ledger_postings.rs`); `qip-ledgerd/src/telemetry.rs` counts `unbalanced_refused`, no test name read | test | P |
| V12-68 | Idempotent event IDs; immutable lineage intent to order to fill to settlement to position (`p28 s15`; `GCP3 p27 s27`) | exists in part: `qip-ledgerd/tests/store.rs`; ADR 0100 s8 test 4 ("duplicate delivery, posted once") not found by name; PROPOSED `a_fill_the_venue_reported_posts_exactly_one_ledger_entry` | test | P |
| V12-69 | Every model, strategy, capital and risk decision stores its version identifiers (`p28 s15`) | exists in part: `a_cycle_that_sizes_a_proposal_journals_the_solver_and_the_classical_baseline_it_was_measured_against` (`qip-kernel/tests/solver_routing.rs`) | test | P |
| V12-70 | Ledger degraded: new money-risking actions halt; safe cancels and flattening continue (`p45 s28`) | exists in part: `an_exhausted_journal_halts_new_exposure_while_withdrawals_and_fill_confirmations_continue` (`journal_pressure.rs`), journal not ledger | test | P |

Operations, delivery and structure

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-71 | Autonomous remediation only inside pre-approved runbooks (`p46 s29`) | PROPOSED `an_automated_remediation_runs_only_if_a_signed_runbook_names_it` | test | N |
| V12-72 | Every deploy runs simulation, integration, security, performance and paper-trading gates (`p46 s29`) | exists in part: `nothing_deploys_that_has_not_passed_the_test_suite` (`infrastructure.rs`); no performance or paper gate in `deploy.yml` found | infrastructure | P |
| V12-73 | Causal-policy drift shrinks autonomy automatically (`p46 s29`) | PROPOSED `observed_uplift_below_prediction_shrinks_an_action_policys_authority` | test | N |
| V12-74 | No browser access to keys, ledger internals or execution nodes (`p40 s26`) | exists in part: `no_signing_or_withdrawal_path_appears_in_the_venue_tooling_or_the_portal` (`security.rs`); `api_boundary.rs` | test | P |
| V12-75 | Isolated resource budgets stop background cognition starving trading paths; no direct path from ambient model to external action (`p40-41`) | PROPOSED `ambient_work_over_its_budget_is_shed_before_a_reflex_or_ledger_resource_is_touched` | test | N |
| V12-76 | Calibration is a production SLO (`p20 s9.14`; "should") | PROPOSED `every_forecast_family_has_a_calibration_objective_with_a_named_target` (`slo.rs` covers v10.1 s49.1 only) | test | N |
| V12-77 | Evidence acquisition stops when marginal information value falls below cost or deadline (`p20 s9.10`) | PROPOSED `an_information_request_stops_when_marginal_value_drops_below_cost` | test | N |
| V12-78 | Redundant or unproductive work is killed so scale follows value (`p20 s9.12`) | PROPOSED `a_model_population_with_no_marginal_value_is_retired` | test | N |
| V12-79 | The hot lane is Rust-first (`p21 s10`) | `every_source_file_in_the_backend_workspace_is_written_in_rust` (`blueprint_rules.rs`) | test | Y |

### 5.2 v12 s28 failure and degradation model (V12-Fnn)

Each row of `v12 p44-46 s28` is a "Required behavior". "Ledger degraded" is V12-70. Twenty rows.

| ID | Failure and required behaviour (cite) | Named test | Gate | St |
|---|---|---|---|---|
| V12-F01 | Global cognition unavailable: nodes continue on last valid packs until expiry, size narrows (`p45`) | `with_nothing_delivered_the_cell_runs_on_its_last_valid_envelope_until_it_expires` (`qip-edge-node/tests/control.rs`) | test | Y |
| V12-F02 | Peer reflex mesh degraded: local trading continues, distributed cycles disabled or reduced to pre-positioned bands | PROPOSED `a_lost_peer_disables_cross_region_cycles_and_local_trading_continues` | test | N |
| V12-F03 | Quantum unavailable: classical path runs, no execution interruption | `quantum_being_unavailable_produces_a_classical_answer_that_says_why` (`stress.rs`) | test | Y |
| V12-F04 | Data source poisoned or stale: evidence quarantined, dependents downweighted or invalidated | exists in part: `stale_data_is_refused_as_a_price_rather_than_used_as_one` (`stress.rs`), `text_from_a_hostile_page_cannot_become_a_number_a_calculation_depends_on` (`security.rs`) | test | P |
| V12-F05 | Custody or treasury unavailable: trading constrained to available settled capital | exists in part: `cash_below_its_buffer_refuses_the_order_instead_of_borrowing_silently` (`stress.rs`) | test | P |
| V12-F06 | Venue failure: quarantine, cancel, reroute or hedge, reconcile later | `a_halted_or_unreachable_venue_never_receives_an_order` (`qip-routing/tests/routing.rs`); `a_broker_outage_mid_arbitrage_stops_the_plan_and_names_the_exposure_it_stranded` (`stress.rs`) | test | Y |
| V12-F07 | Model drift: fall back to challenger or baseline, shrink capital | `performance_decay_against_the_pilot_baseline_demotes_without_a_human` (`qip-lifecycle/tests/lifecycle.rs`) | test | Y |
| V12-F08 | Tick sequence gap or corrupt book: mark unreliable, recover, keep contaminated windows out of training | exists in part: `a_corrupt_sequence_invalidates_the_book_rather_than_being_absorbed` (`stress.rs`) | test | P |
| V12-F09 | Clock sync degraded: widen uncertainty, disable precision-sensitive strategies, block labels from promotion | PROPOSED `a_degraded_clock_widens_uncertainty_and_blocks_precision_sensitive_labels` | test | N |
| V12-F10 | Replay diverges from live execution: reduce confidence and capital, recalibrate the twin, require renewed promotion | exists in part: `a_venue_the_twin_says_is_filling_the_desk_badly_is_measured_and_put_on_the_record` (`adversary.rs`). **Tension:** `qip_venue_fill_error_bps` is "diagnostic and read by nothing that decides" (`.claude/rules/domains/observability.md`, ADR 0070); v12 requires it to reduce capital (T6) | test | P |
| V12-F11 | World models diverge: preserve branches, raise uncertainty, reduce authority | PROPOSED `diverging_world_models_keep_their_branches_and_narrow_authority` | test | N |
| V12-F12 | Symbolic contradiction: mark infeasible, retain trace, fail closed for money-moving invariants | PROPOSED `an_unsatisfied_constraint_fails_a_money_moving_action_closed_and_keeps_the_trace` | test | N |
| V12-F13 | Ambient storm: rate-limit attention, cluster duplicates, quarantine drifting models | PROPOSED `an_ambient_alert_storm_is_rate_limited_and_cannot_starve_the_cell` | test | N |
| V12-F14 | Causal effect not identifiable: do not claim causation, downgrade, seek discriminating evidence | PROPOSED `a_non_identifiable_effect_is_never_recorded_as_caused` | test | N |
| V12-F15 | Action or channel identity compromised: revoke, stop pending actions, never substitute an identity | PROPOSED `a_compromised_adapter_is_quarantined_and_no_substitute_identity_is_used` | test | N |
| V12-F16 | Conduct gate fails: block the action, retain plan and reasons | PROPOSED `a_failed_conduct_gate_blocks_the_action_and_keeps_the_plan` | test | N |
| V12-F17 | Intervention causes adverse side effect: stop, mitigate, reduce policy authority, require re-evaluation | PROPOSED `an_adverse_side_effect_stops_the_plan_and_withdraws_the_policys_authority` | test | N |
| V12-F18 | Fabric broker loss: leadership moves to an in-sync replica in another zone | PROPOSED `a_broker_killed_mid_stream_loses_no_acknowledged_record` (**not met by this build**: RF1, ADR 0100 s3; FABRIC-077/086 scored BLOCKED(C2)) | test | N |
| V12-F19 | Fabric quorum lost: nodes continue on cached signed policy and local journals; control changes stop; publication queues locally within a bounded disk budget; degradation precedes journal exhaustion | `journal_pressure_narrows_sizing_before_it_halts_new_exposure`, `an_exhausted_journal_halts_new_exposure_while_withdrawals_and_fill_confirmations_continue` (`journal_pressure.rs`); `spool_pressure_reads_narrow_before_exhausted_and_a_fenced_producer_reads_exhausted` (`qip-edge-node/tests/event_fabric_pressure.rs`) | test | Y |
| V12-F20 | Mirror down: local streams stay writable, lag bounded and visible | PROPOSED `a_dead_mirror_leaves_local_streams_writable_and_lag_visible` (BLOCKED, C8) | test | N |

### 5.3 From GCP Blueprint v3.0 (G3-NN)

Items that restate a v12 invariant are folded into that row (named in the cell), not counted twice:
rule 1 and `s27` bullet 1 = V12-38/46; Pub/Sub absent = V12-37; no frontier model on the order path = V12-56;
quantum selection = V12-48/49; capital veto = V12-63; "no agent/TPU/QPU is a synchronous dependency" = V12-46/47.

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| G3-01 | One internal event model: `event_id`, `producer_epoch`, `schema_id/version`, region, logical and source timestamps, `trace_id`, ordering key, checksum, provenance (`p3 rule 3`) | exists in part: `every_bound_body_has_the_shape_its_lock_row_records` (`event_fabric_schema_lock.rs`); presence of `trace_id` not verified | test | P |
| G3-02 | Regions fail independently (`p3 rule 5`, `p28 s27`) | `a_region_that_goes_completely_offline_is_halted_by_scope_and_the_others_keep_trading` (`stress.rs`) | test | Y |
| G3-03 | State ownership is explicit; every workload has a documented state owner, protocols, scaling model, degraded mode (`p3 rule 7`, `p28 s27`) | PROPOSED `every_catalogue_workload_names_its_state_owner_protocols_and_degraded_mode` | infrastructure | N |
| G3-04 | Git and signed digests are deployment truth: no mutable tags, no console-only change, no hand-copied models, no secret values in Git (`p3 rule 8`) | `every_image_under_gitops_is_pinned_by_digest_and_is_either_attested_by_the_pipeline_or_vendored` (`gitops.rs`); `no_secret_value_appears_in_any_committed_configuration` (`security.rs`) | test, secrets | Y |
| G3-05 | Autonomous agents obey the same pipeline; Planner cannot merge or deploy; Coder has no prod secrets; Test agent cannot waive failing gates; Release has no kubectl or SSH to prod (`p3 rule 9`, `p14-15 s13`) | exists in part: `.claude/hooks/guard-dangerous-command` and its test runner (`docs/architecture/current-state.md` delivery); not a `qip-acceptance` test | test | P |
| G3-06 | Never two active owners of one venue session; standby is fenced (`p6 s6`) | PROPOSED `a_standby_cannot_activate_without_a_fencing_epoch_and_a_venue_session_ownership_check` | test | N |
| G3-07 | Packages download outside the hot loop, verify manifest and signature, stage, activate atomically at an epoch boundary, keep rollback (`p6 s6`) | exists in part: `a_verified_value_handed_across_is_applied_exactly_once_at_a_pass_boundary` (`qip-edge-node/tests/control.rs`) | test | P |
| G3-08 | Incompatible schemas never reach production topics (`p8 s7.1`) | `the_lock_has_exactly_one_row_per_bound_body_and_version` (`event_fabric_schema_lock.rs`) | test | Y |
| G3-09 | Fabric and Reflex traffic never passes through mesh sidecars or generic proxies (`p9 s8`, `p29 s28`) | exists in part: `neither_the_fast_path_nor_an_execution_node_can_reach_a_proxy_that_is_not_its_own` (`egress.rs`) | test | P |
| G3-10 | Separate databases for ledger and world graph; Redis is never position or capital truth (`p10`, `p29 s28`) | PROPOSED `no_store_port_a_risk_gate_reads_is_backed_by_a_cache` | test | N |
| G3-11 | The public API never exposes Spanner tables, Fabric broker ports, Reflex nodes or custody keys (`p14 s12`) | exists in part: `the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate` (`api_boundary.rs`); note `qip-api/Cargo.toml` lists `qip-edge` (U19) | test | P |
| G3-12 | Untrusted web content cannot invoke high-authority tools or production secrets (`p14 s12`) | `an_agent_that_has_read_a_hostile_page_still_cannot_reach_a_model_or_the_market` (`security.rs`) | test | Y |
| G3-13 | Promote the same digest; rebuild per environment is prohibited (`p16 s14.2`, `p27 s24`) | `the_kargo_stages_chain_dev_to_test_to_stage_to_prod_from_one_warehouse` (`gitops.rs`) | test | Y |
| G3-14 | Key separation: custody, deployment-signing, fabric identity, user-auth keys are separate trust domains (`p17 s15`) | exists in part: `every_service_account_terraform_creates_runs_something_or_signs_something` (`infrastructure.rs`) | infrastructure | P |
| G3-15 | No logging call may block order processing; observability never blocks execution (`p17 s16`, `p29 s28`) | PROPOSED `a_blocked_metrics_sink_does_not_delay_a_pass` | test | N |
| G3-16 | Cost anomalies never auto-delete production state (`p27 s23`) | exists in part: `the_teardown_stops_the_meter_and_touches_nothing_that_scales_to_zero` (`infrastructure.rs`) | infrastructure | P |
| G3-17 | Every model or policy executing in Reflex traces to source commit, data and evaluation manifest, signature, approval event and activation epoch (`p28 s27`) | PROPOSED `a_running_package_resolves_to_commit_manifest_signature_approval_and_epoch` (`ModelPack` not built) | test | N |
| G3-18 | Workloads are admitted only when supply-chain policy is satisfied (`p28 s27`; Cloud Run form) | `every_cloud_run_service_this_repository_deploys_is_subject_to_the_admission_policy` (`infrastructure.rs`) | infrastructure | P |
| G3-19 | Agents cannot obtain production capital or custody authority or long-lived cloud keys (`p28 s27`) | exists in part: `no_service_account_key_exists_anywhere_in_the_terraform`, `the_pipeline_authenticates_without_a_long_lived_key` (`infrastructure.rs`) | infrastructure | P |
| G3-20 | Replay reconstructs market, decision and outcome history from fabric and archive (`p29 s27`) | exists in part: `every_registry_the_platform_acts_on_rebuilds_from_the_event_log_alone_and_one_flipped_byte_refuses_the_replay` (`truth_loop.rs`); `a_clean_journal_replays_into_three_identical_registries_and_the_run_exits_zero` (`qip-cli/tests/replay.rs`) | test | P |
| G3-21 | Reconciliation rebuilds positions and cash from the ledger plus venue and custody statements without trusting caches (`p29 s27`) | exists in part: `the_learn_stage_halts_a_statement_beyond_its_floor_and_not_one_inside_it` (`qip-kernel/tests/ledger.rs`) | test | P |
| G3-22 | Every critical component is exercised by scheduled game days with observable RTO/RPO (`p29 s27`, `p27 s25`) | PROPOSED `the_quarterly_game_day_workflow_exists_and_names_every_critical_component` (no scheduled workflow exists) | staging E2E | N |
| G3-23 | Digital-twin and synthetic-world jobs can fail or backlog without affecting live trading (`p29 s27.1`) | PROPOSED `a_stalled_twin_job_does_not_change_a_pass_outcome` | test | N |
| G3-24 | Forecast-to-capital steps keep live execution local and deterministic (`p26 s21.5` step 7) | PROPOSED `a_forecast_change_reaches_a_cell_only_as_a_signed_envelope_never_as_a_call` | test | N |

### 5.4 The repository's own invariants (RP-NN)

Sources: `CLAUDE.md`, `.claude/rules/**`, the ADRs they cite.

| ID | Invariant (cite) | Named test | Gate | St |
|---|---|---|---|---|
| RP-01 | Paper layer 1: Terraform refuses `supervised_live`, `limited_autonomous_live`, `autonomous_live` at plan time and plans the paper rungs (`01-security-and-safety.md`) | `no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue`, `the_autonomy_ceiling_variable_accepts_only_the_declared_levels` (`infrastructure.rs`); `infrastructure/terraform/tests/paper-boundary.tftest.hcl` per `infrastructure/CLAUDE.md` | infrastructure | Y |
| RP-02 | Paper layer 2: composition roots refuse a live ceiling at start-up, never lower it | `a_configured_live_ceiling_stops_the_deployment_instead_of_being_lowered_to_paper`, `every_binary_that_reads_the_configured_ceiling_routes_it_through_the_refusal` (`paper_boundary.rs`) | test | Y |
| RP-03 | Paper layer 3: `Cell` has no constructor taking a ceiling other than paper; the `Required` determinism arm returns a type that cannot name a model rung | `a_cell_is_assembled_with_a_paper_ceiling_and_no_way_to_raise_it` (`qip-edge/tests/cell.rs`); `a_decision_that_must_be_deterministic_cannot_be_routed_to_any_model_tier` (`cost_router.rs`); `the_three_layers_the_safety_rules_name_each_still_have_a_test` (`paper_boundary.rs`) | test | Y |
| RP-04 | Fourth fence: the ledger refuses any fill not marked simulated (ADR 0100 s9) | `a_fill_not_marked_simulated_parks_that_cell_counts_it_and_posts_nothing` (`qip-ledgerd/tests/store.rs`); `a_fill_not_marked_simulated_has_no_representation_in_the_ledger` (`qip-portfolio/tests/ledger_postings.rs`) | test | Y |
| RP-05 | A cell or broker handed a live-class gateway places nothing | `a_cell_handed_a_live_class_gateway_places_nothing_and_names_the_gate`, `a_live_class_gateway_is_refused_on_every_pass_and_not_only_the_first` (`qip-edge/tests/paper_boundary.rs`); `a_live_venue_refuses_to_submit_even_if_called_directly` (`qip-execution-engine/tests/execution.rs`) | test | Y |
| RP-06 | The tape feed is accepted only with `QIP_VENUE_FEED=simulated` | `a_tape_is_refused_unless_the_feed_mode_is_simulated` (`qip-edge-node/tests/tape.rs`) | test | Y |
| RP-07 | The venue credential is readable only where the ceiling could use it | `the_venue_credential_is_unreadable_where_live_trading_is_impossible` (`infrastructure.rs`) | infrastructure | Y |
| RP-08 | The UI renders PAPER TRADING wherever posture is shown | `every_surface_that_renders_a_platform_fact_still_says_paper_trading`, `a_halted_paper_platform_still_says_paper_trading_on_every_surface` (`qip-web/tests/web.rs`); `frontend/portal/tests/boundary.spec.ts` | test, portal | Y |
| RP-09 | No UI control could submit an order | exists in part: `no_signing_or_withdrawal_path_appears_in_the_venue_tooling_or_the_portal` (`security.rs`); Playwright test names not read | test, portal | P |
| RP-10 | Autonomy changes only through `AutonomyController::request_change` with an authenticated operator identity; the CLI cannot raise autonomy | `every_operatoridentity_is_built_from_the_principals_durable_subject_not_a_session_value` (`security.rs`); `the_application_layer_signs_nothing_but_the_centres_policy_and_halt` (`api_boundary.rs`) | test | Y |
| RP-11 | The kill switch's release is as permanent as its engagement; the console cannot clear one | `the_kill_switchs_release_is_as_permanent_as_its_engagement` (`qip-events/tests/backbone.rs`); `a_tripped_kill_switch_stops_the_assembled_platform` (`qip-kernel/tests/kernel.rs`) | test | Y |
| RP-12 | Two dependencies only: `serde`, `serde_json` (ADR 0002, 0009) | `no_crate_declares_a_third_party_dependency_beyond_the_two_permitted`, `the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two` (`architecture.rs`); `scripts/check-dependencies.sh`; `cargo deny check` | test, dependency policy, cargo-deny | Y |
| RP-13 | No async runtime (ADR 0001, 0011) | exists in part: made impossible by RP-12 (no `tokio` in the allowlist); no test names an async runtime | dependency policy, cargo-deny | P |
| RP-14 | Dependencies point inward only: lib not on service, service not on runtime, nothing on an app | `a_library_never_depends_on_a_service_or_an_application`, `the_dependency_graph_is_acyclic`, `only_the_composition_root_assembles_the_platform` (`architecture.rs`). Service to runtime and "nothing depends on an app" are not asserted; `qip-cli` depends on `qip-api` and `qip-api` on `qip-web` (U19) | test | P |
| RP-15 | No library, service, runtime or edge crate reads the process environment | `no_library_service_runtime_or_edge_crate_reads_the_process_environment` (`architecture.rs`) | test | Y |
| RP-16 | Only the two named libraries open sockets (`qip-transport`, and `qip-storage/src/redis.rs`) | PROPOSED `only_the_named_libraries_open_sockets` (the rule file says "SLICE-45 writes" it; no function of that name exists in the tree) | test | N |
| RP-17 | `unsafe_code` is forbidden (ADR 0082) | exists in part: workspace lint `unsafe_code = "forbid"` fails the build in `clippy` and `release build`; no named test | clippy | P |
| RP-18 | No `unwrap()` outside tests | PROPOSED `no_unwrap_or_expect_in_non_test_source` (or `clippy::unwrap_used` deny in `[workspace.lints.clippy]`; **neither exists**) | clippy | N |
| RP-19 | `todo!`, `unimplemented!`, `panic_in_result_fn` denied | exists in part: workspace clippy lints deny them; CI sets `-D warnings` | clippy | P |
| RP-20 | Secrets reach processes as files, never as environment values | `no_secret_this_repository_deploys_reaches_a_process_as_an_environment_value`, `every_cloud_run_service_is_internal_and_mounts_secrets_as_files_never_as_environment` (`infrastructure.rs`) | infrastructure | Y |
| RP-21 | Workload Identity Federation only; no service-account keys | `no_service_account_key_exists_anywhere_in_the_terraform`, `the_pipeline_authenticates_without_a_long_lived_key` (`infrastructure.rs`) | infrastructure | Y |
| RP-22 | No token, key or account identifier in code, logs, fixtures | `no_secret_value_appears_in_any_committed_configuration` (`security.rs`); `scripts/check-secrets.sh` | test, secrets | Y |
| RP-23 | The event log is hash-chained and verifiable | `the_event_log_hash_chain_survives_a_full_run` (`acceptance.rs`); `the_event_log_chain_verifies_after_a_long_chaotic_run` (`resilience.rs`) | test | Y |
| RP-24 | Sealed history is not editable | `the_evidence_written_by_the_loop_cannot_be_revised_afterwards` (`truth_loop.rs`) | test | Y |
| RP-25 | Every decision is reproducible from the log alone | `the_whole_platform_is_deterministic` (`acceptance.rs`); `two_journeys_through_the_loop_produce_byte_identical_outcomes` (`truth_loop.rs`) | test | Y |
| RP-26 | Licensing posture is evaluated before a source is used | `a_source_whose_licensing_is_undetermined_cannot_become_tradeable_input` (`security.rs`); `a_dataset_the_catalogue_licenses_for_research_is_refused_a_trade_by_the_named_mechanism` (`compliance_proof.rs`) | test | Y |
| RP-27 | Exact attribution of a fill to the hypothesis the order was released for (ADR 0007) | `an_order_travels_the_control_path_and_produces_an_exact_attribution`, `a_fill_is_attributed_to_the_hypothesis_the_order_was_released_for` (`acceptance.rs`) | test | Y |
| RP-28 | Limits are checked before an order object exists; only a composition root holds an order manager; a limit that cannot fire is a defect | `nothing_outside_a_composition_root_holds_an_order_manager` (`architecture.rs`); `the_expected_shortfall_limit_can_actually_fire` (unit, `qip-kernel/src/platform.rs`) | test | Y |
| RP-29 | Pre-trade deterministic checks never route to a model | `no_safety_critical_engine_can_reach_a_language_model` (`architecture.rs`); `exactly_one_rung_is_deterministic_and_it_is_the_bottom_one` (`cost_router.rs`) | test | Y |
| RP-30 | Bounded retention and working sets | exists in part: `a_long_run_does_not_accumulate_unbounded_state` (`resilience.rs`); `every_streaming_estimator_refuses_a_configuration_past_its_named_memory_ceiling` (`streaming_estimators.rs`); no whole-workspace check | test | P |
| RP-31 | No second source of truth for a fact the event log holds | PROPOSED `no_store_duplicates_a_fact_the_event_log_already_records` (hard to state; needs a store inventory) | test | N |
| RP-32 | Refuse invalid input, never clamp it | PROPOSED `no_validation_path_clamps_where_it_should_refuse` (scan `.clamp(` against an allowlist) | test | N |
| RP-33 | `BTreeMap`/`BTreeSet` wherever iteration order reaches output | exists in part: determinism tests (RP-25) would catch a reorder that reaches a hashed output | test | P |
| RP-34 | `Decimal` for money, never `f64` | exists in part: `the_interface_holds_money_as_text_it_was_handed_and_never_as_a_number_it_owns` (`api_boundary.rs`) | test | P |
| RP-35 | An alert policy names only a metric some binary records | `every_metric_an_alert_policy_queries_is_one_the_platform_emits`, `every_metric_name_the_platform_declares_is_one_something_records` (`manifest_wiring.rs`) | test | Y |
| RP-36 | Composition-root order: ports bound and storage proven writable before reporting healthy | PROPOSED `a_root_with_an_unwritable_journal_never_reports_healthy` | test | N |
| RP-37 | `prod` is never deployed or applied automatically | `the_infrastructure_workflow_cannot_touch_production`, `production_is_never_deployed_automatically` (`infrastructure.rs`); `prod_is_promoted_by_nobody_until_an_adr_says_otherwise` (`gitops.rs`) | infrastructure | Y |
| RP-38 | No `${{ vars.* }}` in a workflow | `no_workflow_depends_on_a_repository_variable` (`infrastructure.rs`) | infrastructure | Y |
| RP-39 | Binary Authorization on every deployed image; upstream images pinned by digest | `every_cloud_run_service_this_repository_deploys_is_subject_to_the_admission_policy` (`infrastructure.rs`); `every_image_under_gitops_is_pinned_by_digest_and_is_either_attested_by_the_pipeline_or_vendored` (`gitops.rs`) | infrastructure | Y |
| RP-40 | The execution node has no external address and no container runtime | `the_execution_node_has_no_external_address_and_no_container_runtime` (`infrastructure.rs`) | infrastructure | Y |
| RP-41 | The egress proxy offers no route to a venue and carries no credential | `the_proxy_offers_no_route_to_a_venue_and_cannot_carry_an_order`, `the_proxy_holds_no_credential_and_no_identity_of_its_own` (`egress.rs`) | test | Y |
| RP-42 | Every ADR number claimed in the index has a body | exists in part: `every_decision_record_is_listed_in_the_index` (`documentation.rs`) checks files into the index only; 0097 and 0101 are claimed with no body (`RM-P0-12`); PROPOSED `every_index_row_links_an_existing_body` would fail today | test | P |
| RP-43 | Every new test is mutation-verified | PROPOSED process gate: PR template field plus a reviewer check; nothing automated exists | none | N |
| RP-44 | Tests run with `--no-fail-fast` | `ci.yml` `test` job passes the flag; `docs/architecture/current-state.md` says the Makefile `test` target omits it (not re-verified here) | test | P |
| RP-45 | The backend is Rust; non-Rust tooling is an accepted set (ADR 0001) | `every_source_file_in_the_backend_workspace_is_written_in_rust`, `the_non_rust_tooling_outside_the_browser_layer_is_exactly_the_accepted_set` (`blueprint_rules.rs`) | test | Y |
| RP-46 | Capability-gated agents: a deep-brain agent holds no market-touching capability; the fast brain refuses a hosted agent that could call a model | `a_research_agent_cannot_be_granted_a_market_touching_capability`, `an_agent_that_holds_a_language_model_cannot_touch_the_market` (`security.rs`, `architecture.rs`) | test | Y |
| RP-47 | Every mutating API route raises a typed intent and requires its role | `every_mutating_route_is_reviewed_here_and_each_raises_a_typed_intent` (`api_boundary.rs`); `an_unauthenticated_caller_cannot_reach_a_privileged_route`, `a_caller_with_the_wrong_role_cannot_reach_a_privileged_route` (`security.rs`) | test | Y |
| RP-48 | Terraform validation gates both refuse a bad value and admit a good one | `every_plan_harness_proves_a_refusal_and_an_admission` (`terraform_plan.rs`); `terraform test` in the `infrastructure` job | infrastructure | Y |
| RP-49 | Simulated data in the portal is deterministic and labelled, never mixed unmarked with real figures | exists in part: `frontend/portal/CLAUDE.md` says `tests/theme.spec.ts` enforces the label; that file was not in the first 100 glob results and was not confirmed | portal | P |

### 5.5 Mission-protecting invariants not already above (HM-NN)

`HERMES_MISSION.md` s10. Items 1, 5 and 6 (secrets, blueprint invariants as tests, data terms) are covered by RP-22,
section 5 as a whole and RP-26; item 7 is HM-04.

| ID | Invariant | Named test | Gate | St |
|---|---|---|---|---|
| HM-01 | Never force-push `main`; never delete the repo, a release tag, a backup or an unmerged branch (s10.2) | PROPOSED `main_branch_protection_refuses_force_pushes_and_deletion` (scheduled `gh api` check; branch protection is GitHub configuration, not in the tree) | scheduled workflow (to be created) | N |
| HM-02 | Never destroy a database or environment holding data not backed up and restore-tested (s10.3) | exists in part: `every_key_rotates_and_the_ones_that_hold_data_cannot_be_destroyed` (`infrastructure.rs`); no restore proof exists | infrastructure | P |
| HM-03 | Never exceed 25 USD/day; never leave a free tier without a directive (s10.4, s9) | PROPOSED `every_environment_declares_a_budget_with_alerts_at_50_70_and_90_percent` (**no billing budget resource exists**) | infrastructure | N |
| HM-04 | Nothing fabricated; nothing claimed that was not measured (s10.7) | exists in part: `scripts/audit-register.py` re-runs a register's cited commands (not wired into `ci.yml` per `current-state.md`); `no_scored_plan_document_calls_a_search_empty_that_is_not` (`documentation.rs`) | test | P |

## 6. Acceptance criteria: v12 s31 as named tests

`v12 p48-50 s31` has 47 "COMPLETE TARGET" lines and `s31.1` (`p50`) has 10 more: **57**. Numbering follows reading
order. Phase is the `v12 p47-48 s30` phase that builds it. St uses the section 5 vocabulary but for a whole capability:
Y = a named test of the whole claim exists; P = part exists; N = none. Counted by reading: **4 Y, 23 P, 30 N.**
Anything needing live capital or an external action is marked BLOCKED and is tested only in its shadow form.

| ID | Criterion (v12 text, shortened) | Phase | Named test | Nearest existing | St |
|---|---|---|---|---|---|
| AC-01 | Fast reflex lane and slow cognitive lane are explicit and independently deployable (`p48`) | 0-1 | `the_reflex_binary_and_the_cognitive_binaries_build_deploy_and_fail_independently` | binaries and workloads tested (`every_deployable_binary_in_the_workspace_has_a_workload`); no independence test | P |
| AC-02 | The warm lane connects slow and fast with no global service on the local hot path | 4 | `no_warm_service_is_called_between_a_tick_and_an_order` | `a_cell_cut_off_from_the_centre_spends_only_its_grant_and_then_stops` | P |
| AC-03 | Global discovery autonomously finds, assesses, registers, feeds and retires sources | 2 | `a_discovered_source_is_assessed_registered_fed_and_retired_without_a_human` | `the_platform_walks_from_a_discovered_source_to_a_learned_lesson` | P |
| AC-04 | World information is evidence-scored and provenance-tracked; scraping is never truth | 2 | `a_scraped_claim_enters_the_world_model_only_as_scored_evidence` | `a_record_that_fails_its_quality_gate_is_refused_by_name_and_never_reaches_the_world_model` | Y |
| AC-05 | World model, causal graph, episodic, semantic, procedural memory, counterfactuals, self-model exist | 2 | `each_memory_kind_the_blueprint_names_has_a_store_and_a_reader` | `qip-world-model`, `qip-ai` episodic, `qip-twin` counterfactual; no semantic or procedural store found | P |
| AC-06 | Planners and specialist agents run in the slow lane and use tools under policy | 8 | `a_specialist_cannot_act_outside_its_charter` | `qip-investment-agents`, `cognition.rs` | P |
| AC-07 | Model Foundry supports large, specialist and distilled reflex models | 2 | `a_large_model_is_distilled_to_a_bounded_reflex_artifact_that_passes_its_budget` | `qip-training` fit and shrink; hosted language-model adapter ADR 0037 | P |
| AC-08 | Quantum Foundry supports optimisation and experimental learning, benchmarked against classical | 6 | `a_quantum_path_never_runs_without_its_classical_baseline_in_the_same_record` | `the_classical_baseline_is_always_computed`, `a_cycle_that_sizes_a_proposal_journals_the_solver_and_the_classical_baseline_it_was_measured_against` | Y |
| AC-09 | Asset, Capital, Risk, Treasury, Ledger and Settlement brains are separate first-class domains | 3 | `each_brain_has_its_own_contract_and_cannot_read_another_brains_private_state` | crates exist for four; no Settlement brain, no asset registry | P |
| AC-10 | Regional reflex nodes operate independently and communicate with peers | 4 | `a_cell_that_loses_the_centre_keeps_working_and_still_hears_its_peers` | independence tested; peer mesh absent | P |
| AC-11 | 2 to 20 leg arbitrage as an executable graph with reservations, coordination, hedging, unwind | 4-5 | `a_failed_third_leg_runs_the_compensations_of_the_first_two_in_reverse` | `qip-edge/tests/arbitrage.rs`; saga only in a draft in the main checkout | P |
| AC-12 | Market making and governed market creation supported | 5, 7 | `a_quote_that_exhausts_the_message_budget_is_narrowed_not_sent`; creation: `a_market_creation_request_is_a_shadow_draft_that_no_venue_receives` (BLOCKED real) | `qip-edge/tests/quoting.rs`, `quote_loop.rs` | P |
| AC-13 | Prediction markets first-class; wagering isolated by eligibility and jurisdiction | 7 | `an_event_market_resolves_only_from_its_named_resolution_source` | `qip-prediction` tests | P |
| AC-14 | Physical product purchase and resale with commerce, logistics, inventory | 7 | `a_purchase_executor_exists_only_as_a_paper_ledger_entry` | none (BLOCKED real) | N |
| AC-15 | All asset classes have an extensible registry | 3 | `an_instrument_without_a_registered_class_is_refused_at_ingestion` | none | N |
| AC-16 | Ledger authoritative, double-entry across cash, positions, obligations, collateral, physical inventory | 0, 3, 7 | `a_posting_is_applied_once_when_the_consumer_replays_the_same_envelope` | `qip-portfolio/tests/ledger_postings.rs`, `qip-ledgerd/tests/store.rs` (cash and positions only) | P |
| AC-17 | Continuous evaluation learns from actions, declines, partial fills, failures | 2 | `a_declined_opportunity_is_scored_against_the_alternative_it_declined_for` | LEARN stage `score_declined`, `score_filled` (`.claude/rules/domains/observability.md`); `qip-twin/tests/twin.rs` | P |
| AC-18 | AIOps can operate and remediate within runbooks and safety envelopes | 8 | `an_automated_remediation_runs_only_if_a_signed_runbook_names_it` | none | N |
| AC-19 | Legal capability gates are separate from technical capability | 3 | `a_venue_that_is_technically_reachable_but_not_eligible_is_researched_and_never_sent` | `a_venue_promoted_to_the_simulator_still_cannot_receive_an_order_at_a_live_class_broker` | P |
| AC-20 | Every dependency has a degraded mode; quantum and global cognition never block the reflex path | all | `every_external_dependency_names_a_degraded_mode_and_each_is_exercised` | `qip-contracts/tests/contracts.rs::nothing_in_the_degradation_table_halts_the_platform`; stress suite | P |
| AC-21 | An Intelligence Expansion Engine ranks research and capability work | 9 | `a_gap_signal_becomes_a_ranked_curriculum_item_with_a_named_cause` | none | N |
| AC-22 | Source, ontology, memory, brain, tool, model, research, capability registries are versioned and extensible | 9 | `a_capability_absent_from_its_registry_cannot_be_invoked` | source catalogue only | N |
| AC-23 | New specialist brains and tools are created in sandboxes, independently evaluated, promoted only on measured gain | 9 | `a_created_tool_cannot_open_a_socket_or_read_the_environment` | none | N |
| AC-24 | Self-generated curriculum and active learning choose the next data and experiments | 9 | `the_next_label_requested_is_the_one_with_highest_expected_information` | none | N |
| AC-25 | Regional experience merges into global learning as typed episodes without a hot-path dependency | 9 | `a_regional_update_reaches_the_centre_only_as_a_bounded_delta` | none (`RegionalEpisode` no symbol) | N |
| AC-26 | Memory consolidation, ontology expansion, cross-domain synthesis | 9 | `consolidating_an_episode_into_semantic_memory_keeps_its_source_ids` | none | N |
| AC-27 | Quantum-assisted learning never bypasses classical benchmarks, evidence rules or promotion gates | 6 | `a_qaoa_result_that_does_not_beat_the_baseline_is_not_shipped` | `a_quantum_answer_that_ties_loses_to_the_classical_baseline` | Y |
| AC-28 | Raw and canonical tick history is a governed training asset with manifests, entitlements, deterministic reconstruction | 2 | `a_tick_written_to_the_lake_is_listed_by_its_bitemporal_instants` | tape/replay in `qip-market-ingestion`; `ReplayManifest` in `qip-contracts::replay`; no lake | P |
| AC-29 | Replay models latency, queues, fees, rejects, partial fills, impact, multi-leg failure and unwind | 1-5 | `a_replayed_day_models_every_cost_and_failure_the_blueprint_lists` | `qip-simulation-engine/tests/market_conditions.rs`, `stress.rs` | P |
| AC-30 | World and microstructure intelligence are time-aligned and fused | 2 | `a_world_belief_and_a_market_signal_fuse_into_one_belief_with_both_provenances` | none in this tree (draft `fusion.rs` in the main checkout) | N |
| AC-31 | Microstructure models are recalibrated from live telemetry and distilled only after independent evaluation | 2 | `a_residual_above_threshold_between_replay_and_live_opens_a_curriculum_item` | `qip-twin/src/regret.rs`, `adversary.rs`; distillation gate absent | P |
| AC-32 | World intelligence is a federation; disagreement and branch lineage explicit | 2 | `two_world_models_that_disagree_on_one_entity_are_both_kept_and_flagged` | single model only | N |
| AC-33 | A Symbolic and Neuro-Symbolic Reasoning Fabric exists | 2 | `a_symbolic_rule_that_contradicts_an_established_causal_edge_is_rejected` | none (C5) | N |
| AC-34 | Ambient models run continuously, keep forecasts and surprise scores, wake specialists unprompted | 8 | `an_ambient_model_that_raises_a_surprise_launches_a_scan_nobody_asked_for` | none | N |
| AC-35 | Quantum research can assist symbolic and combinatorial search, every result benchmarked and verified | 6 | `the_search_experiment_reports_a_classical_and_a_quantum_result_on_one_instance` | none | N |
| AC-36 | Ambient cognition has attention and resource budgets and cannot starve reflex, risk, ledger, custody | 8 | `ambient_work_over_its_budget_is_shed_before_a_reflex_or_ledger_resource_is_touched` | none | N |
| AC-37 | Desired outcomes are explicit `GoalSpec` objects | 10 | `a_goal_with_no_reachable_affordance_is_reported_unreachable_not_guessed` | none | N |
| AC-38 | A Causal Agency Engine maps goals to levers and executes only through typed authorised tools | 10 | `a_plan_is_ranked_by_estimated_effect_and_always_carries_a_no_action_baseline` | none (draft `qip-causal` only in the main checkout, no ADR) | N |
| AC-39 | External communication is an audited action surface with conduct checks; deception infeasible by policy | 10 | `an_outbound_message_without_a_conduct_gate_pass_is_never_emitted` (BLOCKED real) | none | N |
| AC-40 | Intervention learning estimates effect and uncertainty; correlation alone cannot grant autonomy | 10 | `an_effect_is_attributed_to_an_action_only_against_its_no_action_counterfactual` | none | N |
| AC-41 | Action Outcome Memory and `EffectAttribution` update world models and policies | 10 | `an_action_outcome_is_recalled_with_its_plan_its_baseline_and_its_effect` | none | N |
| AC-42 | Ambient models may propose actions but cannot perform external ones | 10 | `an_ambient_signal_reaches_the_planner_only_through_the_attention_router` | none | N |
| AC-43 | Quantum intervention search ranks action sets but cannot bypass classical validation or gates | 10 | `a_quantum_ranked_plan_is_rescored_classically_before_any_gate_sees_it` | none | N |
| AC-44 | Pub/Sub and core Kafka are not internal runtime dependencies | 0 | `no_internal_stream_binds_to_a_pubsub_or_kafka_transport` | `pubsub.rs` returns `Unavailable`; dependency policy | P |
| AC-45 | A fabric outage cannot block market-event-to-order execution | 0-1 | `a_stalled_fabric_does_not_change_pass_latency_or_decisions` | `ship_returns_without_waiting_when_the_writer_is_stalled_and_the_entries_stay_unshipped` | P |
| AC-46 | Direct Reflex Mesh traffic stays separate from durable fabric traffic | 4 | `a_peer_mesh_outage_leaves_journaling_and_control_distribution_untouched` | none | N |
| AC-47 | Every durable stream declares partitioning, ordering, retention, replication, overload, mirroring | 0 | `the_committed_catalogue_validates_and_declares_every_field_of_every_stream` | same name, exists | Y |
| AC-48 | A NOW Brain estimates latent state with uncertainty and freshness | 11+ (not scheduled in `v12 s30`) | `the_state_covariance_never_shrinks_when_an_observation_is_withheld` | none here (draft `state_estimator.rs` in the main checkout) | N |
| AC-49 | A Temporal Forecast Lattice yields calibrated distributions from microseconds to years | 11+ | `a_forecast_population_is_retired_when_its_horizon_has_passed` | none here | N |
| AC-50 | Thousands of models compete in a scored Tournament and Forecast Market | 12+ | `a_models_influence_changes_only_through_resolved_forecast_scores` | none | N |
| AC-51 | Synthetic futures and digital twins explore worlds absent from history | 13+ | `a_synthetic_branch_is_pruned_when_an_observation_contradicts_its_assumptions` | `qip-twin` counterfactual only | P |
| AC-52 | Active Sensing chooses observations by expected value of information | 16+ | `an_information_request_stops_when_marginal_value_drops_below_cost` | none | N |
| AC-53 | Adversarial verifier societies attack important predictions | 12+ | `a_leading_forecast_without_a_recorded_independent_challenge_carries_no_authority` | `redteam.rs`, no test read | N |
| AC-54 | Meta-Intelligence allocates attention, agents and compute by marginal value | 16+ | `a_budget_allocation_records_its_expected_value_and_is_scored_afterwards` | none | N |
| AC-55 | A Cognitive Compiler distils slow-lane discoveries into bounded packages | 16+ | `a_validated_discovery_compiles_to_a_reflex_artifact_within_its_latency_and_failure_budgets` | none | N |
| AC-56 | Capital, hedge, model-risk and survival systems are independent of alpha and can veto or reduce exposure | 15+ | `no_hedge_or_survival_output_can_increase_an_exposure_limit` | risk engine veto path exists; other systems partial | P |
| AC-57 | Quantum is a native asynchronous substrate in a benchmarked hybrid compute fabric, never a hot-path dependency | 6, 14+ | `the_compute_router_never_selects_a_substrate_on_the_order_path` | async research workloads tested; router and fabric not built | P |

Phases "11+" to "16+" are GCP v3.0 `s26.1` phases 11 to 16 (`GCP3 p28`); `v12 s30` itself schedules none of the `s31.1` items (`docs/blueprint/v12-delta-master.md` section C, item 2).
GCP v3.0's own acceptance lists (`GCP3 p28-29 s27`, `s27.1`) are folded into section 5.3 and into the rows above.

## 7. Unknowns, contradictions and candidate ADRs

Every item here is a statement the sources do not settle, or two sources that disagree. None is resolved by this file.
Per `HERMES_MISSION.md` s2.3 each becomes an ADR or a recorded owner decision.

**Unreadable or missing source content**

- U1. `v12 p3 s1.2` "Non-Negotiable Safety/Correctness Boundary" has a heading and no body in the text extraction (the page
  is a graphic or the extraction dropped it). It is the one section that states the safety boundary by name. Read page 3 of the PDF.
- U2. The two v12 figures (`p21`, `p26`) are graphics; their content is unreviewed (`docs/blueprint/v12-delta-master.md` C.11).
- U3. `CapitalGrant` is defined twice with different fields: `v12 p39 s24.9` (amount, strategy, region, venue, risk budget, liquidity reserve, expiry) and `p42 s27` (owner/mandate, region/strategy, amount, collateral class, leverage, expiry, drawdown, liquidity constraints). The code has one (`qip-contracts::capital`).

**Terms the blueprint uses without defining**

- U4. Capital Kernel (`p26`) vs Capital Survival Kernel (`p27`); Forecast (`p20`) vs ForecastState (`p38`); Compute Intelligence Router (`p24`) vs Hybrid Compute Router (`p25`); Cognitive Attention Market (`p20`) vs Attention Router (`p18`). Solver Registry (`p25`) is not among the eight registries of `p35 s23.2`.
- U5. Who may veto or shrink exposure: Hedge Brain, Model-Risk Brain, Capital Kernel or the Risk Gate (`p28` vs `p27`, `p50`). "Can veto/reduce" (`p50`) collides with "the Risk Gate enforces" (`p27`).
- U6. Whether the Forecast Market's synthetic capital (`p19`, `p32`) is ever convertible to a real `CapitalGrant`. Not stated.
- U7. "market microstructure probe" (`p20 s9.10`): whether a probe is ever a live order. Treated as `EXTERNAL_ACTION` until defined.
- U8. No numbers: calibration SLO target (`p20`), "thousands of models" (`p19`), marginal-value thresholds (`p20`), what "contract automatically" means (`p28`).
- U9. Where NOW state, the lattice, branch trees and the scoreboard are stored (`p18-20`, `p32`): ledger, event log or a new store. A new store collides with "no second source of truth for a fact the event log holds".
- U10. `v12 p51 s33` drops the sentence listing "identity, authority, truthfulness, conduct, feasibility, audit and rollback controls" that v11.6 carried; the body (`p4`, `p38`) still states them. Phase 8 is still named "Financial AGI Autonomy" (`p47`).

**Blueprint against blueprint, and blueprint against repo**

- U11. v12 and GCP3 specify Tokio, QUIC with mTLS, prost/Protobuf, BLAKE3 and a Rust Raft (`v12 p41-42`, `GCP3 p7`); ADR 0100 builds plaintext TCP, RF1, bearer tokens (s3, s7) because C2 refuses those dependencies. The delivered fabric does not meet FABRIC-077/086 and says so.
- U12. Reflex VM series: `GCP3 p6` says C4D/C4, `v12 p39` says C3/C3D, the Terraform module accepts only C3/C3D (`modules/execution-node/variables.tf`). Fabric disk: Hyperdisk (`GCP3 p8`, `p20`) vs Persistent Disk (`GCP3 p22`).
- U13. Build phases: v12 s30 has Phases 0 to 10; GCP3 s26 has Phases 0 to 10 with different content plus 11 to 16. The two phase tables are not the same plan.
- U14. Cell count: seven (`CLAUDE.md`, ADR 0008), nine (`qip-edge/src/mesh.rs` doc), three regions (`GCP3 p4`). Environment regions: `dev` `us-east4`, others `europe-west2`.
- U15. `docs/MASTER_ROADMAP.md` against the tree: `RM-P4-02` lists `qip-mesh` as the peer mesh, but `qip-mesh`'s own doc says "point-in-time data mesh ports" and no cell-to-cell peer code was found; `RM-P1-08` cites `qip-edge/src/journal_v2.rs`, which is not in `qip-edge/src/` here (only `journal.rs`); `RM-P2-06` calls `qip-fabricd/src/archiver.rs` a stub while the file exists.
- U16. `one_market_event_travels_all_seven_stages_of_the_truth_loop` says seven while the `Stage` enum has eight variants (confirmed in `cycle.rs`). Either the truth loop is a seven-stage subset or the name is stale; unresolved.
- U17. Nothing was executed (section 0). The only measured test result is the baseline's failure to link. Every "exists" in sections 5 and 6 is a name found by search.
- U18. `unwrap()`/`expect()` outside tests: CLAUDE.md prohibits it; no lint or test enforces it (RP-18).
- U19. Dependency direction: the rule is "a service may not depend on the runtime; nothing may depend on an app" (`.claude/rules/architecture/00-boundaries.md`). `qip-cli` depends on `qip-api`, `qip-api` on `qip-web` (both under `apps/`), and `qip-api/Cargo.toml` lists `qip-edge` while `api_boundary.rs` is named "depends on no execution, venue, capital or edge crate" (the entry may be a dev-dependency; not read). The architecture suite asserts only the library direction.
- U20. Two control paths carry grants and policy to a cell: the cell-to-centre mesh (ADR 0011, `qip-transport/src/mesh.rs`) and fabric stream P0 (ADR 0100). Which is canonical for the first deployment is unstated.
- U21. `qip-fastbrain` and `qip-deepbrain` have `invokers = []` and no network edge to `qip-api`; each builds its own `Platform`. How the three share state is not described anywhere found.
- U22. Who the users are. `CLAUDE.md` says the research and risk desk, "not external customers", and lists retail distribution as a non-goal; `v12 p28 s15` has user/mandate subledgers, `GCP3 p14 s12` has customer identity, and the portal ships sign-up and marketing pages. Unresolved, and it changes the journeys in section 1.3.
- U23. v12 is not adopted (ADR 0101 has no body); `HERMES_MISSION.md` s2.1 ranks "the newest blueprint" above the code, while ADR 0099 keeps every contradicted standing decision in force. This map follows the mission order and marks collisions with C1 to C8 as BLOCKED.
- U24. Existence rule (`HERMES_MISSION.md` s2.4): the items listed at the end of section 4 (TPU7x, Agent Engine, Z3, OR-Tools, prost, a Raft crate, a Qiskit client version) cannot enter an ADR until installed or called and version-captured. No IBM Qiskit Runtime version is pinned anywhere readable.
- U25. `docs/blueprint/v12-delta-gcp.md` records zero cost figures in GCP3. The 25 USD/day ceiling has no cost basis in either blueprint, and no Terraform budget exists (HM-03).
- U26. `ADR 0070`'s diagnostic-only fill-error series conflicts with `v12 p45` row "Replay model diverges from live execution: reduce confidence and capital" (V12-F10).

**Candidate ADRs this map implies** (none written): T1 adopt v12 on ADR 0099's terms (this is ADR 0101, `RM-P0-12`); T2 canonical control path (U20); T3 the `qip-causal` crate (exists only in the main checkout, no ADR, services depending on services per the roadmap); T4 whether the brains are one deployment or three (U21, with C7); T5 dependency-direction rules and the app-to-app edges (U19); T6 whether the fill-error series may narrow a capital grant (U26, restriction-only so compatible with fail-closed); T7 a lint or test for `unwrap` (U18); T8 a billing budget resource in Terraform (HM-03); T9 who the users are (U22).
