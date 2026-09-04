# Wave 7 — cycle conformance review: what each stage says versus what it does

**Scope.** A read-only trace of `Platform::run_cycle` in
`backend/crates/runtime/qip-kernel/src/platform.rs`, comparing the one-line
description each of the eight named stages carries in `CLAUDE.md` ("Seven
stages run in one cycle: SENSE, UNDERSTAND, DISCOVER, REASON, SIMULATE,
DECIDE, ACT, LEARN") and the fuller per-stage table in
`docs/architecture/README.md` against what the corresponding `fn stage_*`
actually does. No code in this repository was changed to produce this
report.

**Method.** `grep -n "fn stage_"` against `platform.rs` found the eight
functions at the line numbers cited below; each was read in full, along with
the helper functions it calls, and cross-checked against imports to see
which crate a stage's work actually reaches. Stage order and composition are
fixed by `backend/crates/runtime/qip-kernel/src/cycle.rs:17-26` (the `Stage`
enum) and driven in that order by `Platform::run_cycle`,
`platform.rs:2499-2513`.

**What CLAUDE.md says, verbatim.** "Seven stages run in one cycle: SENSE,
UNDERSTAND, DISCOVER, REASON, SIMULATE, DECIDE, ACT, LEARN." (The document
says "seven" while naming eight; that is a defect in the sentence, not in the
code — the code has an eight-variant `Stage` enum, `cycle.rs:17-26`, and all
eight run.) The fuller blueprint description used below as "what the design
says" for each stage is the table in `docs/architecture/README.md:44-53`,
which is the more specific and testable claim ("What it produces" per
stage), and is treated as the standing design description each stage is
scored against.

---

## SENSE

**Design says** (`docs/architecture/README.md:46`): crates
`qip-market-ingestion`, `qip-normalization`; produces "observations with
provenance."

**Code does** (`stage_sense`, `platform.rs:2799-2824`): reports counts —
`self.price_history.len()` (instrument count) and the sum of per-instrument
observation vectors (`observation count`), plus how many sources are
registered in `self.data_finder.registry()` (`platform.rs:2806`). It reads
state that was populated earlier by `Platform::observe`
(`platform.rs:2191`), which absorbs a `Vec<SensedRecord>` — the type is
imported from `qip_market_ingestion::adapter::SensedRecord`
(`platform.rs:98`) — into `price_history`, `volume_history`,
`spread_history`, etc. `observe` is a separate public method, not one of the
eight `stage_*` functions, and is not called from inside `run_cycle`
(`platform.rs:2474-2514` has no call to `self.observe`); it is called by
whatever composition root feeds records into the platform before a cycle
runs. `qip_normalization` is not imported anywhere in `platform.rs` (grep for
`qip_normalization` in the file returns no matches); if normalization
happens, it happens upstream of the kernel, inside `qip-market-ingestion` or
its caller, not visibly in this file.

**Verdict: PARTIAL.** The stage's own function does not sense anything — it
reports on sensing that already happened via `observe`, a method outside the
cycle's eight stages. That is a defensible split (absorption is
request-driven, "SENSE" as a stage is a report on the current state of
absorption, consistent with the code's own comment at
`platform.rs:2802-2805` that this is deliberately reporting "what the
platform has decided it should be collecting, next to what it is actually
receiving"), but it means the stage named SENSE does not itself perform the
work the design table credits to it; that work happens in `observe`, called
from outside the cycle. `qip-normalization`'s presence is not verifiable from
this file at all. No ADR was found addressing this split (searched
`docs/adr` and `docs/architecture` for `stage_sense`; no hits).

---

## UNDERSTAND

**Design says** (`docs/architecture/README.md:47`): crates
`qip-entity-resolution`, `qip-world-model`; produces "a bitemporal model of
the world."

**Code does** (`stage_understand`, `platform.rs:2826-2887`): calls
`self.world.state_at(now, now)` (`platform.rs:2831`) — both time arguments
are `now`, i.e. it reads the world model at the current instant in both the
valid-time and knowledge-time dimensions, which is exactly the bitemporal
read the design promises. It reports `state.object_count`,
`state.entity_count`, `state.relationship_count`, `state.causal_claim_count`,
`state.features.len()`, `self.world.index().len()` (documents), liquidity
coverage from `self.liquidity`, held market events, and — when a chain is
configured — chain height and confirmed-trade counts
(`platform.rs:2854-2871`). The comment at `platform.rs:2827-2830` explicitly
states this replaced an earlier version that quoted a price-history count
"while the model sat empty," i.e. the stage was previously not reading the
world model at all and has since been fixed to do so.

