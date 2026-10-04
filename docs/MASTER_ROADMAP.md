# Master roadmap (v12.0 build order)

Source: `docs/blueprint/source/algorik-master-blueprint-v12.0.txt` section 30 (Phases 0-10), plus
the v12 sections that section 30 names (9.5 Ambient Model Mesh, 9.6 NOW Brain, 9.7 Temporal
Forecast Lattice). Measured against this worktree at `37644d0f` on 2026-10-04.

This is a **plan and a measurement, not a delivery claim.** No test was run to write it. Status
words come from reading the tree (`ls`, `grep`), not from a green gate. Nothing is deployed:
`execution_nodes = {}` everywhere and the GitOps plane is suspended (ADR 0093).

## Ground rules

- **Paper trading is absolute** (ADR 0003, ADR 0021, `.claude/rules/01-security-and-safety.md`).
  Any item that needs live capital or an external action is `BLOCKED`, and is built only as far as
  its shadow, paper or simulated form. Nothing here proposes weakening the three paper layers.
- **v12 is not yet the architecture of record.** ADR 0099 adopted v11.6; ADR 0101 is claimed
  in `docs/adr/README.md` (2026-10-04, body not written) to adopt v12.0 on 0099's terms. Until it
  lands, every standing decision a v12 item contradicts keeps its force. That is item RM-P0-12.
- **Two dependencies only** (ADR 0002, 0009). Items whose v12 wording implies a new class of
  dependency (consensus, async runtime, QML libraries, Z3/OR-Tools) cite the conflict (C2, C5 in
  ADR 0099) as their blocker and are built only as far as in-tree code allows.
- **Status vocabulary**: `COMPLETE` (none claimed here), `PARTIAL` (code exists, named
  gap), `IN-FLIGHT` (uncommitted in the main checkout, read-only evidence, unverified, not in
  this worktree), `NOT STARTED` (no matching file or symbol found), `BLOCKED` (cannot proceed
  without live capital, an external action, an owner decision or a superseding ADR).
- **Acceptance test names are proposals** unless the cell says an existing file. They follow
  `.claude/rules/architecture/01-testing-strategy.md` (full sentence of the property) and every one
  needs mutation verification before it counts. Cross-cutting ones belong in
  `backend/crates/tests/qip-acceptance/tests/`.
- **Owner role** means a `.claude/agents/` role: `solution-architect` (SA), `backend-engineer`
  (BE), `data-ai-engineer` (DAI), `cloud-platform-engineer` (CPE), `security-engineer` (SEC,
  review only), `sre-release-engineer` (SRE), `test-engineer` (TE), `technical-writer` (TW),
  `product-manager` (PM), `frontend-engineer` (FE). Owner is who implements; SEC and
  `code-reviewer` review independently.
- **Ready** = status is not BLOCKED, and every dependency is either absent or itself ready or
  already PARTIAL-enough to build against. Y/N only; it is a scheduling hint, not a promise.

## In-flight work in the main checkout (read, not modified)

Uncommitted in `/home/david/Development/quantum-ai-platform`, so not in this worktree and not
counted as delivered. Observed defects mean none of it should be taken as working:

| Path | Lines | What it is | Observed problem (read, not compiled) |
|---|---|---|---|
| `backend/crates/services/qip-causal/` (scm, do_calculus, integration, 3 tests) | new crate | SCM and do-calculus; ACE estimation | New crate with no ADR (`CLAUDE.md`: no crate without one). Services-depend-on-services: it pulls `qip-world-model` and `qip-financial`. Not yet in `backend/Cargo.toml` of this worktree. ADR 0054/0087 already own causal edges in `qip-world-model/src/causal.rs`; overlap undecided |
| `qip-deepbrain/src/fusion.rs` | 70 | Cognitive Fusion: goal to intervention over an SCM | Depends on `qip-causal`; apps may depend on services, so layering holds, but the crate is unrecorded |
| `qip-deepbrain/src/lattice.rs` | 230 | Forecast Lattice, populations per horizon | `use serde` only; not wired to any cycle. Fits v12 9.7 |
| `qip-deepbrain/src/state_estimator.rs` | 166 | NOW Brain Kalman filter over `qip_numerics::matrix::Matrix` | `f64` state; fits v12 9.6 |
| `qip-mesh/src/saga.rs` | 149 | Saga coordinator and compensation frames | `pub const SAGA_TOPIC: Topic = Topic::Custom("..".to_string())` cannot be a const (`String::to_string` is not const); will not compile as read |
| `qip-fabricd` `archiver.rs` / `config.rs` / `main.rs` | +154/-28 | Tick Lake archiver and config, replacing `Implemented in SLICE-38` stubs | Config `StorageSettings` use; `std::env::vars()` in `from_env` is allowed in apps. Not verified |

Treat these as drafts to be adopted through the stream owners below, each behind its own gates,
not as completed items.

## Parallel streams

Streams are file-disjoint lanes that can run concurrently. ADR numbers must be claimed in
`docs/adr/README.md` before writing a body, per `00-boundaries.md`, because lanes run in parallel.

