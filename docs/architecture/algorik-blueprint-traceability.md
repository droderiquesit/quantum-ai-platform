# Algorik Master Blueprint v10.1-4 — traceability against this repository

**Scored against the working tree on branch
`claude/algorik-architecture-refactor-pmp0zy`, from commit `d8b3597`.**

**This is the live scorecard.** ADR 0022 makes the Algorik Master Blueprint
v10.1-4 and its companion diagram the architecture of record, so every row
below is scored against the blueprint and nothing else.

[`diagram-reconciliation.md`](diagram-reconciliation.md) and
[`canonical-platform.md`](canonical-platform.md) score the **superseded**
reference — the "World's Smartest Multi-Regional AI + Quant Trading Platform"
diagram. They are retained for history and are not merged into this file: a
component ALIGNED against the old diagram can be MISSING against the
blueprint, and collapsing that loses the finding. Do not score new work
against them.

**Method.** Every row was derived by reading the source or the manifest named
in its evidence column. A row is ALIGNED only where an implementation path and
a passing named test both exist. Where a type exists but no deployable binary
composes it, the ceiling is UNVERIFIED, whatever its own tests say.

**Status vocabulary.** ALIGNED · PARTIAL · CONTRADICTS · MISSING-CURRENT (the
blueprint requires it at or before the phase this repository has reached) ·
PLANNED-FUTURE (the blueprint puts it in a later phase; it is backlog, not a
gap) · UNVERIFIED · NOT-APPLICABLE.

## Where the platform actually sits on the blueprint's roadmap

The honest answer is that capability and phase have come apart, and the four
gates are the reason it matters.

| Gate | Blueprint question | Status | Evidence |
|---|---|---|---|
| End of Phase 2 | Does a family survive holdout with honest significance after cumulative trial correction? | **NOT PASSED** | The machinery exists — `qip-simulation-engine/src/validation.rs`, `qip-lifecycle/src/gates.rs`, `qip-lifecycle/src/evidence.rs`. The gate is an empirical question about real market data, and every deployment's data is synthetic or replayed. A family surviving a holdout of data the platform generated is not the gate |
| End of Phase 3 | Does it survive contact with a live venue, inside its holdout band? | **CANNOT PASS** | Structurally unreachable: paper trading is absolute. See ADR 0021 |
| End of Phase 6 | Is calibrated probability better than the market's implied on prediction contracts? | **NOT PASSED** | `qip-prediction` has `market.rs`, `oracle.rs`, `pricing.rs`, `resolution.rs`; no Brier comparison against a live venue's implied probability exists |
| End of Phase 8 | Does regime-conditional allocation beat unconditional out of sample? | **NOT PASSED** | Regime detection exists (`qip-cost-router/src/context.rs`, `qip-simulation-engine/src/conditions.rs`); no out-of-sample comparison against an unconditional baseline is computed |

**No gate has passed.** Every one of the four is an empirical claim about real
data or real venues, and this repository has neither. Code existing is not a
gate passing, and the distance between the two is the single most important
fact in this document.

Capability, meanwhile, is spread from Phase 1 to roughly Phase 15: the
research loop, multi-leg execution, champion/challenger, the cost router, the
quantum adapter with its mandatory classical baseline and a three-region
topology all exist. That is ahead-of-phase work in the blueprint's terms. It
is not deleted — it is useful research — but it is labelled here so that
nothing in it reads as a gate that was cleared.

## Constraints and architectural rules (§2, §3, §39)

