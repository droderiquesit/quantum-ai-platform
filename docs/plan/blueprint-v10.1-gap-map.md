# Blueprint v10.1 gap map

> **Superseded for status by [`PROJECT-PLAN.md`](PROJECT-PLAN.md).** This
> document's structural inventory is kept as history.
>
> **Re-scored 2026-09-05 at `1107305`.** Eleven rows moved and the top-line
> counts were recomputed rather than adjusted. The valuation plane went from
> 0 of 6 engines to 6 of 6 built, **4 of 6 with a production caller at that
> commit and 5 of 6 in the working tree** — the liquidity ladder's risk read
> landed uncommitted while this was being written, and is scored, flagged and
> cited by symbol rather than waited for. **Amended 2026-09-06:** that fifth
> caller committed at `b060df2`, and the claim its row made for it — "so it is
> a control that can fire" — was withdrawn on evidence. Wired, yes;
> trustworthy, no. The
> self-model, wallet, treasury/corridor and per-account-entitlement rows had
> been ABSENT since the first pass and stopped being true at `e5dc8fc`,
> `5546a24` and `0599092`; and two first-pass findings were themselves wrong
> and are corrected in place — the credit row's "No spread decomposition"
> (it has existed in exact `Decimal` since `b3ebc7f`) and the term-structure
> row's BUILT-UNWIRED, which understated a defect that silently dropped a
> duplicate tenor under an unstable comparator. Rows not named in this note
> were not re-verified in this pass and carry their original evidence and
> date; a row's silence is not a claim that it was rechecked.
>
> **Amended again 2026-09-06, and this second correction is the instructive
> one, because the withdrawal outlived the defect it described.** Both
> findings behind "trustworthy, no" were repaired at `5ccaea9`, and the
> liquidity-ladder row went on calling the repairs "in the working tree,
> uncommitted at this writing" after they had been committed — which told a
> reader not to rely on a floor that now fires. **A document wrong
> pessimistically is wrong.** The floor fails closed, and each holding's days
> to exit now come from its own record rather than from its rung's bucket.
> One defect of the same family is open and is named in the row rather than
> summarised. See the liquidity-ladder row, and `PROJECT-PLAN.md`'s valuation
> row, which is the current score.

A first-pass structural inventory: for each capability the "Algorik —
Cognitive Investment Platform, Blueprint v10.1" document names, does anything
resembling it exist in the actual tree, and if so, does a real caller reach
it? This is **not** a judgment of whether what exists is correct, complete, or
good — only whether code exists and whether it is called from somewhere that
runs.

## Provenance and scope

The blueprint was supplied as a single HTML file
(`/root/.claude/uploads/483b7741-5097-5e6c-93a8-eee0877fec38/7dd87a9f-index.html`,
1,627 lines) and is copied unmodified into this worktree at
`docs/plan/blueprint-v10.1.html` for reference. It was read in full, section by
section (lines 203–990, 1004–1330, 1499–1563 read verbatim; the section
headers for every remaining `<h2>`/`<h3>` were enumerated by grep and cross-
checked against the read passages) covering all eleven tabs: Overview,
Cognition, Valuation & assets, Intelligence & quantum, Capital & money,
Execution, Software, Cloud & network, Web & mobile, Experience, and Roadmap &
evidence.

**This document does not exist in a vacuum.** `docs/adr/0022-the-algorik-
blueprint-is-the-architecture-of-record.md` already names this same blueprint
(v10.1-4) as the architecture of record, and
`docs/architecture/algorik-blueprint-traceability.md` (624 lines) is an
existing, actively maintained scorecard against it, referenced from
`docs/plan/completion-plan.md`. That contradicts the framing this task was
given — that current plan documents "do not mention this v10.1 document at
all" — and the assigning agent should know that before treating this file as
the first word on the subject. This document was nonetheless produced by
independent grep/read verification against the tree as it stands today,
without reading or relying on the existing traceability matrix's conclusions,
per the task's instruction to treat this as a fresh mapping. Where this
document's findings and that matrix's findings might disagree, neither should
be assumed correct without checking the citation.

**Coverage is a representative sample, not an exhaustive enumeration.** The
blueprint names on the order of 70 services and dozens of named types across
34 "bounded domains" in one section alone. This pass verified roughly four
dozen of the most load-bearing, specifically-named mechanisms across every
plane — the ones the document's own prose treats as its central claims — by
grep and direct file reads against `backend/crates/**` and `frontend/**`, and
states plainly which sub-items within a plane were **not** individually
checked. A capability not listed below was not evaluated in this pass and
should not be assumed either present or absent from this document's silence.

Every row cites the grep or read that produced it. Where a search returned
nothing, multiple plausible names were tried before the row was marked
ABSENT, and the terms tried are named in the evidence column.

---

## Cognition plane

