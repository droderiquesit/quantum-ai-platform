# Current state, as measured

Established by running the gates and reading the tree. Every number here came
from a command whose output was read, and each row names the commit it was read
at, because a number without a commit is a number nobody can re-measure.

## Shape

| Fact | Value |
|---|---|
| Rust crates | 59, in 8 groups (`libs`, `services`, `apps`, `edge`, `agents`, `quant`, `runtime`, `tests`) |
| Tests | **Measured at `29ce828`**, on a clean checkout, `cargo test --workspace --no-fail-fast`: 302 test binaries, 3,485 passed, 1 failed — recorded in the message of `397c144`, the first commit after the run. The failure was `qip-api`'s scrape test, whose premise (an empty page before the first cycle) `78026e2` had made false; `397c144` repaired it, mutation-verified, and `cargo test -p qip-api` then reported `3 passed; 0 failed` for that module, so the figure at `397c144` is 3,486 passing across 302 binaries — implied from the module run, not re-measured as a whole. **Re-run at `851c0ed`** by the writer of this row, on a tree that was not clean: `git status --short \| wc -l` was 0 when it started and 7 when it finished — one of them this writer's own edit to `docs/operations/external-dependencies.md`, the other six other owners' uncommitted edits to `qip-lifecycle`, `qip-simulation-engine` and `catalogue.tf`. `grep -c '^test result'` on the log gives 299 binaries reporting; summed over them, 3,487 passed, 0 failed, 0 ignored. Three doctest binaries — `qip-api`, `qip-cli`, `qip-deepbrain` — produced no result line because rustdoc could not find a `qip_kernel` rlib that a concurrent build in the shared `target/` had replaced (`error[E0463]`), and the run exited 101 on that, not on a test. Read it as 3,487 across 299 of 302 binaries, three unmeasured, on a tree that was not clean; the figure that can be cited without a caveat is the one at `29ce828`. Before this row: 3,355 across 280 from `a4f673c`'s message; before that, 3,308 from the PR #5 body for `fef0c97` |
| Full gate | At `29ce828`, clean checkout: `cargo fmt --all --check` exit 0; `cargo clippy --workspace --all-targets -- -D warnings` exit 0; tests as above; `./scripts/check-dependencies.sh` → `dependency policy: 11 third-party package(s), all permitted`; `./scripts/check-secrets.sh` → `secret scan: nothing found`; `git diff --check` clean. Terraform **not run** — no binary here. Frontend **not run** — no frontend source changed. PR #6 then merged at `851c0ed` (`baffcd8`..`b1e709c`, 89 commits, `git log --oneline baffcd8..b1e709c \| wc -l`) with the thirteen checks `ci.yml` declares — format, clippy, test, release build, dependency policy, security audit, sbom, portal, landing, vulnerability scan, trunk meta-linter, infrastructure, secret scan — reported green on `b1e709c`. Reported: this environment has no `gh`, so the check runs were not read here |
| Clippy | 0 warnings, `--all-targets -- -D warnings`, at `29ce828` |
| Third-party crates | 11 packages, all permitted (`serde`, `serde_json` and their trees), at `29ce828` |
| Frontend | Next.js + TypeScript, 47 tracked files |
| Cloud | GCP, as Terraform nothing has applied: Cloud Run for the three central binaries and the two frontends, a Compute Engine execution node per region (`execution_nodes = {}` in every environment's tfvars), Secret Manager as mounted files, KMS, Binary Authorization, WIF. The GKE cluster, edge-cell and console-ingress modules, the Helm chart, the raw manifests and the Argo CD stack were removed at `808ca32`, `67b3e92` and `7d79161`; no `terraform` binary exists here, so none of what replaced them has been through `fmt`, `validate` or a plan |
| Pipelines | `ci.yml`, `deploy.yml`, `infra.yml` — all deriving identity from committed tfvars |
| Decision records | 24 ADRs, `0001`–`0024`, every one listed in `docs/adr/README.md` (`ls docs/adr \| grep -c '^[0-9]'` = 24 at `851c0ed`) |

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
| LEARN | Attributes fills exactly, and now grades and prices as well: every resolved thesis is settled against the platform's own series and graded through `Platform::learn_from` (`platform.rs:3931`, `:4052` at `296e187`), so the belief calibration is a Brier score on a gauge rather than a function nothing called; every refused order is priced through `Platform::evaluate_alternatives` once its horizon has passed (`:3948`, `:5028`), capped at eight per cycle with the excess counted and deferred. `qip-kernel/tests/learning.rs`: `a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`, `a_refused_order_is_priced_once_its_horizon_has_passed_and_charged_to_its_gate`, `declined_paths_past_the_per_cycle_cap_are_counted_as_deferred_and_priced_next_cycle` (`04738ee`, `b9e2242`) |

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
  correctly so: the endpoints exist, but no deployed process has been observed
  to scrape. The collector that was declared — a `PodMonitoring` for the two
  brains — left with the cluster; on the runtime the tree now describes, the
  execution node's startup template declares an Ops Agent receiver on its
  health port and the Cloud Run services have no collector attached
  (`infrastructure/terraform/modules/observability/NOT-SCRAPED.md`). Flipping
  the flag requires an observed scrape, and there is nothing to observe.
- **The secret-mount chain has never been exercised live.** It was the Secret
  Manager CSI driver on the cluster; at `808ca32` it is Cloud Run's mounted
  secret files. Neither has ever served a credential to a running process.
- **`infra.yml down` has never been run.** At `b85684f` it targets the
  execution nodes, the one thing that bills while idle; there are none.
- **No TLS egress proxy is running, and the one that was described is gone.**
  The two byte-identical manifest copies with their `Deployment` commented out
  were deleted with the chart and the raw manifests at `7d79161`. What exists
  instead, since `c924191` and wired at `808ca32`, is `modules/egress-proxy`
  — the same Envoy bootstrap (`infrastructure/egress/envoy.yaml`) rebound to
  loopback and published to a bucket whose object preconditions refuse a host
  `egress_allowed_upstreams` does not name — mounted by `modules/cloudrun` as
  a sidecar the workload container waits for. It has never been planned,
  applied or observed: no `terraform` binary exists here. Its allowlist names
  five clusters, all Google or IBM endpoints, and no market-data vendor
  (`envoy.yaml:392-492`). Because the in-tree HTTP client speaks plaintext
  HTTP/1.1 by design, a deployed process still has no outbound HTTPS path,
  which is what stands between the wired live source above and any evidence
  of it. One consequence for the gate: at `296e187` the egress acceptance
  suite still read `infrastructure/kubernetes/base/egress.yaml` (`egress.rs:46`
  and `:1096`), a path that no longer existed, so for five commits the suite
  that tells a described proxy from a deployed one could not pass; `81dd1cd`
  retargeted it at the sidecar (14 passed, per its message), and what it now
  holds is that nothing has been applied.

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

**Resolved at `2753911`.** The edge owner's work landed and the same proof
was applied: thirty-two of the thirty-three were removed across the nine
manifests (`Cargo.lock` lost the same thirty-two lines), `cargo check -p
<crate> --all-targets` finished clean after each, and the architecture
suite passed 25 with `./scripts/check-dependencies.sh` saying all
permitted. `qip-edge -> qip-arbitrage`, which the sweep had also flagged,
was not on the list by then because it had stopped being dead — the cell
constructs the desk (`71f9465`). The one that stays is `qip-edge ->
qip-execution-engine`, named by no source file: the architecture suite's
`only_the_edge_cell_itself_holds_an_order_manager` uses "the cell reaches
`qip-execution-engine`" as its vacuity guard, so removing the edge fails the
guard rather than the property. The manifest says so beside the line; the
guard needs re-stating against the cell's real order path (its `Placer`
seam) before the edge can go.

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
design decision, or the function is a limit knob or test support (29, down
from 35 — the six whose doc named a consumer that did not exist are
resolved below, and the list here is what remains):

