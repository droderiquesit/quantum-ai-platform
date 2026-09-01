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

Runtime evidence below comes from the flow trace in
[`integration-truth-pass.md`](integration-truth-pass.md), not from the crate
names. **No plane was given a service because the blueprint names one.** In
every case the question asked first was whether separate deployment is
justified today by security, scaling, failure isolation, cadence or ownership;
in every case the answer was that crate and interface alignment is sufficient
at current scale, and process proliferation was rejected.

- **[PLANE 1/7 — Ingestion]** *Ownership:* `qip-market-ingestion`,
  `qip-normalization`, `qip-entity-resolution`, `qip-data-finder`.
  *Placement:* global, once — correct per §4.2. *I/O:* sources → normalised
  bitemporal records; eleven record kinds absorbed at `platform.rs:1129-1243`.
  *State:* bounded working set, licensing posture evaluated before use.
  *Authority:* **none — observes only**, which matches §46.1's requirement that
  the widest external surface reach nothing that moves money; enforced by the
  absence of an edge to `qip-capital`. *Degradation:* mechanism-level only —
  a stale book supplies nothing (`edge/qip-edge/src/seam.rs:53-61`). §6.2's
  capability-level row is typed and unwired. *Tests:* `absorption.rs`,
  `sense.rs`, `rest_feed.rs`. *Separate service justified?* No — one cadence,
  one owner, no isolation argument.

- **[PLANE 2/7 — Cognition]** *Ownership:* split across `qip-world-model`
  (including a real causal graph — `world.rs:41`, `:192`, `causal.rs:234`),
  `qip-agents/src/memory.rs` (episodic), `qip-twin` (counterfactual),
  `qip-reasoning-engine` (hypotheses, `bayes.rs` for Bayesian updating).
  *Placement:* global. *I/O:* events → theses. *State:* bitemporal.
  *Authority:* **none — informs only**, matching §39 layer 3.
  *Degradation:* undefined. *Tests:* `understanding.rs`, `reasoning.rs`.
  *Gaps:* **no belief stage in the cycle** — `grep -n belief
  runtime/qip-kernel/src/platform.rs` returns one doc-comment line and no code
  — and **no self-model at all**. Confidence-weighted sizing per §11.2 is not
  the mechanism here. *Separate service justified?* Not yet; the split across
  four crates already provides the isolation, and a fifth process would add
  deployment surface without adding a boundary.

- **[PLANE 3/7 — Valuation]** *Ownership:* **none.** *Placement:* n/a.
  *Authority:* would be informs-only per §39 layer 4. *State:* n/a.
  *Degradation:* n/a. *Tests:* none. MISSING-CURRENT, blueprint Phase 14.
  **Deliberately not scaffolded** — six engines named by §16.1 with no consumer
  would be six empty crates.

- **[PLANE 4/7 — Intelligence]** *Ownership:* `qip-lifecycle` (statistical
  gates, `gates.rs`, `evidence.rs`), `qip-training`, `qip-evolution`
  (champion/challenger, wired at `apps/qip-deepbrain/src/evolution.rs:426`),
  `qip-simulation-engine/src/validation.rs`. *Placement:* global.
  *I/O:* outcomes → promoted strategies and risk policy. *State:* model
  registry, drift reports recorded at `apps/qip-deepbrain/src/learning.rs:279`.
  *Authority:* **promotes within approved families** — §39 layer 2, matches.
  *Degradation:* undefined. *Tests:* `lifecycle.rs`, `evolution.rs`,
  `training.rs`. *Gap:* corridor policy has no owner because corridors do not
  exist (Phase 12). *Separate service justified?* Cadence differs from the hot
  path and it already runs in its own binary, `qip-deepbrain`. Satisfied.

- **[PLANE 5/7 — Optimisation]** *Ownership:* `qip-optimization-engine`,
  `qip-quantum`. *Placement:* global. *I/O:* problem → policy.
  *State:* solver results with a classical baseline computed every time
  (ADR 0006, `router.rs`). *Authority:* **sets budgets inside the envelope**
  (§39 layer 7) and is now structurally the *only* zone that reaches the
  solver, in both directions —
  `architecture.rs::nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver`
  and `::a_quantum_solver_cannot_reach_anything_that_vetoes_executes_or_moves_money`.
  *Degradation:* a QPU outage narrows nothing, because the classical baseline
  always runs. *Tests:* `optimization.rs`, `architecture.rs`.

