# ADR 0430: A hard limit is evaluated only against a deterministic gate the cost router mints, and the risk monitor demands it in its signature

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-02 (wave580). An in-repository
  design decision; it adds a witness to a call, removes no control, widens no
  authority and touches no paper-trading layer.
- **Date**: 2026-11-12
- **Reads**: [ADR 0127](0127-the-capital-side-brains-are-crate-boundaries-proven-by-closure-tests-and-loops-the-kernel-composes-not-new-binaries.md)
  (the survival-kernel extraction, which this does not perform).
- **Rows**: R12-RISK-3022 (stays PARTIAL).

## Context

R12-RISK-3022: "The Risk Brain only recommends; hard limits are enforced by
deterministic gates that never route to a model." Verification: a decision
marked `Determinism::Required` cannot be routed to any model tier.

The router half held: `Router::determine` returns `DeterministicRouting`, a
type that cannot name a model rung, and the kernel obtains one before the
pre-trade check and, since 2026-10-24, before the risk monitor's pass in ACT.
The skeptic's finding on 6c80831e was that the monitor routing is a
**record**: `Platform::observe_book_under_routing` stored it in
`monitor_routing` and then called `RiskMonitor::observe`, which took no
routing. A second call site written without asking the router compiled and
evaluated the limits exactly as well; only a source scan inside a kernel unit
test noticed.

The obvious fix — have `observe` take `&DeterministicRouting` — is not
available. `qip-cost-router` depends on `qip-ai`, `qip-risk-engine` is in
`SAFETY_CRITICAL`, and `no_safety_critical_engine_can_reach_a_language_model`
refuses any edge from it to a crate that reaches a model.

## Decision

1. **A model-free witness type, `qip_risk::gate::DeterministicGate`.** It
   lives in `qip-risk`, which is model-free, already in `SAFETY_CRITICAL` and
   already a dependency of the risk engine. It carries the routing's
   rationale, and is deliberately neither `Clone` nor `Deserialize`, so it
   cannot be duplicated past the routing that minted it or read off a wire.
2. **The cost router mints it.** `qip-cost-router` gains an inward edge to
   `qip-risk` (a service on a library), and `DeterministicRouting::gate()`
   mints one from the routing's own rationale. That method is the only
   production call of the constructor.
3. **`RiskMonitor::observe` demands it** as its first argument and copies its
   rationale onto every `Observation` as `routed`. A caller that never asked
   the router has nothing to pass, and the verdict's routing is on the record
   of the evaluation itself, not in a field kept beside the call.
4. **Forgery is refused by scan, and this ADR says so.** Rust cannot limit a
   public constructor to one other crate. `qip-acceptance`'s
   `architecture::only_the_cost_router_mints_a_deterministic_gate` refuses
   the constructor's name in the production source (every `src/` file up to
   its first test module) of every crate but `qip-cost-router`'s
   `router.rs`, requires that file to mint exactly once, and requires
   `observe`'s signature to name `gate: &DeterministicGate`. Tests may mint
   one: a unit test of the monitor is not a path an order takes.

## What this does not decide

- It does not move the survival waterfall (`qip-kernel/src/survival.rs`) out
  of the kernel. ADR 0127 decided that extraction and R12-CAPITAL-7204 owns
  it; lane t50-19 holds that row in the same wave, and two lanes moving one
  file is a conflict, not progress.
- It does not put the gate on `PreTradeChecker::check`, `MinLiquidity` at
  submit, sign-off and release gating, or the kill-switch path beyond
  `RiskMonitor::enforce`. Those remain kernel-side gates without a witness in
  their signature, and are the row's remaining gap.

## What it costs

- One new edge, `qip-cost-router` on `qip-risk`. It points inward and adds
  nothing to what `qip-risk` reaches.
- Every caller of `RiskMonitor::observe`, tests included, must hold a gate.
  Tests mint their own; production must ask the router.
- The guarantee against forgery is a source scan, not the type system. A
  scan can be defeated by code it does not read (a macro, a build script),
  and that weakness is accepted rather than hidden.
- The observation record grows one string per pass. `Observation` is held in
  memory by the monitor and is not persisted by any caller today.

## Consequences

- A second production call of the monitor without a router-minted gate fails
  to compile (no gate) or fails the architecture suite (forged gate).
## What would make this wrong

A path that holds a `DeterministicRouting` obtained other than from
`Router::determine` — `DeterministicRouting` derives `Deserialize`, so one
read off a wire could mint a gate. No production code deserialises one
today; if any starts to, the witness proves less than its name says.