**Verdict: CONFORMS.** The function reads and reports the bitemporal world
model as the design table describes. `qip-entity-resolution`'s presence is
not directly visible in this file (no `qip_entity_resolution` import found
in the region read), so the entity-count figure is presumably produced
upstream by whatever populates `self.world`, in the same pattern as SENSE —
this stage reports the model's state rather than performing resolution
itself, but unlike SENSE the reported facts (bitemporal state, entity and
causal-claim counts) match the table's product description closely enough
that this is a CONFORMS rather than PARTIAL call.

---

## DISCOVER

**Design says** (`docs/architecture/README.md:48`): crate
`qip-opportunity-engine`; produces "ranked opportunities."

**Code does** (`stage_discover`, `platform.rs:2889-2943`): ages out expired
market events (`platform.rs:2894-2895`), builds a `DetectionContext` from
price/volume/spread/observation history and events
(`platform.rs:2897-2917`), calls `self.opportunities.scan(&detection,
&self.context)` (`platform.rs:2919`) to find and rank opportunities, extends
the work queue with them, then drops any that expired while queued
(`platform.rs:2924-2927`), reporting counts found/queued/suppressed/expired.

**Verdict: CONFORMS.** This is a direct match: the stage detects, ranks
(via `opportunities.scan`, presumably `qip-opportunity-engine`'s entry
point), queues, and reports expiry — precisely "ranked opportunities."

---

## REASON

**Design says** (`docs/architecture/README.md:49`): crates
`qip-investment-agents`, `qip-reasoning-engine`; produces "reviewed
hypotheses."

**Code does** (`stage_reason`, `platform.rs:3204-3460`): takes the head of
the opportunity queue, prices it as a `DecisionContext`
(`reason_decision_context`, `platform.rs:2963-2999`), asks
`self.cost_router.select(&context)` where the decision belongs on the
intelligence ladder (`platform.rs:3236`), and — the key gate at
`platform.rs:3273-3303` — convenes the panel (`IntelligenceTier::
MultiAgentReasoning`, the only reasoner rung implemented, per the comment at
`platform.rs:3261-3267`) only if the router's assessment says the panel is
worth what it costs. If convened, `self.organisation.dispatch(&brief, now,
lineage)` runs the agent panel (`platform.rs:3334`), failures and permission
violations are counted (`platform.rs:3338-3367`), and `self.synthesise(...)`
(`platform.rs:3373`) turns the report into a reviewed, confidence-scored
hypothesis, which is recorded as a falsifiable prediction
(`self.record_prediction`, `platform.rs:3380`) and, if approved, converted to
a sizeable thesis (`self.thesis_from`, `platform.rs:3416`).

**Verdict: CONFORMS.** `qip-investment-agents` maps to `self.organisation`;
`qip-reasoning-engine` maps to `self.synthesise`/`reasoned.review` (the
`ReasoningOutcome` type at `platform.rs:3573` is
`qip_reasoning_engine::engine::ReasoningOutcome`). The stage genuinely
produces reviewed hypotheses, gated by cost, exactly as the design states.

---

## SIMULATE

**Design says** (`docs/architecture/README.md:50`): crate
`qip-simulation-engine`; produces "outcome distributions and stress
results."

**Code does** (`stage_simulate`, `platform.rs:3827-3844`, full function):

```
fn stage_simulate(&mut self, _now: Timestamp) -> StageOutcome {
    let longest = self.price_history.values().map(Vec::len).max().unwrap_or(0);
    if longest < 60 {
        return StageOutcome::ran(Stage::Simulate, 0,
            format!("{longest} observation(s) is too little history to simulate from"));
    }
    StageOutcome::ran(Stage::Simulate, longest,
        format!("{longest} observation(s) available to resample"));
}
```

This is the entire function. It computes the longest price history held for
any instrument and reports whether there is "enough to simulate from" (a
threshold of 60 observations). It does not resample anything, does not
construct a distribution, and does not run a stress test — it produces a
readiness statement about data volume, nothing more.

The crate the design table names, `qip-simulation-engine`, is imported
exactly once in `platform.rs`: `use qip_simulation_engine::costs::CostModel;`
(`platform.rs:122`). That import is used at `platform.rs:5497`, inside
counterfactual pricing (`TwinMarket::new(bars, CostModel::liquid_equity(),
COUNTERFACTUAL_IMPACT_WINDOW)`), which is called from
`self.evaluate_alternatives` (`platform.rs:5501`), reached from
`score_declined` — a function called from `stage_learn`
(`platform.rs:4344`, "`Price what the gates declined`"), **not** from
`stage_simulate`. `qip-simulation-engine` also ships a Monte Carlo module
(`backend/crates/services/qip-simulation-engine/src/montecarlo.rs`, found by
`Glob`/`Grep` for `MonteCarlo`/`resample`), which produces zero matches for
`montecarlo` or `MonteCarlo` anywhere under
`backend/crates/runtime/qip-kernel/src` — the resampling capability the
crate provides is never called from the kernel at all, by any stage.

