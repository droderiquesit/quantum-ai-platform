# Current-state audit

**Re-audited at commit `dc9ee9a`, and updated at `237c0f0`, which returned
the workspace to green.** Every count below was measured, not
recalled, by the same commands that produced the first set: `.rs` files under
`*/src/*` and under `*/tests/*` counted separately with `wc -l`, crate
directories under `crates/*/*`, and the passing-test total summed out of
`cargo test --workspace --no-fail-fast`. The workspace was measured in a clean
checkout, so no half-finished edit is counted as landed.

This document exists because the build order says to audit before implementing.
Its purpose is to stop the next phase rebuilding things that already work, and
to stop it reporting things as built that are not.

The first audit was written when eight crates held a full implementation on
disk behind a three-line `lib.rs` that declared no modules. Those crates are
now compiled and tested, which moves a great many rows — and creates a new
failure mode this document has to guard against, because a port that names its
missing credential is much easier to mistake for an integration than an empty
file was.

---

## 1. What is here

| | Measured | Was |
|---|---|---|
| Crates | 57 | 49 |
| Rust source | 120,864 lines | 87,496 |
| Rust tests | 63,601 lines | 43,680 |
| Tests passing | 2,052 | 1,554 |
| Tests failing | 0 | 0 |
| Terraform | 26 files, 2,191 lines | unchanged |
| Kubernetes manifests | 6 | unchanged |
| Documentation | 26 files (10 ADRs) | 19 |
| CI workflows | 2 (`ci.yml`, `deploy.yml`) | unchanged |
| Third-party dependencies | 2 (`serde`, `serde_json`) | unchanged |

Layout: `crates/{libs,edge,services,agents,quant,runtime,apps,tests}` — 14
libraries, 8 edge crates, 25 services, 1 agent organisation, 1 quant crate, 1
composition root, 6 applications, 1 workspace test crate.

The whole platform is Rust. There is no JavaScript, no Python, and no build
step outside `cargo`.

**The workspace is green**, at 2,052 passing and none failing. It was not when
this audit was first written: two market-condition tests failed
deterministically, and both are now fixed — see section 6, which keeps the
account because the diagnosis is the useful part. Count the suite with
`make count`, never by summing `test result: ok` lines: a failing binary prints
`FAILED` instead and contributes nothing, so that sum under-reports rather than
erroring, and a document written from it says the suite is green when it is
not. This one did, for a revision.

The eight lockfile packages behind the two declared dependencies bring the
third-party total to 11, all permitted. `serde_json` now carries the
`float_roundtrip` feature: without it serde_json's fast parse path is accurate
only to within one unit in the last place, so no `f64` survived a JSON round
trip bit-for-bit and every content digest taken over one — model digests
especially — was a within-process identity rather than a content identity. That
was a silent correctness bug in anything that compared two models by hash, and
it is now pinned by a regression test written from bit patterns.

---

## 2. Mapping onto the target architecture

Honest labels: **Built** means implemented and tested. **Partial** means the
mechanism exists but not the capability described. **Missing** means nothing
exists. A **port** is a named interface that reports `Unavailable` and lists
what a deployment must supply; a port is never Built, because the capability
described is the connection and the connection does not exist.

### Layer 1 — Autonomous Data Mesh

| Capability | State | Where |
|---|---|---|
| Provider adapters (market, news, fundamental, macro, alt) | Partial | `qip-market-ingestion` — pull-based `DataAdapter`, synthetic implementations only |
| Normalisation, dedup, time sync, enrichment | Built | `qip-normalization` |
| Sequence, gap, clock discipline | Built | `qip-sequencing` — A/B arbitration, PTP-style estimation, failover |
| Wire decoders (FIX, ITCH, SBE, framed JSON) | Built | `qip-protocols` |
| Point-in-time mesh (lakehouse, analytics, hot series, master, graph) | Partial | `qip-mesh` — ports and local adapters; managed services report unavailable |
| Write-once evidence store | Built | `qip-mesh::EvidenceStore` |
| Catalog with lineage and licensing | Partial | `qip-mesh::Catalog` — has lineage and entitlements, has no discovery |
| **Autonomous Data Finder** | Built, offline | `qip-data-finder` — 14 modules: discovery, scoring, robots.txt, licensing, schema-drift detection, replacement search, health. Every network fact arrives through one `SourceProbe`; `NetworkProbe` reports unavailable and `InMemoryProbe` answers from a script. **It opens no sockets.** |
| Universal event envelope, dedup, replay | Built | `qip-streaming` — wraps `qip_events::AnyEvent` rather than duplicating it; the durable transport is real |
| **Google Pub/Sub backbone** | **Missing** | `qip-streaming::pubsub` is now a named port — GCP project, topic, subscription, gRPC-with-TLS client, workload-identity binding — returning `Unavailable`. It never falls back to the local log |

