# ADR 0150: Solver scope is the portfolio QUBO and the bounded in-tree constraint solver; no other quantum family without a measured benefit, no neural coupling, no solver crate

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval ("I
  allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises this record to fix the *scope* of the
  platform's solvers and to say what is not built. It does not authorise a
  dependency, an edit to `scripts/dependency-allowlist.txt`, a risk number, a
  hosted quantum provider or any change to the paper-trading layers; each of
  those is listed under "Not decided here".
- **Date**: 2026-10-19
- **Extends**: ADR 0006 (a classical baseline every time), ADR 0134 (the
  hybrid solver router is `ComputeRouter`; feature-subset selection named the
  next family) by adding the entry condition 0134 did not state, ADR 0125 (an
  advisory stage may veto or narrow, never enlarge) by deciding what an
  incomplete proof trace does, ADR 0130 (venue `SettlementTerms` are the
  settlement calendar) by naming it the only door for a business-day rule.
- **Keeps**: ADR 0099 conflict C5 and ADR 0122 (the offline research lane may
  use any numerical ecosystem; no solver or Python process is ever a step in a
  cycle), ADR 0109 (a crate is admitted on a measured cost, from a committed
  list), ADR 0003 (paper only).

## Context

Eighteen register rows ask, in four dialects, one question: how far does the
platform's solving machinery extend, and what do its results do to a size?

**The quantum side.** The router (`ComputeRouter`) already runs the classical
solve every time and adopts a quantum answer only if it is feasible and beats
the baseline by a margin. `Foundry::platform()` registers one family,
`PortfolioAllocation`, and records the other six `FamilyKind`s as not built;
`no_blueprint_family_is_silently_absent_from_the_platform_foundry` pins that.
The rows ask what hands the router a quantum-eligible problem (QUANT-4032,
4064, 4018), whether collateral, funding and placement get a QUBO (4057,
4034), whether a continuous problem should be solved as a QUBO too (4063),
whether quantum outputs may become model features (4061), and whether the
symbolic stage should pose a problem so that a family can be registered
(4040).

**The symbolic side.** `symbolic_stage::derive` composes a rule pack, a
timeline, a Bayesian network, a constraint problem, a lifecycle and an
intervention into one `SymbolicDerivation` per hypothesis, and
`symbolic_stage::verify` re-derives every part from the record and refuses the
first that does not reproduce (five corruption tests). The rows still call
PARTIAL because the requirement text names a neural model (REASON-1132 to
1136), a business-day calendar (3055), a tick grid (3057), separate compute
(9005), a SAT/SMT/MILP solver (1179, and the tail of 1136), a refusal for a
conclusion without a complete trace (COVERAGE-7181) and a rule that an
unresolved contradiction blocks money movement (RES-7106).

**What the code says, where the rows are stale.** Read before deciding; the
code wins where it differs.

- The rows say the default `RoutingPolicy` needs fifty assets. It does not:
  `minimum_assets_for_quantum` defaults to the exact-enumeration limit plus
  one, because at or below that limit the baseline is exhaustive and a quantum
  answer can at best tie. The consequence is sharper than the rows state:
  against `qip_quantum::statevector::MAX_QUBITS` the window in which a problem
  is both too large for exact enumeration and small enough for the in-tree
  simulator is a handful of variables wide (two, with today's constants).
  `ComputeRouter::with_quantum` refuses a policy no provider can reach.
- The rows say `quantum_enabled` is set by no app. `qip-deepbrain` now sets it
  through `ComputeSettings::from_lookup` when `QIP_QUANTUM_PROVIDER=simulated`,
  which the catalogue renders from `deepbrain_quantum_provider`, and the dev
  and test tfvars say `simulated`. `hosted` is refused by name. So the arm is
  attached in dev and test deep brains, on the simulator, and off elsewhere.
- The row for QUANT-4064 cites vacuous text-search tests in
  `arch_033_quantum_path.rs`. That file was rewritten to drive the real chain;
  the citation is stale.
- `Foundry`'s `Registered` status means "has an encoding, a baseline and a
  threshold", not "has shown a benefit". `PortfolioAllocation` is Registered
  while `RoutingDecision::quantum_note` says on every result that simulating
  the circuit costs more than solving the problem it encodes. The word
  `Registered` therefore must not be read as a claim of advantage.
- `symbolic_stage.rs` and `constraint.rs` cite C5 as "Z3 refused". C5 itself
  decided only the research-runtime half (ADR 0122). The in-platform solver
  question was never decided in its own right; this record decides it.
- The settlement lag in `SymbolicInput.settlement` comes from the asset-class
  registry's convention, in calendar days. The weekday and holiday machinery
  exists in `qip_edge::settlement::SettlementTerms`, and `qip-kernel` does not
  depend on `qip-edge`.

Locate: `grep -n 'pub enum FamilyKind\|pub fn platform' backend/crates/libs/qip-quantum/src/foundry.rs`,
`grep -n 'minimum_assets_for_quantum\|quantum_on_continuous' backend/crates/services/qip-optimization-engine/src/router.rs`,
`grep -n 'pub const MAX_QUBITS' backend/crates/libs/qip-quantum/src/statevector.rs`,
`grep -n 'fn with_compute_settings' backend/crates/apps/qip-deepbrain/src/main.rs`,
`grep -n 'pub const MAX_ASSIGNMENTS\|Z3' backend/crates/services/qip-reasoning-engine/src/constraint.rs`.

## Decision

### A. Quantum families

1. **The portfolio QUBO is the only quantum family.** No QUBO is formulated for
   collateral, funding, capital placement (QUANT-4057, QUANT-4034) or any other
   problem the platform solves today. `Platform::pre_position` chooses the
   cheapest signed corridor, a polynomial problem with an exact answer; a
   quantum path on it could at best tie, and ADR 0006 never adopts a tie. The
   capital-invariant and transaction-cost check the row asks for is already
   closed for the one family that has a quantum path
   (`a_quantum_allocation_that_wins_only_by_ignoring_transaction_cost_is_refused`)
   and is not built for families that have none; its absence is the
   consequence of there being nothing to check, not a gap.
2. **A family is built only on a stated benefit case.** ADR 0134 decision 5
   named `FeatureEnsembleSelection` first in line; this record turns "first in
   line" into an entry condition. A new family's encoding, baseline and
   benchmark may be written only when a committed note shows (a) the sizes the
   platform actually poses lie above the exact-enumeration limit, so the
   classical baseline is a heuristic that something could beat, and (b) they lie
   at or below the provider's qubit ceiling, so the arm can run at all. A
   family whose instances are enumerable, or whose instances exceed the
   simulator, is not eligible however attractive its encoding. Greedy
   forward selection over fewer than the enumeration limit's worth of features
   fails (a) and is therefore not built on today's constants.
   `FeatureEnsembleSelection` stays recorded as not built; that is the
   existing state and this record keeps it deliberately.
3. **Quantum stays off by default, and no default-path problem is invented to
   switch it on.** No fixture, seam or mandate is added to hand the router the
   held book plus the thesis, or four simultaneous approved theses, merely so
   the arm runs end to end in the default composition (QUANT-4032, QUANT-4064).
   A cycle approves one thesis, so DECIDE's QUBO has one variable and the router
   chooses exact enumeration; that is the router working. The quantum arm's
   behaviour is proven by tests that script the provider
   (`a_position_sized_from_a_winning_quantum_answer_is_still_refused_by_pre_trade_risk`,
   `a_discrete_solve_is_stated_in_the_quantum_contract_with_its_classical_baseline_recomputed`,
   the `arch_033_quantum_path.rs` chain) and by dev and test deployments that
   attach the simulator. A problem engineered to be quantum-eligible would
   produce a benchmark record for a workload nobody has, which reads as
   evidence and is not.
4. **A continuous problem has no QUBO and gets none** (QUANT-4063).
   `quantum_on_continuous` stays false; `ComputeRouter` does not discretise a
   quadratic program to state it in the quantum contract. The contract's
   classical result is a classical solve of the *same QUBO the quantum arm was
   sent*, and for a continuous problem no such QUBO exists; inventing one so a
   record can be written would compare the quantum arm to a problem the
   platform does not pose. `quantum_routing::contract_of` returning `None` for a
   continuous solve is the correct, journaled absence. The default
   `Mandate.maximum_holdings` stays `None`: whether the default mandate carries
   a holdings cap is a risk setting, not a solver setting (see "Not decided
   here").
5. **No quantum-informed candidate is produced, and no quantum-derived feature
   is admitted** (QUANT-4018, QUANT-4061). `promote_model_from` keeps its
   refusals (a gateway output is never a candidate; a `QuantumInformed` one
   needs a named, built family and a calibrated baseline) and gains no
   producer. Because `Registered` is not a benefit claim (Context), the seam is
   necessary and not sufficient: no binary may construct
   `ModelProducer::QuantumInformed` until a family has a committed benefit
   record against a non-simulated provider, which `ComputeSettings` refuses by
   name today. A model trained on features computed by a simulator that costs
   more than the problem it encodes inherits a cost it cannot explain.
6. **No cycle problem is made QUBO-admissible** (QUANT-4040). The symbolic
   stage does not pose a 0/1 linear-equality problem each cycle to give
   `SymbolicCombinatorialSearch` a caller. `acting()` has Boolean domains and
   `Or` conditions that `Problem::to_qubo` refuses by name, and it should: it
   is a handful of variables the exact solver decides instantly. The family
   stays `NotBuilt` with its existing reason, which is true. The benchmark of a
   constraint QUBO against a quantum entrant stays what it is, a capability
   test of the encoder.

### B. Symbolic scope

7. **Neural coupling is out of scope** (REASON-1132, 1133, 1134, 1136). The
   symbolic stage's guarantee is that a person can re-derive a conclusion
   step by step and a machine can re-check it; a neural output has no such
   trace, and the platform's rule is that nothing it cannot replay may touch a
   size. No neural model exists in the symbolic crates (`grep -rli neural
   backend/crates/runtime/qip-kernel backend/crates/services/qip-reasoning-engine`
   finds nothing) and none is admitted here. Recorded-and-reproducible,
   which each of those four rows already holds with a fired mutation, is the
   complete requirement. If a trained model's output is ever to inform
   reasoning, it enters as a signed model pack (ADR 0122) and as an ordinary
   forecast under ADR 0125's two powers, not as a coupled stage.
8. **Reasoning stays in-process; the operator surface is `qip replay`**
   (REASON-9005). Reasoning on separately provisioned compute is not wanted:
   a derivation that cannot be re-run by the binary that wrote it is a claim
   from elsewhere, and ADR 0122 already fixes that no solver process is a step
   in a cycle. What is wanted is that the in-tree checker runs somewhere other
   than tests. `qip replay` already verifies the chain and compares three
   registries rebuilt from the log; it gains a fourth answer,
   `symbolic_stage::replay`, with the same exit-code contract. Where the
   replayed platform's causal graph is not the one the log would rebuild, the
   intervention part is reported as not checked, never as agreeing.
9. **The settlement lag stays a calendar-day lower bound until the kernel holds
   venue terms** (REASON-3055). A weekend or holiday can only make settlement
   later, so a lower bound can wrongly *admit* a link but never wrongly refuse
   one; the error runs in the direction that costs a missed finding, not a
   false veto. A business-day rule enters only by consuming the venue's
   `SettlementTerms` (ADR 0130), whose weekdays and operator-entered holidays
   are committed data, and never from a fetched vendor calendar, whose
   licence would first have to be evaluated and whose contents change under a
   replay. State-transition reasoning beyond hypothesis status is out of scope:
   `Lifecycle` judges admissibility, and a planner is separate large work with
   no consumer.
10. **No tick-grid or contract-specification rule yet** (REASON-3057). The
    tape the platform holds is `f64` (`price_history`), while the instrument's
    `tick_size` is a `Decimal`; an on-tick rule over the former rules against
    every synthetic price, which is a control that fires on noise. The rule
    waits for a decimal tape. Contract rules wait for the derivative
    extension's expiry to be carried into `SymbolicInput`. Accounting stays
    the one `i30` invariant: double-entry already lives in the ledger (ADR
    0130), and a second implementation inside a pre-trade advisory would be a
    second source of truth for a fact the log holds.

### C. No solver crate

11. **No SAT, SMT or MILP crate is admitted, and none is hand-rolled beyond
    what exists** (REASON-1179). The scope of constraint solving is
    `qip-reasoning-engine::constraint`: finite-domain backtracking search up to
    `MAX_ASSIGNMENTS`, bounded linear integer arithmetic, minimal conflict by
    deletion, `minimise`, and `to_qubo`. A problem above the bound is
    *refused*, never left to run. Admission under ADR 0109 decision 2 needs
    "a measured cost the in-tree alternative cannot meet"; there is none:
    the only problem the cycle poses is `acting()`, a few Boolean variables.
    Growth beyond the bound is a new decision with a measurement, not a drift.
    Z3 and OR-Tools remain available to the offline research lane (ADR 0122).
12. **Revival condition.** A committed measurement showing a constraint problem
    the platform really poses (not a fixture) is refused for exceeding
    `MAX_ASSIGNMENTS`, plus a named pure-Rust crate with a deterministic model
    order and checkable certificates, reopens this. The admission itself is then
    an edit to `scripts/dependency-allowlist.txt`, which is not this record's
    to make.

### D. Sizing consequences of unproven or contested reasoning

13. **A conclusion without a journaled derivation forms no proposal; one whose
    derivation has a refused part is narrowed, not vetoed** (COVERAGE-7181).
    Completeness is defined on the derivation that already exists, not on a trace
    field added to `ReasoningOutcome` (a second copy of the trace would be a
    second thing to disagree). Two states, extending ADR 0125 decision 2:
    - *Absent.* No `SymbolicDerivation` was formed or it was not journaled
      (`reason_symbolically` returns an empty detail when the opportunity names
      no anomaly or subject, and reports a journal failure only as a problem
      string). The conclusion cannot be reproduced from the log alone, which
      the product direction makes a precondition, so the hypothesis forms no
      proposal. No number is involved.
    - *Present with a `Step::Refused` part.* A malformed timeline is a finding
      about the evidence (ADR 0125 decision 2 says so); it may not silence the
      hypothesis. It narrows the size through the same `(0, 1]` map as every
      other advisory narrowing. The multiplier's value is a risk number and is
      not decided here; until the risk lane supplies it the narrowing arm is not
      wired and the part stays recorded in `violated_assumptions`, which is the
      status quo and loosens nothing.
14. **A contested thesis stays held small and scored; an unresolved
    contradiction does not block a proposal** (RES-7106).
    `Hypothesis::confidence_for_sizing` narrows on dissent and is deliberately
    not folded into the action bar. Blocking would convert "hold this contested
    thesis small and then score it" into "never form a view on a contested
    question", which stops the platform learning about exactly the cases it was
    least sure of. The row's clause that an unresolved contradiction marks the
    dependent proposal infeasible is withdrawn. `ContradictionResolution` has
    one consumer, `trust_score`; a derivation trace and an alternates search
    over `ResolutionMethod` may be built later, advisory only (ADR 0125), when
    something named consumes them.

## Dissent: admit a solver crate

The strongest case against decision 11, stated as its holder would state it.

ADR 0109 says the cost of a dependency is the supply chain and audit surface,
and that "at real-money scale an audited crate is usually the smaller risk
than an in-tree rewrite". Constraint solving is the textbook place that holds.
Solvers are subtle: symmetry, conflict analysis and arithmetic corner cases
have produced wrong UNSAT answers in mature tools. The in-tree search is a
backtracking enumerator whose "minimal conflict" is found by deletion and whose
only independent check is `minimise` on the same enumerator, so it is
cross-checked against itself. It refuses past 2^20 assignments, so it can only
ever gate toy problems: thirty Boolean mandate or jurisdiction flags are out of
reach, and real position, lot and compliance constraints are MILP-shaped. The
platform has admitted `rustls` and `ring` on audit; refusing a pure-Rust SAT
solver while admitting TLS is inconsistent. And the asymmetry favours the
crate: a satisfying model is checked in linear time by the in-tree evaluator,
and an UNSAT answer can carry a proof a small in-tree checker verifies, so the
solver would sit *outside* the trusted base, supplying certificates that the
product direction ("evidence a person can check, not something the system
asserts about itself") wants more than an enumerator's bare assignment.

**Why it lost.** (a) ADR 0109 decision 2.1 demands a measured cost, and there is
none: nothing the cycle poses is near the bound, and the dissent's thirty-flag
problem is not posed by any code. Admitting a solver for problems the platform
does not have buys an audit surface for no use. (b) Reproducibility.
`verify` re-solves the acting problem and compares the *answer*. A CDCL or
simplex solver may return a different but valid model after a version bump or
a heuristic change, and the platform's replay would then fail on a derivation
that is correct; the in-tree search tries values in declared order and is
deterministic by construction. A proof-checking layer would be needed first,
and that is a second build larger than the problem. (c) The veto path of ADR
0125 rides on `RulePack::verify`, not on the constraint solver, so a crate
would change no decision. (d) The research need is met without it: the offline
lane may use any solver and deliver a signed pack (ADR 0122). (e) Admission is
a protected edit and cannot be made here.

**What would revive it.** Decision 12: a measured, refused, real problem; a
deterministic audited crate with certificates; then its own ADR.

## Consequences

- The register rows split cleanly: fourteen close as decisions with corrected
  wording and no new code; REASON-9005 gets a small code slice in
  `qip-cli`; QUANT-4018 and QUANT-4061 share one guard test; and
  COVERAGE-7181 gets a kernel slice bound to ADR 0125's wiring.
- Nothing is switched off: dev and test keep the simulated provider and every
  existing refusal stays.
- `Registered` in the foundry is documented as "built", not "beneficial".
- Decision 2 narrows what ADR 0134 allows: `FeatureEnsembleSelection` is not
  built on a sub-enumeration-limit feature count.

## What it costs

- No quantum experiment on anything but the portfolio problem, so the platform
  accrues no evidence on other families and cannot say they are worthless; it
  can only say it will not build them to find out on a simulator.
- The quantum arm is exercised end to end only by scripted tests and by dev and
  test deployments; a regression that only appears under a real eligible cycle
  would not be seen until one occurs.
- Rows stay PARTIAL on their original wording unless the register is reworded;
  the register text now carries "closed by decision", which a reader hunting
  for capabilities will read as less than the blueprint wanted.
- A lower-bound settlement check misses findings a calendar would catch, and
  the constraint solver cannot grow without a new decision.
- Every conclusion with a refused derivation part waits on a risk number before
  it narrows; until then it is sized as it is today.

## What would make this wrong

- A committed benchmark on non-simulated hardware showing a family beating its
  baseline by its threshold at sizes the platform poses: decision 2 is too
  strict, and the entry condition becomes the evidence of record.
- A constraint problem the platform poses and the search refuses, observable as
  a `MAX_ASSIGNMENTS` refusal in a journaled derivation: decision 11 is wrong.
- Declined theses with an incomplete derivation whose counterfactual score is
  persistently positive: decision 13's absence veto is silencing real edge, and
  it should narrow too.
- Contested theses sized small that score worse than uncontested ones by more
  than the narrowing explains: decision 14 under-protects.

## Code it constrains

- `backend/crates/libs/qip-quantum/src/foundry.rs`, `Foundry::platform`
  (`grep -n 'pub fn platform' ...`).
- `backend/crates/services/qip-optimization-engine/src/router.rs`,
  `RoutingPolicy`, `ComputeRouter::with_quantum`.
- `backend/crates/runtime/qip-kernel/src/model_serving.rs`,
  `require_eligible_workload`, `Platform::promote_model_from`.
- `backend/crates/runtime/qip-kernel/src/quantum_routing.rs`, `contract_of`;
  `platform.rs`, `route_quantum_decisions`, `reason_symbolically`,
  `reason_about_the_queue`.
- `backend/crates/runtime/qip-kernel/src/symbolic_stage.rs`, `derive`,
  `verify`, `replay`, `Step`.
- `backend/crates/services/qip-reasoning-engine/src/constraint.rs`,
  `Problem::solve`, `MAX_ASSIGNMENTS`; `hypothesis.rs`,
  `Hypothesis::confidence_for_sizing`.
- `backend/crates/apps/qip-cli/src/replay.rs`, `verify`.

## Rows unblocked

- R12-QUANT-4018: no producer of `QuantumInformed` is built; a guard test pins it.
- R12-QUANT-4032: no default-path eligible problem is invented; the arm is proven by scripted and dev/test attachment.
- R12-QUANT-4064: same; the PreTradeChecker refusal test stands as the end-to-end evidence.
- R12-QUANT-4057: the clause is scoped to the portfolio family; no other family has a quantum path.
- R12-QUANT-4063: no QUBO for a continuous problem; holdings cap default is a risk matter.
- R12-QUANT-4034: placement stays polynomial; no QUBO.
- R12-QUANT-4061: no quantum-derived features.
- R12-QUANT-4040: no cycle problem is made QUBO-admissible; the family stays not built.
- R12-REASON-1132, 1133, 1134: symbolic-only reproducibility is the requirement; neural coupling is out of scope.
- R12-REASON-1136: same, and no SAT/SMT/MILP (decision 11).
- R12-REASON-3055: calendar-day lower bound stays; venue terms are the only door for a business-day rule.
- R12-REASON-3057: tick grid waits for a decimal tape; accounting stays one invariant.
- R12-REASON-9005: separate compute not wanted; `qip replay` gains the symbolic check.
- R12-REASON-1179: no solver crate admitted; the bounded in-tree solver is the scope.
- R12-COVERAGE-7181: absent derivation forms no proposal; a refused part narrows (value pending).
- R12-RES-7106: contested theses stay held small and scored; blocking withdrawn.

## Not decided here

- **RISK**: the narrowing multiplier for a derivation with a refused part (decision 13); and whether the default `Mandate.maximum_holdings` should stop being `None` (decision 4). Rows: R12-COVERAGE-7181, R12-QUANT-4063.
- **PROTECTED**: any admission of a solver crate is an edit to `scripts/dependency-allowlist.txt`, with its ADR, and is not made here. Rows: R12-REASON-1179, R12-REASON-1136.
- **BILLED**: a hosted quantum provider, its credential and per-job budget, which would be the only route to a real benefit record. Rows: R12-QUANT-4018, R12-QUANT-4061.
- **PAPER-BOUNDARY**: nothing here touches an order path; the pre-trade checker still decides after every solver, unchanged.