| Blueprint element | Required invariant | Implementation | Status | Evidence | Minimal action | Risk / blast radius | Phase | Validation |
|---|---|---|---|---|---|---|---|---|
| §2.1, §40 | Every application is Rust; one Leptos codebase for the experience layer | Backend is 59 Rust crates; `frontend/portal` and `frontend/landing` are Next.js/TypeScript — now **transitional**, not a sanctioned exception (ADR 0022) | CONTRADICTS | `frontend/portal/package.json`; ADR 0001's browser exception is superseded in direction by ADR 0022 | None now, and none authorised. Identify contracts and Playwright coverage, then define the Leptos replacement boundary. Do not mass-translate; a vertical slice only if it adds no dependency | High — a rewrite of the whole customer surface, and it is the only customer-facing thing there is | 13 | Playwright + contract tests before any slice |
| §2.1 | Managed services are Google Cloud or IBM only | GCP + IBM Quantum; no third-party SaaS at runtime | ALIGNED | `infrastructure/terraform/modules/`; `libs/qip-quantum/src/provider.rs` | None | — | 0 | `infrastructure` suite |
| §2.2 | No strategy sends an order | Strategies produce theses/proposals; only a composition root holds an order manager | ALIGNED | `architecture.rs::nothing_outside_a_composition_root_holds_an_order_manager`, `::only_the_edge_cell_itself_holds_an_order_manager` | None | — | 3 | `architecture` suite |
| §2.2, §39 | No language model touches a trade, cycle or transfer | Enforced by absent dependency edges, transitively | ALIGNED | `architecture.rs::no_safety_critical_engine_can_reach_a_language_model`, `::nothing_that_decides_or_executes_names_the_language_model_interface`, `::an_agent_that_holds_a_language_model_cannot_touch_the_market` | None | — | 0 | `architecture` suite |
| §2.2, §39 | Quantum output is policy, never a live instruction | No crate that **vetoes, executes, transfers or issues** reaches `qip-quantum`, in either direction; no edge crate does either | PARTIAL | `architecture.rs::nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver`, `::no_edge_cell_can_reach_a_quantum_solver`, `::a_quantum_solver_cannot_reach_anything_that_vetoes_executes_or_moves_money` | Residual, and deliberate: `qip-portfolio-engine -> qip-optimization-engine -> qip-quantum` is uncovered. See the argued exemption below | Low — sizing from policy is the intended consumption path, not a veto | 15 | `architecture` suite |
| §2.2, ADR 0006 | A classical baseline runs every time | Computed on every quantum path | ALIGNED | ADR 0006; `services/qip-optimization-engine/src/router.rs` | None | — | 15 | `optimization` tests |
| §2.2 | Deterministic pre-trade checks never route to a model | `Determinism::Required` returns a type that cannot name a model rung | ALIGNED | `services/qip-cost-router/src/router.rs:404`; `context.rs:27` | None | — | 3 | `cost_router` tests |
| §2.2 | Risk reads aggregates, never strategy lists | Risk state is aggregate counters | PARTIAL | `libs/qip-risk/src/limits.rs`; `services/qip-risk-engine/` | Assert the O(1)-in-strategy-count property with a test | Low | 10 | A test at two strategy counts |
| §2.2 | Feasibility precedes profitability | No feasibility gate exists as a named stage | MISSING-CURRENT | `grep -rln "[Ff]easibilit"` returns only `qip-numerics` (LP feasibility) and the optimiser's router — not an execution gate | Backlog. The gate belongs beside the pre-trade path in `qip-execution-engine` | Medium — a control that does not exist cannot fire | 3 | Passing-and-vetoing fixtures |
| §2.2 | Strategies are compiled, not interpreted | `qip-strategy` evaluates; no shared compiled plan with CSE | PARTIAL | `edge/qip-strategy/` | Backlog | Low at current strategy counts | 10 | Netting-ratio measurement |
| §2.2 | After-tax return is the only return | No tax engine, no lot selection | MISSING-CURRENT | No `taxlot`/`tax_engine` in the tree | Backlog | Low while paper-only | 3 | — |

## The seven planes (§1.2, §5, §4.2)

Planes are bounded investment responsibilities and are deliberately **not**
the same axis as the seven layers below. Do not rename one as the other, and
do not fold Cognition into Intelligence — §4.1 argues the split and the
argument still holds here.