The Data Finder was the single largest gap in the first audit, and the gap it
described — "no source discovery, scoring, robots.txt handling, schema-change
detection, or replacement-source search" — is closed. Two properties are worth
naming because they are structural rather than procedural: a `SourceCandidate`
cannot become a `Source` without `ProbeEvidence`, so a function that must not
run on hearsay takes the type that cannot be built from it; and a source whose
licence is not discoverable is refused, because unknown is not permission.

What remains open is the transport. The discovery logic is complete and has
never been pointed at the internet.

### Layer 2 — Regional AI Brains

| Capability | State | Where |
|---|---|---|
| Market understanding, order books | Built | `qip-orderbook` — L3 with queue position, L2, auctions |
| Liquidity and impact | Built | `qip-market`, square-root impact model |
| Local alpha, incremental features | Built | `qip-feature-dag` — compute-changed-once, shared |
| Ultra-fast decisioning | Built | `qip-strategy` — typed IR, bounded runtime, no LLM reachable |
| Local arbitrage | Built | `qip-arbitrage` — 2-leg, triangular, N-leg |
| Local risk and limits | Built | `qip-risk-engine`, `qip-risk` (includes VaR/CVaR) |
| Cell assembly and hot path | Built | `qip-edge`, `qip-edge-node` |
| Strategy and program travel together | Built | `qip-edge::Cell::deploy` takes the `Program` its plan indexes into and refuses a mismatch — see below |
| Anomaly detection | Partial | `qip-opportunity-engine` exists but runs centrally, not per-cell |
| Cash and inventory per region | Partial | `qip-capital` tracks centrally; `qip-capital-fabric::LocationBalance` models per-location balances for planning; cells still hold a utilisation counter only |
| **Multi-region deployment** | Partial | Seven cells — Dallas, Chicago, NY/NJ, London, Frankfurt, Singapore, Tokyo — are *specified* in `infrastructure/environments/development/terraform.tfvars`. Staging and production still name one (`london-1`). `venues` is empty on all seven. Nothing has been applied |

`Cell::deploy` previously took a compiled strategy and an envelope but not the
`Program` the strategy's plan indexes into, so a cell could be assembled with an
empty arena, accept a strategy compiled against a real one, report itself
healthy, and refuse on every pass of `work`. Because `NodeRef` is an index the
worse case was the quieter one: in an arena large enough the index resolves, to
a node belonging to some other strategy, and the cell emits a signal computed
from arithmetic nobody wrote for it. Deployment now validates the program,
checks every planned node against it, checks the envelope names this cell and
this strategy, and gives each deployment its own runtime rather than sharing one
per cell — which removes the aliasing rather than checking for it.

Two of the seven cells are recorded as being in the wrong city: Google Cloud has
no region in Chicago or the New York/New Jersey metro, so those cells sit
roughly 400km and 330km from the venues they are meant to be adjacent to. They
are listed with the distance against each rather than dropped, because that
distance is the measurement ADR 0008's own reversal condition turns on.

### Layer 3 — Global Opportunity Brain

| Capability | State | Where |
|---|---|---|
| Global knowledge graph | Built | `qip-world-model` — bitemporal, causal |
| Multi-leg arbitrage engine | Built | `qip-arbitrage` |
| Regime and cycle detection | Built | `qip-opportunity-engine` — HMM |
| Structural mispricing | Partial | `qip-quant` factors; no dedicated detector |
| Cross-market correlation | Partial | `qip-quant`; not assembled into one brain |
| Event impact and catalyst detection | Partial | `qip-reasoning-engine` forms hypotheses; no catalyst detector |
| Liquidity topology | Missing | — |
| **Strategy discovery / generation** | Built | `qip-evolution` — a grammar over a typed palette generates specs from a seed; `discovery::FeatureScreen` refuses an undeclared screen size and raises the bar a feature must clear by `sqrt(2·ln(m)/n)` |

