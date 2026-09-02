# Current state, as measured

Established by running the gates and reading the tree. Every number here came
from a command whose output was read, and each row names the commit it was read
at, because a number without a commit is a number nobody can re-measure.

## Shape

| Fact | Value |
|---|---|
| Rust crates | 59, in 8 groups (`libs`, `services`, `apps`, `edge`, `agents`, `quant`, `runtime`, `tests`) |
| Tests | 3,308 workspace tests passing (`cargo test --workspace --no-fail-fast`), as recorded in the PR #5 body for `fef0c97` — the most recent full run with cited output. Not re-measured at `68b7da6`. The figure this row carried before, 3,192, was measured on the §6.2 degradation-contract commit and had been overtaken by three merges |
| Clippy | 0 warnings, `--all-targets` |
| Third-party crates | 11 packages, all permitted (`serde`, `serde_json` and their trees) |
| Frontend | Next.js + TypeScript, 47 tracked files |
| Cloud | GCP: GKE, Secret Manager CSI, KMS, Binary Authorization, WIF |
| Pipelines | `ci.yml`, `deploy.yml`, `infra.yml` — all deriving identity from committed tfvars |
| Decision records | 23 ADRs, `0001`–`0023`, every one listed in `docs/adr/README.md` |

## Against the canonical architecture

104 component ids scored in `docs/architecture/diagram-reconciliation.md`:

| Status | Count |
|---|---|
| Partially implemented | 66 |
| Complete and verified | 19 |
| Implemented but unverified | 13 |
| Missing | 6 |
| Implemented differently | 4 |
| Obsolete or duplicated | 2 |
| Stub or mock only | 1 |

"Complete and verified" requires both an implementation path and a **named
passing test**. Any component no deployable binary composes is capped at
"implemented but unverified" however good its own tests are — a crate nothing
composes is a crate the platform does not run.

## What the eight cycle stages actually do

| Stage | State |
|---|---|
| SENSE | Absorbs 11 record kinds. Works. `feed.rs` can now open `Live` as well as `Synthetic` and `Replay`, but no deployment has been observed running on it |
| UNDERSTAND | Entity resolution and the bitemporal world model. Works |
| DISCOVER | Detectors run; opportunities queue newest-highest-value first. Works |
| REASON | **Now routed.** Prices the decision, asks the cost router where it belongs, convenes the panel when it is affordable, records the rationale either way |
| SIMULATE | Resampling and counterfactuals. Works |
| DECIDE | Sizes approved theses into proposals with legs; records a `nothing_to_do` proposal on quiet cycles |
| ACT | **Now closed.** Two controls sign, legs become orders, orders go through the deterministic pre-trade path, released proposals are retired |
| LEARN | Attributes fills exactly. Now has fills of its own to attribute |

## The three breaks found and closed in the spine

Each was invisible until the one before it was fixed:

1. `stage_act` counted approved proposals and returned. No release loop existed.
2. `Proposal::approve` had **no production caller**, so nothing was ever
   releasable and the release loop was unreachable even once written.
3. Approved proposals were never retired, so one sizing decision would have
   been re-offered every cycle and pyramided into a position nobody chose.

## Known gaps, stated plainly

Seven gaps this document reported when it was written have since been closed,
and are recorded below as closed rather than deleted — a plan that quietly
drops what it once said was broken cannot be audited against. One gap an
earlier revision recorded as closed, the egress proxy, is reopened below: it
was closed on the strength of a committed manifest, and the manifest deploys
nothing.

**Closed:**

- ~~Multi-leg execution is unbuilt.~~ `services/qip-execution-engine/src/multileg.rs`
  carries leg risk, deadlines and unwind, on the invariant that a group which
  cannot complete is unwound rather than abandoned.
- ~~Champion/challenger and drift detection are unwired.~~ Both now have
  production callers: `EvolutionEngine::contest` in
  `apps/qip-deepbrain/src/evolution.rs` is called from the engine's production
  round (`:447` at `68b7da6`) against the `SuccessionDesk` constructed at
  `:228`, and `apps/qip-deepbrain/src/learning.rs:279` records a drift report
  built at `:425` — all four above their files' `#[cfg(test)]` boundaries
  (`learning.rs:516`, `evolution.rs:913`), which is the check that
  distinguishes a wired control from a tested one.