| Capability | Status | Evidence |
|---|---|---|
| World model / entity graph (`Entity`, `EntityRelation`, `WorldEvent`) | BUILT+WIRED | `qip-world-model` crate exists (`backend/crates/services/qip-world-model/src/world.rs`); `qip_world_model::WorldModel`, `::graph::{Node,NodeKind}`, `::liquidity::LiquidityTopology` imported and used in `backend/crates/runtime/qip-kernel/src/platform.rs:135-138` |
| Entity resolution | BUILT+WIRED | `backend/crates/services/qip-entity-resolution/src/{entity,resolver,matching}.rs` exists; `qip_entity_resolution::entity::{Entity,EntityKind,EntityRecord}` and `::resolver::Resolver` imported in `qip-world-model/src/world.rs:8-9`, and `qip-world-model` is itself wired into the kernel (above) |
| Causal inference / causal graph | BUILT+WIRED | `qip-world-model/src/causal.rs` exists; `qip_world_model::causal::Mechanism` used at `qip-kernel/src/platform.rs:5842`; `qip-reasoning-engine/src/hypothesis.rs` defines `CausalChain`/`CausalStep`, imported at `platform.rs:3633` |
| Episodic memory (full blueprint shape: state vector, regime, causal context, beliefs, actions, outcome, surprise; ANN retrieval over millions of episodes) | PARTIAL | `qip-agents/src/memory.rs` defines `Episode`/`ResearchMemory` (occurred_at, question, conclusion, evidence, conviction, outcome) and `qip_agents::memory::ResearchMemory` is imported and used in `qip-kernel/src/platform.rs:44`. This is a real, wired episodic store, but it holds one agent's research conclusion and outcome, not the blueprint's richer episode (compressed market/world state vector, regime label, active causal edges, declined actions, multi-horizon outcome, surprise score) or approximate-nearest-neighbour retrieval over a large corpus — no HNSW, no vector index, and no "surprise" field were found (`grep -rli "hnsw\|nearest.neighbour\|surprise"` on `crates/libs/qip-agents` returns no such fields) |
| Belief state with confidence and TTL, shipped to regions as priors | PARTIAL | `qip-reasoning-engine/src/bayes.rs` implements Bayesian log-odds confidence updating (`BeliefContribution`, `BeliefUpdate`, `to_log_odds`/`from_log_odds`) and the reasoning engine is wired into the kernel (`qip_reasoning_engine::engine::{ReasoningEngine,ReasoningOutcome}` at `platform.rs:115`). No `struct Belief` with a TTL field exists anywhere in the tree (`grep -rn "struct Belief\b" backend/crates` returns nothing), and no mechanism ships a belief prior from a centre to a region — `qip-edge` has no belief-cache module (`grep -rli "belief" backend/crates/edge` matches only `qip-strategy/src/ir.rs` and test/telemetry files, not a cache) |
| Counterfactual learning / shadow execution of vetoed paths | BUILT+WIRED | `qip-twin` crate (`counterfactual.rs`, `capture.rs`, `regret.rs`, `asof.rs`) is wired: `qip_twin::counterfactual`, `::capture::{Action,Decision,OutcomeCapture,RealisedOutcome}`, `::asof::TwinMarket` imported at `platform.rs:129-134`. Per `.claude/rules/domains/observability.md`, `Platform::evaluate_alternatives` (counterfactual scoring) is reached from `score_declined`, itself called from `stage_learn` — a real production call path in the LEARN stage of the cycle |
| Self-model (capability estimate, coverage, calibration tracking) | BUILT+WIRED | Re-scored 2026-09-05; the ABSENT verdict above was true when written and stopped being true at `e5dc8fc`. `qip-learning-engine/src/self_model.rs` holds every component's calibration, fed from the LEARN stage and feeding the reasoning engine's origin factors on supporting evidence only; it is served at `GET /cognition/self-model`. It is also one of the three §6.2 rows the centre now *sizes against* rather than reads as a constant: `SELF_MODEL_HORIZON` is 7 days (`qip-contracts/src/degradation.rs:265`) and `Platform::central_degradation` observes it before `construct_from` narrows the budget by `central_sizing_multiplier()` (`qip-kernel/src/platform.rs`, cited by symbol). `grep -rl self_model backend/crates --include=*.rs` returns `qip-learning-engine/src/lib.rs`, `qip-contracts/src/degradation.rs`, `qip-kernel/src/platform.rs` and two test files |
| Exploration budget (UCB/Thompson sampling, declared capital share for information gain) | ABSENT | `grep -rli "thompson\|ucb\b\|exploration.budget\|explorationbudget"` across `backend/crates` returns nothing |
| Hypothesis generation with falsification | BUILT+WIRED | `qip-reasoning-engine/src/hypothesis.rs` (`Claim`, `CausalChain`) and `src/redteam.rs` (adversarial challenge against a hypothesis's structure, `Severity`, `ReviewOutcome::rejection_rate`) both wired into `qip-kernel/src/platform.rs:116,3630-3633,5843-5847` |
| Degradation ("narrow rather than halt") | BUILT+WIRED (edge code path exists and has a real caller; that caller is exercised only in tests, no deployment) | `.claude/rules/domains/observability.md` documents extensively that `qip-edge`'s `Cell` narrows on stale capability freshness with per-source telemetry (`qip_edge_capability_freshness{capability}`), each recording site proven by `backend/crates/edge/qip-edge/tests/telemetry.rs`; `Cell::work` is reached from `qip-edge-node/src/pass.rs:118` → `main.rs:586`, but only when `QIP_VENUE_FEED=simulated`, and `execution_nodes = {}` in every Terraform environment, so no deployed process exercises it today |
| Source discovery (surface/deep/dark-web tiers, discovery crawler, `SourceCandidate`, `DeepWebAdapter`) | PARTIAL | `qip-data-finder/src/{source,finder,legal,robots,probe}.rs` define `SourceCandidate` (`source.rs:82`) and a robots.txt-respecting finder with a legality/licensing gate; `SourceCandidate`, `DataFinder`, `RegistrationDecision` are imported and used in `qip-kernel/src/platform.rs:75-78,5084`, so the licensing-gated source-registration path is real and wired. But the blueprint's specific three-tier model (surface/deep/dark web) does not exist — `grep -rn "surface_web\|SurfaceWeb\|DarkWeb\|dark_web"` returns nothing — nor does a `DeepWebAdapter` type or its six access modes (`open_query`/`api`/`registered`/`licensed`/`rendered`/`bulk`; `grep -rn "struct DeepWebAdapter"` returns nothing), nor an isolated discovery enclave, nor dark-web defensive monitoring |

Re-scored 2026-09-05, source-discovery row only: the three-tier model now
exists as typed policy in `qip-data-finder/src/tier.rs` — `SourceTier`
(`surface_web`/`deep_web`/`dark_web`), classified by `SourceTier::classify`
from a `TierEvidence` built from the candidate before the probe and from the
`Source` after it, refusing on insufficient evidence rather than defaulting to
the surface web; `DeepWebAdapter` with the six `AccessMode` arms
(`open_query`/`api`/`registered`/`licensed`/`rendered`/`bulk`), each carrying
what it needs (a `CredentialReference` by name only, a licence identifier
checked against the `Declared` posture, a `RenderingBudget`, a `BulkCadence`
with a retention bound) and an `admissible()` rule per arm; `DiscoveryEnclave`
as the isolation record the `rendered` and `bulk` modes are refused without;
and `DefensiveMonitoring` as a watch-list record with no fetch path. The tier
is wired into `DataFinder::assess` (`finder.rs`, `route_by_tier`): a
hidden-service host is rejected before any probe call, an unplaceable source
is deferred, and the routing decision records tier, mode and refusal in the
`classify` and `route` reasoning steps. Proven by
`qip-data-finder/tests/tiers.rs`. Status is now **BUILT+WIRED (policy)**:
still no crawler, no renderer and no Tor client — this crate opens no sockets
— so the tier decides what may be reached and how, and nothing yet reaches it.

## Valuation & assets plane

| Capability | Status | Evidence |
|---|---|---|
| Term structure (yield curves, forward rates) | BUILT+WIRED | Re-scored 2026-09-05 at `1107305`; the previous row said BUILT-UNWIRED and **understated the defect as well as the wiring**. `qip-market/src/curve.rs` defines `TermStructure`/`CurvePoint` with monotone-cubic interpolation via `qip-numerics/src/interpolate.rs`, and `TermStructure::new` is now called from `CreditRegister::from_universe` (`qip-kernel/src/valuation.rs:114`), itself called at `Platform::new` (`platform.rs:1952`) — one sovereign curve per currency — and read back through `CreditRegister::discounted_expected_loss` (`valuation.rs:159`). The unwiring was not the whole finding: `TermStructure::new` carried `points.dedup_by(\|a, b\| (a.tenor_years - b.tenor_years).abs() < 1e-12)`, so a vendor publishing the ten-year point twice at two yields had one **silently dropped**, and which one survived depended on a comparator falling back to `Ordering::Equal` — not stable across the two orders the same file can arrive in, so the curve interpolated differently on a replay than it did live. A negative tenor re-anchored the front end and a `NaN` tenor compared equal and landed wherever the input put it. All three are refusals now, naming the tenor and both quoted values (`curve.rs`, the `# Refusals` block on `new`; `DISTINCT_TENOR_YEARS`), proven by `qip-market/tests/structure.rs` |
| Credit engine (default probability, recovery, spread decomposition, covenant state) | BUILT+WIRED | Re-scored 2026-09-05 at `1107305`. **The previous row was wrong in one direction**: it said "No spread decomposition", and `RiskCharacteristics::spread_decomposition` has existed in exact `Decimal` since `b3ebc7f` (`qip-financial/src/risk_profile.rs:209`; `git log -1 -S spread_decomposition -- .../risk_profile.rs` returns `b3ebc7f`). What was genuinely absent — survival curve, hazard rate, seniority-derived recovery, covenant state, and any caller — is supplied by `CreditProfile` (`qip-financial/src/credit.rs:234`, with `CovenantState` at `:74` and its own `spread_decomposition` at `:459`). `covenant_state()` returns `Option<CovenantState>`, because an obligor nobody wrote a covenant for has had nothing tested and reporting `Compliant` for it would be the `MaxExpectedShortfall` shape by another name. Wired: `CreditProfile::from_object` at `qip-kernel/src/valuation.rs:88` builds one profile per credit claim, and the register reaches `CycleReport` twice per cycle — `self.credit.summary()` into the UNDERSTAND stage's detail string and `self.credit.problems()` as stage problems (`platform.rs:4781-4800`) |
| Volatility surface | **BUILT-UNWIRED** | Re-scored 2026-09-05 at `1107305`; previously ABSENT and now built but reached by nothing. `qip-market/src/volatility.rs:151` defines `VolatilitySurface`, interpolating across expiries linearly in **total variance** rather than in volatility, because interpolating volatility directly manufactures negative forward variance — a calendar arbitrage the interpolator invented. Nothing extrapolates: both smile and surface guard the domain and refuse a strike inside one bracketing smile and outside the other as a hole. 711 lines of tests in `qip-market/tests/volatility.rs`. **No production caller**, and this is not an oversight waiting on a wire: nothing in this platform ingests option quotes, and `OptionDetails` (`qip-financial/src/extensions.rs:411`) is constructed nowhere outside fixtures. `grep -rl VolatilitySurface backend/crates --include=*.rs` returns only `qip-market/src/{lib,volatility}.rs` and `qip-market/tests/volatility.rs`. Wiring one is a data source with a licensing evaluation ahead of it, not a code change |
| Illiquid valuation (mark with method + confidence: comparables, DCF, model, last round, cost) | BUILT+WIRED | Re-scored 2026-09-05 at `1107305`; previously ABSENT. `qip-financial/src/valuation.rs:332` defines `IlliquidValuator`, and it is on the **sizing path**: `IlliquidValuator::mark_object` is called at `qip-kernel/src/platform.rs:767`, feeding `Platform::mark_confidence_multiplier` (`platform.rs:5815`) which narrows the construction budget at `platform.rs:5969`, inside `construct_from` — reached from `stage_decide` (`platform.rs:6289`, call at `:6324`). An asset with no residual value and nothing called beyond what was distributed is **refused** with "this plane refuses to invent a mark" rather than marked, and an unmarkable private asset refuses the construction outright, because a number nobody could observe returned as a valuation is the one failure a valuation plane must not have |
| Cashflow forecasting / commitments / capital calls | BUILT+WIRED | Re-scored 2026-09-05 at `1107305`; previously ABSENT. `qip-financial/src/cashflow.rs:179` defines `CashflowForecast`, and the kernel holds a `CommitmentBook` (`platform.rs:372`, `qip_financial::cashflow::CommitmentBook`). It is on the **sizing path**: `Platform::deployable_capital` (`platform.rs:5879`) is free capital less `self.commitments.unfunded_total(now)?`, and `construct_from`'s budget line reads it (`platform.rs:5956`) where it previously read `self.reservations.free(now)`. It **refuses rather than flooring at zero** when obligations meet or exceed what is free — a called commitment that cannot be met forfeits the position, and sizing against a budget of zero would report that as an ordinary quiet cycle |
| Corporate actions (splits, dividends, mergers, spinoffs, delistings) | BUILT+WIRED | `qip-market/src/corporate_action.rs` defines `CorporateActionKind`; imported and switched on at `qip-kernel/src/platform.rs:96,2317,5804-5812` (`corporate_action_class`, covering `Split`, `CashDividend`, `StockDividend`, `RightsIssue`, `Merger`, `Spinoff`, `Delisting`) |

## Intelligence & quantum plane

| Capability | Status | Evidence |
|---|---|---|
| The ten shipped model classes / ML training pipeline (`burn`, `linfa`, `polars`, `tract`, spot-GPU jobs) | ABSENT | The blueprint's stack requires `burn`, `linfa`, `polars`, `arrow-rs`, `parquet`, `tract` as dependencies. `CLAUDE.md` states the workspace permits **serde and serde_json only** (ADR 0002, ADR 0009), enforced by `./scripts/check-dependencies.sh`; none of the blueprint's ML crates are in `backend/Cargo.lock`'s permitted set. No training pipeline, ONNX promotion, or model registry exists in the tree |
| Quantum optimisation with a classical baseline computed every run | BUILT+WIRED | `qip-quantum/src/provider.rs` defines `QuantumProvider`, `SimulatedProvider`, `HostedProvider` (refuses to accept a hosted result unless the backend affirmatively reports non-simulated); `qip_optimization_engine::router::ComputeRouter` and `qip_quantum::provider::SimulatedProvider` imported at `qip-kernel/src/platform.rs:106,114`. ADR 0006 ("classical baseline always") is enforced structurally per `.claude/rules` and confirmed by the provider's own doc comments on refusing to blur simulated/hardware provenance |
| Meta-learning (which model works where, warm starts, cross-asset transfer) | ABSENT | `grep -rli "meta.learn"` hits are all unrelated (governance/manifest/topic/DNA/factor-file substrings), not a meta-learning implementation. No dedicated meta-learner code found |
| Adversarial modelling (flow classification informed/uninformed, pattern-leakage detection, crowding, response) | ABSENT | The only "adversarial" hit that is an actual implementation is `qip-reasoning-engine/src/redteam.rs`, which challenges a *hypothesis's own evidentiary structure* before it can be acted on — a different mechanism from the blueprint's market-counterparty adversary model (who is causing adverse selection, is my fingerprint being learned, correlated external flow). No flow-classification or fingerprint-randomisation code was found |
| Market simulation with adaptive agents (passive, informed, momentum, competitor, maker), calibrated against actual fills | PARTIAL | `qip-simulation-engine` (`backtest.rs`, `market.rs`, `montecarlo.rs`, `scenario.rs`, `validation.rs`) exists and is wired — `qip_simulation_engine::costs::CostModel` imported at `qip-kernel/src/platform.rs:122`. `grep -n "struct\|enum\|Agent" qip-simulation-engine/src/market.rs` shows `SyntheticMarket`, `MarketSimulator`, `SimulationRun` but no `Agent` types for the five named counterparty behaviours; this is a synthetic-price/backtest simulator, not a multi-agent adversarial one |

## Capital & money plane

| Capability | Status | Evidence |
|---|---|---|
| Capital engine (deployed/reserve/exploration split, bounded expiring grants) | BUILT+WIRED | `qip-capital::reservation::ReservationLedger`, `AllocationLimits`, `CapitalAllocator`, `DrawdownSchedule` and `qip_capital_fabric` imports at `qip-kernel/src/platform.rs:47-49`; `qip_capital_fabric::evaluate(&plan, realised)` called at `platform.rs:5661` |
| Risk envelope including a per-causal-driver concentration limit | BUILT+WIRED | Confirmed by `.claude/rules/domains/risk-and-execution.md`: `RiskState::with_tail_risk` fills expected shortfall per-limit, and `the_expected_shortfall_limit_can_actually_fire` in `qip-kernel/src/platform.rs` proves the veto fires rather than reading as decorative protection |
| Liquidity ladder (rung-by-rung withdrawal ordering, cash → PE commitments) | **BUILT+WIRED as a risk read, committed at `b060df2`; the "not trustworthy" verdict this cell carried is withdrawn — both findings behind it were repaired at `5ccaea9`; BUILT-UNWIRED at `1107305`** | Re-scored four times: ABSENT at the first pass, twice on 2026-09-05, once on 2026-09-06 after an adversarial review of the wiring, and again on 2026-09-06 after the repairs landed and this cell went on describing them as pending. `qip-financial/src/ladder.rs:238` defines `LiquidityLadder`; monotonicity is compared by cross-multiplication so money never leaves `Decimal`, and the rung order is the enum's declaration order — a property of the type rather than of a sort call someone can forget. At `1107305` it had **no production caller**. It has one now, committed at `b060df2` (`git merge-base --is-ancestor b060df2 HEAD` succeeds; the row said "not yet committed" and that is stale): `Platform::liquidity_ladder` builds one from the aggregates, `Platform::risk_state_from` fills `RiskState::days_to_liquidate` and `RiskState::liquidatable_within` from it, and `stage_act` calls it. Those fields are read by `LimitKind::MaxDaysToLiquidate` and `LimitKind::MinLiquidity` (`qip-risk/src/limits.rs:298,300`). **This row then said "so it is a control that can fire", and that sentence is withdrawn.** It did not survive a second adversarial pass, and the identical over-claim was withdrawn from `PROJECT-PLAN.md`'s valuation row; leaving it here would let the retired claim survive in the history document. Two findings, **both closed at `5ccaea9`** — and this cell said "repairs are in the working tree, uncommitted at this writing" after they had landed, which is the same error in the opposite direction to the over-claim above it. `git merge-base --is-ancestor 5ccaea9 HEAD` succeeds. First, the floor **used to fail open**: `risk_state_from` read the ladder through `if let Ok(ladder) = self.liquidity_ladder(figures) { … }`, so a refused ladder left both maps untouched and empty, `MinLiquidity` took its `None` arm, and every order was accepted — a silently unevaluated floor and a satisfied floor were the same empty map. `git show HEAD:backend/crates/runtime/qip-kernel/src/platform.rs \| grep -n "if let Ok(ladder)"` now returns nothing. In its place, `RiskState::unevaluated` (`qip-risk/src/limits.rs:333`) carries the refusal, `risk_state_from` files it (`platform.rs:7720`), and `PreTradeChecker::check` rejects while an entry stands, before weighing any limit and not reducibly (`qip-risk-engine/src/pretrade.rs:219-222`). Second, **`LiquidationHorizon::least_days` over-counted liquidity**: `Rung::classify` puts any non-negotiated holding with `days_to_liquidate > 1.0` on `BondsAndLessLiquidListed`, whose horizon is `Days`, whose `least_days` is `2.0`, so a record stating forty-five days read as two. `LadderEntry::days_to_exit` (`qip-financial/src/ladder.rs:339`) now returns the larger of the record's own measurement and that floor, the kernel carries the record's figure onto the entry (`platform.rs:7790`), and `reachable_within` filters on it (`ladder.rs:653`). **What survives is the instrument the first finding was measured with**: `LiquidityProfile::illiquid()` still hardcodes `typical_spread_bps: 250.0` (`qip-financial/src/costs.rs:57-60`) while `listed()` takes the caller's spread (`:46`), and `ladder_reference_of` admits a wider quote silently at assembly (`platform.rs:786-804`), so a listed name at 300 bps still inverts the per-rung cost rate and makes the monotonicity proof refuse the whole ladder — and now that the floor fails closed, that refusal stops every order in the cycle rather than none of them. **BUILT+WIRED is a structural verdict about whether a caller exists, and this row's own preamble says these counts are not a judgment of correctness. Take it that way and no further** — `PROJECT-PLAN.md`'s valuation row carries the current score; this row is history. **The blueprint's own caller stays structurally forbidden**: serving a withdrawal from the top of the ladder needs a *granted* withdrawal, and ADR 0021 leaves `WithdrawalEntitlement` with exactly one arm, `Refused` — no `Granted` to construct and no `Deserialize` to smuggle one through (`qip-capital/src/ledger/entitlement.rs:155`). So the ladder is wired along the one seam that is admissible here and will never be wired along the one the blueprint names. Verify before quoting: `grep -rn 'liquidity_ladder(' backend/crates/runtime/qip-kernel/src/platform.rs` for the caller, and read the `match` in `risk_state_from` — the `if let Ok(` that used to be there is what made every sentence about the limit firing false, and its absence is what makes them true. The ladder is execution-adjacent by subject and inert by construction: its plan legs carry an object id, a rung, an amount and a cost, and a test asserts the serialised field set is exactly those four so a future venue or side field trips it |
| Compounding policy (reinvestment cadence, fee-tier accumulation, withdrawal drag) | ABSENT | `grep -rli "compounding"` returns nothing |
| Treasury / signed corridors / MPC withdrawal gates | PARTIAL (records and refusals only, by ADR 0021) | Re-scored 2026-09-05; the ABSENT verdict above was true when written and stopped being true at `5546a24`. `qip-capital-fabric/src/corridor.rs` defines `CorridorId:33`, `CorridorCaps:125`, `CorridorStage:245`; `destination.rs` defines `DestinationKey:80`, `DestinationRecord:230`, `DestinationRegistry:249`; `gate.rs` defines the seven-veto transfer gate, reached from the kernel at `qip-kernel/src/platform.rs:3751` (`GateCheck::ALL`, seven checks); `custody.rs` defines `CorridorKind:95` and custody policy as data; `journal.rs` defines `CorridorAction:156`, `CorridorStep:199`, `CorridorStanding:282`. **What is deliberately still absent is the half that would move money**: there is no transfer engine and no MPC signer, so the gate's `Approved` has no consumer, and ADR 0021 forbids building one until Phase 12. This is a refusal, not a gap |
| Wallet (read-only balance aggregation, signing crate deliberately unlinked) | BUILT+WIRED (read model only) | Re-scored 2026-09-05; ABSENT above was true when written and stopped being true at `5546a24`. `qip-capital-fabric/src/wallet.rs` is the read model with halting reconciliation, imported into the kernel at `qip-kernel/src/platform.rs:63` beside the journal at `:59`, and served at `GET /wallet`. The signing half is unlinked exactly as the blueprint's own parenthesis asks, and as ADR 0021 requires |
| Tax-lot accounting (FIFO/LIFO/highest-cost/lowest-cost, holding period) | BUILT-UNWIRED | `qip-portfolio/src/lot.rs` defines `Lot`, `LotMethod` (`FirstInFirstOut`/`LastInFirstOut`/`HighestCost`/`LowestCost`), `RealisedTrade`, re-exported from `qip-portfolio/src/lib.rs:20,25`. `grep -rln "qip_portfolio::lot\|portfolio::lot::"` finds only `qip-portfolio/tests/accounting.rs` — no caller outside the crate's own tests, so no production code path computes a realised gain from a lot today |
| Settlement calendar (cut-off, value date, settlement days) | BUILT+WIRED | `qip-capital-fabric/src/settlement.rs` defines `SettlementCalendar`; `qip-capital-fabric` (`plan.rs`, `settlement.rs`, `forecast.rs`) is imported and used at `qip-kernel/src/platform.rs` (settlement-calendar hits present in `platform.rs`) |

## Execution plane

| Capability | Status | Evidence |
|---|---|---|
| Executable graph, cycles, intent netting, mirrored inventory | BUILT+WIRED (in the codebase; not exercised by any deployed process) | `qip-edge/src/cell.rs` implements intent netting (`NetIntent`); `qip-edge/src/mesh.rs`, `journal.rs`, `cell.rs`, `qip-sequencing/src/identity.rs` implement mirror/mesh concepts. Per `.claude/rules/domains/observability.md`, these paths are proven by tests in `qip-edge/tests/telemetry.rs` and reached from `qip-edge-node/src/pass.rs:118`, but `execution_nodes = {}` in every Terraform environment, so nothing deployed runs them |
| Leg coordinator (saga-style latency-equalised dispatch with compensation) | ABSENT | `grep -rli "leg.coord\|LegCoordinator"` returns nothing anywhere in `backend/crates` |
| Eight execution paths as a named, distinguishable set (intra-venue, cross-venue, mirrored, hedged bridging, passive anchoring, firm-quote bridging, representation basis, payoff equivalence) | NOT INDIVIDUALLY VERIFIED | Not checked path-by-path in this pass; `qip-arbitrage`, `qip-routing` crates exist under `backend/crates/edge/` but their coverage of each named path was not confirmed. Listed here rather than silently omitted, per the instruction not to assert presence or absence without checking |

## Software / stack

| Capability | Status | Evidence |
|---|---|---|
| Full blueprint dependency stack (`crossbeam`, `bumpalo`, `core_affinity`, `io-uring`, `tract`, `burn`, `linfa`, `polars`, `arrow-rs`, `parquet`, `rayon`, `argmin`, `nalgebra`, `statrs`, `ring`, `blake3`, `axum`, `tonic`, `google-cloud-rust`, `leptos`, `plotters`, `opentelemetry`) | ABSENT | `CLAUDE.md`: "Two dependencies only — `serde`, `serde_json` (ADR 0002, ADR 0009)", enforced by `./scripts/check-dependencies.sh`. None of the blueprint's ~20 named crates are permitted dependencies in this tree today. This is a structural, not incidental, gap: the blueprint's execution-node performance model (lock-free structures, `io-uring`, pinned cores) and its ML/quantum/crypto/web stack assume a dependency set the workspace's own policy currently forbids |
| GCE execution nodes, one per region, three regions | ABSENT (deployed); an infrastructure module exists in code | ADR 0024 named in `CLAUDE.md` as "never yet applied"; `docs/plan/completion-plan.md` confirms `execution_nodes = {}` in every environment |

## Cloud & network

Not independently re-verified in this pass beyond what the domain rule files
already state as settled fact: `CLAUDE.md` and `.claude/rules/domains/
infrastructure.md` describe Cloud Run + one GCE execution node per region as
the target, Terraform 1.9.8 with `hashicorp/google ~> 6.12`, and no
Kubernetes in the target state (with a documented transitional GKE cluster
per ADR 0022, itself since removed per `docs/plan/completion-plan.md`'s "the
cluster's Terraform, chart, manifests and Argo CD stack removed"). This
document does not re-litigate that; it is reported here as read, not
re-verified against a live plan.

## Web & mobile

| Capability | Status | Evidence |
|---|---|---|
| Leptos-based portal (one Rust codebase, shared types with backend, SSR + WASM) | ABSENT | `frontend/portal/` is Next.js + TypeScript (`.claude/rules/domains/frontend.md`: "Next.js + TypeScript, and the one part of this platform that is not Rust"). ADR 0022 records Leptos as "the target experience layer" and that the Next.js frontend is "transitional"; today it is what is deployed |
| Installable PWA for mobile | BUILT+WIRED | `frontend/mobile/README.md` documents the PWA-as-mobile-app decision and names concrete artefacts: `frontend/portal/src/app/manifest.ts`, `frontend/portal/public/sw.js`, `frontend/portal/src/components/chrome/InstallApp.tsx`, `AppShell.tsx`. These paths were confirmed present in the source tree |
| Investor portal surfaces (portfolio, strategies, risk, execution, capital, agents, intelligence, research, operations) | BUILT+WIRED | `frontend/portal/src/app/(portal)/` contains route directories for `portfolio`, `strategies`, `risk`, `execution`, `capital`, `agents`, `intelligence`, `research`, `operations`, `command`, `orders`, `signals`, `models`, `data-sources`, `integrations`, `admin`, `system` — confirmed by directory listing |
| Wallet / treasury / withdrawal UI | PARTIAL | Re-scored 2026-09-05; ABSENT above was true when written and stopped being true at `e5dc8fc`. `frontend/portal/src/app/(portal)/treasury/` now holds six surfaces — `ledger`, `wallet`, `corridors`, `transfer-gate`, and, since `bcf7330`, `products` and `accounts` — reached from `src/lib/nav.ts:253,265`, with Playwright coverage in `tests/treasury-*.spec.ts`. **There is no withdrawal UI and there may not be one** (ADR 0021, ADR 0023): the only write the treasury surface carries is an operator's eligibility decision, and the accounts page states in its own source that the account shown is one an operator selected, never the signed-in person's own, because the gateway forwards one deployment credential and no session-to-ledger-account binding exists below it |
| Passkey-only sign-in (WebAuthn, "passwords none, anywhere") | PARTIAL | `frontend/packages/auth/src/index.ts:24` defines `AuthMethod = "password" | "google" | "passkey" | "saml" | "oidc" | "development"` — `passkey` is one option among several, and `password` is explicitly still a listed method, which is the opposite of the blueprint's "no passwords, anywhere" requirement. No WebAuthn ceremony implementation was inspected beyond this type definition |
| Per-account entitlements (`can_invest`, `can_withdraw`, `can_view_execution_trace`, etc.) | BUILT+WIRED | Re-scored 2026-09-05; the ABSENT verdict above was true when written and stopped being true at `0599092`. `qip-capital/src/ledger/entitlement.rs:118` defines `Entitlement` carrying `can_view`, `can_invest` and `can_withdraw` — per user, per product, at one instant, with private fields, no builder and no `Deserialize`, so the only way to hold one is to have evaluated one (`Entitlement::evaluate`, `:137`). `can_withdraw` is a `WithdrawalEntitlement` whose sole arm is `Refused` (`:155`), which is ADR 0021 expressed in the type rather than in a flag. Served: `qip-api/src/ledger_views.rs:120` projects `EntitlementView` with all three capabilities (`:314-316`) onto every row of `GET /ledger/users`. The blueprint's `can_view_execution_trace` specifically is **not** among them — the field is `can_view`, and no execution-trace capability is gated |

## Experience / governance

| Capability | Status | Evidence |
|---|---|---|
| User mandate object (capital, risk tolerance, permitted families, drawdown ceiling) | BUILT+WIRED | `qip-portfolio-engine/src/construction.rs:68` defines `struct Mandate`; imported and held as a field at `qip-kernel/src/config.rs:15,159,302` (`pub mandate: Mandate`) |
| Explanation object (plain-language, from a decision back through evidence to an entity) | BUILT-UNWIRED | `qip-compliance/src/model_risk.rs:374` defines `struct Explanation` (model reference, output, baseline, contributions, reconciling exactly in `Decimal` arithmetic) — but this explains a *model's numeric output*, a narrower scope than the blueprint's full attribution chain (fill → strategy → family → mandate; intent → belief → causal edge → world event → entity). `grep -rln "model_risk::Explanation"` finds only the crate's own `lib.rs` re-export — no external caller |
| Prediction market integration (event resolution, base rates) | BUILT+WIRED | `qip-prediction/src/{resolution,market,pricing,oracle}.rs` exists; `qip_prediction::resolution::{...}` imported at `qip-kernel/src/platform.rs:110` |
| Observability (`/metrics`, alert policies) | PARTIAL | Per `.claude/rules/domains/observability.md`: both the central and edge planes emit metrics with a real recording caller (`Platform::learn_from`, `Platform::evaluate_alternatives`, both reached from `stage_learn`), but nothing has been shown to actually be scraped by any deployed collector, and all seven alert policies remain gated off behind `workload_metrics_exist = false` in every environment. This document does not re-verify that domain's own count; it is cited as already-settled and current |

---

## Top-line counts

**Re-counted 2026-09-05 at `1107305`.** The figures below were 26 of 44 and 16
of 44 at the first pass; they are recomputed here from the rows as they now
read, not adjusted by hand. The command, and its output:

```
$ awk -F'|' '/^\| .+ \| .+ \| .+ \|$/ && $2 !~ /^ *Capability *$/ && $3 !~ /^-+$/ \
    {gsub(/^ +| +$/,"",$3); s=$3; gsub(/\*/,"",s); \
     if (s ~ /^NOT INDIVIDUALLY/) {n++; next} \
     if (s ~ /^BUILT\+WIRED/) w++; else if (s ~ /^BUILT-UNWIRED/) u++; \
     else if (s ~ /^PARTIAL/) p++; else if (s ~ /^ABSENT/) a++; else other++; t++} \
    END {print t, w, u, p, a, other+0, n}' docs/plan/blueprint-v10.1-gap-map.md
classified rows: 44
BUILT+WIRED: 24
BUILT-UNWIRED: 3
PARTIAL: 8
ABSENT (incl. the one qualified "ABSENT (deployed)"): 9
unmatched: 0
excluded NOT INDIVIDUALLY VERIFIED: 1
```

24 + 3 + 8 + 9 = 44, and the one excluded row is the "eight execution paths"
row, which was never individually verified. Counting only rows individually
classified above:

- **35 of 44** identified capabilities have *any* production code resembling
  them (BUILT+WIRED, BUILT-UNWIRED, or PARTIAL) — 24 + 3 + 8. It would be 36
  on a looser reading: the GCE execution-node row is ABSENT *as deployed* and
  says in its own status that an infrastructure module exists in code. It is
  counted with ABSENT here, because the question that row asks is whether a
  node runs, and none does.
- **24 of 44** identified capabilities have a wired production caller
  (BUILT+WIRED only — code that exists and is reached from a composition root
  or from a service the kernel actually calls). **One of the 24 was
  uncommitted when this was written and is committed now** (`b060df2`): the
  liquidity ladder's risk read. A reader checking out `1107305` still counts
  23 and 4, not 24 and 3. This bullet went on to say the control it feeds
  "abstains silently on a class of book"; **that stopped being true at
  `5ccaea9`** — the ladder's refusal now travels as `RiskState::unevaluated`
  and `PreTradeChecker::check` rejects on it before weighing a limit. Read its
  row anyway, and not as good news: the abstention became a blanket refusal,
  which is the safe direction and is still a book the platform cannot trade.
  Either way it is a correctness question this document's counts do not ask.
  Of the 24, several more are wired in the
  codebase sense but reach no *deployed* process, most notably the entire
  edge/execution plane (`execution_nodes = {}` in every environment) — that
  distinction is called out per-row above and matters more than the count.
- **3 of 44** are BUILT-UNWIRED. The valuation result is the interesting one:
  six engines were built at `1107305`, four wired then and a fifth wired
  since, leaving **the volatility surface as the only one reached by nothing**
  — and for a reason that is not "nobody got to it yet". Nothing in this
  platform ingests option quotes, so wiring it is a data source with a
  licensing evaluation ahead of it. A future re-score should check the caller
  rather than the type, and should not use `grep -rl VolatilitySurface` for
  it: since `5ccaea9` that lists `qip-kernel/src/valuation.rs` too, where the
  one occurrence is a doc comment at `:221`. The ladder's wiring survived into
  a commit (`b060df2`), and the question this bullet said to check next —
  whether the limits it feeds can be made to fire — **was answered at
  `5ccaea9`: they can.** The bullet's own answer, "as of 2026-09-06 they
  cannot on any book whose ladder is refused", is withdrawn; a refused ladder
  is now itself a refusal. What a re-score should check *now* is narrower and
  is in the row: the 250 bps `illiquid()` default that decides which books get
  refused.

These are **structural counts** — does code exist, is it called — not a
judgment of correctness, completeness, or fitness of what exists. A
BUILT+WIRED row may still be a thin or partially-correct implementation; a
PARTIAL row may be closer to complete than a BUILT+WIRED one that only
handles a narrow case. This is a first-pass inventory, not a quality
assessment, and it covers a representative sample of the blueprint's
capability surface, not all of it — the document itself describes on the
order of 70 services and dozens of named types that this pass did not each
individually verify.

## Most significant ABSENT capabilities

**Re-scored 2026-09-05.** Six of the nine bullets this list carried have been
closed or narrowed since it was written, and leaving them would send a reader
to build a thing that exists. What each was replaced by is stated rather than
deleted, because the point of this list is which gaps are real *today*.

Still absent, after trying multiple plausible names:

- **The entire ML training pipeline stack** (`burn`, `linfa`, `polars`,
  `tract`, ONNX model promotion) — blocked at the root by the workspace's own
  two-dependency policy (serde/serde_json only), not merely unbuilt. Unchanged
  and structural.
- **Exploration budget** — no UCB/Thompson allocator and no capital line item
  for information gain. `grep -rl ExplorationBudget backend/crates
  --include=*.rs` returns nothing. This is the half of the self-model bullet
  that survived.
- **Compounding policy** — no reinvestment cadence, no fee-tier accumulation,
  no withdrawal drag.
- **Leg coordinator** and **meta-learning** — `grep -rl LegCoordinator
  backend/crates --include=*.rs` returns nothing; the meta-learning search is
  unchanged from the first pass.
- **A passkey-only, password-free sign-in** — the frontend's own auth-method
  type still lists `password`. ADR 0038 is *proposed*, not applied, and turns
  on four Identity Platform questions nobody has answered.
- **Leptos** — ADR 0025 is a record with no code; the portal is Next.js and
  ADR 0022 calls that transitional.
- **Asset class registry, venue onboarding, DeFi execution model,
  cross-margin model, hedge map** — none found; not individually detailed as
  full rows above because each returned zero hits on first search, but
  recorded here since the section is a named blueprint capability.

No longer absent, and each row above now carries the evidence:

- **Treasury, corridors, and wallet** — closed at `5546a24` as far as ADR 0021
  permits: corridor, destination, transfer-gate, custody and journal types
  exist and the wallet read model is served at `GET /wallet`. What stays
  refused is the transfer engine and the MPC signer, so the gate's `Approved`
  reaches no consumer. That is a decision, not a gap.
- **The six valuation engines** — this bullet said three of six had no type at
  all, a fourth had two struct fields and a fifth was unwired. At `1107305`
  **all six exist**, and the number that matters is that **four have a
  production caller and two do not**: term structure, credit, cashflow
  commitments and illiquid valuation are reached from `Platform`, the last two
  on the sizing path itself; the volatility surface and the liquidity ladder
  are BUILT-UNWIRED. The bullet was also wrong about credit in one direction —
  `spread_decomposition` had existed in exact `Decimal` since `b3ebc7f`.
- **Self-model** — built at `e5dc8fc`, served at `GET /cognition/self-model`,
  and since `0829b29` one of the three §6.2 rows the centre sizes against.
- **Liquidity ladder** — the type exists and, since `b060df2`, is
  read by `RiskState` and by the `MaxDaysToLiquidate` and `MinLiquidity`
  limits. **Wired is not working**: as committed, a refused ladder leaves both
  figures empty and the floor accepts everything, and a forty-five-day holding
  reads as two days. The row above carries the probes. The *ordering it would
  serve* still does not exist and may not,
  because serving a withdrawal needs a granted withdrawal and ADR 0021 leaves
  `WithdrawalEntitlement` with one arm: wired along the admissible seam,
  unwirable along its blueprint one.
- **Per-account entitlements** — `Entitlement` carries `can_view`,
  `can_invest` and `can_withdraw`, and `GET /ledger/users` serves all three.
  The blueprint's `can_view_execution_trace` specifically is still not a
  capability anything gates.