The pieces still are not composed into one opportunity brain. `qip-opportunity-
engine` ranks anomalies by novelty and decay; `qip-capital` ranks strategy
proposals on `sharpe − k·stderr` with capacity; neither ranks opportunities
across regions by net edge, capacity and risk together, and there is no single
region-aware ranking anywhere. That remains a real gap rather than a naming
quibble.

The generation gap is closed and its most likely abuse is closed with it. A
challenger whose search size was never declared is refused with an error rather
than scored low, because a low score is a position on a scale and an undeclared
search is not on the scale at all; and `cost_model::NetReturns` cannot be built
from a return series without a cost series beside it, so a generated strategy
cannot be promoted on gross alpha by forgetting rather than by lying.

### Layer 4 — Capital Brain

| Capability | State | Where |
|---|---|---|
| Opportunity scoring, expected alpha | Built | `qip-contracts::NetEdge` — nine deductions, refuses an incomplete edge |
| Position sizing and allocation | Built | `qip-capital` — sizes on `sharpe − k·stderr`, capacity-aware |
| Cash, collateral, margin | Built | `qip-capital::margin` |
| Risk engine (VaR/CVaR) | Built | `qip-risk::metrics` |
| FX and multi-currency exposure | Partial | `qip-capital-fabric::transfer` now has `FxRates` and a `FundingCurve`, used for pre-positioning; the trading path still prices a leg in `LegStep::priced_in` and does not convert |
| Hedge and inventory optimisation | Partial | `qip-arbitrage` plans hedge legs; no inventory optimiser |
| **Compute cost and data cost in the alpha equation** | Built | `DeductionKind` has nine variants; `ComputeCost` and `DataCost` are priced by `qip-cost-router::CostEngine` from a compute ledger and an amortised licence model |
| **Predictive Capital Fabric** | Built, unfunded | `qip-capital-fabric` — forecasts demand per location and kind, plans pre-positioning against a transfer cost model and a settlement calendar, refuses a plan over budget. It moves no money: there is no treasury connection |

The cost engine is the change with the widest reach, because it is in the
contract every other layer reads. `ComputeCost` and `DataCost` sit in the
deduction list rather than in a separate compute budget, on the argument that an
opportunity earning less than it cost to find is not an opportunity, and the
only place that arithmetic can be done is next to the gross figure it has to
survive. Data cost is amortised rather than charged whole, because a source read
once a day and one read a million times cost the same to licence.

### Layer 5 — Regional Execution Mesh

| Capability | State | Where |
|---|---|---|
| OMS, EMS, order state machine | Built | `qip-execution-engine` |
| Smart order routing, venue selection | Built | `qip-routing` — routes on net cost, not quoted price |
| Order slicing, partial fills, child orders | Built | `qip-routing::children` |
| Pre-trade risk | Built | `qip-execution-engine::oms` gate order |
| Reconciliation | Built | `qip-edge::dropcopy`, OMS `reconciliation_breaks` |
| Broker/gateway adapter framework | Built, no live class | `qip-brokers` — `VenueAdapter` extends `Broker` with session, logon, heartbeat, amendment; a `ReadyTicket` with no public constructor makes "submit to a venue that is not ready" unwriteable; `Secret` implements neither `Serialize` nor `Deserialize`; `SimulatedExchange` really matches, with queue position, residuals, exact commission and seeded rejections |
| **A live venue** | **Missing** | `AdapterClass` has `Simulated` and `Sandbox` and no third variant. There is no live adapter to instantiate, no feature flag that adds one, and no string that deserialises into one — both facts are pinned by doctests |
| **Multi-region mesh** | Partial | Same state as Layer 2: seven cells specified, none applied |

### Layer 6 — Outcomes Capture