| Stream | Name | Phases / items | Primary crates (existing only) | Depends on streams |
|---|---|---|---|---|
| **A** | Governance and ADRs | RM-P0-12 and every blocker record named below | `docs/adr/`, `docs/blueprint/` | none |
| **B** | Fabric, ledger, contracts | RM-P0-01,02,08,09,10 | `qip-contracts`, `qip-events`, `qip-streaming`, `qip-transport`, `qip-fabricd`, `qip-ledgerd` | A (for 09) |
| **C** | Reflex cell, tick capture, replay | RM-P1-*, RM-P2-06 | `qip-edge`, `qip-orderbook`, `qip-market-ingestion`, `qip-routing`, `qip-cli` | B (spool/fabric), A |
| **D** | Cognitive loop | RM-P2-*, RM-P6 symbolic/ensemble | `qip-world-model`, `qip-twin`, `qip-reasoning-engine`, `qip-learning-engine`, `qip-training`, `qip-evolution`, `qip-deepbrain`, in-flight `qip-causal` | B (events), C (tick lake), A (C5) |
| **E** | Capital, asset, risk | RM-P3-* | `qip-capital`, `qip-capital-fabric`, `qip-risk-engine`, `qip-risk`, `qip-portfolio-engine`, `qip-financial` | B (ledger) |
| **F** | Mesh, arbitrage, market making | RM-P4-*, RM-P5-* | `qip-mesh`, `qip-arbitrage`, `qip-edge`, `qip-routing`, `qip-opportunity-engine`, `qip-optimization-engine` | B, C, E |
| **G** | Quantum Foundry | RM-P6-* | `qip-quantum`, `qip-optimization-engine` | D (scenario ensembles), F (graph problems) |
| **H** | Prediction, markets, commerce | RM-P7-* | `qip-prediction`, `qip-capital`, `qip-compliance`, `qip-brokers` | E |
| **I** | Delivery, observability, identity | RM-P0-03,04,05,06,07,11 | `.github/workflows`, `infrastructure/`, `qip-observability`, `qip-acceptance`, `qip-brokers` | none; feeds all |
| **J** | Autonomy, expansion, agency | RM-P8-*, RM-P9-*, RM-P10-* | `qip-agents`, `qip-investment-agents`, `qip-ai`, `qip-lifecycle`, `qip-twin`, `qip-deepbrain`, in-flight `qip-causal` | D, E, G; shadow-only until an owner decision |

Dependency graph by stream: A and I run first and in parallel; B follows A for the fabric record;
C and E start on B's contracts; D follows C's tick capture; F follows B, C, E; G follows D and F;
H follows E; J last and gated.