| Plane | Blueprint responsibility | Implementation | Status | Evidence | Minimal action | Phase |
|---|---|---|---|---|---|---|
| 1 Ingestion | Observe world + prices; resolve to entities; pass-through, not accumulation | `qip-market-ingestion`, `qip-normalization`, `qip-entity-resolution`, `qip-data-finder` (licensing posture before use) | PARTIAL | `platform.rs` absorbs 11 record kinds; `Feed::Live` exists at `apps/qip-fastbrain/src/feed.rs:61` and `::live` at `:108` | Prove one live source end to end; no deep-web tier exists | 1, 5 |
| 2 Cognition | World model, causal graph, episodic memory, belief, counterfactual, self-model, hypotheses | `qip-world-model`, `qip-agents/src/memory.rs`, `qip-twin`, `qip-reasoning-engine/src/hypothesis.rs` | PARTIAL | World model and hypotheses present; counterfactuals in `qip-twin` | **No self-model exists** (`grep -rln "SelfModel"` empty); no dedicated causal graph over drivers; belief is scattered rather than a plane | 7, 8, 9 |
| 3 Valuation | Price what has no price: term structure, credit, vol surface, illiquid, cashflow, corporate actions | Corporate actions absorbed in `platform.rs`; `qip-financial/src/extensions.rs` carries illiquid-adjacent types | MISSING-CURRENT | No term-structure, credit or vol-surface engine | Backlog — Phase 14 | 14 |
| 4 Intelligence | Train, generate and statistically gate strategies, set risk and corridor policy | `qip-training`, `qip-lifecycle`, `qip-evolution`, `qip-simulation-engine/src/validation.rs` | PARTIAL | Statistical gate, champion/challenger and promotion exist | Corridor policy has no owner because corridors do not exist | 2, 10 |
| 5 Optimisation | Allocation across families/regimes/horizons; quantum + classical; policy only | `qip-optimization-engine`, `qip-quantum` | PARTIAL | Routing gate and classical baseline present; authority boundary now structural | Family clustering and multi-horizon reconciliation absent | 15 |
| 6 Execution | Regional nodes, shipped policy, microseconds, local decisions | `qip-edge` (structurally paper-only), `qip-edge-node`, `qip-orderbook`, `qip-routing`, `qip-execution-engine` | PARTIAL | `cell.rs:143-148` — no constructor takes a non-paper ceiling | Runs as a pod, not the blueprint's bare C3. See ADR 0020. No intent netting, no inventory reservation | 3, 16 |
| 7 Ledger, wallet, treasury | Authoritative money state per user and per strategy; reconcile every holding; move capital in signed corridors | `qip-capital`, `qip-capital-fabric` (`transfer.rs`, `settlement.rs`), hash-chained event log | PARTIAL | Capital allocation, envelopes and exposure exist | **No wallet, no corridor, no transfer gate, no destination registry, no custody engine** — `grep` for each returns nothing. Phase 12, and bounded by ADR 0021 | 12 |

### Plane detail — the format the programme asks for

`[PLANE n/7 — Name] Ownership | Placement | Inputs/outputs | State | Authority | Degradation | Tests`

- **[PLANE 1/7 — Ingestion]** Ownership: `qip-market-ingestion`. Placement: global, once. I/O: sources → normalised bitemporal records. State: bounded working set. Authority: none — observes only. Degradation: world model ages; price-only strategies continue (§6.2 row 1) — *not implemented as an ordered narrowing*. Tests: `absorption.rs`, `sense.rs`.
- **[PLANE 2/7 — Cognition]** Ownership: split across `qip-world-model`, `qip-twin`, `qip-reasoning-engine`. Placement: global. I/O: events → beliefs/hypotheses. State: bitemporal. Authority: **none — informs only**, which matches §39 rows 3. Degradation: undefined. Tests: `understanding.rs`, `reasoning.rs`.
- **[PLANE 3/7 — Valuation]** Ownership: none. Placement: n/a. Authority: would be informs-only. Status MISSING-CURRENT; Phase 14.
- **[PLANE 4/7 — Intelligence]** Ownership: `qip-lifecycle` (gates), `qip-training`, `qip-evolution`. Placement: global. Authority: promotes within approved families (§39 layer 2) — matches. Degradation: undefined. Tests: `lifecycle.rs`, `evolution.rs`.
- **[PLANE 5/7 — Optimisation]** Ownership: `qip-optimization-engine`. Placement: global. Authority: sets budgets inside the envelope (§39 layer 7) — matches, and is now the *only* zone reaching the solver. Degradation: classical baseline always runs, so a QPU outage narrows nothing. Tests: `optimization.rs`, `architecture.rs`.
- **[PLANE 6/7 — Execution]** Ownership: `qip-edge`. Placement: regional. Authority: veto-only gates plus order placement inside a granted envelope (§39 layers 9–12). Degradation: stale book supplies nothing (`seam.rs:53`), venue health in `qip-routing/src/health.rs` — mechanism-level, not §6.2's capability-level order. Tests: `e2e.rs`, `resilience.rs`, `chaos.rs`.
- **[PLANE 7/7 — Ledger/wallet/treasury]** Ownership: `qip-capital` + the event log. Placement: global. Authority: records (§39 layer 14). Degradation: undefined. Tests: `truth_loop.rs`, `compliance_proof.rs`. Wallet and treasury do not exist.

### The argued exemption: `qip-portfolio-engine`

