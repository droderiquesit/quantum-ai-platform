# Quantum problem sizes against ADR 0150 A2, measured 2026-11-14

Tree read: main at `ae810895` (`git rev-parse --short HEAD` in the read-only worktree). Nothing was built or run. Every figure below is a read of a constant or a committed data file, with its location.

## What A2 asks

ADR 0150 decision A.2 (`docs/adr/0150-solver-scope-...md`, "A. Quantum families", item 2) allows a new family's encoding, baseline and benchmark only when a committed note shows both:

- (a) the sizes the platform actually poses lie above the exact-enumeration limit, so the classical baseline is a heuristic something could beat;
- (b) they lie at or below the provider's qubit ceiling, so the arm can run at all.

A family whose instances are enumerable, or exceed the simulator, is not eligible. A.6 separately keeps `SymbolicCombinatorialSearch` NotBuilt. Owner decision 24 and the EXPAND-6036 ruling (`docs/blueprint/owner-decisions-2026-10-31.md:995`, `:1639-1645`) restate the test as: a posed size in (22, 24].

The two bounds:

- Exact-enumeration limit: 22 variables. `DEFAULT_EXACT_ENUMERATION_LIMIT: usize = 22`, `backend/crates/services/qip-optimization-engine/src/router.rs:88`. Enumeration is used when `problem.dimension() <= exact_enumeration_limit` (`router.rs:2033`).
- Provider qubit ceiling: 24. `pub const MAX_QUBITS: usize = 24`, `backend/crates/libs/qip-quantum/src/statevector.rs:23`. A hosted provider is refused by name today (ADR 0150 Context), so the simulator's 24 is the only ceiling in force.

Eligible window: 23 or 24 binary variables. Two sizes wide.

## What the platform poses today

- Committed universe: 6 instruments. `python3` count of `instruments` in `data/datasets/universe.json` (NWSC, VNTG, MRDN, ATFB, HLCX, RSRC). The loop tape (`data/datasets/loop-demonstration-tape.json`) holds 2400 observations; each is one OHLCV bar for one of those objects, not a feature or a problem instance. `data/datasets/yield-shelf.json` has 0 instruments.
- Scenario library: 6 scenarios. `standard_library()` at `backend/crates/services/qip-simulation-engine/src/scenario.rs:112` lists equity-crash-1987, credit-crisis-2008, pandemic-2020, inflation-shock-2022, liquidity-freeze, correlation-breakdown.
- Decision 24 records that no single constant bounds the parked families' sizes and that no per-cycle cap on approved theses was found. This note found the same.

## Table

Sizes are binary variables (qubits for an encoding that maps one variable to one qubit). "Ceiling" means a code bound, not a size the platform poses.

