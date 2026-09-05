# Blueprint v10.1 gap map

> **Superseded for status by [`PROJECT-PLAN.md`](PROJECT-PLAN.md).** This
> document's structural inventory is kept as history.

A first-pass structural inventory: for each capability the "Algorik —
Cognitive Investment Platform, Blueprint v10.1" document names, does anything
resembling it exist in the actual tree, and if so, does a real caller reach
it? This is **not** a judgment of whether what exists is correct, complete, or
good — only whether code exists and whether it is called from somewhere that
runs.

## Provenance and scope

The blueprint was supplied as a single HTML file
(`/root/.claude/uploads/483b7741-5097-5e6c-93a8-eee0877fec38/7dd87a9f-index.html`,
1,627 lines) and is copied unmodified into this worktree at
`docs/plan/blueprint-v10.1.html` for reference. It was read in full, section by
section (lines 203–990, 1004–1330, 1499–1563 read verbatim; the section
headers for every remaining `<h2>`/`<h3>` were enumerated by grep and cross-
checked against the read passages) covering all eleven tabs: Overview,
Cognition, Valuation & assets, Intelligence & quantum, Capital & money,
Execution, Software, Cloud & network, Web & mobile, Experience, and Roadmap &
evidence.

**This document does not exist in a vacuum.** `docs/adr/0022-the-algorik-
blueprint-is-the-architecture-of-record.md` already names this same blueprint
(v10.1-4) as the architecture of record, and
`docs/architecture/algorik-blueprint-traceability.md` (624 lines) is an
existing, actively maintained scorecard against it, referenced from
`docs/plan/completion-plan.md`. That contradicts the framing this task was
given — that current plan documents "do not mention this v10.1 document at
all" — and the assigning agent should know that before treating this file as
the first word on the subject. This document was nonetheless produced by
independent grep/read verification against the tree as it stands today,
without reading or relying on the existing traceability matrix's conclusions,
per the task's instruction to treat this as a fresh mapping. Where this
document's findings and that matrix's findings might disagree, neither should
be assumed correct without checking the citation.

**Coverage is a representative sample, not an exhaustive enumeration.** The
blueprint names on the order of 70 services and dozens of named types across
34 "bounded domains" in one section alone. This pass verified roughly four
dozen of the most load-bearing, specifically-named mechanisms across every
plane — the ones the document's own prose treats as its central claims — by
grep and direct file reads against `backend/crates/**` and `frontend/**`, and
states plainly which sub-items within a plane were **not** individually
checked. A capability not listed below was not evaluated in this pass and
should not be assumed either present or absent from this document's silence.

Every row cites the grep or read that produced it. Where a search returned
nothing, multiple plausible names were tried before the row was marked
ABSENT, and the terms tried are named in the evidence column.

---

## Cognition plane