Independent review found that `qip-portfolio-engine` reaches the solver
transitively and is not covered by the boundary test, while the test's own
comment claimed to cover "every crate that holds a veto, places an order, or
issues capital". The comment was wrong and has been rewritten; the omission
was right and is kept, for a reason worth stating.

Blueprint §39 puts the optimiser at layer 7 with authority over "allocation,
cycle selection, path assignment inside the envelope", and the strategy engine
at layer 8 proposing against it. Optimiser output *exists in order to be
consumed* as policy — grants, budgets, targets, whitelists, limits. A
portfolio engine that turns approved hypotheses into constrained target
portfolios is that consumption working exactly as designed. Forbidding the
edge would outlaw the intended path and protect nothing.

So the enforced property is narrower than "touches money" and is now stated as
what it is: **nothing that vetoes, executes, transfers or issues may reach a
solver, and no solver may reach any of them.** `qip-portfolio-engine` sizes
from policy and does none of those four. It is exempt on that argument, not by
oversight, and `every_service_crate_is_classified_for_money_authority` is what
stops a future crate from being exempt by oversight.

## §6.2 — the degradation order

Implemented as a capability-level type in
`backend/crates/libs/qip-contracts/src/degradation.rs`. It composes with, and
does not replace, the mechanism-level rules already in the tree — a stale book
supplying nothing (`edge/qip-edge/src/seam.rs:53`) and venue health
(`edge/qip-routing/src/health.rs`) answer a different question from "the causal
graph has not been re-estimated, so how large may we size?".

Rows exist only for capabilities this repository actually has. A row for a
capability that can never be unavailable is a control that cannot fire, and
this repository has already been bitten by that nine times.

| §6.2 row | Required behaviour | Status | Where |
|---|---|---|---|
| Ingestion stalls | Event-driven and prediction-market strategies pause; price-only continue unaffected | ALIGNED | `DegradationState::pauses`; `contracts.rs::an_ingestion_stall_pauses_the_strategies_that_need_the_world_and_no_others` |
| Causal graph stale | Regime-conditional allocation reverts to unconditional; sizing more conservative | ALIGNED | `allocation_mode`, `sizing_multiplier`; `::a_stale_causal_graph_reverts_to_unconditional_allocation_and_sizes_smaller` |
| Episodic memory unavailable | Situational-recognition strategies pause; the rest continue | ALIGNED | `::episodic_loss_pauses_only_the_strategies_that_recognise_situations` |
| Belief state stale beyond TTL | Fixed conservative multiplier; nothing halts | ALIGNED | `::a_belief_state_stale_beyond_its_ttl_falls_back_to_a_fixed_multiplier_and_halts_nothing` |
| Counterfactual scoring down | No trading impact whatsoever | ALIGNED | `::losing_counterfactual_scoring_changes_no_trading_decision_whatsoever` |
| Self-model stale | Exploration budget reverts to flat | PLANNED-FUTURE — Phase 9 | No self-model exists (`grep -rln "SelfModel"` returns nothing). Deliberately not represented |
| Valuation engine down | Illiquid assets frozen at last mark and flagged | PLANNED-FUTURE — Phase 14 | No term-structure, credit or vol-surface engine exists. Deliberately not represented |

Two properties are held beyond the table itself, because both are the kind that
erode quietly:

- **Absence fails closed.** A capability nobody has reported on reads as
  `Unavailable`, so a dead reporter cannot be mistaken for a healthy subsystem.
- **Nothing halts.** `halts()` is a method returning false rather than an
  absence, so a later change that wants to halt has to come through it and
  explain itself. Halting belongs to the kill switch an operator holds.

**Not yet wired.** The type has no production caller. It precedes its consumer
on purpose: the consumer is the signed twelve-item payload (§41.5), whose
stale-item narrowing is defined in exactly these terms, and that is the next
slice. Until then this is UNVERIFIED at the platform level by this document's
own rule, and is recorded as such rather than counted as a working control.

## The seven layers (§40.5, §41, §45, §46, §47, §48)

`[LAYER n/7 — Name] Current | Keep | Change | Remove | Defer | Verification`