## Phase 0 - Foundations

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status measured from the tree | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P0-01 | Canonical types and contracts | SA, BE | none | `a_contract_that_changes_its_wire_form_fails_the_schema_lock` (extends `qip-acceptance/tests/event_fabric_schema_lock.rs`) | PARTIAL. `qip-contracts` has 25 files (capital, edge, feasibility, ledger, market_event, replay, reflex, venue, wire). v11.6 register: CONTRACT 51 reqs, 29 PARTIAL, 19 MISSING. v12-specific contracts (opportunity token, goal spec, action outcome) absent | ADR 0101 for v12 additions | Y |
| RM-P0-02 | Ledger | BE | RM-P0-01 | `a_posting_is_applied_once_when_the_consumer_replays_the_same_envelope` | PARTIAL. `qip-ledgerd` has config, consumer, read_api, store, telemetry. ADR 0100 specifies a separate single-writer ledger. Never run outside tests. C4 (Spanner) is still a conflict | C4 storage record for any managed store; in-tree file-backed is allowed | Y |
| RM-P0-03 | Simulation harness | TE, BE | RM-P0-01 | `a_seeded_simulation_run_reproduces_the_same_event_log_hash` | PARTIAL. `qip-simulation-engine` (23 files), `qip-twin`, `qip-acceptance/tests/event_fabric_harness.rs`, `chaos.rs`, `stress.rs` exist | none | Y |
| RM-P0-04 | Observability | BE, SRE | none | `every_alert_policy_names_a_series_a_binary_records` (existing alert/metric tests in `qip-acceptance/tests/terraform_contract.rs`) | PARTIAL. `qip-observability` (6 files) emits; per `.claude/rules/domains/observability.md` no process is proven scraped, 9 alert policies gated on `workload_metrics_exist=false` | Proof of ingestion needs a running process (external: billing disabled); Cloud Run GMP sidecar refused on a CVE (rules file) | N |
| RM-P0-05 | CI/CD and GitOps | CPE, SRE | none | `the_gitops_plane_bootstraps_in_dev_and_every_image_passes_the_scan_gate` (`qip-acceptance/tests/gitops.rs`) | PARTIAL / BLOCKED. `ci.yml` is the working gate. GitOps never bootstrapped (`current-state.md`: Argo CD fails Trivy gate), dev suspended | ADR 0093; C3 (GKE vs Cloud Run); external: billing disabled, upstream CVE fix | N (gate-only work is Y) |
| RM-P0-06 | Identity and security | SEC review, BE | none | `a_fabric_client_without_a_grant_is_refused_before_any_topic_is_read` | PARTIAL. `qip-api/src/auth.rs` (4 roles), `qip-streaming/src/event_fabric/{acl,admission}.rs`, `qip-transport/src/event_fabric/auth.rs`. No downloaded keys policy holds | none | Y |
| RM-P0-07 | Venue adapter SDK | BE | RM-P0-01 | `a_venue_adapter_that_returns_a_live_class_is_refused_at_registration` | PARTIAL. `qip-brokers/src/adapter.rs`, `qip-market-ingestion/src/adapter.rs`; exchange, dex, matching simulators. Paper only | live venues BLOCKED by ADR 0021 | Y |
| RM-P0-08 | Native Rust Event and Control Fabric contracts | SA, BE | RM-P0-01 | `an_envelope_round_trips_through_the_codec_and_keeps_its_hlc_and_schema_id` | PARTIAL. `qip-events/src/event_fabric/{envelope,codec,schema_id,hlc,policy,catalogue,crc32c}.rs`, broker in `qip-streaming/src/event_fabric/`, `qip-fabricd` 6 files, ADR 0100 in-tree single-node broker. Control-plane (as against event) contracts: not found | C2 for QUIC/mTLS/Protobuf | Y |
| RM-P0-09 | 3-zone regional quorum | SA, CPE | RM-P0-08, RM-P0-12 | `a_write_acknowledged_by_two_of_three_zones_survives_the_loss_of_one_zone` (not writable in-tree until C2) | NOT STARTED. ADR 0100 says its availability requirements "are not met" by the single-node build. No replication in tree | C2 consensus dependency record; ADR 0100 "Replication appearing in-tree before C2's record" is a named wrong; external: GCP zones and billing | N |
| RM-P0-10 | Rust SDK | BE | RM-P0-08 | `a_producer_that_loses_the_broker_spools_and_drains_in_order_on_return` | PARTIAL. `qip-transport/src/event_fabric/{producer,consumer,transport,protocol}.rs`; spool exists per ADR 0100 | none | Y |
| RM-P0-11 | Benchmark harness | TE | RM-P0-08 | `the_fabric_benchmark_reports_percentile_latency_against_the_declared_slo` (`qip-acceptance/tests/slo.rs`, `performance.rs`) | PARTIAL. `performance.rs`, `slo.rs` exist; `qip-quantum/src/benchmark.rs` is quantum-only. No fabric throughput harness found | none | Y |
| RM-P0-12 | Adopt v12.0 as architecture of record (ADR 0101) | SA, TW | none | `the_adr_index_links_a_body_for_every_claimed_number` (`qip-acceptance/tests/documentation.rs`) | NOT STARTED. Number 0101 claimed 2026-10-04 in `docs/adr/README.md`, body to follow. 0097 is also a claimed-and-unwritten number | Owner acceptance of ADR text; must keep ADR 0021 and the three paper layers verbatim | Y |

## Phase 1 - One Reflex Cell

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P1-01 | One asset, one venue slice | BE | RM-P0-07, RM-P0-08 | `a_single_venue_cell_runs_a_pass_from_tape_to_journal_with_no_central_call` (`qip-edge-node/tests/pass.rs` is the nearest) | PARTIAL. `qip-edge-node` 50 files, runs `Cell::work` only with `QIP_VENUE_FEED=simulated`; no node deployed | Deployment external (C8, billing) | Y |
| RM-P1-02 | Local fast path | BE | RM-P1-01 | `an_order_intent_reaches_the_gateway_without_a_blocking_call_to_the_fabric` | PARTIAL. `qip-edge/src/cell.rs`, `qip-sequencing`, `qip-routing`; v11.6 register has INCORRECT rows in the hot path (ADR 0100) | ADR 0100 closes the INCORRECT group | Y |
| RM-P1-03 | Deterministic risk | BE, SEC review | RM-P0-01 | `a_limit_that_cannot_fire_is_refused_at_construction` (see `the_expected_shortfall_limit_can_actually_fire` in `qip-kernel/src/platform.rs`) | PARTIAL. `qip-risk` (13 files), `qip-risk-engine` (5), cell drawdown, whitelist, feasibility | none | Y |
| RM-P1-04 | Ledger link to the cell | BE | RM-P0-02, RM-P1-01 | `a_fill_the_venue_reported_posts_exactly_one_ledger_entry` | PARTIAL. `qip-ledgerd` consumer exists; edge fills carry `fills_confirmed` | none | Y |
| RM-P1-05 | Paper trading | BE, SEC review | none | `no_constructor_takes_a_live_ceiling` (`qip-acceptance/tests/paper_boundary.rs`) | PARTIAL, strongest item. Three layers in tree; `paper_boundary.rs` and `qip-edge/tests/paper_boundary.rs` exist | none; boundary must not be weakened | Y |
| RM-P1-06 | Canonical tick capture | DAI | RM-P0-08 | `a_captured_tick_stream_replays_byte_for_byte_from_its_segment` | PARTIAL. `qip-market-ingestion/src/{tape,replay,depth}.rs`; `qip-fabricd` archiver IN-FLIGHT, unverified | none | Y |
| RM-P1-07 | Book reconstruction | BE | RM-P1-06 | `a_book_rebuilt_from_the_feed_matches_the_venue_snapshot_at_every_sequence` (`qip-orderbook/tests/replay.rs` is the nearest) | PARTIAL. `qip-orderbook` 15 files; `qip-protocols/src/itch.rs` | none | Y |
| RM-P1-08 | Deterministic replay | TE | RM-P1-01, RM-P1-06 | `replaying_a_journal_reproduces_every_decision_and_its_hash` (`qip-cli/tests/replay.rs`) | PARTIAL. `qip-edge/src/journal.rs`, `journal_v2.rs`, `qip-cli` replay subcommand | none | Y |

