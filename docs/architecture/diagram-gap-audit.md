# Architecture-diagram audit — code as of 2026-08-24

Workspace state at audit time: `./scripts/count-tests.sh` reports **2,862 passed, 0 failed** ("the suite is green").
Prior audit (`docs/architecture/current-state-audit.md`) was measured at 2,086 tests — it is stale and was treated
only as a list of claims to re-check. Every verdict below was re-derived by grep/read of the current tree.
`infrastructure/terraform/` and `backend/crates/apps/qip-cli/` were excluded per instruction (other agents editing).

**Verdict key**
- **BUILT+WIRED** — implemented and reachable from a running binary (`qip-fastbrain`, `qip-deepbrain`, `qip-api`, `qip-edge-node`) or the composed `qip_kernel::Platform` they construct.
- **BUILT-UNWIRED** — implemented and tested; constructed by nothing that runs (test-only reachability).
- **PORT** — an interface that refuses with a named requirement.
- **PARTIAL** — some sub-capabilities real, others absent (stated which).
- **ABSENT** — no code.
- **DIVERGED** — deliberately replaced, with the ADR named.

**The single most important composition fact**, established first because it colors every layer:
the three central binaries construct `Platform` (`backend/crates/apps/qip-fastbrain/src/main.rs:104`,
`backend/crates/apps/qip-deepbrain/src/main.rs:124`, `backend/crates/apps/qip-api/src/main.rs:66`), and the cycle they run is real —
but **the deployed composition cannot place an order**:

1. `stage_decide` unconditionally emits a `nothing_to_do` proposal — there is no code path that expresses an
   approved thesis as a trade (`backend/crates/runtime/qip-kernel/src/platform.rs:1321-1330`; the only call is
   `self.constructor.nothing_to_do(...)`, reason string hardcoded).
2. `Platform::submit_order` (the full control path: pre-trade risk, autonomy, kill switch, capture) is called only
   by tests (`backend/crates/runtime/qip-kernel/tests/kernel.rs:294-366`, `tests/platform_outcomes.rs:147`); `qip-api`
   exposes only `GET /orders` (`backend/crates/apps/qip-api/src/routes.rs:370`), no submit/approve/release endpoint.
3. `Cell::deploy` (the only way a strategy enters the hot path) is called exclusively in tests
   (`backend/crates/edge/qip-edge/tests/*.rs`, `backend/crates/tests/qip-acceptance/tests/{stress,e2e,e2e_live,chaos}.rs`);
   `qip-edge-node/src/main.rs` assembles the cell, gateway, mesh link and journal but never deploys a strategy —
   it prints `cell.deployed_strategies().len()` (main.rs:442), which is 0 in every deployment.

---

## Layer 1 — Autonomous Data Mesh

| Element | Verdict | Evidence |
|---|---|---|
| Market Data (all asset classes) | PARTIAL | Types + synthetic environment wired: `qip-market-ingestion/src/synthetic/`, pulled by fastbrain's feed (`backend/crates/apps/qip-fastbrain/src/feed.rs` — doc: "Nothing here is production-grade"). A live REST market-data adapter exists (`qip-market-ingestion/src/rest.rs:232 RestMarketDataAdapter`) but is consumed only by `backend/crates/tests/qip-acceptance/tests/e2e_live.rs:738` — no binary polls it. |
| Order Books L1/2/3 | BUILT+WIRED | `backend/crates/edge/qip-orderbook/src/{l2,l3,auction,venue}.rs`; venue state used by the edge cell (per-venue books; gateway drains fills, `qip-edge-node/src/gateway.rs`); depth handling in `qip-market-ingestion/src/depth.rs`. |
| News & Social Events | PARTIAL | `NewsItem` type + synthetic narratives (`qip-market-ingestion/src/narrative.rs`, `synthetic/narrative.rs`); `WorldModel::absorb_news` exists (`qip-world-model/src/world.rs:213`) but is never called at runtime — `Platform::observe` discards all non-bar records via a `_ =>` arm (platform.rs, observe body; see Layer 3). No live news adapter. |
| Company Data (filings, earnings) | PARTIAL | `FundamentalUpdate` + `absorb_fundamental` (`world.rs:303`); same discard path; synthetic only. |
| Macro & Econ Indicators | PARTIAL | `MacroObservation` + `absorb_macro` (`world.rs:345`); same discard path; synthetic only. |
| On-Chain/DeFi (NFTs, wallets) | PARTIAL | `qip-chain` is real and wired: `Platform::observe_chain`/`confirmed_chain` (platform.rs:1824-1897), chain summarized in `stage_understand`; modules `amm, block, bridge, finality, gas, mempool, rpc, state`. **NFTs and wallets: ABSENT** — zero hits for nft/wallet in `backend/crates/services/qip-chain/src`. `rpc.rs` is the port to a real node. |
| Alt Data (satellite, IoT, mobility, web) | PARTIAL | `qip-market-ingestion/src/alternative.rs` models exactly these ("Satellite imagery, IoT telemetry, mobility counts, web-scraped panels", line 3; `satellite.parking_lot_counts`, `footfall`, `container_throughput` at 470-980) with tri-temporal instants. Synthetic/config-driven only; discarded by `observe`. |
| Reference Data (prices, FX, rates, IDs, corp actions) | PARTIAL | `SensedRecord::{CorporateAction, ReferenceData}` (`qip-market-ingestion/src/adapter.rs:31-43`); corp-action handling in `qip-normalization/src/{normalizer,contract}.rs`; instrument identity in `qip-financial`. No live source; normalization itself is unwired (below). |
| AI Feed Discovery Agents | BUILT-UNWIRED | `qip-data-finder` (14 modules incl. `scoring.rs, quality.rs, robots.rs, legal.rs, replacement.rs`) is constructed inside `Platform` (platform.rs:63-66), but discovery is only driven by `Platform::assess_sources`, whose sole caller is `qip-acceptance/tests/e2e.rs:499`. `NetworkProbe` remains a refusing port; the API's `/sources` route answers `NO_DATA_FINDER` (`qip-api/src/missing.rs:40`, routes.rs:398). |
| Continuous Collectors (Rust) | PARTIAL | The collector loop is real and wired — fastbrain polls a `DataAdapter` every tick (`qip-fastbrain/src/main.rs:93-141`) — but the only adapters it can open are `SyntheticEnvironment` and `ReplayAdapter` (`feed.rs:20-22`). |
| Deduplication + Normalization | PARTIAL | Dedup is wired: per-body idempotency keys on stream envelopes (`qip-streaming/src/envelope.rs:393-400`), bus-level dedup (`qip-events/src/bus.rs:262`), and the mesh spine "refuses a redelivery it has already absorbed" (`qip-mesh/src/spine.rs:16`). The **normalization service is BUILT-UNWIRED**: `qip_normalization` is consumed only by `qip-acceptance/tests/{truth_loop,performance}.rs`. |
| Time Sync & Enrichment | PARTIAL | Time discipline wired: `qip-sequencing` Sequencer in the edge cell (`qip-edge/src/cell.rs:28`), plus `qip-streaming/src/{sequencing,processing}.rs` and ingestion depth. Enrichment is not: the normalizer is test-only and entity resolution lives inside the never-written WorldModel (`world.rs:43 resolver`). |
| Source Scoring & Quality | BUILT-UNWIRED | `qip-data-finder/src/{scoring,quality,health}.rs`; reachable only through the test-only `assess_sources` path above. |
| Event Fingerprinting | PARTIAL | Two real mechanisms, neither is cross-source event-identity fingerprinting: (a) schema/contract fingerprints (`qip-events/src/registry.rs:25-26, 66, 109-116` — sha256 over topic+version+fields); (b) content digests + dedup keys on envelopes (`qip-streaming/src/envelope.rs:393-431`). Both wired via the journal publish each cycle. There is no "same real-world event seen via two feeds" fingerprint. |