- ~~Nothing writes to `Telemetry`.~~ The kernel now records at the seams where
  facts become known, and a collector scrapes the brains. The deeper defect
  this uncovered: the four Cloud Monitoring alert policies queried metric names
  that appeared in **zero** Rust files, so the alerting layer was unreachable
  by construction rather than merely gated off. Both halves now name the same
  series, and a test asserts they keep doing so.
- ~~`RiskState::expected_shortfall` is always empty.~~ Both it and
  `value_at_risk` are populated from the book's own realised equity path, keyed
  by each configured limit's own confidence. Two limits that
  `conservative_default` ships can now fire, and tests prove they fire on a
  book with a tail and stay quiet on one without.
- ~~The mesh is turned on in no manifest.~~ Wired, with a test asserting every
  variable a manifest sets is one its binary reads, and the converse.
- ~~Every stage reported `Duration::ZERO`.~~ `StageOutcome::with_elapsed` had
  existed since the loop was written and was never called.
- ~~Capital reservation is unbuilt.~~ Closed by `0c6c17f`:
  `qip_capital::ReservationLedger` is wired into the kernel, the decide stage
  reserves what each sized proposal was granted and sizes the next against
  equity minus active holds, and the act stage releases a refused proposal's
  hold. Two concurrent proposals no longer pass against the same free balance.

**Still open:**

- **No live data source has been proven end to end.** The wiring is no longer
  the gap: `apps/qip-fastbrain/src/feed.rs` declares a `Feed::Live` arm
  alongside `Synthetic` and `Replay`, and `Feed::live` constructs it behind
  the licensing gate. What is still missing is evidence — no deployment
  has been observed absorbing a cycle of real data through it, so the honest
  statement is that the path exists and has not been exercised, which is a
  different gap from the one this document used to describe.
- **`workload_metrics_exist` is still `false`** in every environment, and
  correctly so: the endpoints exist and a collector is declared, but no pod has
  been observed to scrape. Flipping it requires that evidence.
- **The Secret Manager CSI credential chain has never been exercised live.**
- **`infra.yml down` has never been run against a live cluster.**
- **No TLS egress proxy is running.** The manifest is committed in two
  byte-identical copies (`infrastructure/helm/qip/templates/egress.yaml`,
  `infrastructure/kubernetes/base/egress.yaml`) with its `ServiceAccount` and
  `Deployment` commented out, so Argo CD renders the chart and no proxy pod
  exists; the Cloud Run module has no equivalent. Commit `64b765a` made the
  egress suite distinguish a described proxy from a deployed one. Because the
  in-tree HTTP client speaks plaintext HTTP/1.1 by design, a pod currently has
  no outbound HTTPS path at all, which is what stands between the wired live
  source above and any evidence of it.

## Latency

No end-to-end latency class has been measured. This document makes no latency
claim, and neither should any other until a reproducible benchmark exists that
records hardware, topology, dataset and percentiles. The canonical diagram's
"microseconds" is an aspiration for a colocated path, not a measured property
of anything in this repository.

## Orphan cleanup

An orphan sweep ran across the workspace on the refactor branch. Every
removal below was proven unused before it went, and the proof is stated
next to it; what could not be proven, or belonged to a crate another owner
was editing at the time, is listed rather than touched.

### Dependency edges

For every `[dependencies]` and `[dev-dependencies]` entry in every crate
manifest, a grep for `<ident>::`, `use <ident>`, `<ident> as` and
`extern crate <ident>` (hyphens mapped to underscores) across the crate's
`src`, `tests`, `benches`, `examples` and `build.rs` found the entries
nothing named. 110 such edges were removed from 35 manifests, plus the
root `[workspace.dependencies]` alias `qip-acceptance`, which no manifest
consumed; `cargo check --workspace --all-targets` then finished clean. The
one test that moved was the contract-layer pin in `architecture.rs`, which
named the exact set `qip-contracts` declared and so encoded five unused
edges as expected; `qip-contracts` contains no `qip_financial::`,
`qip_market::`, `qip_numerics::`, `qip_portfolio::` or `qip_risk::` path,
so the pin now names `qip-core` alone.