- **[PLANE 6/7 — Execution]** *Ownership:* `qip-edge` (structurally paper-only,
  `cell.rs:143-148`), `qip-edge-node`, `qip-orderbook`, `qip-routing`,
  `qip-execution-engine`. *Placement:* regional — three cells in stage tfvars,
  none in dev, and **as Kubernetes pods rather than the blueprint's bare C3**
  (ADR 0020). *I/O:* signed capital envelope down, `CellStateDelta` up.
  *State:* local books, inventory, journal. *Authority:* veto-only gates plus
  placement inside a granted envelope (§39 layers 9–12); a cell cannot mint its
  own capital or promote its own strategy. *Degradation:* stale book supplies
  nothing; venue health in `qip-routing/src/health.rs`; the cell self-halts when
  its fills disagree with the venue drop-copy (`cell.rs:774-786`). *Tests:*
  `e2e.rs`, `resilience.rs`, `chaos.rs`, `apps/qip-edge-node/tests/mesh.rs`.
  *Gaps:* no feasibility gate, no inventory reservation. Two earlier gaps are
  now closed and are recorded here rather than deleted, because the fix is what
  the row is evidence of: the halt reaches a cell (`VerifiedHalt`, the policy
  downlink), and **intent netting exists** — `Cell::work` builds one `Intent`
  per firing strategy, nets them on instrument, venue and representation, and
  places what survives (`libs/qip-contracts/src/intent.rs`,
  `edge/qip-edge/src/cell.rs`, `apps/qip-edge-node/tests/gateway.rs`).

- **[PLANE 7/7 — Ledger, wallet and treasury]** *Ownership:* `qip-capital`
  (allocation, envelope, exposure), `qip-capital-fabric` (internal placement),
  and the hash-chained event log. *Placement:* global. *I/O:* approved requests
  → signed grants. *State:* **there is no `Ledger` type** — money state is
  capital allocation plus the log, which is a different shape from §43.3's
  per-user, per-strategy authoritative ledger. *Authority:* records (§39 layer
  14); issuance requires two signatures and a fresh credential
  (`qip-compliance/src/approval.rs`). *Degradation:* the log is append-only and
  hash-chained. *Tests:* `truth_loop.rs`, `compliance_proof.rs`.
  *Gaps:* **no wallet, corridor, transfer gate, destination registry or custody
  engine** — Phase 12, bounded by ADR 0021 and enforced by
  `security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`.
  Capital reservation is unbuilt, so two concurrent proposals can pass against
  one balance.

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

**Now wired.** The consumer arrived with the payload slice: a cell derives its
narrowing from the applied payload every pass (`qip-edge/src/cell.rs` —
`narrowing()`, consumed in `work()`), the multiplier sizes real orders, the
pause gate refuses by strategy class, and a cell with no payload sits at the
conservative floor. Mutation-verified end to end — a policy-less cell reading
as fully available, the pause gate removed, and the multiplier pinned to one
each fail named tests.

## The seven layers (§40.5, §41, §45, §46, §47, §48)

`[LAYER n/7 — Name] Current | Keep | Change | Remove | Defer | Verification`

- **[LAYER 1/7 — Experience]** *Current:* Next.js portal and landing on Cloud Run; blueprint wants one Leptos codebase. *Keep:* the whole surface, maintained — it works, it is the only customer-facing thing there is, and ADR 0022 makes it transitional rather than disposable. *Change:* nothing this pass. *Remove:* nothing. *Defer:* the Leptos replacement boundary, direction settled and execution unauthorised — identify contracts and Playwright coverage first; a vertical slice only if it adds no dependency. *Verification:* `npm run lint`, `npm run build`, Playwright.
- **[LAYER 2/7 — Public edge and identity]** *Current:* Identity Platform is the only identity store (ADR 0019); sealed-cookie sessions; console reaches the platform over the VPC as viewer (ADR 0018). *Keep:* all of it — it matches §46.1's "Application and identity" zone, including "never a node, a venue, a QPU or a key". *Change:* none. *Remove:* none. *Defer:* passkeys (§51 Phase 0) — not present. *Verification:* `console_route.rs`, `security.rs`.
- **[LAYER 3/7 — Application and API]** *Current:* `qip-api` composes reads and holds no independent financial state. *Keep.* *Change:* none. *Remove:* none. *Defer:* the typed-intent surface (§40.9). An `Intent` type now exists (`libs/qip-contracts/src/intent.rs`) but it is the *execution* vocabulary, produced and consumed inside one cell; application APIs still raise no intents, they read. The gap is the API surface, not the type. *Verification:* `documentation.rs::every_documented_endpoint_exists`.
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

### C1 — the destination is agreed; the opening is gated and unexecuted

**Status: no longer a conflict. Sequenced work with a hard precondition.**

The owner has decided that **real trading is the intended end state and paper
trading is the correctness harness on the way there** (ADR 0023). That aligns
the repository with the architecture of record rather than against it —
blueprint §1.3 describes exactly this relationship, calling small capital "a
correctness harness, not an engine — real money at risk to prove the plumbing,
which is exactly what the Phase 3 gate exists for".