## Phase 2 - Cognitive Data Loop

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P2-01 | Scout Fabric | DAI | RM-P0-08 | `a_source_whose_licence_is_unevaluated_never_reaches_the_catalogue` | PARTIAL. `qip-data-finder` (39 files), `qip-deepbrain/src/discovery.rs`, `connectors.rs`; `GET /data-sources` answers fixed "unavailable" | Egress proxy never applied; external | Y |
| RM-P2-02 | Evidence and provenance | DAI | RM-P2-01 | `a_claim_without_a_provenance_chain_cannot_enter_the_world_model` | PARTIAL. `qip-streaming/src/provenance.rs`, `qip-reasoning-engine/src/evidence.rs`, `qip-lifecycle/src/evidence.rs`; EVID/WORLD register has INCORRECT rows | none | Y |
| RM-P2-03 | World model | DAI | RM-P2-02 | `a_feature_is_unreadable_before_its_knowable_instant` | PARTIAL. `qip-world-model` 24 files (state, graph, causal, granger, exposure, falsification) | none | Y |
| RM-P2-04 | Episodic memory | DAI | RM-P2-03 | `an_episode_recalled_by_time_returns_only_what_was_knowable_then` | PARTIAL. `episodic` occurs in `qip-edge`, `qip-kernel`, `qip-ai`, `qip-events`, `qip-api`; no dedicated store crate | Storage record if a new store (C4) | Y |
| RM-P2-05 | Market Intelligence and Tick Learning Fabric | DAI | RM-P2-06, RM-P1-06 | `a_tick_learner_trained_on_a_window_cannot_see_ticks_after_the_window` | PARTIAL. `qip-learning-engine` (7 files), `qip-training` (13), `qip-feature-dag` (7). Not fed from a tick lake | RM-P2-06 | N |
| RM-P2-06 | Tick Lake | DAI, BE | RM-P1-06, RM-P0-08 | `a_tick_written_to_the_lake_is_listed_by_its_bitemporal_instants` | NOT STARTED in this worktree. `qip-fabricd/src/archiver.rs` is a stub here (`Implemented in SLICE-38`); the in-flight edit in the main checkout is a draft. Only `qip-cli` greps for "tick lake" | ADR 0100 retention class (ADR 0089); C4 if a managed store | Y |
| RM-P2-07 | Replay / digital twin | DAI | RM-P1-08 | `a_twin_replay_of_a_captured_day_scores_the_same_counterfactual_twice` | PARTIAL. `qip-twin` (asof, capture, counterfactual, regret) | none | Y |
| RM-P2-08 | World plus market fusion | DAI | RM-P2-03, RM-P2-06 | `a_world_belief_and_a_market_signal_fuse_into_one_belief_with_both_provenances` | IN-FLIGHT. `fusion.rs` (70 lines) bridges to in-flight `qip-causal`. Nothing committed | Crate decision for `qip-causal` (ADR); overlap with `qip-world-model::causal` | N |
| RM-P2-09 | Model and strategy Foundry | DAI | RM-P2-07 | `a_challenger_is_registered_at_the_bottom_rung_and_never_promoted_by_its_own_evidence` | PARTIAL. `qip-evolution` (12 files), `qip-training`, `qip-strategy`, `qip-lifecycle`; `qip-deepbrain/src/{evolution,succession,learning}.rs` | C5 if Python/JAX; in-tree Rust only | Y |
| RM-P2-10 | World Model Federation | SA, DAI | RM-P2-03 | `two_world_models_that_disagree_on_one_entity_are_both_kept_and_flagged` | NOT STARTED. Only `qip-cli` mentions "federation" | RM-P0-12 | N |
| RM-P2-11 | Model-disagreement representation | DAI | RM-P2-03 | `a_belief_carries_every_models_distribution_not_only_their_mean` (ADR 0088) | PARTIAL. `disagreement` is present in `qip-kernel`, `qip-edge`, `qip-lifecycle`, `qip-market-ingestion` and 7 acceptance files | ADR 0088 slot versioning | Y |
| RM-P2-12 | Symbolic reasoning service | DAI | RM-P2-03 | `a_symbolic_rule_that_contradicts_an_established_causal_edge_is_rejected` | NOT STARTED (no symbolic engine; `qip-reasoning-engine/src/bayes.rs` is probabilistic) | C5 (Z3/OR-Tools) | N |
| RM-P2-13 | Temporal reasoning service | DAI | RM-P2-03 | `temporal_precedence_alone_never_establishes_more_than_a_candidate_edge` (ADR 0054) | PARTIAL. `qip-world-model/src/granger.rs`, ADR 0054 | none | Y |
| RM-P2-14 | Causal reasoning service | DAI | RM-P2-13 | `a_causal_edge_failing_its_conditions_three_passes_running_is_retired` (ADR 0087) | PARTIAL. `qip-world-model/src/{causal,confounder,falsification}.rs`; in-flight `qip-causal` SCM/do-calculus adds interventions, unverified | Crate record for `qip-causal` | Y |
| RM-P2-15 | NOW Brain state estimator (v12 9.6) | DAI | RM-P2-03 | `the_state_covariance_never_shrinks_when_an_observation_is_withheld` | IN-FLIGHT. `state_estimator.rs` (166 lines, Kalman) | none beyond review | N |
| RM-P2-16 | Temporal Forecast Lattice (v12 9.7) | DAI | RM-P2-15 | `a_forecast_population_is_retired_when_its_horizon_has_passed` | IN-FLIGHT. `lattice.rs` (230 lines); unwired | none beyond review | N |