| Capability | State | Where |
|---|---|---|
| Fills, P&L, attribution | Built | `qip-learning-engine` — exact reconciliation |
| Slippage and costs | Built | `qip-market` transaction costs |
| Risk and limit utilisation | Built | `qip-risk` |
| **Missed opportunities** | Built | `qip-twin::regret` — aggregates by *kind* of alternative, shrinks the win rate and the mean gap through the same `Conviction::shrunk` a strategy's own conviction uses, so three observations of a large win come out at a fraction of face value |
| **Counterfactual digital twin** | Built | `qip-twin` — an as-of market view that is exclusive by borrow, so holding the decision instant and the horizon at once does not compile; the fill model is `qip-simulation-engine`'s rather than a second, more generous one |

The twin's load-bearing property is a type, not a convention: a simulated figure
is `Simulated<Decimal>`, `Decimal + Simulated<Decimal>` does not exist, and a
`compile_fail` doctest asserts it. That is what stops a counterfactual number
reaching a realised P&L — the failure mode that turns a twin into a machine for
manufacturing performance.

### Layer 7 — Evolution Brain

| Capability | State | Where |
|---|---|---|
| Lifecycle gates (holdout → paper → shadow → pilot → scaled) | Built | `qip-lifecycle` |
| Model validation and backtesting | Built | `qip-simulation-engine` — purged k-fold, embargo, deflated Sharpe, walk-forward; the market-conditions layer's two defects are fixed (section 6) and it now refuses a crossed book outright |
| Model registry and governance | Built | `qip-ai::registry`, `qip-compliance::model_risk` |
| Strategy factory and DNA packaging | Built | `qip-kernel::central` |
| **Model training** | Partial | `qip-training` — local teachers, datasets, cadence and a full request shape are built and tested; `vertex` is a port that returns an error naming what is missing. Nothing here promotes a model: `qip-lifecycle` owns the one path to capital |
| **Policy distillation** | Built | `qip-training::distill` fits a `DistilledModel` to the teacher's *outputs* and measures the gap on four axes before returning; `Distillation` has private fields and one constructor, so an unmeasured student is not a reachable state |
| **Strategy evolution / mutation** | Built | `qip-evolution::mutate` — the edits available for a spec are computed from the spec, so the seeded stream advances identically whatever shape a champion turned out to have |
| **IBM Quantum** | **Missing** | `qip-quantum::solver::IbmQuantumSolver` is a port: it returns `Unavailable` and names every missing item. The device is not reachable from this build |

### Cross-cutting

| Layer | State |
|---|---|
| A. Counterfactual Digital Twin | **Built** — `qip-twin` |
| B. Contextual Model / Agent Router | **Built** — `qip-cost-router` selects the cheapest tier reaching a required confidence inside a deadline, and charges what it used against an agent budget that can refuse. `ComputeRouter` still routes solvers; these are now two different things and both exist |
| C. Predictive Capital Fabric | **Built, unfunded** — `qip-capital-fabric` plans; nothing moves cash |
| D. Confidential Global Intelligence | **Missing** |
| E. Quantum-Centric Learning Fabric | Partial — simulator and a port, no device |
| Governance and guardrails | **Built** — `qip-compliance`, six controls enforced structurally, with recorded caveats |
| Observability | Partial — `qip-observability` types exist and are shaped for OTLP; the collector is in-tree and nothing exports |
| **Cost engine** | **Built** — `qip-cost-router::CostEngine`, feeding `DeductionKind::ComputeCost` and `DataCost` |
| Operator console | **Built** — `qip-web::console`, nine server-rendered views. A collection is a `Panel` carrying whether it can be believed, so "zero exposure" and "no cell is reporting" are different markup. Nothing acts except tripping the kill switch; clearing a halt is not offered, because a page cannot establish a freshly verified operator identity |

### Technology stack

| Named | State |
|---|---|
| GKE, VPC, IAM, KMS, Secret Manager | Built (Terraform) |
| Cloud Storage | Built — write-once evidence bucket, locked retention |
| Artifact Registry | Built |
| Pub/Sub, Dataflow, BigQuery, Spanner, Bigtable, Vertex AI | **Missing** — ports exist in `qip-mesh`, `qip-storage`, `qip-streaming` and `qip-training`, each naming its exact missing credential |
| Confidential VMs, Security Command Center | Missing |
| IBM Quantum | Missing — simulator, a benchmark, and a port |

---

## 3. The decision this phase turned on