This row has now moved twice and the current position is the one that matters:

| Was | Then | Now |
|---|---|---|
| "Which document is authoritative?" | "The authoritative design specifies something this platform deliberately refuses" | **"The destination is agreed; the opening is gated and unexecuted"** |

**Nothing is open.** ADR 0023 records intent and a ten-step sequence and
authorises no step of it. ADR 0003 and ADR 0021 both stand and are superseded
only at step 5 of that sequence, explicitly and with recorded approval. All
three layers are intact, and
`security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
stays.

**The precondition is the constraint, and it is the blueprint's own.** §51.1
words the Phase 2 gate more strongly than any other in the document: *"Does a
family survive holdout with honest significance after cumulative trial
correction? If no: Stop. Do not build execution infrastructure. This is the
most important gate in the document."* Zero of the four gates have passed, so
the authoritative design itself defers live execution. The destination being
agreed does not advance the sequence by one step.

**What this makes critical.** `docs/plan/gap-matrix.md` ordered-work item 6 —
proving one live market source — is now step 1 of the opening sequence and
therefore on the critical path to live trading. It was a plan item; it is now
the first thing between this platform and its stated destination.

Steps 1 to 4 of ADR 0023's sequence touch no boundary and are buildable today.
Steps 5 onward change the platform's safety properties and are deliberately
last. Capital movement (step 10) is a separate boundary from order submission
and stays closed under ADR 0021 regardless: a platform can trade live while
capital movement is shut, and probably should, first.

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

### F3 — in-tree cryptography, and the slice that widened its blast radius

**Status: standing matter for the owner. Recorded, not acted on.**

`qip-core/src/hash.rs` carries hand-written SHA-256 and HMAC-SHA-256. It
predates this programme, and ADR 0009 forbids in-tree cryptography — the
primitive has lived in the gap between that rule and the two-dependency rule
that leaves no room for a vetted crypto crate without an ADR.

The payload slice **consciously extended its blast radius**. What that MAC
guarded before was the capital-envelope channel; it now also guards the
centre-to-region command channel — every policy payload, and the halt itself.
Security review of the slice found the *usage* sound (constant-time compare,
one trust root, injective signing strings after the H1 hardening), which is a
statement about how the primitive is called and not about the primitive.

The decision this queues is the owner's, twice over: admitting a vetted
cryptographic dependency is an ADR-level change to ADR 0002/0009, and the
blueprint's own §46.2 ambitions (real signatures, post-quantum for corridor
material) require one anyway. Until that ADR exists, nothing further should be
built onto the in-tree primitive without restating this note in the diff.

### F5 — §27.2's venue consolidation · NOT-APPLICABLE, and why that is not "done"

The blueprint asks that intents for one instrument reachable at several venues
consolidate before routing, so the platform picks one venue with the whole size
rather than splitting it across venues by accident of which strategy fired.

**This repository cannot express the situation.** `Cell::venue_for` resolves a
venue from the cell's own configured list *before* an intent is constructed, by
finding the first venue whose book is reachable. Every intent in a cell
therefore already names a venue that the cell chose, not one a strategy asked
for, and two intents on one instrument in one cell always name the same venue.
Netting on `(instrument, venue, representation)` consolidates them for the same
reason §27.2 wants consolidation, but it does so by construction rather than by
a consolidation step.

Scored NOT-APPLICABLE rather than ALIGNED, because the row becomes live the
moment either of two things changes: a strategy gains a venue-agnostic intent,
or `venue_for` starts returning a set instead of the first reachable venue.
Recording it as satisfied would hide that trigger. There is no test, because a
test would assert a property of a situation that cannot arise — which is the
control-that-cannot-fire pattern this document exists to avoid.

### F6 — reservation is central-only where the blueprint puts it per-region

**Status: CONTRADICTS. Recorded, not acted on.**

Found by the placement audit of the node's composition roots.
`qip-capital-fabric`'s `ReservationLedger` — the thing that holds capital a
passing check approved, so a second concurrent proposal is refused against a
balance the first already spent — is composed in the kernel and exists once,
centrally. The blueprint's §26/§33 shape is a **per-region reservation table**
consulted at the cell, because that is the only placement at which a
disconnected cell can still refuse its own second proposal.

The consequence is precise and worth stating rather than generalising: a cell
that has lost contact with the centre spends against its capital envelope,
which bounds it correctly, but nothing at the edge reserves within that
envelope. Two strategies in one cell are now netted, which removes the case
that motivated this note most sharply; two *cells* under one grant are not, and
the centre is the only thing that can see both.

Not fixed here, and deliberately: moving reservation to the edge is a
placement change to the capital path, it interacts with envelope accounting,
and it needs its own slice and its own review. The row is recorded so the next
reader does not infer from a working central ledger that the property holds
regionally.