A comment in a neighbouring function makes the gap explicit rather than
this report inferring it: `construct_from`'s doc comment
(`platform.rs:3475-3476`) says the covariance used for sizing is drawn "from
the closes this platform observed — **the same series the simulate stage
resamples**." That sentence describes resampling as something the SIMULATE
stage does; the function shown above does not resample — it counts and
reports a threshold.

**Verdict: DIVERGES.** The function named `stage_simulate` performs a data-
sufficiency check, not a simulation. It produces neither an outcome
distribution nor a stress result — the two things
`docs/architecture/README.md:50` says it produces. The crate the design
attributes to this stage (`qip-simulation-engine`) is used exactly once in
the kernel, for counterfactual cost modelling inside `stage_learn`, and its
Monte Carlo capability is wired into no stage at all. No ADR was found
addressing or authorising this — `docs/adr` and `docs/architecture` contain
no hits for `stage_simulate`, `SIMULATE stage`, or `montecarlo`. This is not
a defensible thin-slice reading of the design; the design's own word
("resample") is used elsewhere in the same file to describe what this stage
is supposed to do, and the stage does not do it.

---

## DECIDE

**Design says** (`docs/architecture/README.md:51`): crates
`qip-optimization-engine`, `qip-portfolio-engine`; produces "a sized
proposal."

**Code does** (`stage_decide`, `platform.rs:3846-3939`): resyncs the
reservation ledger against current equity (`platform.rs:3852-3861`), drains
`self.pending_theses` (approved theses from REASON) and, if any exist, calls
`self.construct_from(&theses, now)` (`platform.rs:3881`, defined at
`platform.rs:3481-3568`). `construct_from` estimates a covariance matrix from
observed returns (`qip_numerics::stats::covariance`, `platform.rs:3521`),
then calls `self.constructor.construct(...)` (`platform.rs:3539`), where
`self.constructor` is a `PortfolioConstructor`
(`platform.rs:156`/`1404`). `PortfolioConstructor::construct`
(`backend/crates/services/qip-portfolio-engine/src/construction.rs:153-232`)
builds a `PortfolioProblem` (mean-variance objective, bounds, budget
constraint, optional cardinality/turnover terms) and calls
`self.router.solve(&problem)` (`construction.rs:232`), where `router` is a
`qip_optimization_engine::router::ComputeRouter`
(`platform.rs:106`, `1311`, `1404`) — the classical-baseline-always compute
router (ADR 0006). The resulting weights become order legs; if nothing was
sized, a `nothing_to_do` proposal records why (`platform.rs:3872-3899`).
Capital demand is also forecast and reported alongside the sizing outcome
(`platform.rs:3915-3928`).

**Verdict: CONFORMS.** Both named crates are genuinely in the call path —
`qip-portfolio-engine` via `PortfolioConstructor`, `qip-optimization-engine`
via the `ComputeRouter` it holds and calls internally — and the stage
produces exactly what the table says: a sized proposal (or an explicit
record of why none was sized).

---

## ACT

**Design says** (`docs/architecture/README.md:52`): crates `qip-risk-engine`,
`qip-execution-engine`; produces "orders and fills."

**Code does** (`stage_act`, `platform.rs:3941-4282`): runs the risk monitor
unconditionally (`self.monitor.observe(...)`, `platform.rs:3946-3948`,
`self.monitor.enforce(...)`, `platform.rs:3949-3950`) and records limit-
breach and risk-evaluation metrics regardless of whether anything is
proposed (`platform.rs:3952-3972` — the comment at `platform.rs:3942-3944`
states this runs "whether or not there is anything to trade"). It then
requires two independent sign-offs before releasing anything — the risk
monitor's permission (`action.permits_new_risk()`) and a compliance report
covering "all six governance controls" (`platform.rs:3988-4001`) — and only
proposals that clear both are approved (`platform.rs:4025-4065`). Approved,
non-empty proposals are turned into `Order`s and submitted through
`self.submit_order(order, now)` (`platform.rs:4183`), described in the
surrounding comment (`platform.rs:4123-4129`) as "the only path to a venue,"
re-running pre-trade controls, the autonomy ceiling and the kill switch.
Reservation holds are committed or released depending on whether an order
was actually placed (`platform.rs:4223-4255`), and a risk refusal is logged
on the hash chain even when nothing was releasable
(`platform.rs:4076-4092`).