| Family (rows) | Measured size | Source | (a) above 22? | (b) at or below 24? | Verdict |
|---|---|---|---|---|---|
| GraphSearch (QUANT-1077, 4033) | No instance. No cycle whitelist is committed; the only reference is a comment in the execution-node template, `infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl:335`. Ceilings: 256 conversions per pass, cycles of 2 to 20 legs | `backend/crates/apps/qip-edge-node/src/arbitrage.rs:105`; `backend/crates/edge/qip-arbitrage/src/search.rs:27,37` | Not shown | Not shown | Does not qualify. No posed size exists to compare. The 256 ceiling is above 24; a candidate-cycle count over it would exceed the simulator. |
| FeatureEnsembleSelection (QUANT-1078, 4035) | No committed feature catalogue. Ceilings: 256 features per estimator set; blueprint budget about 200 | `backend/crates/services/qip-training/src/estimators.rs:71` and its doc comment | Not shown | Ceiling 256 is above 24 | Does not qualify. The only committed data is 6 objects of price bars; a posed count is unmeasured, and the ceilings exceed 24. ADR 0150 A.2 already names this family as not built. |
| KernelsAndVariationalModels (QUANT-1079, 4036) | No posed binary-variable count. A kernel or variational model's qubit count is set by its own feature map, not by a platform problem | `backend/crates/libs/qip-quantum/src/foundry.rs:166-188` (NotBuilt) | Not applicable | Not applicable | Does not qualify. There is no enumerable QUBO baseline to exceed, and no size to test. |
| ReservoirAndExtremeLearning (QUANT-1080, 4037, 4054) | Same as kernels: no posed binary-variable count | `foundry.rs:166-188` | Not applicable | Not applicable | Does not qualify, for the same reason. QUANT-4054 depends on this family and is blocked with it. |
| ScenarioHypothesisSelection (QUANT-1081, 1145, 4041) | Library: 6. Ceilings: 256 scenario branches; 64 synthesised hypotheses per call | `scenario.rs:112`; `backend/crates/apps/qip-deepbrain/src/research.rs:1495`; `backend/crates/services/qip-world-model/src/synthesis.rs:39` | No: 6 is at or below 22, so exhaustive search decides it | n/a | Does not qualify. The library is enumerable (A.2 excludes it). The 64 and 256 ceilings are not posed sizes and would exceed 24 anyway. |
| SymbolicCombinatorialSearch (QUANT-1145) | At most 20 Boolean variables: the exact solver refuses a space above 2^20 | `backend/crates/services/qip-reasoning-engine/src/constraint.rs:37` (`MAX_ASSIGNMENTS = 1 << 20`) and `:157`. The cycle's acting problem is "a handful of variables" (ADR 0150 A.6) | No: at most 20 | n/a | Does not qualify. Enumerable by construction, and A.6 independently keeps it NotBuilt. |
| Portfolio QUBO (EXPAND-6036; the active family, not parked) | One variable per approved thesis (`dimension()` is `assets.len()`, `qip-optimization-engine/src/problem.rs:162-164`). One thesis per cycle by ADR 0150 A.3. Retained queue ceiling 256 | `qip-kernel/src/platform.rs:1591` (`PROPOSAL_HISTORY`), `:18306-18312` | No: 1 per cycle, 6 distinct objects committed | Not reached | Does not qualify. The 256 retention ceiling is storage, not a per-cycle size, and decision 24 records that no per-cycle cap exists. |
| QUANT-1158 (umbrella: five parked families) | Follows the five rows above | owner-decisions line 1819 | n/a | n/a | Does not qualify. Stays PARTIAL with six families not built. |

Row coverage: 1077, 1078, 1079, 1080, 1081, 1145, 1158, 4033, 4035, 4036, 4037, 4041, 4054, EXPAND-6036. All fourteen appear above.

## Conclusion

No family qualifies to be built now. Each family either has no committed instance to size (graph, feature, kernel, reservoir), or its measured size is at or below 22 and so enumerable (scenario library 6, symbolic at most 20, one thesis per cycle), or it has only ceilings above 24 that are not sizes the platform poses. No measured size lies in (22, 24]. A size is not stretched: a ceiling is not a posed instance, and a ceiling above 24 would fail (b) in any case.

The parked patches stay unlanded, and no gate is weakened.

## What would change a verdict

A verdict changes only on a committed, measured instance, not a ceiling. For graph search: a shipped cycle whitelist whose candidate set, in the encoding the family would use, has 23 or 24 binary variables. For feature selection: a committed feature catalogue whose selection problem has 23 or 24 candidates. For scenario or hypothesis selection: a real hypothesis or scenario set of 23 or 24 members posed in one solve. For the portfolio QUBO: a per-cycle approved-thesis count of 23 or 24, which needs a cap or a measured run over a real universe (decision 24; a cap is a figure for the owner to give). Kernel and reservoir need a stated binary encoding first. Raising the enumeration limit above 22 or the qubit ceiling above 24 would move the window; the first is refused above 30 at `router.rs:512`, and a hosted provider would need its own ADR, so neither is assumed here. Even a size in the window only permits the encoding, baseline and benchmark to be written; ADR 0006 still requires a measured win over the classical baseline before any quantum answer is adopted.