## Phase 3 - Multi-asset, capital and risk

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P3-01 | Asset registry | SA, BE | RM-P0-01 | `an_instrument_without_a_registered_class_is_refused_at_ingestion` | NOT STARTED (`asset_registry` has no hits). ASSET register: 19 reqs, 4 MISSING | none | Y |
| RM-P3-02 | Asset brain | BE | RM-P3-01 | `an_asset_brain_proposes_nothing_for_a_class_it_has_no_registry_entry_for` | NOT STARTED as a crate; ADR 0091 says brains are library crates in five binaries | ADR 0091 (C7) | N |
| RM-P3-03 | Capital brain | BE | RM-P0-02 | `capital_is_reserved_before_an_order_object_exists` | PARTIAL. `qip-capital` (26 files), `qip-capital-fabric` | none | Y |
| RM-P3-04 | Risk brain | BE, SEC review | RM-P1-03 | `a_pre_trade_check_never_routes_to_a_model` | PARTIAL. `qip-risk-engine`, `qip-cost-router` `Determinism::Required` | none | Y |
| RM-P3-05 | Treasury | BE | RM-P0-02 | `a_treasury_transfer_over_the_corridor_limit_is_refused_and_journaled` | PARTIAL. `qip-capital-fabric/src/{transfer,gate,wallet,settlement}.rs`, `qip-api/src/ledger_views.rs` (write surfaces settle nothing) | Real transfers BLOCKED: LIVE_CAPITAL (ADR 0021) | Y (simulated only) |
| RM-P3-06 | Custody corridors | BE | RM-P3-05 | `a_corridor_with_no_custodian_attestation_is_never_treated_as_open` | PARTIAL. `qip-capital-fabric/src/{custody,corridor,destination}.rs`; simulated | Real custody BLOCKED: EXTERNAL_ACTION + LIVE_CAPITAL | Y (simulated only) |
| RM-P3-07 | Multiple asset classes | BE | RM-P3-01 | `a_basket_across_two_asset_classes_is_sized_by_one_risk_engine` | PARTIAL. `qip-financial` (28 files), `qip-prediction`, `qip-brokers/src/dex.rs`, `qip-acceptance/tests/cross_margin.rs`, `decentralised_venue.rs` | none | Y |

## Phase 4 - Multi-region reflex mesh

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P4-01 | Americas/Europe/APAC nodes | CPE, SRE | RM-P1-01 | `three_region_cells_boot_from_one_image_each_with_a_distinct_region_table` (`qip-edge/tests/region_table.rs` nearest) | NOT STARTED as a deployment. Code supports regions; `execution_nodes = {}` | C8; external: billing disabled; owner cost ceiling | N |
| RM-P4-02 | Peer mesh | BE | RM-P0-08 | `a_cell_that_loses_the_centre_keeps_working_within_its_envelope_and_still_hears_its_peers` | PARTIAL. `qip-mesh` (spine, delta, state, adapters), `qip-edge/src/mesh.rs`, `qip-api/src/mesh.rs`; no wired config anywhere | none | Y |
| RM-P4-03 | Opportunity tokens | SA, BE | RM-P4-02, RM-P0-12 | `an_opportunity_token_can_be_redeemed_by_exactly_one_cell` | NOT STARTED (no symbol). `qip-opportunity-engine` is the central engine only | Contract needs ADR 0101 | N |
| RM-P4-04 | Distributed reservations | BE | RM-P4-02 | `two_cells_cannot_reserve_the_same_capital_twice` | PARTIAL. `qip-edge/src/reservation.rs`, `qip-capital` reservation, ADR 0039 region share | none | Y |
| RM-P4-05 | 2-5 leg cycles | BE | RM-P4-04 | `a_five_leg_cycle_is_refused_when_any_leg_fails_feasibility` | PARTIAL. `qip-arbitrage/src/{graph,legs,search,plan}.rs`, `qip-routing/src/pathcycle.rs`, `qip-edge/tests/arbitrage.rs` | none | Y |
| RM-P4-06 | Cross-region tick alignment | DAI | RM-P1-06, RM-P4-02 | `ticks_from_two_regions_align_to_one_clock_with_a_bounded_skew` | NOT STARTED (`qip-events` HLC is the only primitive) | none | N |
| RM-P4-07 | Lead/lag learning | DAI | RM-P4-06 | `a_lead_lag_estimate_is_discarded_when_it_fails_out_of_sample` | NOT STARTED (one hit in `qip-numerics`, primitive only) | none | N |
| RM-P4-08 | Distributed opportunity replay | TE | RM-P4-03, RM-P1-08 | `a_distributed_opportunity_replays_to_the_same_decision_in_every_cell` | NOT STARTED (replay is single-cell) | none | N |
| RM-P4-09 | Selective cross-region fabric mirroring | SA, BE | RM-P0-09, RM-P4-02 | `latency_sensitive_state_never_travels_the_mirror_link` | NOT STARTED; `qip-streaming/src/{mesh,tiered}.rs` are partial primitives | Needs RM-P0-09 (quorum) and C2 | N |