| Capability | Status | Evidence |
|---|---|---|
| World model / entity graph (`Entity`, `EntityRelation`, `WorldEvent`) | BUILT+WIRED | `qip-world-model` crate exists (`backend/crates/services/qip-world-model/src/world.rs`); `qip_world_model::WorldModel`, `::graph::{Node,NodeKind}`, `::liquidity::LiquidityTopology` imported and used in `backend/crates/runtime/qip-kernel/src/platform.rs:135-138` |
| Entity resolution | BUILT+WIRED | `backend/crates/services/qip-entity-resolution/src/{entity,resolver,matching}.rs` exists; `qip_entity_resolution::entity::{Entity,EntityKind,EntityRecord}` and `::resolver::Resolver` imported in `qip-world-model/src/world.rs:8-9`, and `qip-world-model` is itself wired into the kernel (above) |
| Causal inference / causal graph | BUILT+WIRED | `qip-world-model/src/causal.rs` exists; `qip_world_model::causal::Mechanism` used at `qip-kernel/src/platform.rs:5842`; `qip-reasoning-engine/src/hypothesis.rs` defines `CausalChain`/`CausalStep`, imported at `platform.rs:3633` |
| Episodic memory (full blueprint shape: state vector, regime, causal context, beliefs, actions, outcome, surprise; ANN retrieval over millions of episodes) | PARTIAL | `qip-agents/src/memory.rs` defines `Episode`/`ResearchMemory` (occurred_at, question, conclusion, evidence, conviction, outcome) and `qip_agents::memory::ResearchMemory` is imported and used in `qip-kernel/src/platform.rs:44`. This is a real, wired episodic store, but it holds one agent's research conclusion and outcome, not the blueprint's richer episode (compressed market/world state vector, regime label, active causal edges, declined actions, multi-horizon outcome, surprise score) or approximate-nearest-neighbour retrieval over a large corpus — no HNSW, no vector index, and no "surprise" field were found (`grep -rli "hnsw\|nearest.neighbour\|surprise"` on `crates/libs/qip-agents` returns no such fields) |
| Belief state with confidence and TTL, shipped to regions as priors | PARTIAL | `qip-reasoning-engine/src/bayes.rs` implements Bayesian log-odds confidence updating (`BeliefContribution`, `BeliefUpdate`, `to_log_odds`/`from_log_odds`) and the reasoning engine is wired into the kernel (`qip_reasoning_engine::engine::{ReasoningEngine,ReasoningOutcome}` at `platform.rs:115`). No `struct Belief` with a TTL field exists anywhere in the tree (`grep -rn "struct Belief\b" backend/crates` returns nothing), and no mechanism ships a belief prior from a centre to a region — `qip-edge` has no belief-cache module (`grep -rli "belief" backend/crates/edge` matches only `qip-strategy/src/ir.rs` and test/telemetry files, not a cache) |
| Counterfactual learning / shadow execution of vetoed paths | BUILT+WIRED | `qip-twin` crate (`counterfactual.rs`, `capture.rs`, `regret.rs`, `asof.rs`) is wired: `qip_twin::counterfactual`, `::capture::{Action,Decision,OutcomeCapture,RealisedOutcome}`, `::asof::TwinMarket` imported at `platform.rs:129-134`. Per `.claude/rules/domains/observability.md`, `Platform::evaluate_alternatives` (counterfactual scoring) is reached from `score_declined`, itself called from `stage_learn` — a real production call path in the LEARN stage of the cycle |
| Self-model (capability estimate, coverage, calibration tracking) | ABSENT | `grep -rli "self.model\|selfmodel"` finds only unrelated hits (model-risk self-reference in `qip-compliance`, an AI embedding self-reference, agent factory/DNA code). `grep -rln "CapabilityEstimate"` returns nothing. No type resembling the blueprint's self-model (estimator reliability, coverage, calibration, capacity, regime experience, blind spots) exists |
| Exploration budget (UCB/Thompson sampling, declared capital share for information gain) | ABSENT | `grep -rli "thompson\|ucb\b\|exploration.budget\|explorationbudget"` across `backend/crates` returns nothing |
| Hypothesis generation with falsification | BUILT+WIRED | `qip-reasoning-engine/src/hypothesis.rs` (`Claim`, `CausalChain`) and `src/redteam.rs` (adversarial challenge against a hypothesis's structure, `Severity`, `ReviewOutcome::rejection_rate`) both wired into `qip-kernel/src/platform.rs:116,3630-3633,5843-5847` |
| Degradation ("narrow rather than halt") | BUILT+WIRED (edge code path exists and has a real caller; that caller is exercised only in tests, no deployment) | `.claude/rules/domains/observability.md` documents extensively that `qip-edge`'s `Cell` narrows on stale capability freshness with per-source telemetry (`qip_edge_capability_freshness{capability}`), each recording site proven by `backend/crates/edge/qip-edge/tests/telemetry.rs`; `Cell::work` is reached from `qip-edge-node/src/pass.rs:118` → `main.rs:586`, but only when `QIP_VENUE_FEED=simulated`, and `execution_nodes = {}` in every Terraform environment, so no deployed process exercises it today |
| Source discovery (surface/deep/dark-web tiers, discovery crawler, `SourceCandidate`, `DeepWebAdapter`) | PARTIAL | `qip-data-finder/src/{source,finder,legal,robots,probe}.rs` define `SourceCandidate` (`source.rs:82`) and a robots.txt-respecting finder with a legality/licensing gate; `SourceCandidate`, `DataFinder`, `RegistrationDecision` are imported and used in `qip-kernel/src/platform.rs:75-78,5084`, so the licensing-gated source-registration path is real and wired. But the blueprint's specific three-tier model (surface/deep/dark web) does not exist — `grep -rn "surface_web\|SurfaceWeb\|DarkWeb\|dark_web"` returns nothing — nor does a `DeepWebAdapter` type or its six access modes (`open_query`/`api`/`registered`/`licensed`/`rendered`/`bulk`; `grep -rn "struct DeepWebAdapter"` returns nothing), nor an isolated discovery enclave, nor dark-web defensive monitoring |

Re-scored 2026-09-05, source-discovery row only: the three-tier model now
exists as typed policy in `qip-data-finder/src/tier.rs` — `SourceTier`
(`surface_web`/`deep_web`/`dark_web`), classified by `SourceTier::classify`
from a `TierEvidence` built from the candidate before the probe and from the
`Source` after it, refusing on insufficient evidence rather than defaulting to
the surface web; `DeepWebAdapter` with the six `AccessMode` arms
(`open_query`/`api`/`registered`/`licensed`/`rendered`/`bulk`), each carrying
what it needs (a `CredentialReference` by name only, a licence identifier
checked against the `Declared` posture, a `RenderingBudget`, a `BulkCadence`
with a retention bound) and an `admissible()` rule per arm; `DiscoveryEnclave`
as the isolation record the `rendered` and `bulk` modes are refused without;
and `DefensiveMonitoring` as a watch-list record with no fetch path. The tier
is wired into `DataFinder::assess` (`finder.rs`, `route_by_tier`): a
hidden-service host is rejected before any probe call, an unplaceable source
is deferred, and the routing decision records tier, mode and refusal in the
`classify` and `route` reasoning steps. Proven by
`qip-data-finder/tests/tiers.rs`. Status is now **BUILT+WIRED (policy)**:
still no crawler, no renderer and no Tor client — this crate opens no sockets
— so the tier decides what may be reached and how, and nothing yet reaches it.

## Valuation & assets plane

| Capability | Status | Evidence |
|---|---|---|
| Term structure (yield curves, forward rates) | BUILT-UNWIRED | `qip-market/src/curve.rs` defines `TermStructure`/`CurvePoint` with monotone-cubic interpolation via `qip-numerics/src/interpolate.rs`. `grep -n "TermStructure\|curve::" backend/crates/runtime/qip-kernel/src/platform.rs` returns no hits — no production caller found in the kernel, the only place services are composed |
| Credit engine (default probability, recovery, spread decomposition, covenant state) | PARTIAL | `qip-financial/src/risk_profile.rs:170,184,191` and `extensions.rs:244` carry `default_probability`/`recovery_rate` fields on a risk-profile struct and an `indicative_default_probability()` method — a data holder, not an engine. No spread decomposition, no covenant state, and no dedicated credit crate or `CreditProfile` type (`grep -rln "CreditProfile"` returns nothing) |
| Volatility surface | ABSENT | `grep -rn "struct VolSurface\|struct VolatilitySurface"` returns nothing. The only hits for "vol surface" are doc-comment prose in `qip-numerics/src/interpolate.rs:3` ("Yield curves, volatility surfaces...") and an unrelated `surface` field name in `qip-feature-dag/src/state.rs:32` — no type, no skew/term modelling |
| Illiquid valuation (mark with method + confidence: comparables, DCF, model, last round, cost) | ABSENT | The only "illiquid" hits are a liquidity-regime classifier and a cost multiplier (`qip-financial/src/costs.rs:57` `illiquid(days_to_liquidate)`; `qip-kernel/src/platform.rs:601-602,3048,3120` `MarketRegime::Illiquid` / `ILLIQUID_SPREAD_MULTIPLE`). No `Valuation` type carrying a mark, a method enum, and a confidence exists |
| Cashflow forecasting / commitments / capital calls | ABSENT | `grep -rli "cashflow\|CashflowForecast"` and `grep -rli "commitment.*capital.call\|CapitalCall"` both return nothing across `backend/crates` |
| Corporate actions (splits, dividends, mergers, spinoffs, delistings) | BUILT+WIRED | `qip-market/src/corporate_action.rs` defines `CorporateActionKind`; imported and switched on at `qip-kernel/src/platform.rs:96,2317,5804-5812` (`corporate_action_class`, covering `Split`, `CashDividend`, `StockDividend`, `RightsIssue`, `Merger`, `Spinoff`, `Delisting`) |

## Intelligence & quantum plane

| Capability | Status | Evidence |
|---|---|---|
| The ten shipped model classes / ML training pipeline (`burn`, `linfa`, `polars`, `tract`, spot-GPU jobs) | ABSENT | The blueprint's stack requires `burn`, `linfa`, `polars`, `arrow-rs`, `parquet`, `tract` as dependencies. `CLAUDE.md` states the workspace permits **serde and serde_json only** (ADR 0002, ADR 0009), enforced by `./scripts/check-dependencies.sh`; none of the blueprint's ML crates are in `backend/Cargo.lock`'s permitted set. No training pipeline, ONNX promotion, or model registry exists in the tree |
| Quantum optimisation with a classical baseline computed every run | BUILT+WIRED | `qip-quantum/src/provider.rs` defines `QuantumProvider`, `SimulatedProvider`, `HostedProvider` (refuses to accept a hosted result unless the backend affirmatively reports non-simulated); `qip_optimization_engine::router::ComputeRouter` and `qip_quantum::provider::SimulatedProvider` imported at `qip-kernel/src/platform.rs:106,114`. ADR 0006 ("classical baseline always") is enforced structurally per `.claude/rules` and confirmed by the provider's own doc comments on refusing to blur simulated/hardware provenance |
| Meta-learning (which model works where, warm starts, cross-asset transfer) | ABSENT | `grep -rli "meta.learn"` hits are all unrelated (governance/manifest/topic/DNA/factor-file substrings), not a meta-learning implementation. No dedicated meta-learner code found |
| Adversarial modelling (flow classification informed/uninformed, pattern-leakage detection, crowding, response) | ABSENT | The only "adversarial" hit that is an actual implementation is `qip-reasoning-engine/src/redteam.rs`, which challenges a *hypothesis's own evidentiary structure* before it can be acted on — a different mechanism from the blueprint's market-counterparty adversary model (who is causing adverse selection, is my fingerprint being learned, correlated external flow). No flow-classification or fingerprint-randomisation code was found |
| Market simulation with adaptive agents (passive, informed, momentum, competitor, maker), calibrated against actual fills | PARTIAL | `qip-simulation-engine` (`backtest.rs`, `market.rs`, `montecarlo.rs`, `scenario.rs`, `validation.rs`) exists and is wired — `qip_simulation_engine::costs::CostModel` imported at `qip-kernel/src/platform.rs:122`. `grep -n "struct\|enum\|Agent" qip-simulation-engine/src/market.rs` shows `SyntheticMarket`, `MarketSimulator`, `SimulationRun` but no `Agent` types for the five named counterparty behaviours; this is a synthetic-price/backtest simulator, not a multi-agent adversarial one |

## Capital & money plane

| Capability | Status | Evidence |
|---|---|---|
| Capital engine (deployed/reserve/exploration split, bounded expiring grants) | BUILT+WIRED | `qip-capital::reservation::ReservationLedger`, `AllocationLimits`, `CapitalAllocator`, `DrawdownSchedule` and `qip_capital_fabric` imports at `qip-kernel/src/platform.rs:47-49`; `qip_capital_fabric::evaluate(&plan, realised)` called at `platform.rs:5661` |
| Risk envelope including a per-causal-driver concentration limit | BUILT+WIRED | Confirmed by `.claude/rules/domains/risk-and-execution.md`: `RiskState::with_tail_risk` fills expected shortfall per-limit, and `the_expected_shortfall_limit_can_actually_fire` in `qip-kernel/src/platform.rs` proves the veto fires rather than reading as decorative protection |
| Liquidity ladder (rung-by-rung withdrawal ordering, cash → PE commitments) | ABSENT | `grep -rli "liquidity.ladder"` across `backend/crates` returns nothing |
| Compounding policy (reinvestment cadence, fee-tier accumulation, withdrawal drag) | ABSENT | `grep -rli "compounding"` returns nothing |
| Treasury / signed corridors / MPC withdrawal gates | ABSENT | `grep -rli "corridor\|treasury"` hits only a doc-comment example in `qip-capital-fabric/src/lib.rs:102-116` that uses "treasury" as a location *label* in a worked example (`CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"))`), not a corridor, transfer-gate, or MPC-signing implementation. No `corridor-registry`, `transfer-engine`, `transfer-gate`, or `custody-policy-engine` exists |
| Wallet (read-only balance aggregation, signing crate deliberately unlinked) | ABSENT | No `wallet-aggregator`, `wallet-adapters`, or `wallet-reconciler` service or crate exists anywhere in `backend/crates` or `frontend/**` |
| Tax-lot accounting (FIFO/LIFO/highest-cost/lowest-cost, holding period) | BUILT-UNWIRED | `qip-portfolio/src/lot.rs` defines `Lot`, `LotMethod` (`FirstInFirstOut`/`LastInFirstOut`/`HighestCost`/`LowestCost`), `RealisedTrade`, re-exported from `qip-portfolio/src/lib.rs:20,25`. `grep -rln "qip_portfolio::lot\|portfolio::lot::"` finds only `qip-portfolio/tests/accounting.rs` — no caller outside the crate's own tests, so no production code path computes a realised gain from a lot today |
| Settlement calendar (cut-off, value date, settlement days) | BUILT+WIRED | `qip-capital-fabric/src/settlement.rs` defines `SettlementCalendar`; `qip-capital-fabric` (`plan.rs`, `settlement.rs`, `forecast.rs`) is imported and used at `qip-kernel/src/platform.rs` (settlement-calendar hits present in `platform.rs`) |

## Execution plane

| Capability | Status | Evidence |
|---|---|---|
| Executable graph, cycles, intent netting, mirrored inventory | BUILT+WIRED (in the codebase; not exercised by any deployed process) | `qip-edge/src/cell.rs` implements intent netting (`NetIntent`); `qip-edge/src/mesh.rs`, `journal.rs`, `cell.rs`, `qip-sequencing/src/identity.rs` implement mirror/mesh concepts. Per `.claude/rules/domains/observability.md`, these paths are proven by tests in `qip-edge/tests/telemetry.rs` and reached from `qip-edge-node/src/pass.rs:118`, but `execution_nodes = {}` in every Terraform environment, so nothing deployed runs them |
| Leg coordinator (saga-style latency-equalised dispatch with compensation) | ABSENT | `grep -rli "leg.coord\|LegCoordinator"` returns nothing anywhere in `backend/crates` |
| Eight execution paths as a named, distinguishable set (intra-venue, cross-venue, mirrored, hedged bridging, passive anchoring, firm-quote bridging, representation basis, payoff equivalence) | NOT INDIVIDUALLY VERIFIED | Not checked path-by-path in this pass; `qip-arbitrage`, `qip-routing` crates exist under `backend/crates/edge/` but their coverage of each named path was not confirmed. Listed here rather than silently omitted, per the instruction not to assert presence or absence without checking |

## Software / stack

| Capability | Status | Evidence |
|---|---|---|
| Full blueprint dependency stack (`crossbeam`, `bumpalo`, `core_affinity`, `io-uring`, `tract`, `burn`, `linfa`, `polars`, `arrow-rs`, `parquet`, `rayon`, `argmin`, `nalgebra`, `statrs`, `ring`, `blake3`, `axum`, `tonic`, `google-cloud-rust`, `leptos`, `plotters`, `opentelemetry`) | ABSENT | `CLAUDE.md`: "Two dependencies only — `serde`, `serde_json` (ADR 0002, ADR 0009)", enforced by `./scripts/check-dependencies.sh`. None of the blueprint's ~20 named crates are permitted dependencies in this tree today. This is a structural, not incidental, gap: the blueprint's execution-node performance model (lock-free structures, `io-uring`, pinned cores) and its ML/quantum/crypto/web stack assume a dependency set the workspace's own policy currently forbids |
| GCE execution nodes, one per region, three regions | ABSENT (deployed); an infrastructure module exists in code | ADR 0024 named in `CLAUDE.md` as "never yet applied"; `docs/plan/completion-plan.md` confirms `execution_nodes = {}` in every environment |

## Cloud & network

Not independently re-verified in this pass beyond what the domain rule files
already state as settled fact: `CLAUDE.md` and `.claude/rules/domains/
infrastructure.md` describe Cloud Run + one GCE execution node per region as
the target, Terraform 1.9.8 with `hashicorp/google ~> 6.12`, and no
Kubernetes in the target state (with a documented transitional GKE cluster
per ADR 0022, itself since removed per `docs/plan/completion-plan.md`'s "the
cluster's Terraform, chart, manifests and Argo CD stack removed"). This
document does not re-litigate that; it is reported here as read, not
re-verified against a live plan.

## Web & mobile

| Capability | Status | Evidence |
|---|---|---|
| Leptos-based portal (one Rust codebase, shared types with backend, SSR + WASM) | ABSENT | `frontend/portal/` is Next.js + TypeScript (`.claude/rules/domains/frontend.md`: "Next.js + TypeScript, and the one part of this platform that is not Rust"). ADR 0022 records Leptos as "the target experience layer" and that the Next.js frontend is "transitional"; today it is what is deployed |
| Installable PWA for mobile | BUILT+WIRED | `frontend/mobile/README.md` documents the PWA-as-mobile-app decision and names concrete artefacts: `frontend/portal/src/app/manifest.ts`, `frontend/portal/public/sw.js`, `frontend/portal/src/components/chrome/InstallApp.tsx`, `AppShell.tsx`. These paths were confirmed present in the source tree |
| Investor portal surfaces (portfolio, strategies, risk, execution, capital, agents, intelligence, research, operations) | BUILT+WIRED | `frontend/portal/src/app/(portal)/` contains route directories for `portfolio`, `strategies`, `risk`, `execution`, `capital`, `agents`, `intelligence`, `research`, `operations`, `command`, `orders`, `signals`, `models`, `data-sources`, `integrations`, `admin`, `system` — confirmed by directory listing |
| Wallet / treasury / withdrawal UI | ABSENT | `grep -rli "withdraw\|corridor\|treasury\|wallet" --include=*.tsx` in `frontend/portal/src/app/(portal)/` returns one incidental match in an unrelated news page, no wallet or treasury surface |
| Passkey-only sign-in (WebAuthn, "passwords none, anywhere") | PARTIAL | `frontend/packages/auth/src/index.ts:24` defines `AuthMethod = "password" | "google" | "passkey" | "saml" | "oidc" | "development"` — `passkey` is one option among several, and `password` is explicitly still a listed method, which is the opposite of the blueprint's "no passwords, anywhere" requirement. No WebAuthn ceremony implementation was inspected beyond this type definition |
| Per-account entitlements (`can_invest`, `can_withdraw`, `can_view_execution_trace`, etc.) | ABSENT | The only "entitlement" hits in the tree (`qip-compliance/src/licensing.rs`) are **data-licensing** entitlements — whether a market-data feed may be used for a given purpose — a different concept from the blueprint's per-user, per-account capability gating. No `can_invest`/`can_withdraw`/`can_view_execution_trace` construct exists |

## Experience / governance

| Capability | Status | Evidence |
|---|---|---|
| User mandate object (capital, risk tolerance, permitted families, drawdown ceiling) | BUILT+WIRED | `qip-portfolio-engine/src/construction.rs:68` defines `struct Mandate`; imported and held as a field at `qip-kernel/src/config.rs:15,159,302` (`pub mandate: Mandate`) |
| Explanation object (plain-language, from a decision back through evidence to an entity) | BUILT-UNWIRED | `qip-compliance/src/model_risk.rs:374` defines `struct Explanation` (model reference, output, baseline, contributions, reconciling exactly in `Decimal` arithmetic) — but this explains a *model's numeric output*, a narrower scope than the blueprint's full attribution chain (fill → strategy → family → mandate; intent → belief → causal edge → world event → entity). `grep -rln "model_risk::Explanation"` finds only the crate's own `lib.rs` re-export — no external caller |
| Prediction market integration (event resolution, base rates) | BUILT+WIRED | `qip-prediction/src/{resolution,market,pricing,oracle}.rs` exists; `qip_prediction::resolution::{...}` imported at `qip-kernel/src/platform.rs:110` |
| Observability (`/metrics`, alert policies) | PARTIAL | Per `.claude/rules/domains/observability.md`: both the central and edge planes emit metrics with a real recording caller (`Platform::learn_from`, `Platform::evaluate_alternatives`, both reached from `stage_learn`), but nothing has been shown to actually be scraped by any deployed collector, and all seven alert policies remain gated off behind `workload_metrics_exist = false` in every environment. This document does not re-verify that domain's own count; it is cited as already-settled and current |

---

## Top-line counts

Counting only the rows in this document that were individually classified
above (44 capability rows; the "eight execution paths" and "cloud & network"
rows are explicitly excluded because they were not individually verified):

- **26 of 44** identified capabilities have *any* production code resembling
  them (BUILT+WIRED, BUILT-UNWIRED, or PARTIAL).
- **16 of 44** identified capabilities have a wired production caller
  (BUILT+WIRED only — code that exists and is reached from a composition root
  or from a service the kernel actually calls). Of those 16, several are
  wired in the codebase sense but reach no *deployed* process, most notably
  the entire edge/execution plane (`execution_nodes = {}` in every
  environment) — that distinction is called out per-row above and matters
  more than the count does.

These are **structural counts** — does code exist, is it called — not a
judgment of correctness, completeness, or fitness of what exists. A
BUILT+WIRED row may still be a thin or partially-correct implementation; a
PARTIAL row may be closer to complete than a BUILT+WIRED one that only
handles a narrow case. This is a first-pass inventory, not a quality
assessment, and it covers a representative sample of the blueprint's
capability surface, not all of it — the document itself describes on the
order of 70 services and dozens of named types that this pass did not each
individually verify.

## Most significant ABSENT capabilities

Capabilities the blueprint describes at length for which this pass found
literally nothing resembling them in the tree, after trying multiple
plausible names:

- **The entire ML training pipeline stack** (`burn`, `linfa`, `polars`,
  `tract`, ONNX model promotion) — blocked at the root by the workspace's own
  two-dependency policy (serde/serde_json only), not merely unbuilt.
- **Treasury, corridors, and wallet** — no signed-corridor transfer system,
  no MPC withdrawal gate, no read-only wallet aggregator anywhere in the tree.
- **Volatility surface, illiquid valuation, cashflow/commitments** — three of
  the blueprint's six valuation engines have no corresponding type or engine
  at all (a fourth, credit, has only two struct fields; a fifth, term
  structure, exists but is unwired).
- **Self-model and exploration budget** — no capability-estimate type, no
  UCB/Thompson exploration allocator, no capital line item for exploration.
- **Liquidity ladder and compounding policy** — no rung-by-rung withdrawal
  ordering, no reinvestment-cadence reasoning.
- **Per-account entitlements** (`can_invest`, `can_withdraw`, etc.) and a
  passkey-only, password-free sign-in — the frontend's own auth-method type
  still lists `password` as an option.
- **Leg coordinator** and **meta-learning** — named as distinct blueprint
  mechanisms with no matching code under any plausible name tried.
- **Asset class registry, venue onboarding, DeFi execution model,
  cross-margin model, hedge map** (the blueprint's "registries and lifecycle"
  section) — none found; not individually detailed as full rows above because
  each returned zero hits on first search, but recorded here since the
  section is a named blueprint capability.