Thirty-three zero-hit edges sit in crates another owner had uncommitted
work in and are **deferred to the edge owner**, unverified by a build:

- `qip-arbitrage`: `qip-financial`, `qip-numerics`, `qip-portfolio`, `serde_json`
- `qip-edge`: `qip-execution-engine`, `qip-financial`, `qip-market`,
  `qip-numerics`, `qip-portfolio`, `qip-risk`, `qip-routing`
- `qip-feature-dag`: `qip-financial`, `serde`, `serde_json`
- `qip-orderbook`: `qip-financial`, `qip-market`, `qip-numerics`
- `qip-protocols`: `qip-financial`, `qip-market`, `qip-numerics`
- `qip-routing`: `qip-financial`, `qip-numerics`, `qip-portfolio`, `qip-risk`, `serde_json`
- `qip-sequencing`: `qip-financial`, `qip-market`, `serde_json`
- `qip-strategy`: `qip-financial`, `qip-market`, `qip-numerics`, `qip-risk`
- `qip-edge-node`: `serde_json`

The `qip-edge` entries for `qip-execution-engine` and `qip-routing` deserve
a look from whoever owns that crate: the architecture suite reasons about
what the cell can reach, and a declared edge it never opens widens that
answer for nothing.

### Bench profile

`find backend -name benches -type d` and a grep for `[[bench]]` across
every manifest both returned nothing, so `[profile.bench]` in the workspace
root was removed. It returns with the first real benchmark.

### `qip-normalization` (decision D6 pending; crate untouched)

Callers, from `grep -rn "qip_normalization" backend --include=*.rs`: the
crate's own `tests/canonicalisation.rs`, and the acceptance suites
`truth_loop.rs` and `performance.rs`. `qip-kernel` declared the edge and
never named the crate, so that edge went with the sweep above; after it,
`cargo tree --workspace -i qip-normalization -e normal` lists only the
crate itself, and with `-e normal,dev` adds `qip-acceptance` as a
dev-dependency. **No deployable binary reaches it.** Its `dropping_unmapped`
builder has no caller and was left alone with the rest of the crate.

### Public functions with no caller

Every `pub fn` under `crates/{libs,services,runtime,agents,quant}/**/src`
was checked for whole-word references across every `.rs` in the workspace
and every `.md` under `docs/`, excluding its own definition line. 161 had
none. 99 were removed (accessors, builders and helpers whose doc, where one
existed, restated the signature; nine imports only they used went with
them; no test changed). The remainder are listed here so nobody mistakes
them for wired behaviour.

Kept because the doc names a consumer, a control, a runbook, a UI or a
design decision, or the function is a limit knob or test support (35):

- `qip-storage`: `RedisConfig::with_username` (credential-splitting
  rationale), `RedisConfig::with_key_prefix` (names `DEFAULT_KEY_PREFIX`).
- `qip-financial`: `MarketHours::next_open` (names the scheduler);
  `Universe::not_decision_grade` — **its doc says the kernel logs it at
  start-up; nothing does.** A degraded universe is currently visible to no
  one before it trades.
- `qip-core::testing`: `any_f64`, `any_returns`, `check_approx`.
- `qip-observability`: `Logger::set_echo`.
- `qip-quantum`: `SolverBenchmark::with_validator` (the classical baseline,
  ADR 0006), `QuantumInspiredSolver::with_replicas`, `with_schedule`.
- `qip-numerics`: `Qubo::to_dense` (names the quantum backends).
- `qip-agents`: `AuditTrail::for_correlation` (audit query).
- `qip-events`: `EventBus::reset_deduplication` — **its doc says replay
  calls it; nothing does**; `EventLog::replay_filtered`.