## Streaming backbone

| Element | Verdict | Evidence |
|---|---|---|
| Google Pub/Sub | DIVERGED (ADR 0011) + residual PORT | `docs/adr/0011-everything-in-rust-on-kubernetes.md` replaces Pub/Sub with `qip-transport` (in-tree HTTP/1.1 + mesh). The old port still exists and refuses (`qip-streaming/src/pubsub.rs:108-110`). |
| The in-tree mesh, "genuinely wired?" | **PARTIAL — cell half wired, central half unwired** | Cell half: `qip-edge-node/src/mesh.rs` (MeshLink = `CellUplink` publishing state deltas + `CapitalDownlink` pulling signed envelopes, one exchange per tick; peer from `QIP_MESH_PEER`). Central half: `qip-mesh/src/spine.rs` (`CapitalDispatcher`, `CellDeltaReceiver`) is constructed by **no binary** — its only consumers are `qip-acceptance/tests/e2e_live.rs:151,1086`. Nothing in `qip-api` or `qip-deepbrain` references the spine; no route ingests a cell report (`ingest_cell_report`'s only non-kernel mention is a doc comment, spine.rs:501); `qip-api`'s `CellRegistry` is never `.record()`ed in src, so `/regions` and `/risk` always answer `NO_CELL_REPORTS` (`qip-api/src/missing.rs:28`, routes.rs:912,1076). A cell in production publishes deltas at a peer address where nothing listens. |
| Durable journal | BUILT+WIRED | `DurableLogTransport` journal written every cycle (`platform.rs:51 journal_cycle`, 1939-1965); deepbrain restores from it on start (`qip-deepbrain/src/node.rs:154 restored_through`). |

## Layer 2 — Regional AI Brains (×7)

| Element | Verdict | Evidence |
|---|---|---|
| ×7 regions | ABSENT as deployment | `infrastructure/kubernetes/base/` has exactly one manifest set (api, fastbrain, deepbrain, edge-cell StatefulSet `replicas: 2`, journal-storage, namespace) — no region overlays. The kernel's region is the constant `HOME_REGION = "home"` (platform.rs:245). Region is a per-cell env var; nothing instantiates seven. |
| Market Understanding | BUILT+WIRED | `stage_understand` (platform.rs:993 "world model covers N instrument(s)"), orderbook/features in the cell (`FeatureEngine` built in `qip-edge-node/src/main.rs:190`). |
| Anomaly Detection | BUILT+WIRED | `DetectorRegistry::scan` runs every cycle in `stage_discover` (platform.rs — `self.opportunities.scan(&detection, ...)`); 12 anomaly kinds (`qip-opportunity-engine/src/detector.rs`). Runs in each brain node's loop, not per-edge-cell. |
| Liquidity & Impact | PARTIAL | Square-root impact + transaction cost models wired (`qip-financial/src/costs.rs`, used by twin and constructor). The venue-level liquidity map is unwired — see Layer 3 Liquidity Topology. |
| Local Alpha & Arbitrage | BUILT, dormant | `qip-arbitrage` (2-leg, triangular, N-leg: `scan.rs`, `netedge.rs`) reachable through the cell seam (`qip-edge/src/seam.rs:14`) — but no strategy is ever deployed into a running cell, so the path executes for no one. |
| Cash & Inventory | PARTIAL | Capital envelopes + utilisation wired end-to-end on the central side (`CentralPlane` envelopes/recalls, `/capital` route, routes.rs:997-1063); per-location balances exist only as planning types (`qip-capital-fabric`). No cell inventory module (zero "inventory" hits in `qip-edge/src`). |
| Risk & Limits | BUILT+WIRED | `PreTradeChecker` in OMS submit path; cell autonomy ceiling gates live venues (`qip-edge-node/src/main.rs:200-208`); `LimitSet` with expected-shortfall limits (`qip-risk/src/limits.rs:97,234,499`). |
| Ultra-Fast Decisioning | BUILT+WIRED | fastbrain refuses to host any agent that could call a language model or exceed 50ms (`qip-fastbrain/src/roster.rs` — `FAST_PATH_AGENTS`, `MAXIMUM_BUDGET = 50ms`, checked before assembly). |
| "Rust Engine + Local Models" | PARTIAL | Rust engine yes. "Local models": `DistilledModel` is the only learned function the hot path may run and distillation is real (`qip-training/src/distill.rs` — fidelity measured before promotion) — but nothing produces-and-ships one to a cell (see "Deploy to All"). The wired language model is the deterministic stand-in (`qip-ai/src/language.rs`); the real LLM adapter is a PORT (language.rs:467-521: no TLS-capable client in this build, reports unavailable). |
| Publish Regional State Deltas etc. | PARTIAL | Uplink publishes deltas with breaker/retry/dead-letters (`qip-edge-node/src/mesh.rs`; `qip-edge/src/mesh.rs:315 published`); no deployed receiver (see backbone row). |

## Layer 3 — Global Opportunity Brain

| Element | Verdict | Evidence |
|---|---|---|
| Global Knowledge Graph | BUILT-UNWIRED (constructed, never written) | `WorldModel { graph: KnowledgeGraph, causal: CausalGraph, features, resolver, index }` (`qip-world-model/src/world.rs:39-47`) is built into the agents' Desk (platform.rs:412-439). But `Platform::observe` keeps only bar close/volume as `Vec<f64>` and discards News/Fundamental/Macro/CorpAction/Alt/Reference via `_ =>` (platform.rs:744-764). The acceptance suite itself documents it: "**The platform's world model is never written**… there is no `&mut` accessor to it from anywhere" (`qip-acceptance/tests/e2e_live.rs:71-78`). Agents reason over an empty graph. |
| Cross-Market Correlation | PARTIAL | `CorrelationBreakdown` detector wired via scan; factor covariance in `qip-risk/src/factor.rs:165`. No cross-market graph is populated (graph unfed). |
| Multi-Leg Arbitrage Engine (2/3/N-leg) | PARTIAL | Engine BUILT (`qip-arbitrage/src/scan.rs`, netedge) and reachable from the cell seam (dormant, no deployed strategy). Centrally: the API's `/paths` route answers `NO_ARBITRAGE_ENGINE` — "no arbitrage engine is wired into this [process]" (`qip-api/src/missing.rs:34`, routes.rs:386). |
| Event Impact & Catalyst Detection | BUILT+WIRED | `qip-opportunity-engine/src/catalyst.rs` (`KnownEvents` gate, detector.rs:139 `catalyst: Option<CatalystLink>`); engine links anomalies to catalysts (`engine.rs:212`); kernel maps `AnomalyKind::Catalyst` to a repricing hypothesis (`platform.rs mechanism_for`). |
| Structural Mispricing Detection | PARTIAL | `StructuralBreak`/`PriceMove` detectors wired; hypothesis formation is a single-mechanism sentiment claim (`mechanism_for`), not a structural-pricing model. |
| Strategy Discovery (statistical + AI) | BUILT-UNWIRED | Full pipeline exists — `qip-evolution/src/{grammar,generate,mutate,scoring,promotion,challenger,discovery}.rs` composed by `StrategyFoundry` (`qip-kernel/src/central/foundry.rs:96`) — but `StrategyFoundry::new` is called only in `qip-kernel/tests/foundry.rs:65`. The "AI" half is the deterministic model; the hosted LLM adapter is a PORT (no TLS transport, `qip-ai/src/language.rs:467-521`). |
| Liquidity Topology Understanding | BUILT-UNWIRED | `qip-world-model/src/liquidity.rs` — bitemporal per-instrument venue-depth map, usable-vs-total depth, staleness honesty. `LiquidityTopology` has **zero consumers** outside its own crate (grep across workspace) and is not a `WorldModel` field. |
| Regime & Cycle Detection | PARTIAL | `AnomalyKind::RegimeChange` detector wired; regime scenarios in `qip-simulation-engine/src/{conditions,scenario}.rs` (e.g. the 2022 rates scenario, scenario.rs:165). No standing regime model feeding decisions. |
| Global View | PARTIAL | `stage_understand` reports coverage + chain view; `CentralPlane` aggregates cell books into `AggregateExposure` (`qip-capital/src/exposure.rs:126`) — but no cell report can arrive (backbone row), so the aggregate is empty in every deployment. |
| Relationship Discovery | BUILT-UNWIRED | `qip-world-model/src/relationship.rs` + `WorldModel::relate` (world.rs:165) exist; the graph is never fed. Causal propagation is wired into agent reasoning (`qip-investment-agents/src/reasoning.rs:80 causal.propagate`) — over an empty causal graph. |
| Opportunity Graph Search | BUILT-UNWIRED | `SearchIndex` with `HashingEmbedder` inside WorldModel (world.rs:44,63) and `WorldModel::retrieve` (world.rs:446) — zero callers of `.retrieve(` anywhere outside the crate. |
| Real-Time Scoring | BUILT+WIRED | Cost-adjusted score orders the opportunity queue (`qip-opportunity-engine/src/opportunity.rs:68-89 — score / cost.sqrt()`); scan + retain-by-TTL each cycle in `stage_discover`. |

## Layer 4 — Capital & Strategy Brain

| Element | Verdict | Evidence |
|---|---|---|
| Opportunity Scoring | BUILT+WIRED | As above. |
| Expected Alpha Calculation | PARTIAL | `NetEdge` with nine named deductions is real (`qip-contracts/src/edge.rs`; `qip-arbitrage/src/netedge.rs` — latency-unhedged window etc.); the kernel computes and hands back its own two deductions (compute + data) per cycle (`platform.rs:2158-2168 cost_deductions`). The full nine-deduction calculation runs only in the (dormant) cell arbitrage path. |
| Position Sizing & Capital Allocation | PARTIAL | Sizing exists — `PortfolioConstructor::construct` (`qip-portfolio-engine/src/construction.rs:153`) — but the loop only ever calls `nothing_to_do` (platform.rs:1324). Allocation is wired centrally: `CapitalAllocator` + envelope issuance/recalls in `CentralPlane` (plane.rs:264-293). |
| Cash/Collateral/Margin Management | PARTIAL | Demand kinds Cash/Collateral/FxFunding/Inventory forecast from realized fills (`qip-capital-fabric/src/forecast.rs:50-58`; recorded in the fill loop, platform.rs:1635). Transfers are planned, never executed (`transfer.rs` is a plan type). **No margin model anywhere.** |
| FX & Multi-Currency Exposure Mgmt | PARTIAL | `FxRates` conversions in capital-fabric plans (plan.rs, transfer.rs; used by `pre_position`, platform.rs:2100); currency-bucket concentration checks (`qip-capital/src/exposure.rs:811`). But the composed portfolio is single-currency USD (platform.rs:423), there is no FX rate source, and no exposure-hedging action exists. |
| Risk Engine Real-Time VaR/CVaR | PARTIAL | VaR/CVaR math is real (`qip-risk/src/metrics.rs:24 historical_var, :37 parametric_var`; expected-shortfall limits, limits.rs:97,234,499) and the `RiskMonitor` runs every cycle (`stage_act`, platform.rs:1375-1380). **But the state it watches is a constant**: `Platform::risk_state()` returns hardcoded equity/cash 10M with defaults (platform.rs:1520-1526). Real-time in cadence, not in content. |
| Hedge & Inventory Optimization | ABSENT | No hedging engine or inventory optimizer exists. Grep yields only doc comments (`qip-arbitrage/src/netedge.rs:20` "unhedged between the first leg and the last"), a scenario description, and the `/paths` route *summary string* promising "hedge state" (routes.rs:195) for a handler that answers `NO_ARBITRAGE_ENGINE`. |
| Trade/No-Trade Decision | PARTIAL | The refusal machinery is genuinely wired (OMS `RefusalReason` gates: kill-switch, autonomy, venue, pre-trade risk — platform.rs:2180-2196 `gate_of`), and refusals are captured on the hash chain. The *trade* half never fires: decide is unconditionally no-trade (platform.rs:1321-1330). |
| Actions (size, venues, TTL, …) | PARTIAL | Opportunity TTL wired (`queue.retain(|o| o.is_live(now))`, stage_discover); envelope expiry + recall backstops wired (routes.rs:1032-1052). Sizing/venue/hedge actions are never generated (no trade path). |
| "Return − Costs − Risk − Slippage − Latency-Decay … = Expected Usable Alpha" | PARTIAL | Exactly the NetEdge nine-deduction contract (`qip-contracts/src/edge.rs`; kernel doc: "two of NetEdge's nine… the other seven are the market's", platform.rs:2158-2164). Computed in full only on the dormant cell path. |

## Layer 5 — Regional Execution Mesh (×7)

| Element | Verdict | Evidence |
|---|---|---|
| Execution Engines | BUILT+WIRED (paper); PORT (live) | OMS + `SimulatedBroker` in the kernel; cell + `SimulatedGateway`/`RestGateway` in `qip-edge-node/src/gateway.rs`. Live order entry (`qip-brokers/src/rest.rs RestOrderEntryAdapter`, opens a socket) is behind the named acknowledgement env (`venue.rs ACKNOWLEDGEMENT_VARIABLE`) — and can only speak plain HTTP (no TLS in the build). |
| Smart Order Routing | BUILT-UNWIRED | `qip-routing/src/router.rs` — marginal-cost slice-walking across venues with conservation proofs (RouteSlice, exclusions-with-reasons). No consumer: the only outside uses of `qip_routing` are `GatewayCredential` reuse in `qip-brokers/src/{credential,connection}.rs`; no caller constructs a `RoutingRequest` outside the crate's tests. |
| Venue Selection | PARTIAL | Deployed selection is static config: `VenueChoice::from_env` picks one adapter for the *first* venue in `QIP_VENUES` (`qip-edge-node/src/main.rs:203-213`). Dynamic selection/health scoring exists in `qip-routing/src/{venue,health}.rs` — unwired. |
| Smart Order Slicing | BUILT-UNWIRED | `RouterSettings::slices` (default 8), `RouteSlice`, `children.rs` child-order machinery — same unwired crate. |
| Fill Optimization | PARTIAL | Price-time matching, lot/tick admission, commissions and rejection draws in the simulated venue (`qip-brokers/src/{exchange,matching}.rs`, wired via gateway). Optimization across venues is the unwired router. |
| Hedging | ABSENT | See Layer 4. |
| Inventory | ABSENT (cell level) | No inventory code in `qip-edge/src`; only the capital-fabric forecast *kind*. |
| Failover | PARTIAL | Feed A/B arbitration + gap handling BUILT in `qip-sequencing` and the cell tracks sequence gaps (`qip-edge/src/cell.rs:844 gap_detail`); transport has breaker/retry/spool (`qip-transport`, `qip-mesh/src/spine.rs` spooled capital path). Venue failover (a second gateway) does not exist — one gateway per node. |
| Pre-trade risk checks | BUILT+WIRED | `PreTradeChecker` inside `OrderManager::submit`; every refusal named and counted (platform.rs:2180-2196). |
| Partial fill handling | BUILT+WIRED (sim) | `ExecutionProfile.partial_fills` + `remaining_quantity` fill loop (`qip-execution-engine/src/broker.rs:32-33, 261-282`); drop-copy reconciliation in the cell (`qip-edge/src/dropcopy.rs`, gateway drains fills on the independent channel). |
| Dynamic repricing | ABSENT | No cancel/replace, amend, or requote logic anywhere in `qip-edge`, `qip-routing`, or `qip-execution-engine`. The only "requote" is a *cost assumption* in venue-health scoring (`qip-routing/src/health.rs:40 requote_cost_bps_f64`). |
| "Microsecond execution" | PARTIAL / honest-but-narrower | Per-operation microsecond ceilings are asserted (`qip-acceptance/tests/performance.rs:94-108`), and the file itself says "None of this is end-to-end latency… wire-to-wire, tick-to-order" are not measured. |
| Resilient & redundant | PARTIAL | Cell keeps trading inside its envelope through a partition (ADR 0008; `qip-edge-node/src/mesh.rs` "A cell with no peer still runs"); journal restore on restart (deepbrain node.rs:154); k8s PDB + autoscaler for api (`infrastructure/kubernetes/base/api.yaml`), StatefulSet ×2 for edge cells. |

## Layer 6 — Evolution Brain

| Element | Verdict | Evidence |
|---|---|---|
| Counterfactual Simulation | PARTIAL | Capture side wired: every fill and every refusal lands on `OutcomeCapture`'s hash chain (submit path, platform.rs:1533-1650; risk refusals in stage_act:1392-1407). The counterfactual side — `CounterfactualEngine`, `Platform::evaluate_alternatives` (platform.rs:1972-2045), `TwinMarket` as-of replay (`qip-twin/src/asof.rs`) — has no callers outside `qip-kernel/tests/platform_outcomes.rs`. |
| Model Training (Vertex AI) | DIVERGED (ADR 0011) + PORT + BUILT-UNWIRED | ADR 0011: Vertex → in-tree `qip-training` (ridge, boosted stumps, distillation). The Vertex adapter remains a PORT (`qip-training/src/vertex.rs:40-65, 227-245` — refuses without token; needs a TLS proxy). Nothing runs training: `central/models.rs register_fit` is called only in `qip-kernel/tests/models.rs:19`; API `/runs` answers `NO_TRAINING_SERVICE` (missing.rs:46). |
| Strategy Engine | BUILT+WIRED (engine), dormant (content) | `qip-strategy` typed IR with bounded runtime, compiled into `Cell::deploy` — which nothing calls in a deployed binary. |
| Backtest Engine | PARTIAL | As-of twin replay (`qip-twin/src/asof.rs`), scenario library (`qip-simulation-engine/src/scenario.rs`), evolution challenger/holdouts (`qip-evolution/src/challenger.rs`, foundry `HoldoutInputs`) — all reachable only from tests (foundry unwired). No standalone backtest binary/route. |
| Model Validation & Backtesting | PARTIAL | Distillation fidelity is enforced by construction (`qip-training/src/distill.rs` — `approved_student` re-checks a `FidelityPolicy` on every call); lifecycle gates require scenario evidence (`qip-lifecycle/src/gates.rs` uses the simulation engine). Same unwired drivers. |
| Deploy to All (small models) | ABSENT (as a path) | The pieces exist — `DistilledModel`, `StrategyDna` sealing (`central/dna.rs:136`), `Cell::deploy` — but there is **no distribution channel**: the mesh downlink carries capital envelopes only (`qip-edge-node/src/mesh.rs` — "an uplink that publishes state deltas and a downlink that pulls signed capital envelopes"; no DNA/strategy frame in `qip-mesh/src/spine.rs`), and `Cell::deploy` is test-only. |
| IBM Quantum Optimization | PORT (+ simulated wired behind a default-off flag) | `qip-quantum/src/provider.rs` hosted adapter: refuses without token, requires a TLS-terminating egress proxy ("qip_transport::http has no TLS stack… IBM Quantum is https only", provider.rs:71-77), never falls back to the simulator. `IbmQuantumSolver` reports unavailable (solver.rs:24). `SimulatedProvider` attaches to the compute router only when `quantum_enabled` (default `false`, `qip-kernel/src/config.rs:157,237`). API `/jobs`: `NO_QUANTUM_JOBS` (missing.rs:51). Per ADR 0011, IBM Quantum is *the only permitted external integration* — and the build's own transport cannot reach it without a proxy. |
| Hard problems (portfolio opt, N-leg search, allocation, scenario search) | PARTIAL | `qip-optimization-engine` solves with a classical baseline always (ADR 0006; `router.rs:290 solve`), wired into `PortfolioConstructor` (platform.rs:446-448,521) — which only ever emits nothing-to-do. N-leg search lives in the dormant arbitrage crate; scenario search in the unwired foundry. |

## Outcome Telemetry

| Element | Verdict | Evidence |
|---|---|---|
| Fills & P&L | PARTIAL | Fill capture with hash chain wired in the submit path (Action::Filled + realised outcome, platform.rs:1615-1650) — but the only callers of that path are tests, and deployed cells have no strategies to generate fills. Attribution in `stage_learn` feeds the real `Attributor` with **degenerate periods** — zero spread cost, zero impact, zero realised P&L, no hypotheses (platform.rs:1466-1495). |
| Slippage & Costs | PARTIAL | Signed slippage-bps computed per fill and stored on the outcome (platform.rs:1629-1631, 2228-2243); costs via `TransactionCostModel`. Same reachability caveat. |
| Partial Fills & Rejects | PARTIAL | Refusals are captured and countable by gate (`gate_of`, platform.rs:2180); rejection draws in the sim venue; partial fills tracked via `remaining_quantity`. No dedicated reject/partial telemetry series. |
| Missed Opportunities | BUILT-UNWIRED | `Action::MissedOpportunity { would_have_earned, … }` exists with the right semantics (`qip-twin/src/capture.rs:83-186` — "a refusal is an outcome"); its only constructor call is `qip-acceptance/tests/e2e.rs:814`. Expired queue opportunities are *counted* in stage_discover but never captured as misses. |
| Market Impact | PARTIAL | Impact is *modeled* (square-root law in `qip-financial/src/costs.rs`; twin `impact_window` for counterfactual pricing, asof.rs:57-77). **Realized** impact is never measured against the model. |
| Strategy Attribution | PARTIAL | The exact-attribution machinery exists (`qip-learning-engine/src/attribution.rs`, ADR 0007; `qip-evolution/src/attribution.rs`) and runs each cycle — on stub inputs (above). The API refuses: `/attribution` answers `NO_ATTRIBUTION` (missing.rs:65, routes.rs:395). |
| Exposures Over Time | ABSENT | `AggregateExposure` is point-in-time from cell reports (`qip-capital/src/exposure.rs`, no history/snapshot series); no exposure time-series is kept anywhere. |
| Risk & Limit Utilization | PARTIAL | `LimitSet` checks + `RiskMonitor` run every cycle — against the hardcoded `risk_state()` (platform.rs:1520-1526). API `/limits` answers `NO_LIMIT_UTILISATION` (missing.rs:72, routes.rs:1156). |

## Cross-Cutting Fabrics

| Fabric | Verdict | Evidence |
|---|---|---|
| A. Counterfactual Digital Twin | PARTIAL | Decision/refusal capture on a verifiable hash chain: wired. "Simulate every decision and alternative": `evaluate_alternatives` + `CounterfactualEngine` + `TwinMarket` are invoked by kernel tests only (platform.rs:1983; sole caller `tests/platform_outcomes.rs`). |
| B. Contextual Model Router | PARTIAL — **metering wired, routing unwired** | Metering half real and running: `CostEngine`/`ComputeLedger` charge every cycle per intelligence tier (platform.rs:843-854, 876-913 `charge_cycle`; `compute_spend`, `cost_deductions`). The actual contextual router — `Router::select` over `DecisionContext` with the Determinism type-gate ("a decision requiring determinism can never route above the deterministic rung"), value-based rung refusal, escalation, region-scoped reputation (`qip-cost-router/src/{router,context,reputation}.rs`) — has **zero callers outside its crate**: the kernel imports only `{ComputeLedger, CostEngine, DataCostModel, DataReads, IntelligenceTier}` (platform.rs:62). Nothing anywhere selects a model per context at runtime. |
| C. Predictive Capital Fabric | PARTIAL — forecasting wired, pre-positioning unwired | Wired in-cycle: demand recorded from every fill (platform.rs:1635 `record_capital_demand` — "the only source of it the loop actually sees") and forecast lanes reported in stage_decide (platform.rs:1330-1344). Unwired: the actual pre-positioning plan (`pre_position`, `evaluate_pre_positioning`, platform.rs:2092-2135; `PrePositioningPlanner` field at :173) is called only by `tests/platform_outcomes.rs:397-422`; no transfer is ever planned-and-executed; `qip-capital-fabric/src/transfer.rs` is a plan type with no executor. |
| D. Confidential Global Intelligence | BUILT-UNWIRED | `qip-confidential` (differential-privacy noise, query gating, release budget: `{noise,query,release,budget,contribution}.rs`) is consumed by `CellInsights` (`qip-kernel/src/central/insights.rs:47-58`), which is constructed as a Platform field (platform.rs:150, 510) — and then **nothing ever calls it**: `insights_mut` (platform.rs:591) has zero callers in any binary or route; only `qip-kernel/tests/insights.rs`. |
| E. Quantum-Centric Learning Fabric | PARTIAL/PORT | Simulated quantum solver wired into the compute router behind `quantum_enabled` (default false); hosted IBM path is a PORT (see Layer 6); nothing connects quantum output to strategy discovery at runtime (foundry unwired). |

## Governance & Guardrails

| Element | Verdict | Evidence |
|---|---|---|
| Global Risk Policies | BUILT+WIRED | `LimitSet` on Platform + desk `RiskView` (platform.rs:428-435); concentration limits in CentralPlane (plane.rs:279). |
| Regulatory Controls / Compliance Rules | PARTIAL | `qip-compliance` wired inside CentralPlane (`compliance: CompliancePlane`, plane.rs:268; incident model with the false-stop asymmetry, `qip-compliance/src/incident.rs`). `Platform::compliance_report` (platform.rs:606) has **no callers** — no route or binary surfaces it. |
| Audit Trail (immutable) | BUILT+WIRED | Hash-chained event log with `verify_chain` (`qip-events/src/log.rs:4-6,23`); hash-chained outcome capture; journal read back on restart so "the chain continues from the last record on disk" (platform.rs:404-411). |
| Kill Switch | BUILT+WIRED | `POST /kill-switch` (routes.rs:427); `RiskMonitor::enforce` trips it in stage_act (platform.rs:1379-1380); OMS refuses `Halted`; a cell reconciliation break trips the *platform's* switch, scoped to the cell (platform.rs:610-631). |
| Model Governance | PARTIAL | Gate ladder wired (`qip-contracts/src/gate.rs GateStage::next` "refuses a capital-holding rung" skip; `central/factory.rs` in CentralPlane; `/strategies` route reads it, routes.rs:945). Model *registry* is test-only (`central/models.rs`; API `/models` answers `NO_MODEL_REGISTRY`, missing.rs:58). |
| Data Lineage | BUILT+WIRED | `Lineage`/`CorrelationId` threaded through every cycle and every capture (platform.rs:56-58, run_cycle); provenance types in `qip-streaming/src/provenance.rs`; catalog lineage in `qip-mesh`. |
| Stress Testing | PARTIAL | Scenario library incl. 2022 joint equity-bond drawdown (`qip-simulation-engine/src/scenario.rs:165`); lifecycle promotion gates consume scenario evidence (`qip-lifecycle/src/gates.rs`); a heavy acceptance stress suite exists (`qip-acceptance/tests/stress.rs`). Runtime driver absent: `learn_from_cells` (the edge that would trigger gate re-evaluation, platform.rs:639-645) has no callers. |
| Capital & Liquidity Limits | BUILT+WIRED | Total/per-strategy/per-cell/per-venue budgets + signed envelopes with expiry + recall register with backstop expiry ("an expired envelope admits nothing whatever the cell does"), all served at `/capital` (routes.rs:997-1063). |

## Non-Functional

| Element | Verdict | Evidence |
|---|---|---|
| Ultra-Low Latency | PARTIAL | µs/op ceilings asserted, explicitly not wire-to-wire (`performance.rs:23-28,94-108`); 50ms fast-path agent budget enforced (fastbrain roster). |
| Massive Scalability | UNVERIFIED | No load evidence beyond the stress suite; single-process Platform per node. |
| High Availability | PARTIAL | PDB + autoscaler for api (`base/api.yaml:196-199`); edge-cell StatefulSet replicas 2; cells survive partition inside envelopes. |
| Auto-Scaling | PARTIAL | HPA for api only (`base/api.yaml` — "The floor lives in `minReplicas`"). |
| Disaster Recovery | PARTIAL | Durable journal + restore-on-start (deepbrain node.rs:154; `journal-storage.yaml`); no cross-region story. |
| Multi-Region Active | ABSENT | One `base/` kustomization, no overlays; `HOME_REGION = "home"`. |
| Observability | PARTIAL | `qip-observability` (metrics/logs/trace/slo) BUILT; `/health` `/metrics` routes wired (routes.rs:361,374); but the kernel loop records almost nothing into it — no `.counter(`/`.observe(` calls on Telemetry in `qip-kernel/src` or the apps (grep). |
| Cost Efficiency | BUILT+WIRED | Per-cycle compute ledger, monotone `compute_spend`, cost-per-opportunity deductions (platform.rs:2138-2168). |

## Technology Stack (Google Cloud)

DIVERGED wholesale by ADR 0011 ("Everything in Rust on Kubernetes; IBM Quantum is the only integration") — its own
table maps: Pub/Sub → `qip-transport` mesh; Spanner → `qip-storage` embedded engine + WAL; BigQuery → the
hash-chained journal ("the weakest substitution", ADR's words); Vertex → `qip-training`; Dataflow → `qip-streaming`;
GKE unchanged. "Google Cloud remains a *host*… Nothing in the running platform calls a Google API."

Residual in-tree GCP adapters are PORTS: `qip-storage/src/gcp/{bigquery,storage,auth}.rs` refuse
(bigquery.rs:109,510-511); `provider.rs:30,112` names Spanner as a provider kind that refuses; the same file warns
Redis transit is plaintext (provider.rs:271-273). Security Center / Confidential VMs: no code (host-level concern per
ADR; terraform out of audit scope). Dataflow/Cloud Storage: covered by the divergence.

## Deployment Model

| Element | Verdict | Evidence |
|---|---|---|
| Multi-Region Active-Active | ABSENT | See Non-Functional. |
| Colocated Execution / Edge / Private Links | ASPIRATIONAL | ADR 0011 itself notes Chicago/NY cells sit 400km/330km from their venues on GCP; edge-node is *built* to run venue-adjacent but no colocation exists. |
| Environments DEV/TEST/STAGE/PROD | BUILT | `infrastructure/environments/{dev,test,stage,prod}/terraform.tfvars` — all four, ramped 0/1/3/1 cells. Never applied. |
| Zero Trust | PARTIAL | Real pieces: constant-time token auth with 5-role RBAC (`qip-api/src/auth.rs` — Monitor/Viewer/Analyst/Approver/Operator); capital envelopes verified by signature at the cell, never trusted for arriving over the mesh (`qip-edge-node/src/mesh.rs:20-23`); LLM numeric-leaf guard keeps models out of arithmetic (`qip-ai/src/language.rs:284`). Fatal caveats: **no TLS anywhere**, and the central signing secret is derived from the config seed — "reproducible and therefore useless as a production secret" (platform.rs:2233-2247), with `key_is_reproducible` recorded (plane.rs:272-277). |
| Data Encryption in transit | ABSENT (in the app) | `qip-transport/src/http.rs:26-28,213`: "this build has no TLS stack", `https` refused by name; Redis AUTH "sent in the clear" (`qip-storage/src/redis.rs:48-54`). |
| Data Encryption at rest | ABSENT (in the app) | `qip-storage/src/engine/mod.rs:113`: "**No encryption at rest and no access control.** File permissions are…". (KMS-level encryption would live in terraform — out of audit scope, and irrelevant to the app-level claim.) |
| IAM/RBAC | BUILT+WIRED | auth.rs roles enforced per route (Operator required for kill-switch/autonomy). |
| Compliance Frameworks | PARTIAL | See Governance. |

---

# Ranked: what the diagram most overstates

Ranked by the gap between what the box promises and what runs — not by effort to fix.

1. **The system cannot trade.** Layer 4's "Trade/No-Trade Decision" and all of Layer 5's execution promises sit on
   three dead ends: `stage_decide` is hardwired to nothing-to-do (platform.rs:1321-1330), `submit_order` is
   test-only, and no deployed cell ever receives a strategy (`Cell::deploy` test-only; edge-node deploys none).
   Every downstream box — fills, P&L, slippage, attribution, capital demand learning — starves on this.
2. **"Global Knowledge Graph" is constructed and never written.** All of Layer 3's graph-shaped boxes (knowledge
   graph, relationship discovery, opportunity graph search, liquidity topology) exist as tested code that the
   composed loop feeds nothing: `observe` discards every non-bar record, and the acceptance suite itself certifies
   "the platform's world model is never written" (e2e_live.rs:71-78). The agents' reasoning runs against an empty world.
3. **The mesh backbone is half a bridge.** Cells publish state deltas and pull envelopes (wired), but the central
   receiving end (`CellDeltaReceiver`/`CapitalDispatcher`) runs in no binary — only in an acceptance test. The
   diagram's whole Layer-2→Layer-3 "publish regional state" arrow, the global exposure aggregate, and the
   capital-envelope distribution channel all terminate in a peer that does not exist in any deployment.
4. **Fabric B "Contextual Model Router" routes nothing.** The genuinely elegant router (determinism type-gate,
   value-priced rungs, escalation, regional reputation) has zero callers; what is wired is only the cost *metering*.
   No decision in the platform is ever assigned a model tier by context.
5. **Layer 6 "Evolution Brain" never turns.** Strategy foundry, evolution pipeline, training, model registry,
   distillation, promotion gates, counterfactual evaluation — every driver is a test. The API says so itself:
   `/runs`, `/models`, `/attribution`, `/jobs`, `/paths`, `/sources` all answer "not wired into this process"
   (`qip-api/src/missing.rs`).
6. **"Deploy to All (small models)" has no channel.** Distillation is real and fidelity-gated, DNA sealing exists,
   but the mesh carries only capital envelopes — there is no path by which any model or strategy reaches a running cell.
7. **"Real-Time VaR/CVaR" watches a constant.** The monitor runs each cycle, the VaR math is correct, and the input
   is hardcoded `equity: 10M, cash: 10M` (platform.rs:1520-1526).
8. **Hedge & Inventory Optimization / Dynamic Repricing are pure diagram.** No hedging engine, no inventory
   optimizer, no cancel/replace or requote logic exists at all — the closest artifacts are a route summary string
   and a requote *cost assumption*.
9. **Fabric C is "predictive" only up to the plan.** Demand recording and forecasting are wired in-cycle;
   the pre-positioning plan is test-only and nothing can execute a transfer.
10. **Fabric D shares nothing.** The differential-privacy machinery is real and sits on the Platform as a field
    no code ever calls.
11. **All "AI" is a deterministic stand-in.** The LLM adapter, Vertex, IBM Quantum, Pub/Sub, Spanner, BigQuery are
    ports; ADR 0011 makes IBM Quantum the *only* permitted integration — and the build's own transport cannot reach
    it (https refused; needs a TLS proxy that is a named, unmet production requirement).
12. **"×7 regions / Multi-Region Active" is one region named "home".** One kustomize base, no overlays,
    `HOME_REGION = "home"`.
13. **Layer 1's breadth is type-deep.** Every data class has a type and a synthetic generator; the only continuously
    collected data in any deployment is synthetic or replayed. The one live market-data adapter is test-only.
    NFTs/wallets (named on the diagram) have zero code.
14. **Outcome Telemetry, box by box, is mostly capture-machinery without producers.** Missed-opportunity capture is
    test-only; exposures-over-time has no history store; realized market impact is never measured; attribution runs
    on zeroed inputs.

# The inverse gap — what the diagram does NOT show but should worry an owner

1. **No TLS anywhere, by policy.** ADR 0009/0011's two-dependency rule leaves the workspace with no TLS stack and no
   crypto beyond SHA-256 (`qip-transport/src/http.rs:213`; `qip-quantum/src/provider.rs:60-63` "there is no crypto in
   this workspace to sign with"). Consequences the diagram's "Zero Trust / Encryption in transit & at rest" boxes
   invert: every live adapter (market data, order entry, IBM, GCP) is plain-HTTP-plus-external-proxy; Redis AUTH
   travels in the clear; storage is unencrypted at rest by its own doc. The security posture is *honest* — every
   gap is named in code — but it is the opposite of the diagram.
2. **The capital-envelope trust root is a seed-derived key.** `central_signing_secret` is reproducible from
   configuration — "anyone who knows the seed can mint an envelope" (platform.rs:2233-2247). `set_central` exists to
   override it, but nothing in any binary does, and signing is HMAC-by-hand (no asymmetric signing exists —
   named in the same comment).
3. **A live cell trades blind against its center.** By design (ADR 0008) a partitioned cell keeps trading inside its
   envelope — but since no central receiver is deployed at all, *every* cell is permanently partitioned: recalls,
   kill-switch propagation to cells, and the reconciliation-break-trips-the-switch path (platform.rs:610-631) can
   never fire in production topology. The safety design assumes a center that ships in nothing.
4. **The fastbrain/deepbrain nodes and the edge cells are two disjoint systems.** The Platform loop (sense→learn)
   and the cell hot path share no runtime data path in either direction: no market data flows loop→cell, no fills
   flow cell→loop. The diagram draws one system; the tree contains two, connected only in an acceptance test.
5. **`f64` histories underlie the loop's statistics.** `observe` collapses every bar to `close.to_f64()` and discards
   both bitemporal instants — the exact look-ahead discipline the world-model and alt-data modules were built to
   preserve stops "at the platform's front door" (e2e_live.rs:76-78, the suite's own words).
6. **Observability is a shell at the center.** The Telemetry registry is constructed in every binary and essentially
   never written to from the kernel loop — `/metrics` serves a surface with almost no instrumentation behind it,
   while the API's honest `missing.rs` catalogue (12 named absences) is the real status page.
7. **The API's `/paths` summary promises "hedge state" for a handler that refuses.** Route table descriptions
   (routes.rs:195,202) advertise slightly more than handlers deliver; an owner reading the OpenAPI surface would
   over-count capabilities.
8. **Single venue per cell, first-of-list.** `QIP_VENUES` accepts many venues but the gateway opens only the first
   (`main.rs:203-207`) — multi-venue execution, the premise of the routing crate, is not reachable even if the
   router were wired.
9. **Environment naming gap.** DEV/STAGE/PROD exist; the diagram's TEST environment does not
   (`infrastructure/environments/`).
10. **Known-stale prior audit.** `docs/architecture/current-state-audit.md` sections 1-5 self-declare they were not
    re-measured; several of its "Built" rows (e.g. normalization "Built") are, at the composition level, unwired
    today. Anyone planning from that document will over-trust Layer 1.

---

## Corrections and additions at adoption (same day)

Three items above shifted between the audit's reads and this file landing, all
in the direction of narrowing a gap rather than widening one:

* **Fabric D** — `CellInsights` gained its first real consumer path the same
  day: `Platform::insights_mut` with tests driving real cell reports through
  `ingest_cell_report`. The accessor still has no caller inside a serving
  binary, so the row moves from "zero callers" to "wired to the platform,
  unconsumed by the console".
* **The centre's mesh half** now runs in one binary: `qip demo --live`, a
  bounded loopback demonstration. No *serving* binary drains a cell inbox or
  dispatches capital, so the ranked finding stands with that word added.
* **`stage_decide`** is worse than ranked: it does not check for approved
  theses and find none — it **unconditionally** constructs the empty proposal.
  No thesis, however good, can currently become a trade through the cycle.