## Phase 5 - 3-20 leg arbitrage and market making

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P5-01 | Graph optimisation, 3 to 20 legs | BE | RM-P4-05 | `a_twenty_leg_search_finishes_within_its_time_budget_or_declines` | PARTIAL. `qip-arbitrage` graph/search (4318 lines), `qip-optimization-engine` | none | Y |
| RM-P5-02 | Pre-positioned inventory | BE | RM-P3-05, RM-P4-04 | `inventory_positioned_ahead_of_a_leg_is_counted_against_the_region_share` | PARTIAL. `qip-capital-fabric/src/{plan,forecast}.rs`, ADR 0039 | none | Y |
| RM-P5-03 | Saga recovery | BE | RM-P4-02 | `a_failed_third_leg_runs_the_compensations_of_the_first_two_in_reverse` | IN-FLIGHT. `qip-mesh/src/saga.rs` (149 lines) read; the `Topic::Custom(String)` const as read does not compile | Review and fix before adoption | N |
| RM-P5-04 | Quote engine | BE | RM-P1-02 | `a_quote_that_exhausts_the_message_budget_is_narrowed_not_sent` (`qip-edge/tests/quoting.rs`, `qip-acceptance/tests/quote_loop.rs`) | PARTIAL. `qip-edge/src/quoting.rs`, `qip-market/src/quote.rs`; ADR 0084 release schedule | none | Y |
| RM-P5-05 | Cross-class hedge map | DAI | RM-P3-07 | `a_hedge_is_proposed_only_across_classes_with_a_measured_correlation` (ADR 0086) | NOT STARTED (one hit in `qip-kernel`) | ADR 0086 first real grant | N |
| RM-P5-06 | Live market making | n/a | RM-P5-04 | `market_making_never_submits_to_a_live_class_venue` | BLOCKED. Simulated only | LIVE_CAPITAL; ADR 0003, 0021 | N |

## Phase 6 - Quantum Foundry

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P6-01 | Classical benchmark first | DAI | none | `a_quantum_path_never_runs_without_its_classical_baseline_in_the_same_record` (ADR 0006) | PARTIAL. `qip-quantum/src/benchmark.rs`, `ClassicalBaseline` in `qip-quantum`, `qip-optimization-engine` | none | Y |
| RM-P6-02 | Quantum optimisation | DAI | RM-P6-01 | `a_qaoa_result_that_does_not_beat_the_baseline_is_not_shipped` | PARTIAL. `qip-quantum/src/{qaoa,statevector,provider,solver}.rs`; local simulator | IBM hardware: external action, credentials | Y (local) |
| RM-P6-03 | QML research plug-ins | DAI, SA | RM-P6-01 | `a_qml_plugin_is_registered_only_with_a_declared_baseline_and_a_dataset` | NOT STARTED (no `qml` hit) | New ML libraries need an ADR (C5, ADR 0002) | N |
| RM-P6-04 | Symbolic/combinatorial search experiment | DAI | RM-P2-12, RM-P6-01 | `the_search_experiment_reports_a_classical_and_a_quantum_result_on_one_instance` | NOT STARTED | RM-P2-12; C5 | N |
| RM-P6-05 | World-model/scenario ensemble selection experiment | DAI | RM-P2-10, RM-P6-01 | `an_ensemble_selection_is_scored_against_the_greedy_classical_choice` | NOT STARTED | RM-P2-10 | N |

## Phase 7 - Prediction, market factory, commerce

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P7-01 | Event markets | BE | RM-P3-07 | `an_event_market_resolves_only_from_its_named_resolution_source` | PARTIAL. `qip-prediction` (market, oracle, resolution, scoring, arbitrage, cross); `qip-world-model/src/resolution_source.rs` | none | Y |
| RM-P7-02 | Market creation workflows | BE | RM-P7-01 | `a_market_creation_request_is_a_shadow_draft_that_no_venue_receives` | BLOCKED as a real act. Draft form only | EXTERNAL_ACTION (v11.6 18.2, ADR 0099) | N |
| RM-P7-03 | Wagering isolation by eligibility/jurisdiction | BE, SEC review | RM-P7-01 | `a_wager_for_an_ineligible_jurisdiction_is_refused_before_pricing` | PARTIAL. `qip-capital` eligibility, `qip-compliance`, `qip-portfolio` | none | Y |
| RM-P7-04 | Physical commerce adapters | BE | RM-P7-03 | `a_purchase_executor_exists_only_as_a_paper_ledger_entry` | BLOCKED. `logistics` has two hits in `qip-financial` | LIVE_CAPITAL + EXTERNAL_ACTION (v11.6 20) | N |
| RM-P7-05 | Logistics and inventory | BE | RM-P7-04 | `a_shipment_state_change_is_journaled_with_its_source` | NOT STARTED beyond types | EXTERNAL_ACTION | N |