**Verdict: CONFORMS.** `qip-risk-engine` maps to `self.monitor`;
`qip-execution-engine` maps to `self.submit_order`/`Order`/`OrderType`. The
stage produces orders (when approved) and — per the comments describing
`submit_order`'s own recording of accepted/refused outcomes — the record of
fills is a consequence of what `submit_order` does, though this function
itself does not show a fill being read back; it releases orders and commits
or releases the corresponding capital holds. Treated as CONFORMS because
both crates are genuinely exercised and the stage's job (get approved,
controlled, submit) matches the design description; "fills" specifically are
not observed inside this function and would need a trace into
`submit_order`/the simulated broker to confirm, which is outside
`platform.rs`'s `stage_act` and not chased further here.

---

## LEARN

**Design says** (`docs/architecture/README.md:53`): crate
`qip-learning-engine`; produces "attribution, calibration, lessons."

**Code does** (`stage_learn`, `platform.rs:4303-4356`): calls
`self.attribute(now)` (`platform.rs:4306`) for P&L attribution, reports
captured/taken/declined outcome counts (`platform.rs:4310-4321`), calls
`self.calibrate_resolved(&by_hypothesis, now)`
(`platform.rs:4327`, defined `platform.rs:4375-4474`) to settle due
predictions against the platform's own published series and recompute
calibration (Brier score etc., via `self.learn_from`,
`platform.rs:4448`), and calls `self.score_declined(now)`
(`platform.rs:4344`) to price counterfactuals for opportunities the gates
declined — this is the call path that reaches
`qip_simulation_engine::costs::CostModel` and `TwinMarket`, as noted under
SIMULATE above. The comment at `platform.rs:4340-4343` explicitly cites
"Blueprint §12: a platform that learns only from the trades it took is
learning from a heavily selected sample."

**Verdict: CONFORMS**, with a caveat worth surfacing rather than burying: no
`qip_learning_engine` import appears in `platform.rs` (not found by grep),
so "attribution, calibration, lessons" is produced by kernel-local functions
(`attribute`, `calibrate_resolved`, `learn_from`, `score_declined`,
`evaluate_alternatives`) rather than by a dedicated
`qip-learning-engine` crate the design table names. The three products
(attribution, calibration, counterfactual "lessons") are all genuinely
produced here, so the behavioural conformance is real; the crate-boundary
claim in the design table could not be confirmed for this stage from
`platform.rs` alone — verifying it would require reading whether
`qip-learning-engine` exists as a crate and whether the kernel's local
functions actually delegate to it internally (not chased further here, out
of scope for a `platform.rs`-only trace).

---

## Summary

| Stage | Verdict | One-line reason |
|---|---|---|
| SENSE | PARTIAL | Reports on absorption; absorption itself (`observe`, `platform.rs:2191`) runs outside the cycle's eight stages, and `qip-normalization` is not visible in `platform.rs`. |
| UNDERSTAND | CONFORMS | Reads the bitemporal world model at `now, now` and reports its true state (`platform.rs:2826-2887`). |
| DISCOVER | CONFORMS | Detects, ranks, queues and expires opportunities via `opportunities.scan` (`platform.rs:2889-2943`). |
| REASON | CONFORMS | Cost-gates and, when justified, convenes the agent panel and produces a reviewed hypothesis (`platform.rs:3204-3460`). |
| SIMULATE | **DIVERGES** | Performs only a data-sufficiency check; never resamples, never calls the simulation engine's Monte Carlo path (`platform.rs:3827-3844`; zero kernel references to `montecarlo`). No ADR justifies it. |
| DECIDE | CONFORMS | Sizes a proposal through `PortfolioConstructor` and the classical/quantum `ComputeRouter` (`platform.rs:3846-3939`, `3481-3568`). |
| ACT | CONFORMS | Runs risk monitor + two-signature sign-off + `submit_order`, the sole venue path (`platform.rs:3941-4282`). |
| LEARN | CONFORMS | Produces attribution, calibration and counterfactual scoring, though not visibly via a distinct `qip-learning-engine` crate import (`platform.rs:4303-4356`). |

**Score: 6 CONFORMS, 1 PARTIAL, 1 DIVERGES, out of 8 stages.**

The one finding that needs an owner's attention: **`stage_simulate` does not
simulate.** It is a 17-line readiness check that reports how much price
history exists, while the crate the architecture documents
(`qip-simulation-engine`) is invoked exactly once in the kernel — for
counterfactual cost modelling inside `stage_learn`, not inside
`stage_simulate` — and its Monte Carlo module is never called from the
kernel at all. This is stated plainly because the review exists to surface
exactly this class of finding: a stage whose behaviour does not match its
name.
