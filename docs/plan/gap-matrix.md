# Gap matrix and delivery plan

The canonical architecture's nine areas against what the repository has, and
the ordered work that closes the distance. Ordering is by dependency and by
consequence, not by how much of the diagram a task colours in.

## The nine areas

| # | Canonical area | State | Principal gap |
|---|---|---|---|
| 1 | Autonomous Data Mesh | Partial | Absorption, dedup, sequencing and bitemporal stamping all work. A live source is now wired (`feed.rs:61`, `:108`) but no deployment has been observed running on it; the normalizer sits outside the runtime path |
| 2 | Regional AI Brains | Partial | `qip-edge-node` runs and is structurally paper-only. Degraded-mode policy exists; regional partition recovery is tested in `resilience.rs` but not against a live cluster |
| 3 | Global Opportunity Brain | Partial | Detectors, correlation and scoring work. Cross-market graph search is narrower than the diagram implies |
| 4 | Capital & Strategy Brain | Partial | Sizing, cost model and the capital ledger work. `expected_shortfall` is always empty so one limit cannot fire |
| 5 | Regional Execution Mesh | **Closed** | Orders reach the simulated broker through the deterministic control path, and `LegGroup` now bounds the risk between legs of one decision. Capital reservation against a proposal is still unbuilt |
| 6 | Outcome & Telemetry | Partial | Attribution is exact and works. Metrics are emitted at the seams — `qip-kernel/src/platform.rs:1668` counts cycles and `:1728-1755` records stage runs, latencies and gauges — so `/metrics` is no longer empty. What remains is that `workload_metrics_exist` is still `false` in every environment, correctly, because no pod has been observed to scrape |
| 7 | Evolution Brain | Partial | The loop now trades, crowns a champion and challenges it under a trial count. Its backtests produced flat equity curves until an earlier pass, so every candidate registered before that was admitted on evidence from a strategy that never traded. Drift detection is now wired — `apps/qip-deepbrain/src/learning.rs:425` and `:279`. What remains is the Deep Brain's empty reference universe (item 11) |
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
| 6 | **Prove one live market source** | **Next, and now on the critical path to live trading** — it is step 1 of ADR 0023's opening sequence, and the Phase 2 gate cannot be passed on synthetic data. Narrower than it was. The wiring landed: `feed.rs:61` declares `Live(Box<RestMarketDataAdapter>)` and `:108` constructs it behind the licensing gate. What is missing is evidence, not code — no deployment has been observed absorbing real data | A cycle absorbing live data behind the licensing gate |
| 7 | ~~Multi-leg execution~~ | **Done.** Leg risk, deadlines and unwind. The invariant: a group that cannot complete is unwound, never abandoned | 15 tests, 17 mutations fired — one found a deadline test that never reached the deadline branch |
| 8 | ~~Champion/challenger~~ | **Done.** It was unreachable by construction: `Challenger` has no constructor but `Mutator::mutate`, which nothing called. Wiring it exposed that the loop's backtests had never filled an order | 13 mutations across four files, all fired |
| 9 | ~~Drift detection~~ | **Done.** The control now fires: `apps/qip-deepbrain/src/learning.rs:425` builds the report from a live feature window against its reference and `:279` records it, both above the `#[cfg(test)]` boundary at line 516 | A production caller comparing a live feature window against its reference, and a test asserting the report is recorded |
| 10 | ~~Capital reservation~~ | **Done, in two halves.** The mechanism landed crate-locally (`qip_capital::ReservationLedger`, reserve/commit/release with expiry, six mutation-verified tests) and the kernel now calls it: the decide stage anchors free to tracked equity every pass, sizes against equity minus active holds, and reserves what each proposal was granted; the act stage commits a placed proposal's hold and releases a refused one, sweeping lapsed holds on the same clock | `platform.rs::a_second_proposal_is_sized_against_what_the_first_still_holds` — the second proposal's budget is exactly equity minus the first one's hold (200,000 − 16,000 = 184,000 in the test's own numbers) — plus `::a_released_proposal_commits_its_hold_and_a_refused_one_returns_it` and the acceptance anchor test; four kernel-side mutations fired |
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
