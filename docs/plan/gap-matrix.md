# Gap matrix and delivery plan

The canonical architecture's nine areas against what the repository has, and
the ordered work that closes the distance. Ordering is by dependency and by
consequence, not by how much of the diagram a task colours in.

## The nine areas

| # | Canonical area | State | Principal gap |
|---|---|---|---|
| 1 | Autonomous Data Mesh | Partial | Absorption, dedup, sequencing and bitemporal stamping all work. No live source is wired into any deployment; the normalizer sits outside the runtime path |
| 2 | Regional AI Brains | Partial | `qip-edge-node` runs and is structurally paper-only. Degraded-mode policy exists; regional partition recovery is tested in `resilience.rs` but not against a live cluster |
| 3 | Global Opportunity Brain | Partial | Detectors, correlation and scoring work. Cross-market graph search is narrower than the diagram implies |
| 4 | Capital & Strategy Brain | Partial | Sizing, cost model and the capital ledger work. `expected_shortfall` is always empty so one limit cannot fire |
| 5 | Regional Execution Mesh | **Closed** | Orders reach the simulated broker through the deterministic control path, and `LegGroup` now bounds the risk between legs of one decision. Capital reservation against a proposal is still unbuilt |
| 6 | Outcome & Telemetry | **Broken** | Attribution is exact and works. Nothing emits metrics; `/metrics` is empty |
| 7 | Evolution Brain | Partial | The loop now trades, crowns a champion and challenges it under a trial count. Its backtests produced flat equity curves until this pass, so every candidate registered before it was admitted on evidence from a strategy that never traded. Drift detection is still not wired: `DriftReport::compare` has no production caller |
| 8 | Intelligence Fabric | Partial | Cost router now decides and records. Quantum adapter is real with a classical baseline always computed; the digital twin is not driven from production decisions |
| 9 | Governance & Guardrails | Strong | Three independent paper-trading layers, hash-chained log, two-signature approval, deterministic pre-trade checks. Alerting is blocked behind area 6 |

## Ordered work

Consequence-ordered. Each item names the evidence that closes it.

| # | Work | Why it is here | Evidence that closes it |
|---|---|---|---|
| 1 | ~~Close the trading spine~~ | **Done.** Nine diagram rows starved on it and LEARN had no fill to attribute | 3 tests, 4 mutations fired |
| 2 | ~~Emit telemetry~~ | **Done.** It also uncovered that the alert policies named metrics nothing emitted — the layer was unreachable, not merely gated | Metrics at their seams, a collector, and a test binding both halves to the same names |
| 3 | ~~Wire the mesh in manifests~~ | **Done.** | The env-var correspondence test, both directions |
| 4 | ~~Deploy the TLS egress proxy~~ | **Done.** Manifest committed; it unblocks item 6 | Twelve egress tests, each mutation-verified |
| 5 | ~~Compute expected shortfall~~ | **Done.** Two limits, not one — VaR shared the defect | Both firing on a book with a tail, both quiet on one without |
| 6 | **Wire one live market source** | **Next.** Every deployment's data is synthetic or replayed; `feed.rs` opens nothing else. Area 1 cannot be called real until one source is, and the egress proxy has now removed the blocker | A cycle absorbing live data behind the licensing gate |
| 7 | ~~Multi-leg execution~~ | **Done.** Leg risk, deadlines and unwind. The invariant: a group that cannot complete is unwound, never abandoned | 15 tests, 17 mutations fired — one found a deadline test that never reached the deadline branch |
| 8 | ~~Champion/challenger~~ | **Done.** It was unreachable by construction: `Challenger` has no constructor but `Mutator::mutate`, which nothing called. Wiring it exposed that the loop's backtests had never filled an order | 13 mutations across four files, all fired |
| 9 | **Drift detection** | **Next.** `DriftReport::compare` computes PSI, standardised mean shift and volatility ratio, and nothing outside its own module constructs one. `ModelRegistry::record_drift` takes a number from its caller and has only test callers. The model-risk control reads as present and cannot fire | A production caller comparing a live feature window against its reference, and a test asserting the report is recorded |
| 10 | Capital reservation | A proposal that passes a capital check does not hold the capital, so two concurrent proposals can each pass against the same free balance | Reserve/commit/release with expiry, and a test where the second proposal is refused |
| 11 | Reference data for the Deep Brain | Its universe is empty, so its evolution loop now refuses every candidate rather than registering flat lines. Honest, but off | A reference-data source populating the universe, and a round that registers against it |

## Risk register

| Risk | Consequence | Mitigation |
|---|---|---|
| A control that cannot fire | Reads as protection, is not. Found nine times so far: `MaxExpectedShortfall`, `MaxValueAtRisk`, `Proposal::approve`, `Router::select`, `StageOutcome::with_elapsed`, the alert policies, the champion/challenger ladder, and both halves of the evolution loop's backtest | Every closure ships with a test that fails when the control is removed; and the standing question for any new control is which running code path reaches it |
| Documentation drifting ahead of code | An operator believes a control exists | `documentation.rs` refuses overclaims; it has already caught one |
| A test that passes by measuring nothing | Permanent false confidence. Two more caught this pass: a deadline test whose verdict came from the risk branch, and a subject-selection test whose filter did the ranking's work | Mandatory mutation verification; premise-before-conclusion |
| Parallel agents overwriting each other | Work destroyed and redone, discovered late | Disjoint path ownership named in every brief, including the other agents' paths |
| An agent dying mid-task | Half-finished code in the tree | Finish it or park it on a WIP branch; never discard, never leave the tree red |
| A default tuned for a different frequency | Silent and total. A one-day decision lag and ten million of capital are right for daily institutional bars and wrong for minute bars, and applied there they produced a flat equity curve the gate scored as a result | A backtest that produced no fill is refused, not scored; superseded orders are counted rather than dropped |
| Latency claims outrunning measurement | A promise the physics does not support | No latency class stated without a reproducible benchmark |

## Assumptions, each reversible

1. **Paper trading stays absolute.** No live-order path is built. Reversing this
   is an ADR and three deliberate layer changes, not a flag.
2. **Two dependencies stay the limit.** Any addition needs an ADR first.
3. **The simulated broker is the execution target.** Provider sandboxes come
   only after the egress proxy is deployed and certified.
4. **Regional topology stays as the tfvars declare it** — one cell in dev, three
   in stage. Nine cells would cost production money to learn nothing more.
5. **No cloud spend beyond the existing dev environment** without explicit
   approval.
