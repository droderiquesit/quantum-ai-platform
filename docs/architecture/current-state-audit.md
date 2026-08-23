# Current-state audit

**Audited at commit `ac5d64b`.** Every count below was measured, not recalled.

This document exists because the build order says to audit before implementing.
Its purpose is to stop the next phase rebuilding things that already work, and
to stop it reporting things as built that are not.

---

## 1. What is here

| | Measured |
|---|---|
| Crates | 49 |
| Rust source | 87,496 lines |
| Rust tests | 43,680 lines |
| Tests passing | 1,554 |
| Terraform | 26 files, 2,191 lines |
| Kubernetes manifests | 6 |
| Documentation | 19 files |
| CI workflows | 2 (`ci.yml`, `deploy.yml`) |
| Third-party dependencies | 2 (`serde`, `serde_json`) |

Layout: `crates/{libs,edge,services,agents,quant,runtime,apps,tests}` — 14
libraries, 8 edge crates, 17 services, 1 agent organisation, 1 quant crate, 1
composition root, 6 applications, 1 workspace test crate.

The whole platform is Rust. There is no JavaScript, no Python, and no build
step outside `cargo`.

---

## 2. Mapping onto the target architecture

Honest labels: **Built** means implemented and tested. **Partial** means the
mechanism exists but not the capability described. **Missing** means nothing
exists.

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
| **Autonomous Data Finder** | **Missing** | Nothing. No source discovery, scoring, robots.txt handling, schema-change detection, or replacement-source search |
| **Google Pub/Sub backbone** | **Missing** | `qip-events` is an in-process bus with a file-backed log |

The Data Finder is the single largest gap in the whole target. A grep for
source-discovery concepts across 49 crates returns nothing.

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
| Anomaly detection | Partial | `qip-opportunity-engine` exists but runs centrally, not per-cell |
| Cash and inventory per region | Partial | `qip-capital` tracks centrally; cells hold a utilisation counter only |
| **Multi-region deployment** | **Missing** | One cell instantiated (`london-1`). No US East, US West, Europe, APAC, S. America or Middle East |

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
| **Strategy discovery / generation** | **Missing** | `qip-lifecycle` gates strategies; nothing *generates* them |

The pieces exist but are not composed into one opportunity brain. That is a
real gap, not a naming quibble: nothing today ranks opportunities across
regions by net edge, capacity and risk together.

### Layer 4 — Capital Brain

| Capability | State | Where |
|---|---|---|
| Opportunity scoring, expected alpha | Built | `qip-contracts::NetEdge` — seven deductions, refuses an incomplete edge |
| Position sizing and allocation | Built | `qip-capital` — sizes on `sharpe − k·stderr`, capacity-aware |
| Cash, collateral, margin | Built | `qip-capital::margin` |
| Risk engine (VaR/CVaR) | Built | `qip-risk::metrics` |
| FX and multi-currency exposure | Partial | `LegStep::priced_in` added; no FX conversion or funding model |
| Hedge and inventory optimisation | Partial | `qip-arbitrage` plans hedge legs; no inventory optimiser |
| **Compute cost and data cost in the alpha equation** | **Missing** | `DeductionKind` has seven variants; the target requires nine |
| **Predictive Capital Fabric** | **Missing** | Capital is allocated reactively; nothing forecasts where it will be needed |

### Layer 5 — Regional Execution Mesh

| Capability | State | Where |
|---|---|---|
| OMS, EMS, order state machine | Built | `qip-execution-engine` |
| Smart order routing, venue selection | Built | `qip-routing` — routes on net cost, not quoted price |
| Order slicing, partial fills, child orders | Built | `qip-routing::children` |
| Pre-trade risk | Built | `qip-execution-engine::oms` gate order |
| Reconciliation | Built | `qip-edge::dropcopy`, OMS `reconciliation_breaks` |
| Broker/gateway adapter framework | Partial | Trait plus simulated implementation; real venue reports missing credentials |
| **Multi-region mesh** | **Missing** | Same gap as Layer 2 |

### Layer 6 — Outcomes Capture

| Capability | State | Where |
|---|---|---|
| Fills, P&L, attribution | Built | `qip-learning-engine` — exact reconciliation |
| Slippage and costs | Built | `qip-market` transaction costs |
| Risk and limit utilisation | Built | `qip-risk` |
| **Missed opportunities** | **Missing** | Refusals are journalled per cell; nothing aggregates what was declined and what it would have earned |
| **Counterfactual digital twin** | **Missing** | One passing mention in a comment; no implementation |

### Layer 7 — Evolution Brain

| Capability | State | Where |
|---|---|---|
| Lifecycle gates (holdout → paper → shadow → pilot → scaled) | Built | `qip-lifecycle` |
| Model validation and backtesting | Built | `qip-simulation-engine` — purged k-fold, embargo, deflated Sharpe, walk-forward |
| Model registry and governance | Built | `qip-ai::registry`, `qip-compliance::model_risk` |
| Strategy factory and DNA packaging | Built | `qip-kernel::central` |
| **Model training (Vertex AI)** | **Missing** | No training pipeline of any kind |
| **Policy distillation** | **Partial** | `qip-strategy::DistilledModel` can *hold* fixed coefficients; nothing produces them |
| **Strategy evolution / mutation** | **Missing** | — |
| **IBM Quantum** | **Missing** | `qip-quantum` is an in-tree statevector simulator with QAOA. The provider port reports unavailable for a real service |

### Cross-cutting

