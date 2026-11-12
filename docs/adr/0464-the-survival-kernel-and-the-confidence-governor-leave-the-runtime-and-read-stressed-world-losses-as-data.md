# ADR 0464: The survival kernel and the confidence governor leave the runtime, and read stressed world losses as data

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31
  delegation**, session t50-19 (wave580). An in-repository design decision
  carrying out ADR 0127 decision 3 for two of its three modules; it moves
  code, changes no figure, removes no control, widens no authority and
  touches no paper-trading layer.
- **Date**: 2026-11-12
- **Amends**: [ADR 0127](0127-the-capital-side-brains-are-crate-boundaries-proven-by-closure-tests-and-loops-the-kernel-composes-not-new-binaries.md)
  decision 3, on what moves with the survival kernel.
- **Rows**: R12-CAPITAL-7204 (stays PARTIAL: the Hedge Brain has not moved).

## Context

R12-CAPITAL-7204 asks that no alpha or model component can construct, relax
or bypass a capital, hedge, model-risk or survival decision, verified by
"survival/risk crates do not depend on alpha/model crates and alpha crates
cannot construct a permit". The survival kernel (`survival.rs`), the
confidence governor that is the model-risk control (`confidence.rs`) and the
Hedge Brain (`hedge.rs`) were modules of `qip-kernel`, whose closure holds
every alpha crate, so `survival_independence.rs` could only scan their source
for the substring `<alpha_crate>::` — which `use qip_strategy as s;` evades.

ADR 0127 decided they move to `services/qip-survival` and that `StressReport`
moves with them. Doing it showed three couplings the decision did not see:

1. `StressReport` is built from `qip-simulation-engine`'s `ScenarioResult`
   and `AdversarialSequence`, and the simulation engine depends on
   `qip-strategy` and `qip-evolution`. Moving the report would put two alpha
   crates in `qip-survival`'s closure — the property the move exists to
   prove.
2. `confidence.rs` named `qip_cost_router::context::MarketRegime`; the
   router depends on `qip-agents` and `qip-ai`.
3. `confidence.rs` read `FeatureStatisticsReview`, whose shifts carry
   `qip-training`'s `StreamingDrift`. Through that one argument the model-risk
   governor's signature reached the training crate.

## Decision

1. **`services/qip-survival` holds `survival` and `confidence`**, depending
   on `qip-core`, `qip-contracts`, `qip-capital` and `serde` only, and is the
   Capital Brain's second crate (`brain_closure.rs`), so it reaches no other
   brain. `install_survival_limit`, which builds a Risk Brain limit, moves to
   the kernel's `survival_limit.rs`.
   `qip-kernel` re-exports both modules, so `qip_kernel::survival::…` and
   `qip_kernel::confidence::…` paths are unchanged.
2. **The survival kernel reads `StressedWorlds`, a list of `WorldLoss {
   scenario, loss_fraction }`**, and not `StressReport`.
   `StressReport::worlds` in the kernel assembles it from the library, the
   adversarial sequence, every causal driver and the tail worlds — the same
   four sources `worst_world` chained before, minus nothing. Data rather than
   a trait: ADR 0127's "What would make this wrong" names a trait object
   handed in by the kernel as the way a closure test is satisfied while a
   brain still consults alpha at run time. A record of two fields has no
   behaviour to substitute.
3. **The confidence governor reads `DistributionReading`** (each shift's
   population stability index and floor, and the quantised feeds), which
   `FeatureStatisticsReview::distribution_reading` builds in the kernel.
4. **`MarketRegime` moves to `qip_contracts::regime`**, re-exported from
   `qip_cost_router::context`. It is a measurement label, not a model, and
   ADR 0127 named this as one of the two acceptable remedies.
5. **`impl CapitalObligations for CrossMarginReview` moves to the kernel's
   `cross_margin.rs`**, beside the type; the trait stays in `qip-survival`.
6. **`survival_independence.rs` reads the graph from `cargo metadata`** and
   puts `qip-survival` in its survival set: its closure must reach no alpha
   crate, no alpha crate may reach it (the permit half — a crate that cannot
   name `SurvivalGovernor`, `SizingBudget` or `ConfidenceDecision` cannot
   construct one), and `survival.rs` / `confidence.rs` may not reappear in
   `qip-kernel/src`. The source scans of the two moved files are deleted;
   `tail.rs` and `hedge.rs` are still scanned because they are still in the
   runtime. The hand TOML reader is replaced because it missed a
   `[dependencies.qip-x]` table, a target-conditional section and a rename —
   each an edge the absence assertions could not see.

## Consequences

- The Hedge Brain (`hedge.rs`) stays in `qip-kernel`. It reads
  `qip_simulation_engine::scenario::standard_library()` and the scenario
  results themselves, not only their losses, so its move needs the scenario
  library out of the alpha-reaching simulation engine first. Until then its
  independence is still the source scan, and R12-CAPITAL-7204 stays PARTIAL.
- The test `every_survival_world_the_book_fails_withholds_a_reserve_named_for_it`
  moves from `survival.rs` to `tail.rs`: it generates worlds with `tail`,
  which `qip-survival` cannot name.

## What it costs

- One more workspace crate, and two more seams the kernel builds data
  across (`StressReport::worlds`, `distribution_reading`) each cycle — a
  `Vec` of names and losses and one of shifts, both bounded by the stress
  library and the instruments summarised.
- `install_survival_limit` lives in the kernel (`survival_limit.rs`), not
  beside the ceiling it guards; `qip_kernel::survival` re-exports it so no
  caller moved. The survival kernel is the Capital Brain's second crate, so
  it may not name the Risk Brain's limit types, and only the composition
  installs a Risk Brain limit on a Capital Brain figure (ADR 0127
  decision 5). ADR 0127 decision 3's allowance of `qip-risk` is withdrawn.
- A scenario name and a loss are all the survival kernel sees of a world. A
  reserve that needed more of a world would need this record reopened.

## What would make this wrong

- If a world the survival reserve must answer could not be expressed as a
  name and a loss fraction — a reserve that depends on the path of a
  sequence, not its terminal loss — `WorldLoss` would be too narrow and the
  kernel would be tempted to hand a richer, behaviour-bearing type back in.