The first audit recorded that ADR 0002's two-dependency policy and the target
architecture's managed services could not both hold, and recommended a tiered
policy. That is now [ADR 0009](../adr/0009-tiered-dependency-policy.md),
accepted: a named **decision core** of fifteen crates keeps `serde` and
`serde_json`, and an **I/O edge** may take from a vetted allowlist.

**The tier is recorded and not in force.** ADR 0009 says the boundary "is
enforced in `crates/tests/qip-acceptance/tests/architecture.rs`". The test that
exists there — `no_crate_declares_a_third_party_dependency_beyond_the_two_
permitted` — walks every `Cargo.toml` under `crates/` and permits `serde` and
`serde_json` in all of them. It draws no core/edge distinction, and neither does
`scripts/check-dependencies.sh`, which allowlists eleven lockfile packages
workspace-wide. No crate takes an edge dependency, so nothing has yet tested
the boundary the ADR describes.

This is the right order — the decision before the dependency — but the document
and the check currently describe different policies, and the check is the
stricter of the two. The first crate to want a transport dependency will
discover that, and it should discover it from this paragraph instead.

---

## 4. What must not be rebuilt

Listed because the greatest risk to the next phase is rewriting working code:

* The **exact-arithmetic core.** `Decimal` is i128 fixed-point; every price,
  size and money value already uses it.
* **Bitemporality.** Valid-time and known-time are on every fact. A look-ahead
  bug in `absorb_bar` was found and fixed by a test that walks the loop
  stage-by-stage; the apparatus works.
* **Determinism.** No ambient clock, no ambient RNG. Bit-exact replay is
  tested. Anything added must take a `Timestamp` and a seeded RNG. Every crate
  wired this phase holds to it, including the ones that fit models.
* **Validation.** Purged k-fold with embargo, deflated Sharpe, walk-forward and
  overfitting assessment are built and correct — including the counter-intuitive
  sub-threshold regime, which is pinned by its own test. This does *not* extend
  to the market-conditions layer beside it; see section 6.
* **The governance plane.** Six controls, enforced structurally, each carrying
  its honest caveats. The caveats are load-bearing: they are why the report can
  be trusted.
* **The safety controls.** Paper-trading ceiling, dual approval, capital
  envelopes, kill switches, capability gating, the no-LLM-on-the-hot-path
  boundary. All tested, several enforced by absence rather than by check — and
  `qip-brokers` adds another of that kind, an adapter class with no live variant.
* **The unit-in-the-last-place fix.** `serde_json`'s `float_roundtrip` feature
  is what makes an `f64` content digest mean anything. Removing it is silent.

---

## 5. Migration plan

Ordered by dependency, not by ambition. Each phase must leave the workspace
green — a standard this phase missed and then met.

0. ~~Return the workspace to green.~~ — done at `237c0f0`. It was the right
   thing to put first: everything below is eventually judged by a backtest, and
   the backtester's adversity model was wrong in the direction that flatters.
1. ~~Tiered dependency policy~~ — decided as ADR 0009, and now actually
   enforced: `the_decision_core_named_by_adr_0009_is_the_set_actually_held_to_two`
   reads the core's list out of the ADR's own fenced block rather than keeping
   a copy.
2. ~~Cost engine~~ — done. `DeductionKind` has nine variants, priced by
   `qip-cost-router`.
3. ~~Autonomous Data Finder~~ — done, offline. **Still to do:** a real
   `SourceProbe`, which needs item 7's transport.
4. ~~Counterfactual digital twin + missed-opportunity capture~~ — done,
   `qip-twin`.
5. ~~Model / agent router~~ — done, `qip-cost-router`.
6. ~~Predictive capital fabric~~ — done as a planner, `qip-capital-fabric`.
   **Still to do:** a treasury connection, so a plan can become a transfer.
7. **Pub/Sub transport** — the port is written (`qip-streaming::pubsub`) and
   needs the tiered policy actually enforced before it can take a client. Must
   preserve the pull-based `DataAdapter` contract, which is what gives
   backtest/live parity — `qip-streaming::Subscriber::poll` already does.
8. **Vertex AI training** — `qip-training::vertex` is a complete request shape
   behind a port. Distillation is already done and does not wait on this.