- `qip-storage`: `RedisConfig::with_username` (credential-splitting
  rationale), `RedisConfig::with_key_prefix` (names `DEFAULT_KEY_PREFIX`).
- `qip-financial`: `MarketHours::next_open` (names the scheduler).
- `qip-core::testing`: `any_f64`, `any_returns`, `check_approx`.
- `qip-observability`: `Logger::set_echo`.
- `qip-quantum`: `SolverBenchmark::with_validator` (the classical baseline,
  ADR 0006), `QuantumInspiredSolver::with_replicas`, `with_schedule`.
- `qip-numerics`: `Qubo::to_dense` (names the quantum backends).
- `qip-agents`: `AuditTrail::for_correlation` (audit query).
- `qip-events`: `EventLog::replay_filtered`.
- `qip-transport`: `RetryPolicy::worst_case_backoff` (runbook figure).
- `qip-market-ingestion`: `alternative_reading` (names the discovery
  stage); `AlternativeDataAdapter::sense_topics` (compile-time proof).
- `qip-simulation-engine`: `Regime::feed_is_current` (staleness control),
  `Distribution::mean_is_distinguishable_from_zero` (design decision).
- `qip-execution-engine`: `SimulatedBroker::set_liquidity` — the impact
  model's liquidity input, **supplied by nothing**.
