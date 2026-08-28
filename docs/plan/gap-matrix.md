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
| 5 | Regional Execution Mesh | **Closed this pass** | Orders now reach the simulated broker through the deterministic control path. Multi-leg reservation and unwind remain unbuilt |
| 6 | Outcome & Telemetry | **Broken** | Attribution is exact and works. Nothing emits metrics; `/metrics` is empty |
| 7 | Evolution Brain | Partial | Evolution turns in `qip-deepbrain`. Champion/challenger and drift detection are not wired |
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
| 7 | Multi-leg execution | Area 5's remaining half: reservation, deadlines, leg risk, unwind | Deterministic recovery tests for partial and failed legs |
| 8 | Champion/challenger and drift | Area 7's promotion path | Shadow evaluation with a recorded promotion and a rollback |

## Risk register

| Risk | Consequence | Mitigation |
|---|---|---|
| A control that cannot fire | Reads as protection, is not. Already true of `MaxExpectedShortfall` | Item 5; and the rule that a new limit must ship with a test proving it fires |
| Documentation drifting ahead of code | An operator believes a control exists | `documentation.rs` refuses overclaims; it has already caught one |
| A test that passes by measuring nothing | Permanent false confidence | Mandatory mutation verification; premise-before-conclusion |
| Parallel agents overwriting each other | Work destroyed and redone, discovered late | Disjoint path ownership named in every brief, including the other agents' paths |
| An agent dying mid-task | Half-finished code in the tree | Finish it or park it on a WIP branch; never discard, never leave the tree red |
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