- `qip-transport`: `RetryPolicy::worst_case_backoff` (runbook figure).
- `qip-market-ingestion`: `alternative_reading` (names the discovery
  stage); `MarketEventEnvelope::is_knowable_at` — **the point-in-time guard,
  consulted by nothing**; `AlternativeDataAdapter::sense_topics`
  (compile-time proof).
- `qip-cost-router`: `CostEngine::spent_so_far` — the figure a caller is
  meant to compare against value at stake before escalating; **no caller
  compares it**.
- `qip-simulation-engine`: `Regime::feed_is_current` (staleness control),
  `Distribution::mean_is_distinguishable_from_zero` (design decision).
- `qip-execution-engine`: `SimulatedBroker::set_liquidity` — the impact
  model's liquidity input, **supplied by nothing**.
- `qip-chain`: `BridgeLedger::on_reorg` — **a control that cannot fire**:
  no caller fails bridged transfers when their source block is reorganised.
- `qip-data-finder`: `RegisteredSource::quarantine_reason`.
- `qip-capital-fabric`: `PrePositioningPlanner::with_shortfall_buffer_cap`.
- `qip-opportunity-engine`: `OpportunityEngine::supported_anomaly_kinds`
  (documentation and UI).
- `qip-reasoning-engine`: `CausalChain::weakest_links` (red team).
- `qip-training`: `TrainingDataset::design_matrix`.
- `qip-streaming`: `TieredPublisher::route_batch`.
- `qip-world-model`: `FeatureLookup::is_truncated` (bounded-retention
  signal nothing checks); `RelationshipKind::transmits_shock` — **its doc
  says it bounds causal propagation; nothing calls it.**
- `qip-normalization`: `Normalizer::dropping_unmapped` (D6).
- `qip-investment-agents`: `LearningAttribution::with_promotion_policy`,
  `promotion_policy`.

Listed only, because another owner is wiring callers into these crates
during the same refactor (27): `qip-kernel` — `StrategyFactory::
with_demotion_policy`, `submit_evidence`, `set_baseline`; `CellOutcome::
with_drawdown`, `with_losing_days`, `with_realised_cost_bps`;
`CentralPlane::compliance_mut`, `set_concentration_limits`;
`DnaPayload::section_digest`; `PlatformConfig::with_licensed_datasets`,
`with_data_user_agent`, `with_reasoning_confidence_bar`. `qip-lifecycle` —
`StrategyEvidence::stages_evidenced`. `qip-capital` — `CellPosition::
is_short`; `AllocationLimits::with_cell_limit`, `with_venue_limit`;
`Allocation::is_unconstrained`; `CapitalAllocator::with_uncertainty_penalty`;
`EnvelopeTerms::with_order_fraction`, `with_loss_fraction`, `with_venues`.
`qip-compliance` — `ArtifactStore::raw_dataset`, `CompliancePlane::
model_risk_mut`, `ModelRiskFile::has_independent_review`.
`qip-learning-engine` — `PositionAttribution::return_fraction`,
`Attribution::period_return`. `qip-evolution` — `NetReturns::corrected_by`.

`verify_dna` has four callers in `qip-kernel/tests/central.rs`, and
`verify_continuity` is an edge-crate control that
`docs/operations/disaster-recovery.md` names twice; neither is an orphan.

### Frontend

`@algorik/shared-types` exported a configuration reader (`readConfig`,
`describeProblems`, `AlgorikConfig`, `IdentityConfig`) that no `.ts`,
`.tsx`, `.js`, `.mjs` or `.json` under `frontend/` outside `node_modules`
and `.next` imported; it was removed. The two type aliases that remain,
`EnvironmentMode` and `TradingPosture`, also have no importer yet.

### `#[allow(dead_code)]`

Every occurrence is under a `tests/` directory: `qip-transport/tests/
common`, `qip-data-finder/tests/common`, `qip-streaming/tests/common`, and
`qip-market-ingestion/tests/{server,connector_common}`, each documented as
a shared fixture that not every integration binary uses in full. None is in
shipped code; nothing to remove.