- `qip-data-finder`: `RegisteredSource::quarantine_reason`.
- `qip-capital-fabric`: `PrePositioningPlanner::with_shortfall_buffer_cap`.
- `qip-opportunity-engine`: `OpportunityEngine::supported_anomaly_kinds`
  (documentation and UI).
- `qip-reasoning-engine`: `CausalChain::weakest_links` (red team).
- `qip-training`: `TrainingDataset::design_matrix`.
- `qip-streaming`: `TieredPublisher::route_batch`.
- `qip-world-model`: `FeatureLookup::is_truncated` (bounded-retention
  signal nothing checks).
- `qip-normalization`: `Normalizer::dropping_unmapped` (D6).
- `qip-investment-agents`: `LearningAttribution::with_promotion_policy`,
  `promotion_policy`.

Resolved since the sweep (6) — the six whose doc named a consumer that did
not exist, each settled one of two ways:

- **Wired, with a production caller above `#[cfg(test)]` and a test that
  fails when the call is removed (2):** `BridgeLedger::on_reorg` — the
  kernel now holds the ledger and `observe_chain`'s reorganised arm calls
  it (`platform.rs:4736` at `296e187`);
  `bridges.rs::a_reorganisation_that_withdraws_a_deposit_block_fails_the_transfer_riding_on_it`
  (`67b3e92`). `Universe::not_decision_grade` — asked by `Platform::new`
  before the universe moves into the desk (`platform.rs:1075`) and gauged
  under `qip_universe_not_decision_grade`;
  `universe_grade.rs::a_research_only_instrument_is_counted_and_named_at_assembly`
  (`78026e2`).
- **Removed, because the property the doc described is held elsewhere, and
  where a property was worth asserting it now is (4):**
  `MarketEventEnvelope::is_knowable_at` — the withholding is
  `ConnectorRuntime::admit`'s, applied before any envelope exists, and the
  only envelope handover polls at the horizon it strips at, so the guard
  could only ever say yes (`b7d3edc`). `RelationshipKind::transmits_shock`
  — propagation walks claimed causal edges, never relationships;
  `understanding.rs::a_relationship_is_structure_and_never_a_path_a_shock_travels_along`
  asserts it (`b8a8acd`). `EventBus::reset_deduplication` — a replay runs on
  a fresh bus, and the bus that produced a log must treat that log as the
  duplicates it is;
  `backbone.rs::a_log_fed_back_into_the_bus_that_produced_it_dispatches_nothing_twice`
  (`68ff891`). `CostEngine::spent_so_far` — the bounds it restated are
  `Router::escalate`'s and `Router::assess`'s (`ed69a52`).

Listed only, because another owner was wiring callers into these crates
during the same refactor (27; not re-verified at `296e187`, so some may
have gained callers since): `qip-kernel` — `StrategyFactory::
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