| Layer | State |
|---|---|
| A. Counterfactual Digital Twin | **Missing** |
| B. Contextual Model / Agent Router | **Missing** — `ComputeRouter` routes *solvers*, not models or agents |
| C. Predictive Capital Fabric | **Missing** |
| D. Confidential Global Intelligence | **Missing** |
| E. Quantum-Centric Learning Fabric | Partial — simulator only |
| Governance and guardrails | **Built** — `qip-compliance`, six controls enforced structurally, with recorded caveats |
| Observability | Partial — `qip-observability` types exist; no collector wired |
| **Cost engine** | **Missing** |

### Technology stack

| Named | State |
|---|---|
| GKE, VPC, IAM, KMS, Secret Manager | Built (Terraform) |
| Cloud Storage | Built — write-once evidence bucket, locked retention |
| Artifact Registry | Built |
| Pub/Sub, Dataflow, BigQuery, Spanner, Bigtable, Vertex AI | **Missing** — ports exist in `qip-mesh`/`qip-storage`, each naming its exact missing credential |
| Confidential VMs, Security Command Center | Missing |
| IBM Quantum | Missing — simulator only |

---

## 3. The decision this phase turns on

The repository's dependency policy (ADR 0002) permits exactly two third-party
crates. The target architecture mandates Google Pub/Sub, Vertex AI, BigQuery,
Spanner and IBM Quantum. **These cannot both hold.** No amount of in-tree
engineering produces a gRPC client, a TLS stack and a Google auth flow within
two dependencies, and attempting it would be the worst outcome available: a
hand-rolled TLS implementation guarding real money.

The build order says to prefer existing repository patterns. The repository's
own recommendation — recorded before this phase — is a **tiered policy**:

* **The decision core keeps its two dependencies.** Everything that decides,
  prices, sizes or risks a trade: `qip-core`, `qip-contracts`, `qip-numerics`,
  `qip-financial`, `qip-market`, `qip-portfolio`, `qip-risk`, `qip-strategy`,
  `qip-arbitrage`, `qip-capital`, `qip-risk-engine`, `qip-execution-engine`.
  This is testable and already enforced by `tests/architecture.rs`.
* **The I/O edge gets a vetted allowlist.** Transport, serialisation and cloud
  clients, in named crates that hold no decision logic.

This preserves the property where it matters — the code that moves money stays
auditable and offline-buildable — and stops pretending it can hold where it
cannot. The alternative, abandoning it wholesale, throws away something real.

Recorded as ADR 0009 alongside this audit.

---

## 4. What must not be rebuilt

Listed because the greatest risk to the next phase is rewriting working code:

* The **exact-arithmetic core.** `Decimal` is i128 fixed-point; every price,
  size and money value already uses it.
* **Bitemporality.** Valid-time and known-time are on every fact. A look-ahead
  bug in `absorb_bar` was found and fixed by a test that walks the loop
  stage-by-stage; the apparatus works.
* **Determinism.** No ambient clock, no ambient RNG. Bit-exact replay is
  tested. Anything added must take a `Timestamp` and a seeded RNG.
* **Validation.** Purged k-fold with embargo, deflated Sharpe, walk-forward and
  overfitting assessment are built and correct — including the counter-intuitive
  sub-threshold regime, which is pinned by its own test.
* **The governance plane.** Six controls, enforced structurally, each carrying
  its honest caveats. The caveats are load-bearing: they are why the report can
  be trusted.
* **The safety controls.** Paper-trading ceiling, dual approval, capital
  envelopes, kill switches, capability gating, the no-LLM-on-the-hot-path
  boundary. All tested, several enforced by absence rather than by check.

---

## 5. Migration plan

Ordered by dependency, not by ambition. Each phase must leave the workspace
green.

1. **Tiered dependency policy** — ADR 0009, enforced in `tests/architecture.rs`
   so the core boundary is checked rather than intended.
2. **Cost engine** — add compute and data cost to `DeductionKind`. Cheap,
   touches the contract every other layer reads, and better done before more
   callers exist.
3. **Autonomous Data Finder** — the largest missing subsystem, and independent
   of everything else.
4. **Counterfactual digital twin + missed-opportunity capture** — needs only
   the existing decision path; multiplies learning data without capital.
5. **Model / agent router** — needs the cost engine.
6. **Predictive capital fabric** — needs the router and the cost engine.
7. **Pub/Sub transport** — needs the tiered policy. Must preserve the pull-based
   `DataAdapter` contract, which is what gives backtest/live parity.
8. **Vertex AI training + policy distillation** — needs the learning records.
9. **IBM Quantum adapter** — behind the existing solver port, benchmarked
   against the classical baseline, never in the execution path.
10. **Multi-region deployment** — the regional module is written and
    parameterised; instantiating it is configuration plus credentials.
11. **Operator console** — nine views over APIs that mostly exist.

---

## 6. Known-false claims to avoid repeating

Stated here so no later document repeats them:

* **Nothing has been deployed.** No Terraform has ever been run — this
  environment has no GCP credentials and no `terraform` binary. The
  infrastructure is specified and structurally tested, never applied.
* **No real venue, broker or market-data connection exists.** Every adapter is
  synthetic or reports its missing credential.
* **The quantum layer is a simulator.** It is a correct statevector simulator
  with QAOA, not IBM hardware.
* **No latency figure has been measured end to end.** Order-book throughput was
  measured (3.16M messages/second, L3, release profile, whole-loop average on a
  shared container). No percentiles were taken and no microsecond claim is
  supported.
* **The two loops do not yet meet.** `StrategyDna` and `CellReport` exist on
  both sides of the central/edge split; nothing has passed one end to end.