- **[LAYER 1/7 — Experience]** *Current:* Next.js portal and landing on Cloud Run; blueprint wants one Leptos codebase. *Keep:* the whole surface, maintained — it works, it is the only customer-facing thing there is, and ADR 0022 makes it transitional rather than disposable. *Change:* nothing this pass. *Remove:* nothing. *Defer:* the Leptos replacement boundary, direction settled and execution unauthorised — identify contracts and Playwright coverage first; a vertical slice only if it adds no dependency. *Verification:* `npm run lint`, `npm run build`, Playwright.
- **[LAYER 2/7 — Public edge and identity]** *Current:* Identity Platform is the only identity store (ADR 0019); sealed-cookie sessions; console reaches the platform over the VPC as viewer (ADR 0018). *Keep:* all of it — it matches §46.1's "Application and identity" zone, including "never a node, a venue, a QPU or a key". *Change:* none. *Remove:* none. *Defer:* passkeys (§51 Phase 0) — not present. *Verification:* `console_route.rs`, `security.rs`.
- **[LAYER 3/7 — Application and API]** *Current:* `qip-api` composes reads and holds no independent financial state. *Keep.* *Change:* none. *Remove:* none. *Defer:* the typed-intent surface (§40.9) — there is no `Intent` type anywhere, so application APIs raise no intents; they read. *Verification:* `documentation.rs::every_documented_endpoint_exists`.
- **[LAYER 4/7 — Domain contracts and control fabric]** *Current:* `qip-contracts` sits at the bottom of everything sharing it; `qip-transport`/`qip-mesh` carry the fabric. *Keep.* *Change:* none this pass. *Remove:* none. *Defer:* the **signed twelve-item payload (§41.5)** — the fabric ships deltas, not a twelve-item verified-then-atomically-swapped payload, and stale-item narrowing per §6.2 is not implemented. This is the largest single structural gap against the blueprint that is *not* future-phase. *Verification:* `spine.rs`, `mesh.rs`, `manifest_wiring.rs`.
- **[LAYER 5/7 — Data and state]** *Current:* bitemporal records; bounded retention; event log hash-chained; `qip-data-finder` evaluates licensing before use. *Keep.* *Change:* none. *Remove:* none. *Defer:* BigQuery derived series and content-hash manifests for external history. *Verification:* `absorption.rs`, `resilience.rs`, `truth_loop.rs`.
- **[LAYER 6/7 — Cloud and network]** *Current:* GKE + Argo CD + Kargo + Helm + KEDA; frontends on Cloud Run; no GCE instance. The blueprint's target — Cloud Run plus one bare C3, no Kubernetes — is now the architecture of record (ADR 0022), so this layer is a **transitional runtime with a decided direction**. *Keep:* all of it, and maintained rather than merely tolerated — it is what carries the traffic, and transitional does not mean abandoned. *Change:* nothing. *Remove:* **nothing, and not until step 5 of ADR 0020's sequence has both its evidence and recorded human approval.** *Defer:* the entire migration. Direction is settled; **execution is not authorised**, and no step may begin without step-named approval. *Verification:* `terraform fmt -check` and `validate` **NOT RUN — terraform is not installed in this environment**; `infrastructure.rs` suite passed.
- **[LAYER 7/7 — Security, observability, delivery, reliability]** *Current:* three paper layers intact and re-verified by path this pass; WIF only; CSI-projected secrets; Binary Authorization; telemetry emitted at the seams. *Keep.* *Change:* none. *Remove:* none. *Defer:* OpenTelemetry spans with cross-plane correlation (§47) — the current surface is a Prometheus-style metric registry, not spans; and policy-freshness, belief-calibration and reconciliation signals have nothing to emit yet. *Verification:* `security.rs`, `compliance_proof.rs`, `egress.rs`, `infrastructure.rs`.

## Corrections this pass makes to existing documents

Two governed documents disagreed about the same fact, and the disagreement was
resolved by reading the code rather than by preferring the newer document.