9. **IBM Quantum adapter** — behind the existing solver port, benchmarked
   against the classical baseline, never in the execution path. The benchmark
   harness that will judge it exists and already refuses to report a quantum
   answer without a classical one beside it.
10. **Multi-region deployment** — seven cells are parameterised and specified in
    the development environment. Instantiating them is credentials, an `apply`,
    and the venue address ranges that `venues = {}` currently, correctly,
    declines to guess.
11. ~~Operator console~~ — done, nine views.

---

## 6. Known-false claims to avoid repeating

Stated here so no later document repeats them. Each was re-verified against the
tree at `dc9ee9a`.

* **Nothing has been deployed.** No Terraform has ever been run — this
  environment has no GCP credentials, no `gcloud` and no `terraform` binary. The
  infrastructure is specified and structurally tested, never applied, never
  planned, and never validated against a provider schema. Seven edge cells are
  now *specified* in the development tfvars; specification is not deployment,
  and `venues` is empty on every one of them.

* **No real venue, broker or market-data connection exists.** This is now
  stronger than it was, not weaker. `qip-brokers` is a full adapter framework
  and `AdapterClass` has exactly two variants, `Simulated` and `Sandbox`; a
  doctest asserts there is no `Live` to construct and another asserts no string
  deserialises into one. The only `std::net` in the workspace is the inbound
  `TcpListener` in `qip-api` and `qip-edge-node`. There is no HTTP client, no
  TLS, and no outbound socket anywhere — including in the data finder, whose
  `NetworkProbe` reports unavailable rather than connecting. Every fill in every
  test is simulated, and the flag saying so is stamped by the adapter and again
  by the OMS, overwriting whatever a venue message claimed.

* **The quantum layer is a simulator, and "quantum-inspired" is not quantum.**
  `qip-quantum` is 2,467 lines: a correct statevector simulator, QAOA on top of
  it, three solvers behind one trait, and a benchmark. `ClassicalSolver` is
  exhaustive enumeration, Metropolis annealing or local search.
  `QuantumInspiredSolver` is discrete-time path-integral annealing — a
  *classical* Monte Carlo algorithm whose dynamics are borrowed from a
  transverse-field Ising model, and evidence of nothing quantum; `SolverKind`
  keeps `Quantum` and `QuantumInspired` as different variants precisely so the
  two cannot be reported as one. `IbmQuantumSolver` returns `Unavailable` and
  names what is missing. A benchmark report cannot be constructed without its
  classical baseline, and the only usable answer is one re-evaluated
  classically, so "the quantum solver said so" is not something this platform
  can act on.

* **Solver runtimes are modelled, not timed.** Each solver declares a
  `SolverCostModel` — nanoseconds per objective evaluation, a queue delay, a
  price per job — and the reported runtime is that model applied to the work the
  solver actually did. It is reproducible and comparable across machines. It is
  not a stopwatch, and no figure out of `qip-quantum::benchmark` is a
  measurement of elapsed time.

* **No latency figure has been measured end to end.** Still true, and now
  documented at length: [`docs/performance/budgets.md`](../performance/budgets.md)
  opens with a section titled "What has not been measured" saying that no
  wire-to-wire, tick-to-order or cross-region figure exists, and that any such
  figure quoted anywhere should be treated as fabricated. What *is* measured is
  eight stages in isolation, in both profiles, with the fixture already in
  memory. The highest number in it — book apply at 0.04 µs, 24.0 M/s — is one
  function applied to one pre-built message, not a throughput for an assembled
  path. The first audit's 3.16M messages/second order-book figure has been
  superseded by that table and should not be requoted.