## Phase 8 - Financial AGI autonomy

All of this runs under `AutonomyLevel::deployable` and never raises autonomy from a model output.

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P8-01 | Broader tool use under policy | BE, SEC review | RM-P2-09 | `an_agent_holding_a_market_touching_capability_stops_the_deep_brain_at_start` | PARTIAL. `qip-agents`, `qip-investment-agents` (17 files), `qip-ai`; deepbrain refuses market-touching agents | none | Y |
| RM-P8-02 | Self-directed research campaigns | DAI | RM-P2-01 | `a_campaign_cannot_exceed_its_declared_budget_and_licence_set` | PARTIAL. `qip-deepbrain/src/campaign.rs` | none | Y |
| RM-P8-03 | Specialist-agent society | DAI | RM-P8-01 | `a_specialist_cannot_act_outside_its_charter` | PARTIAL. `qip-investment-agents`, `qip-acceptance/tests/cognition.rs` | none | Y |
| RM-P8-04 | Autonomous model/strategy lifecycle under authority envelopes | BE | RM-P2-09 | `a_promotion_beyond_the_envelope_is_refused_and_journaled` | PARTIAL. `qip-lifecycle` (band, gates, trials, venue_ladder), `AutonomyController` | Live rungs BLOCKED (ADR 0021) | Y (paper) |
| RM-P8-05 | Ambient Model Mesh | DAI | RM-P2-15, RM-P2-16 | `an_ambient_model_that_raises_a_surprise_launches_a_scan_nobody_asked_for` | NOT STARTED (`ambient` hits are transport/streaming, unrelated) | none | N |
| RM-P8-06 | Attention Router | DAI | RM-P8-05 | `the_router_never_spends_beyond_the_cost_routers_budget` | NOT STARTED (no symbol) | `qip-cost-router` budget | N |
| RM-P8-07 | Prompt-free research launch | DAI | RM-P8-02, RM-P8-06 | `a_launch_without_a_human_prompt_is_attributed_to_the_router_event_that_caused_it` | NOT STARTED | none | N |

## Phase 9 - Open-ended intelligence expansion

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P9-01 | Curriculum Engine | DAI | RM-P8-04 | `a_curriculum_step_is_chosen_from_a_measured_weakness_and_names_it` | NOT STARTED (only `qip-cli` greps it) | none | N |
| RM-P9-02 | Ontology/source/tool/brain/capability registries | SA, BE | RM-P0-12 | `a_capability_absent_from_its_registry_cannot_be_invoked` | PARTIAL. Source catalogue exists (`qip-data-finder`); ontology, tool, brain and capability registries not found | ADR for registry shape | N |
| RM-P9-03 | Sandboxed tool/agent creation | SA, SEC review | RM-P9-02 | `a_created_tool_cannot_open_a_socket_or_read_the_environment` | NOT STARTED | Sandbox mechanism needs an ADR (no new dependency; `unsafe` forbidden) | N |
| RM-P9-04 | Active learning | DAI | RM-P2-05 | `the_next_label_requested_is_the_one_with_highest_expected_information` | NOT STARTED | none | N |
| RM-P9-05 | Memory consolidation | DAI | RM-P2-04 | `consolidating_an_episode_into_semantic_memory_keeps_its_source_ids` | NOT STARTED | none | N |
| RM-P9-06 | Regional-to-global learning | DAI | RM-P4-02, RM-P2-05 | `a_regional_update_reaches_the_centre_only_as_a_bounded_delta` | NOT STARTED | RM-P4-02 | N |
| RM-P9-07 | Continuous benchmark-gated promotion | BE | RM-P8-04 | `a_model_is_promoted_only_after_beating_the_incumbent_on_a_held_out_window` | PARTIAL. `qip-lifecycle/src/{gates,trials,band}.rs`, `qip-deepbrain/src/succession.rs` | none | Y |
| RM-P9-08 | Microstructure curricula from live-vs-replay residuals; new venue discovery | DAI | RM-P2-07, RM-P9-01 | `a_residual_above_threshold_between_replay_and_live_opens_a_curriculum_item` | NOT STARTED. `qip-twin/src/regret.rs` is the residual primitive | "Live" is read as paper/simulated residual | N |
| RM-P9-09 | Governed spawning/retirement of world models; symbolic rule growth; ambient-signal curricula | DAI | RM-P2-10, RM-P2-12, RM-P8-05 | `a_spawned_world_model_starts_in_shadow_and_retires_on_a_missed_baseline` | NOT STARTED (ADR 0087 retires causal edges, not models) | RM-P2-10, RM-P2-12 | N |