| Claim | Where | Verdict | Evidence |
|---|---|---|---|
| "Nothing currently writes to `Telemetry`" | `.claude/rules/domains/observability.md` | **Stale — the code contradicts it** | `runtime/qip-kernel/src/platform.rs:1668` counts cycles and `:1728-1755` records stage runs, latencies and gauges; that registry is served at `apps/qip-api/src/routes.rs:910-912`, so it is a live path and not merely a constructed type. (`qip-market-ingestion/src/service.rs:153,174,191` also records, but `IngestionService` is composed by nothing — `e2e_live.rs:81-85` — so by this document's own rule it is UNVERIFIED and carries no weight here.) |
| "Telemetry emission was closed" | `docs/plan/gap-matrix.md` item 2 | **Correct** | Same evidence |
| "Live data sources are unwired; `feed.rs` can open `Synthetic` or `Replay` and nothing else" | `docs/plan/current-state.md` | **Stale** | `apps/qip-fastbrain/src/feed.rs:61` declares `Live(Box<RestMarketDataAdapter>)` and `:108` constructs it behind the licensing gate |
| "3,078 tests passing" | `docs/plan/current-state.md` | **Stale** | Measured this pass: 3177 passed, 0 failed, 0 ignored across 290 binaries |

The rule file is **not edited here.** `.claude/rules/` is instruction
configuration and correcting it is an owner's decision, not an agent's, even
when the correction is a plain matter of fact. It is listed instead as
requiring a decision.

## Conflicts, and where each now stands

ADR 0022 made the blueprint the architecture of record. That closed two of
these, settled the direction of two more, and — importantly — made one of them
sharper rather than resolving it.

### C1 — the authoritative design specifies something this platform refuses

**Status: NOT settled. Requires an explicit, separate owner decision.**

This is the one to read carefully, because its status changed in a way that is
easy to misread as resolution.

The blueprint assumes real capital: live venues (§25), treasury transfers and
MPC signing corridors (§37), custody (§37.4), a wallet with a signing path
(§38), and a Phase 3 gate that is thirty days live. That specification is now
the architecture of record.

It was previously a question of *which document is authoritative*. That
question is answered. What remains is the sharper statement:

> **The authoritative design specifies something this platform deliberately
> refuses.**

**Adopting the blueprint is not authorisation to build, enable or ease any
live-order or live-transfer path.** The owner said the blueprint is the
expected design; they did not say to weaken the paper-trading boundary, and
the second does not follow from the first.
`.claude/rules/01-security-and-safety.md` makes that boundary absolute and
says it cannot be weakened by a task instruction — a blueprint revision is not
an exception, and neither is an inference drawn from one.

What would be required to change it, and nothing less: **an explicit and
separate owner decision that supersedes ADR 0003 and amends
`.claude/rules/01-security-and-safety.md`.** No agent may take it, and no
amount of architectural adoption substitutes for it.

Until then: ADR 0021 stands exactly as written; the three layers stay intact
(Terraform at `infrastructure/terraform/variables.tf:105-116`,
`AutonomyLevel::deployable` in all three composition roots, `Cell::new` taking
no ceiling but paper trading); and
`security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
stays.

The standing consequence is unchanged and should be carried forward wherever
the roadmap is discussed: the blueprint's Phase 3 gate is permanently
unreachable here, and because it is upstream, so are Phase 6 and Phase 8 on
the blueprint's own terms. That is a limit on what this repository can
demonstrate, not a defect in it.

### C2 — runtime topology · direction settled, execution NOT authorised

The blueprint's no-Kubernetes target (§41.4, §41.6, §45.1) is the intended end
state. GKE, Argo CD, Kargo, Helm and KEDA are a **transitional runtime**, not a
competing permanent architecture. ADR 0011 and ADR 0017 are superseded in
direction and still govern what runs today.

**No step is authorised.** ADR 0020's sequence is the route, every step of it
requires recorded human approval naming that step, and nothing is migrated,
decommissioned or provisioned. Direction and authorisation are different
decisions.

### C3 — experience layer · direction settled, execution NOT authorised

Leptos over shared types (§40) is the target. `frontend/portal` and
`frontend/landing` are transitional and are **maintained**, not abandoned —
they are the only customer-facing surface there is. ADR 0001's browser
exception is superseded in direction.

No migration now. The sequencing stays in the backlog: identify contracts and
Playwright coverage, define the replacement boundary, and only then consider a
vertical slice — and only if it adds no dependency without an ADR.

### C4 — a stale factual claim in an instruction file · still open

`.claude/rules/domains/observability.md` states that nothing writes to
`Telemetry`. The code contradicts it: `qip-kernel/src/platform.rs:1668` counts
cycles and `:1728-1755` records stage runs, latencies and gauges, served at
`apps/qip-api/src/routes.rs:910-912`.

**Still the owner's, not an agent's.** `.claude/rules/` is instruction
configuration, and correcting it is an owner's decision even when the
correction is a plain matter of fact. Left untouched and flagged.

### C5 — which diagram is authoritative · CLOSED

Closed by ADR 0022. The Algorik blueprint and its companion diagram are the
architecture of record; this file is the live scorecard.
`canonical-platform.md` and `diagram-reconciliation.md` score the superseded
reference and are retained for history, each carrying a banner saying so.