* **The two loops now meet, once, in one test.** This is the row that changed.
  `crates/tests/qip-acceptance/tests/e2e.rs` is a single test — deliberately
  one, because seven tests that each pass in isolation are exactly what a system
  whose parts do not meet looks like — and it passes. It walks: two candidate
  sources assessed and one refused for having no discoverable licence; ingest;
  a cell building a book from wire messages and computing a feature; the central
  loop running every stage; a genuine three-arm dislocation priced net of every
  deduction; an allocator sizing from a risk budget with two human approvals and
  a signed, bounded, expiring envelope; the cell verifying that envelope against
  a key it was given separately, deploying the strategy *with the program its
  plan indexes into*, deciding, and sending one order; an independent drop-copy
  channel reporting a **partial** fill and reconciliation reporting the shortfall
  rather than assuming the rest; outcome capture on a hash chain with the twin
  pricing what was not done; attribution reconciling exactly; and the training
  and IBM Quantum ports each reporting themselves unavailable.

  Its own module doc states what it does not prove, and no later document may
  contradict it: **no network, no venue, no cloud** — every port that would
  leave the process is in-memory or unavailable, so the run proves the seams
  line up and not that the far side of them works; **no latency claim** —
  nothing in it is timed; **paper only** — the platform is assembled at its
  default autonomy ceiling and asserts at the end that it never became
  live-capable. One passing run is evidence that the interfaces compose. It is
  not evidence that the system trades.

* **The backtester was flattering executions, and was found doing it.** Fixed
  at `237c0f0`; kept here because the diagnosis is the part worth remembering.
  Two tests failed deterministically in
  `crates/services/qip-simulation-engine/tests/market_conditions.rs`:

  * `a_slippage_regime_multiplies_what_is_paid_beyond_the_reference` — a
    ten-times slippage regime moves the cost from 5.8002bp to 40.0016bp, about
    seven times, not ten. The regime multiplier does not do what its name says.
  * `injecting_a_condition_never_improves_the_execution` — case 42, with
    `crossed_market` and `latency` injected together, *improves* adversity from
    5.894609bp to 3.767213bp. Adding adversity made the execution better, which
    is the monotonicity the whole conditions model rests on.

  Both arrived with the commit that compiled eight thousand previously-unwired
  lines, which is what a first compile of untested code is for, and both turned
  out to be the same area. The slice was priced against the touch read back
  *after* the sweep, so the order's own impact sat inside the reference instead
  of beyond it — the impact term double-counted, and the reported slippage was
  partly scaled by the regime multiplier and partly not. And a crossed book was
  being filled at the worse of its two touch prices, which sounds conservative
  and is not: the book is built symmetrically about the mid, so at any cross
  width *both* quotes sit inside the calm touch. Charging the worse side of a
  crossed book still charges less than an orderly market. A crossed book is now
  refused outright — the simulator cannot tell a stale quote from a real
  arbitrage and is not entitled to guess.

  Reading the rest of the file the way that property test reads it then found
  two more of the same shape. A book with one side quoted, or neither, has no
  mid — and every cost the regime charges is defined relative to it, so a fill
  measured against nothing escaped all of them and came back a flawless
  execution. And a position the run ended holding and could not mark was valued
  at the last price the generator produced, which no book was showing. Both are
  now refusals, and the report names the positions it could not mark rather
  than pricing them.

  The standing lesson: a backtester's errors are only dangerous in one
  direction, and all four of these ran that way. Not one was found by review. A
  property test over 96 generated condition sequences found the first, and the
  rest came from asking the same question of every other path that had to put a
  number on something it could not measure.

* **A port is not an integration, and this phase created many ports.** This is
  the newly-available overclaim and the reason the section exists. `qip-streaming`
  has a Pub/Sub port; `qip-training` has a Vertex AI port; `qip-quantum` has an
  IBM Quantum port; `qip-data-finder` has a network probe port; `qip-mesh` and
  `qip-storage` have theirs from before; `qip-brokers` has a venue framework with
  no live class. Every one of them is written, tested against a scripted
  counterpart, and returns `Unavailable` naming its missing credential. Not one
  of them has ever exchanged a byte with the thing it names. The count of ports
  rose sharply this phase; the count of connections is still zero. "The Pub/Sub
  transport is implemented" and "`qip-training` trains on Vertex AI" are both
  false, and both are the natural sentence to write after reading the code.

* **The tiered dependency policy is recorded, not enforced.** ADR 0009 states
  that the core/edge boundary is enforced in `tests/architecture.rs`. It is not:
  that test holds *every* crate in the workspace to `serde` and `serde_json`,
  with no tier in it. The ADR is accepted and the allowlist half of it has no
  implementation and no user. Section 3 has the detail.

* **The Data Finder is offline.** It is built, and the first audit's headline
  gap is closed, but the sentence "the platform discovers its own data sources"
  is false in this build. It assesses sources presented to it through an
  in-memory probe. Nothing crawls.