## Phase 10 - Causal agency and intervention intelligence

Begins shadow-only (v12 section 30). No item here sends, publishes, purchases or contacts anything.

| ID | Item | Owner | Deps | Acceptance test (proposed) | Status | Blocker | Ready |
|---|---|---|---|---|---|---|---|
| RM-P10-01 | GoalSpec / action-affordance graph | SA, DAI | RM-P2-14 | `a_goal_with_no_reachable_affordance_is_reported_unreachable_not_guessed` | NOT STARTED (no `GoalSpec` or `affordance` symbol) | RM-P0-12 | N |
| RM-P10-02 | Intervention Planner | DAI | RM-P10-01, RM-P2-14 | `a_plan_is_ranked_by_estimated_effect_and_always_carries_a_no_action_baseline` | IN-FLIGHT. Draft `qip-causal` do-calculus and `fusion.rs` in the main checkout; nothing in this tree | Crate record; review | N |
| RM-P10-03 | Authorised action adapters | BE, SEC review | RM-P10-01 | `an_action_adapter_runs_only_in_shadow_and_records_what_it_would_have_done` | BLOCKED as real adapters | EXTERNAL_ACTION | N |
| RM-P10-04 | Communications / conduct gate | SEC review, BE | RM-P10-03 | `an_outbound_message_without_a_conduct_gate_pass_is_never_emitted` | BLOCKED (`communications` has one hit in `qip-financial`) | EXTERNAL_ACTION (public communications, ADR 0099 C1) | N |
| RM-P10-05 | Causal effect attribution | DAI | RM-P10-02 | `an_effect_is_attributed_to_an_action_only_against_its_no_action_counterfactual` | PARTIAL / IN-FLIGHT. `qip-evolution/src/attribution.rs`, `qip-twin/src/counterfactual.rs`; in-flight ACE test | none | Y |
| RM-P10-06 | Experiment and no-action baselines | DAI | RM-P10-02 | `every_intervention_trial_has_a_matched_no_action_arm` | PARTIAL. `qip-twin` counterfactual and regret | none | Y |
| RM-P10-07 | Action Outcome Memory | DAI | RM-P2-04, RM-P10-05 | `an_action_outcome_is_recalled_with_its_plan_its_baseline_and_its_effect` | NOT STARTED (no symbol) | none | N |
| RM-P10-08 | Ambient-to-agency routing | DAI | RM-P8-06, RM-P10-02 | `an_ambient_signal_reaches_the_planner_only_through_the_attention_router` | NOT STARTED | RM-P8-06 | N |
| RM-P10-09 | Narrow reversible actions after shadow | n/a | RM-P10-03, RM-P10-04 | `no_action_leaves_the_platform_without_an_owner_signed_reversal_path` | BLOCKED | Owner decision superseding the paper-only stance; EXTERNAL_ACTION | N |

## Blocked items (live capital, external action, or an unresolved record)

| Item | Why | Needs |
|---|---|---|
| RM-P0-09, RM-P4-09 | Consensus and a mirror need a dependency or in-tree replication ADR 0100 refuses | C2 record |
| RM-P0-05, RM-P0-04, RM-P4-01 | Billing disabled on dev, GitOps suspended, no scrape evidence | Owner billing and cost ceiling (C8, ADR 0093) |
| RM-P3-05, RM-P3-06 | Real transfer and custody | Never; shadow/simulated only (ADR 0021) |
| RM-P5-06, RM-P7-02, RM-P7-04, RM-P7-05 | Live market making, market creation, purchases, logistics | Owner supersedes ADR 0003; not an agent decision (C1) |
| RM-P10-03, RM-P10-04, RM-P10-09 | Outreach and external actions | Same |
| RM-P2-12, RM-P6-03, RM-P6-04, RM-P9-03 | Symbolic solvers, QML libraries, sandboxing | C5 and ADR 0002 |

## Ready to start now (no blocker, deps satisfiable)

RM-P0-01, 02, 03, 06, 07, 08, 10, 11, 12; RM-P1-01 through 08; RM-P2-01, 02, 03, 04, 06, 07, 09, 11, 13, 14; RM-P3-01, 03, 04, 05 (simulated), 06 (simulated), 07; RM-P4-02, 04, 05; RM-P5-01, 02, 04; RM-P6-01, 02 (local); RM-P7-01, 03; RM-P8-01, 02, 03, 04; RM-P9-07; RM-P10-05, 06.

First in each stream: A, RM-P0-12; B, RM-P0-08 and 10; C, RM-P1-06; D, RM-P2-03; E, RM-P3-01 and 03; F, RM-P4-02; G, RM-P6-01; H, RM-P7-01; I, RM-P0-06 and 11; J, RM-P8-01.

## What this document does not claim

- That any listed test exists, passes or was mutation-verified. Existing files are named only as the nearest evidence.
- That the in-flight main-checkout files compile or are correct. They were read, not built.
- That v12 is adopted. It is not until ADR 0101 has a body and is accepted.
