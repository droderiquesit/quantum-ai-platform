# Algorik Master Blueprint v10.1-4 — traceability against this repository

**Scored against the working tree on branch
`claude/algorik-architecture-refactor-pmp0zy`, from commit `d8b3597`; rows
re-scored at `296e187`, at `584c96b` or at `e04815e` say so inline, and every `path:line`
in a re-scored row is at the commit the row names.** A row that changed keeps what it said before,
marked as history, because a scorecard that quietly overwrites its own
findings cannot be audited against.

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
| End of Phase 2 | Does a family survive holdout with honest significance after cumulative trial correction? | **NOT PASSED** | The machinery exists — `qip-simulation-engine/src/validation.rs`, `qip-lifecycle/src/gates.rs`, `qip-lifecycle/src/evidence.rs` — and since `9332bcb` the correction is cumulative: `qip_lifecycle::trials::TrialBook` keeps one hash-chained journal per family and `lifetime_trial_count_known` (`gates.rs:242`) refuses a promotion whose count is unknown (`lifecycle.rs::a_second_run_is_corrected_against_the_first_runs_trials_as_well`). Re-scored at `584c96b`: the kernel's factory enrols every candidate (`94dd7e2`, `central/factory.rs:281-299`), every root now opens the book durable on its own store through `Platform::open_trial_book` (`aa66c5d`; `qip-kernel/tests/trial_book.rs::a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward`) and the book budgets five hundred trials per family per calendar quarter (`e31aae4`; `qip-lifecycle/tests/lifecycle.rs::the_five_hundredth_trial_of_a_quarter_charges_and_the_five_hundred_and_first_is_refused`), so §20.1's accounting is ALIGNED in code; the earlier per-process count this row named is closed. One thing still stops the gate: it is an empirical question about real market data, and every deployment's data is synthetic or replayed. A family surviving a holdout of data the platform generated is not the gate |
| End of Phase 3 | Does it survive contact with a live venue, inside its holdout band? | **CANNOT PASS** | Structurally unreachable: paper trading is absolute. See ADR 0021. The band it asks about now exists — `HoldoutBand::from_deflated` at the holdout gate (`gates.rs:260`), carried on the admission and two-sided at the demotion monitor (`d0558b4`) — so when the boundary is opened by ADR 0023's sequence there is something to be inside of; there was not when this row was written |
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
| §2.2 | Risk reads aggregates, never strategy lists | Risk state is aggregate counters; since `b9e9e7d` the rule is structural — `qip_risk::aggregate::RiskAggregates` moves running counters on each fill and `LimitSet::check_aggregates` reads book-level figures through the `AggregateFigures` trait, so a test can wrap the book in a probe that counts every figure consulted | ALIGNED — re-scored at `296e187`; was PARTIAL | `libs/qip-risk/src/aggregate.rs`; `qip-risk/tests/aggregate.rs::the_aggregate_check_reads_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve`; the tail figures a limit reads derive from the limit's own confidence through `RiskState::with_tail_risk` (`d94b156`, `990032a`; `platform.rs:4270`) and every `LimitKind` arm has a fixture it admits and one it refuses (`160c4e8`, `qip-risk/tests/limit_fixtures.rs`); at `88eb1e2`, five commits after this re-score, the kernel feeds every desk fill into the aggregate and reads limits from it, so the property is held in production and not only by the lib's own test. Re-scored at `584c96b`: the aggregate is now fed on the axes the limits read — sector, country, asset-class and venue buckets projected from the universe at assembly (`588335a`; `platform.rs:1144-1150`, `exposure_axes_for` at `:4760`, `aggregate_fill` at `:4723`, the pre-trade projection at `:4486-4492`; `qip-kernel/tests/risk_aggregates.rs::a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused`, `::an_order_that_keeps_its_sector_bucket_under_the_cap_is_admitted`) and every venue fill a cell reports, under the cell's id as the strategy axis (`98bc687`; `central/plane.rs:1081`, `charge_cell_fills` at `platform.rs:1720`; `::a_cells_fills_are_charged_into_the_aggregate_and_the_next_desk_order_is_refused_on_leverage`). Two limits that could never fire now can, and the first thing they did was refuse the first order into an empty book — share-of-gross concentration is 100% on any first position — so the two kernel fixtures drop `MaxConcentration` and say why (`tests/capital.rs:57-69`), and the semantics are the risk desk's (plan D13). Re-scored at `e04815e`: the three central roots assemble from the committed catalogue rather than `Universe::new()` — `data/datasets/universe.json`, read from `QIP_UNIVERSE_PATH` and refused unset (`8224509`; `qip-api/src/main.rs:346`, `qip-fastbrain/src/main.rs:282`, `qip-deepbrain/src/main.rs:367`; `qip-financial/src/catalogue.rs::load` at `:154`, the manifest recorded under its hash by `record_manifest` at `:239`), mounted at `/etc/qip/universe.json` on all three Cloud Run workloads (`e40335d`; three `file_name = "universe.json"` blocks in `catalogue.tf`) — so a deployed process would feed every bucket, and the first thing a fed bucket does is what D13 predicted: `qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_refused_by_the_default_concentration_cap_until_adr_0027_is_decided` (`:491`) pins the refusal until ADR 0027, proposed at `360cfd8`, is decided. The deep brain's replay branch keeps the empty universe on purpose (`qip-deepbrain/src/main.rs:230`), because a tape carries bars and not listings | D13, now framed by ADR 0027 | — | 10 | `aggregate.rs` |
| §2.2 | Feasibility precedes profitability | At the cell, yes: `Cell::work` judges every intent in place through `admit_feasible` (`cell.rs:860`) before `net` (`:869`) — minimum quantity, minimum notional, lot, tick, depth at the touch, fee floor, gas floor and a malformed policy constraint, eight gate literals in `qip-edge/src/feasibility.rs:76-83`, each a refusal and never a rounding; a cycle short a leg is vetoed whole | PARTIAL — re-scored at `296e187`; was MISSING-CURRENT | `95a4932`; `qip-edge/tests/feasibility.rs::an_off_lot_intent_is_refused_before_netting_and_never_rides_a_feasible_strategys_order`, `::a_sell_is_judged_against_the_bid_side_and_a_buy_against_the_ask`; `arbitrage.rs::an_infeasible_leg_vetoes_the_whole_cycle_and_no_leg_goes_out` | Not ALIGNED for two reasons this row must keep visible: the central pre-trade path in `qip-execution-engine` has no feasibility step, and — as this row read until `e04815e` — `qip-edge-node` called `Cell::work` on no path. Re-scored at `e04815e`: the node's loop calls it (`6340610`; `run_pass` at `qip-edge-node/src/pass.rs:84`, `cell.work` at `:118`, called from `main.rs:586`) when `QIP_VENUE_FEED=simulated`, the one value `FeedChoice::read` accepts, every other refused at start naming ADR 0003 (`feed.rs:79-82`, `:118-131`); TESTED by `qip-edge-node/tests/pass.rs::a_node_with_the_simulated_feed_runs_a_pass_and_the_pass_time_series_move` and `::a_venue_feed_other_than_the_simulator_is_refused_at_start_naming_adr_0003`, MEASURED nowhere, because `execution_nodes = {}` in every environment | Medium — the control exists, fires in tests, and is in the deployed binary's loop; no node is deployed to walk it | 3 | Passing-and-vetoing fixtures, present |
| §2.2 | Strategies are compiled, not interpreted | `qip-strategy` evaluates; no shared compiled plan with CSE | PARTIAL | `edge/qip-strategy/` | Backlog | Low at current strategy counts | 10 | Netting-ratio measurement |
| §2.2 | After-tax return is the only return | No tax engine, no lot selection | MISSING-CURRENT | No `taxlot`/`tax_engine` in the tree | Backlog | Low while paper-only | 3 | — |

## The seven planes (§1.2, §5, §4.2)

Planes are bounded investment responsibilities and are deliberately **not**
the same axis as the seven layers below. Do not rename one as the other, and
do not fold Cognition into Intelligence — §4.1 argues the split and the
argument still holds here.

| Plane | Blueprint responsibility | Implementation | Status | Evidence | Minimal action | Phase |
|---|---|---|---|---|---|---|
| 1 Ingestion | Observe world + prices; resolve to entities; pass-through, not accumulation | `qip-market-ingestion`, `qip-normalization`, `qip-entity-resolution`, `qip-data-finder` (licensing posture before use; the registered outcome is sealed behind the legality assessment since `47e9b81`, so a catalogue entry cannot skip the gate) | PARTIAL | `platform.rs` absorbs 11 record kinds; `Feed::Live` exists at `apps/qip-fastbrain/src/feed.rs:61` and `::live` at `:108` | Prove one live source end to end; no deep-web tier exists | 1, 5 |
| 2 Cognition | World model, causal graph, episodic memory, belief, counterfactual, self-model, hypotheses | `qip-world-model`, `qip-agents/src/memory.rs`, `qip-twin`, `qip-reasoning-engine/src/hypothesis.rs` | PARTIAL | World model and hypotheses present; counterfactuals in `qip-twin`, and since `b9e2242` every refused order is priced through `Platform::evaluate_alternatives` from LEARN (`platform.rs:5028`); since `04738ee` the belief calibration is a Brier score on `qip_belief_brier_score` (`:1708`) rather than a function nothing called | **No self-model exists** (`grep -rln "SelfModel"` empty); no belief *stage* in the cycle — grading theses after the fact is not sizing by belief before the trade | 7, 8, 9 |
| 3 Valuation | Price what has no price: term structure, credit, vol surface, illiquid, cashflow, corporate actions | Corporate actions absorbed in `platform.rs`; `qip-financial/src/extensions.rs` carries illiquid-adjacent types | MISSING-CURRENT | No term-structure, credit or vol-surface engine | Backlog — Phase 14 | 14 |
| 4 Intelligence | Train, generate and statistically gate strategies, set risk and corridor policy | `qip-training`, `qip-lifecycle`, `qip-evolution`, `qip-simulation-engine/src/validation.rs` | PARTIAL | Statistical gate, champion/challenger and promotion exist; the deflated Sharpe is corrected against the family's lifetime trials (`9332bcb`) with the simulation engine's one Sharpe arithmetic (`436e1fa`), the holdout band is an output of validation and what leaves it is demoted (`d0558b4`), and the kernel's factory enrols every candidate (`94dd7e2`) | Corridor policy has no owner because corridors do not exist. Re-scored at `584c96b`: the trial book is durable in every root (`aa66c5d`) and budgeted per calendar quarter (`e31aae4`), so the per-process count this cell named is closed | 2, 10 |
| 5 Optimisation | Allocation across families/regimes/horizons; quantum + classical; policy only | `qip-optimization-engine`, `qip-quantum` | PARTIAL | Routing gate and classical baseline present; authority boundary now structural | Family clustering and multi-horizon reconciliation absent | 15 |
| 6 Execution | Regional nodes, shipped policy, microseconds, local decisions | `qip-edge` (structurally paper-only), `qip-edge-node`, `qip-orderbook`, `qip-routing`, `qip-arbitrage` (constructed by the cell since `71f9465`) | PARTIAL | No `Cell` constructor takes a non-paper ceiling; policy ships down signed and the cell verifies, applies, narrows and halts on it; intents are judged feasible, netted and crossed, cycles scanned from the cell's own books | Re-scored at `296e187`. Was: "runs as a pod, not the blueprint's bare C3; no intent netting, no inventory reservation". Now: the pod is gone with the cluster's Terraform (`808ca32`) and the node is a Compute Engine module with `execution_nodes = {}` in every environment; netting exists; per-region reservation still does not (F6); and `qip-edge-node` drives no `Cell::work` pass, so everything the cell proves in a pass is reached by no deployed process. Re-scored at `584c96b`: the node installs a desk from the payload's own whitelist once a grant for the desk's strategy and a whitelist carrying `CycleWhitelist::conversions` arrive (`ArbitrageInstaller`, `qip-edge-node/src/arbitrage.rs:249`; `qip-edge-node/tests/arbitrage.rs::the_node_installs_a_desk_from_the_payloads_whitelist_once_capital_for_it_has_arrived`, `::a_whitelist_naming_a_venue_outside_the_configured_list_is_refused_and_installs_nothing`, `::a_degraded_cell_and_an_empty_whitelist_install_no_desk`, `::a_grant_for_another_strategy_is_refused_by_the_installer_rather_than_held`), and nothing in `qip-api` or `qip-kernel` produces the field, so the installer waits — §30 is PARTIAL; and a second halt wire that shares nothing with the mesh polls a file on the node (`ff86473`) — §46.2 is PARTIAL, proven at the cell and the node and measured nowhere. Re-scored at `e04815e`: **the node runs passes** — `run_pass` (`qip-edge-node/src/pass.rs:84`) calls `Cell::work` (`:118`) from the loop (`main.rs:586`) over the in-process simulated venue when `QIP_VENUE_FEED=simulated`, any other value refused at start naming ADR 0003 (`6340610`; `feed.rs:118-131`), and the execution node's template writes that line (`startup.sh.tftpl:174`) — so §41.4's node is TESTED (`qip-edge-node/tests/pass.rs`, six tests, among them `::a_resting_order_the_venue_fills_on_a_later_pass_is_confirmed_and_the_node_keeps_trading`, `b8d18d3`) and MEASURED nowhere. **A fill is a venue fact** (§27, `cb79b46`): what `Placer::execution_reports` returns (`cell.rs:3594`) is what `Cell::confirm_execution_reports` (`:2073`) books as `Decision::Filled` (`:2160`) under `qip_edge_fills_confirmed_total` (`telemetry.rs:46`); an accepted order is an open order until then — `qip-edge/tests/fills.rs::an_order_the_venue_accepted_is_not_a_fill_until_the_order_entry_channel_reports_one`. **Pricing is stated, never guessed** (`383d4e7`): `PricingPolicy::Marketable` or `::RestAtMid { time_to_live }` (`cell.rs:347-365`); a strategy that stated none is refused under `pricing` before anything is placed (`:1442-1450`); a rested order is withdrawn through `Placer::cancel` (`:3609`) when its time to live elapses and counted on `qip_edge_orders_expired_total` (`telemetry.rs:50`) — `qip-edge/tests/pricing.rs::an_intent_with_no_stated_pricing_is_refused_and_nothing_reaches_the_venue`, `::a_resting_order_rests_at_the_mid_and_is_withdrawn_when_its_time_to_live_elapses`; the two acceptance fixtures deploy with a policy since `e04815e`. **The whitelist is produced and shipped** (§30): `CentralPlane::cycle_whitelist_for` (`central/plane.rs:612`) sizes slot 8 from `CentralConfig::arbitrage` and the desk's live grant (`5396679`; `central/whitelist.rs::ArbitragePolicy::whitelist_for` at `:267`; `Platform::issue_cycle_whitelist` at `platform.rs:1572`), and `qip-api`'s `pending_policy` issues it per cell (`91d20f5`; `mesh.rs:663`) from a policy read at `QIP_ARBITRAGE_POLICY_PATH` (`main.rs:374`; registered as read and unset on Cloud Run at `73a1694`; `docs/operations/arbitrage-policy.md`) — `qip-api/tests/mesh.rs::a_cycle_ships_the_desk_a_live_grant_funds_as_a_whitelist_the_cell_verifies`, `::without_a_policy_the_whitelist_ships_empty_and_the_cycle_says_the_policy_is_unset`. The installer the `584c96b` re-score left waiting has its producer; the desk is installed when a node runs with a policy set, and none is deployed. **The third halt direction is tested** (§46.2, `6a515bb`): `qip-edge/tests/telemetry.rs::clearing_the_kill_switch_while_the_polled_flag_is_present_leaves_the_cell_halted` (`:620`). **The wire gap the `e04815e` re-score left open is closed** (`5290bb9`, in six commits from `9e45dc0`): the uplink's `orders` were the pass's placements and `CentralPlane::settle` billed every one as a fill, so a resting order the venue never filled was a position in the centre's books. Now `CellStateDelta` carries `fills: Vec<FillRecord>` beside `orders` (`qip-edge/src/mesh.rs:214`, built from `WorkReport::fills` at `cell.rs:3253`; declared once in `qip-contracts/src/wire.rs:93`, `CELL_DELTA_SCHEMA_VERSION = 4` at `:148`, `MAX_FILLS_PER_DELTA` at `:120`), the centre decodes them as their own half of the interval and reads a v3 delta as having confirmed nothing (`qip-mesh/src/delta.rs:478`), and `settle` (`central/plane.rs:1161`) registers orders as sent and bills from fills only — the Plane 7 row has the centre's half. Round trip: `qip-edge/tests/mesh.rs::a_state_delta_a_cell_produced_arrives_at_the_centre_unchanged` (`:420`, a fill of one against an order of three, so a wire that shipped the order as the fill would arrive as three); `acceptance.rs::the_centre_decodes_a_contributor_vector_out_of_bytes_the_edge_crate_produced` (`:648`, the edge serialiser against the centre's decoder). "Bill what ran, not what was planned" holds on the wire; TESTED, and measured nowhere because no node is deployed | 3, 16 |
| 7 Ledger, wallet, treasury | Authoritative money state per user and per strategy; reconcile every holding; move capital in signed corridors | `qip-capital`, `qip-capital-fabric` (`transfer.rs`, `settlement.rs`), hash-chained event log; since `7ef6063` the centre's per-cell, per-strategy, per-instrument books, settled from each cell's report, and since `5290bb9` settled from the report's venue-confirmed `fills` and never from its `orders` (`CentralPlane::settle`, `central/plane.rs:1161`) | PARTIAL | Capital allocation, envelopes and exposure exist; a fill is booked as the cell's own shares, refused if they do not sum, and a cross moves both books at the recorded mid, closing to the last unit or counting `qip_central_attribution_failures_total` (`qip-kernel/tests/attribution.rs`). What was sent is registered, not booked: `SentOrders` holds 4,096 per cell (`:1526`), counted under `qip_central_orders_sent_total` (`:1187`), and a fill for an order the centre never saw sent, or beyond its unfilled remainder, is a `BreakOrigin::UnsentFill` break (`:1214`) that halts the cell — `tests/attribution.rs::a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing` (`:441`), `::a_fill_on_an_order_the_centre_never_saw_sent_halts_the_cell_and_books_nothing` (`:499`), `::a_fill_beyond_the_quantity_sent_is_the_same_break` (`:567`), `::a_fill_whose_shares_do_not_sum_to_it_is_refused_rather_than_booked_short` (`:635`); `tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate` (`:506`), `::the_same_order_filled_in_the_next_report_charges_exactly_the_fill` (`:562`) | **No wallet, no corridor, no transfer gate, no destination registry, no custody engine** — `grep` for each returns nothing. Phase 12, and bounded by ADR 0021. The books are per strategy, not per user | 12 |

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
  absence of an edge to `qip-capital`. *Degradation:* two levels — a stale
  book supplies nothing (`edge/qip-edge/src/seam.rs`), and the cell's
  `narrowing()` reads the payload's ingestion slot and pauses the strategy
  classes §6.2 names. This bullet used to say the capability row was typed
  and unwired; that stopped being true with the payload slice. *Tests:* `absorption.rs`,
  `sense.rs`, `rest_feed.rs`. *Separate service justified?* No — one cadence,
  one owner, no isolation argument.

- **[PLANE 2/7 — Cognition]** *Ownership:* split across `qip-world-model`
  (including a real causal graph — `world.rs:41`, `:192`, `causal.rs:234`),
  `qip-agents/src/memory.rs` (episodic), `qip-twin` (counterfactual),
  `qip-reasoning-engine` (hypotheses, `bayes.rs` for Bayesian updating).
  *Placement:* global. *I/O:* events → theses. *State:* bitemporal.
  *Authority:* **none — informs only**, matching §39 layer 3.
  *Degradation:* undefined. *Tests:* `understanding.rs`, `reasoning.rs`.
  *Gaps:* **no belief stage in the cycle** and **no self-model at all**.
  Confidence-weighted sizing per §11.2 is not the mechanism here. What did
  arrive, at `04738ee`, is the belief *calibration*: `stage_learn` settles each
  resolved claim against the platform's own series and grades it through
  `Platform::learn_from` (`platform.rs:1680`), a Brier score over a bounded
  window on `qip_belief_brier_score` — `qip-kernel/tests/learning.rs::a_cycle_that_resolves_a_thesis_grades_it_and_moves_the_calibration_series`.
  The grep this bullet once cited (one doc-comment line, no code) is no longer
  the evidence; the absence of a stage before the trade is. *Separate service justified?* Not yet; the split across
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
  `training.rs`. *Trial accounting:* `TrialBook` (`qip-lifecycle/src/trials.rs`)
  keeps one hash-chained journal per family, replayed from the store with the
  chain verified; the holdout gate refuses an unknown lifetime count and the
  band it produces is what the demotion monitor judges against (`9332bcb`,
  `d0558b4`, `94dd7e2`). Re-scored at `584c96b`: every composition root opens the book on its own
  store through `Platform::open_trial_book` (`platform.rs:1624`;
  `qip-api/src/main.rs:106`, `qip-fastbrain/src/main.rs:164`,
  `qip-deepbrain/src/main.rs:178`; `aa66c5d`), a journal that does not verify
  refuses to start, and each family is budgeted five hundred trials per UTC
  calendar quarter under the same hash as the lifetime (`trials.rs:69`,
  `:674`; `e31aae4`) —
  `qip-kernel/tests/trial_book.rs::a_book_reopened_from_the_same_store_carries_the_familys_lifetime_count_forward`,
  `::a_journal_whose_count_was_lowered_by_hand_refuses_to_open_and_nothing_is_attached`,
  `qip-lifecycle/tests/lifecycle.rs::the_five_hundredth_trial_of_a_quarter_charges_and_the_five_hundred_and_first_is_refused`,
  `::a_new_quarter_resets_the_running_count_and_not_the_lifetime`,
  `::the_quarterly_count_replays_from_the_store_and_a_lowered_one_is_refused`.
  §20.1's trial accounting is ALIGNED in code; what it has never counted is a
  real run. *Gap:* corridor policy
  has no owner because corridors do not exist (Phase 12). *Separate service justified?* Cadence differs from the hot
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
  `qip-execution-engine`. *Placement:* regional — and re-scored at `296e187`: the
  cluster's Terraform, the edge-cell module, the chart and the raw manifests
  are gone (`808ca32`, `67b3e92`, `7d79161`); `modules/execution-node` is a
  Compute Engine machine per region, wired from the root module, and every
  environment declares `execution_nodes = {}` because a node needs a venue
  nobody has recorded. This bullet used to say three cells in stage tfvars as
  Kubernetes pods; nothing now describes a pod, and nothing has been applied. *I/O:* signed capital envelope down, `CellStateDelta` up.
  *State:* local books, inventory, journal. *Authority:* veto-only gates plus
  placement inside a granted envelope (§39 layers 9–12); a cell cannot mint its
  own capital or promote its own strategy. *Degradation:* stale book supplies
  nothing; venue health in `qip-routing/src/health.rs`; the cell self-halts when
  its fills disagree with the venue drop-copy (`cell.rs:774-786`). *Tests:*
  `e2e.rs`, `resilience.rs`, `chaos.rs`, `apps/qip-edge-node/tests/mesh.rs`.
  *Gaps:* no inventory reservation at the region (F6). Four earlier gaps are
  now closed and are recorded here rather than deleted, because the fix is what
  the row is evidence of: **the feasibility gate exists** (`95a4932`,
  `admit_feasible` at `cell.rs:860` ahead of `net` at `:869`, eight refusal
  literals in `feasibility.rs:76-83`) and **the arbitrage desk scans the
  cell's own books** and sends each cycle's legs by the one order path
  (`71f9465`, `scan_cycles` at `:851`; a cycle that breaks between legs trips
  the switch, `place_cycle` at `:1628`) — both proven in `qip-edge/tests/` and,
  until `e04815e`, reached by no deployed process because `qip-edge-node`
  called `Cell::work` on no path — since `6340610` its loop calls it over the
  simulated venue, and since `5396679`/`91d20f5` the whitelist its installer
  waits for is produced (see the table row's `e04815e` re-score; no node is
  deployed); the halt reaches a cell by two wires — the
  signed `HaltCommand` on the policy downlink (`VerifiedHalt`) and, since
  `ff86473`, a flag polled on the node's own filesystem (`HaltFlag::poll`,
  `qip-edge-node/src/halt.rs:100`; `Cell::apply_polled_halt`, `cell.rs:558`;
  `qip-edge-node/tests/halt.rs`, five tests), neither of which releases the
  other; **intent netting exists** — `Cell::work` builds one `Intent` per
  firing strategy, nets them on instrument, venue and representation, and places
  what survives; and **internal crossing exists** (§27.1) — the matched part of
  an offsetting net is booked between its own contributors at the book's mid at
  the netting instant, capped at forty percent of gross intent and refused whole
  above it, with a hash-chained journal entry naming both sides and the price.
  Since `153e429` the cap is measured over `CellConfig::crossing_interval`
  (`cell.rs:70`), `Passes(n)` or `Span(d)`, one window per net key; the default
  `None` (`:107`) is the per-net arithmetic, so a full cancellation still never
  crosses until the owner sets an interval (D3) —
  `qip-edge/tests/crossing.rs::over_a_three_pass_interval_a_repeated_full_cancellation_crosses_on_the_second_pass_at_the_mid`,
  `::with_no_interval_the_same_two_passes_never_cross`. §27.1 is PARTIAL:
  code present, interval unchosen, and no root sets one.
  Contributor vectors reach the centre on the uplink at delta schema version 2
  (`libs/qip-contracts/src/intent.rs`, `libs/qip-contracts/src/wire.rs`,
  `edge/qip-edge/src/cell.rs`, `edge/qip-edge/src/journal.rs`,
  `services/qip-mesh/src/delta.rs`, `apps/qip-edge-node/tests/gateway.rs`).

- **[PLANE 7/7 — Ledger, wallet and treasury]** *Ownership:* `qip-capital`
  (allocation, envelope, exposure), `qip-capital-fabric` (internal placement),
  and the hash-chained event log. *Placement:* global. *I/O:* approved requests
  → signed grants. *State:* **there is no `Ledger` type** — money state is
  capital allocation plus the log, which is a different shape from §43.3's
  per-user, per-strategy authoritative ledger. Since `7ef6063` the centre
  also keeps a per-cell, per-strategy, per-instrument book settled from each
  cell's report before the halt step (`central/plane.rs:1018` → `:1161`), the
  §43.4 chain fill → contributor vector → strategy pro rata, and the API sink
  carries the interval's orders, fills and crosses onto that report (`7d79161`,
  `d59505d`; `qip-api/src/mesh.rs:1209`;
  `qip-api/tests/mesh.rs::the_fills_a_cell_reports_reach_the_centres_strategy_books`,
  `::an_order_a_cell_reports_sent_and_unfilled_reaches_no_book_and_charges_nothing`).
  Re-scored at `e04815e`, that report's `orders` carried placements and
  `settle` booked each as a fill — the wire gap named in the Plane 6 row.
  Closed at `5290bb9`: the chain starts from the delta's `fills`, a
  venue-confirmed fill with the cell's own shares (`wire.rs:93`), and an
  order is registered as sent and books nothing. *Authority:* records (§39 layer
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
- **[LAYER 3/7 — Application and API]** *Current:* `qip-api` composes reads and holds no independent financial state. *Keep.* *Change:* none. *Remove:* none. *Defer:* the typed-intent surface (§40.9). An `Intent` type now exists (`libs/qip-contracts/src/intent.rs`) but it is the *execution* vocabulary, produced and consumed inside one cell; application APIs still raise no intents, they read. The gap is the API surface, not the type. *Verification:* `documentation.rs::every_documented_endpoint_exists`; and since `827a40e` the boundary itself is executable — `api_boundary.rs::the_application_layer_depends_on_no_execution_venue_capital_or_edge_crate` from `cargo metadata`, `::the_api_uses_only_the_centre_half_of_the_mesh_and_none_of_its_service_clients` from the sources, with the centre's two signatures pinned by exact expression so a third `.signed(` fails until reviewed.
- **[LAYER 4/7 — Domain contracts and control fabric]** *Current:* `qip-contracts` sits at the bottom of everything sharing it; `qip-transport`/`qip-mesh` carry the fabric. *Keep.* *Change:* none this pass. *Remove:* none. *Defer:* re-scored at `296e187` — this row used to defer the **signed twelve-item payload (§41.5)** as the largest non-future structural gap. The payload landed (PR #3, `61f9392`, `0c91cfa`): the centre ships it signed, the cell verifies, applies it atomically, narrows on stale slots and halts on the signed command; the truth pass's flow 3 is PARTIAL rather than missing. What is still deferred is the *producers* — two of twelve slots have one, and slot 11 (feasibility constraints) has its first consumer at the cell (`95a4932`) and no producer at the centre. Re-scored at `e04815e`: three of twelve — slot 8, the cycle whitelist, is produced by the kernel from an operator policy and the desk's live grant and shipped by the API (`5396679`, `91d20f5`; `central/plane.rs:612`, `qip-api/src/mesh.rs:663`), empty with its reason when `QIP_ARBITRAGE_POLICY_PATH` is unset. The second, independent halt wire this row deferred landed at `ff86473` (re-scored at `584c96b`): a flag polled on the execution node's own filesystem, sharing nothing with `qip-transport`, TESTED at the cell and the node (`qip-edge-node/tests/halt.rs`; `qip-edge/tests/telemetry.rs::a_polled_halt_moves_its_own_gauge_refuses_the_pass_under_its_own_gate_and_no_payload_releases_it`) and MEASURED nowhere, because no node runs. *Verification:* `spine.rs`, `qip-api/tests/mesh.rs`, `qip-contracts/tests/contracts.rs`; `manifest_wiring.rs`, retargeted at the catalogue at `81dd1cd`.
- **[LAYER 5/7 — Data and state]** *Current:* bitemporal records; bounded retention; event log hash-chained; `qip-data-finder` evaluates licensing before use. *Keep.* *Change:* none. *Remove:* none. *Defer:* BigQuery derived series and content-hash manifests for external history. *Verification:* `absorption.rs`, `resilience.rs`, `truth_loop.rs`.
- **[LAYER 6/7 — Cloud and network]** *Current, re-scored at `296e187`:* the root module wires the blueprint runtime — `catalogue.tf` instantiates `modules/cloudrun` once per deployable in its §46.1 zone with the egress sidecar where a workload carries one, `execution_node` per region from `execution_nodes` (empty in every environment), `trust_zones` binding workloads to zones default-deny both ways, and `egress_proxy` publishing the bootstrap (`808ca32`, `c924191`; seventeen `module` blocks in `main.tf`, count `grep -c '^module "' infrastructure/terraform/main.tf`). The GKE cluster, edge-cell and console-ingress modules, the Helm chart, the raw manifests and the Argo CD stack are deleted (`808ca32`, `67b3e92`, `7d79161`); `deploy.yml` moves each Cloud Run service by digest and fails unless the serving revision carries the attested image (`b85684f`). *What this row said before:* GKE + Argo CD + Kargo + Helm + KEDA as a transitional runtime, to be removed only at ADR 0020 step 5 with recorded approval. *What authorised the change:* the owner's instruction — create the new infrastructure while devouring the old — taken by `808ca32` as approval **for the code and not for an apply**. *Keep:* the wired modules. *Change:* nothing further without a plan. *Remove:* nothing further. *Defer:* the apply, and every observation that depends on one. Re-scored at `e04815e` for §41.4: the node the module boots is configured to run passes — `startup.sh.tftpl:174` writes `QIP_VENUE_FEED=simulated` and the binary's loop runs `Cell::work` on it (`6340610`) — TESTED in `qip-edge-node/tests/pass.rs` and MEASURED nowhere, since `execution_nodes = {}` everywhere. *Verification:* `terraform fmt -check`, `validate` and a plan **NOT RUN — no `terraform` binary exists in this environment**, so every precondition in the new modules is asserted and unexercised; `infrastructure.rs` passed 67 at `808ca32` and `b85684f`, could not pass between `7d79161` and `81dd1cd` because it read the deleted manifests, and passed 59 once `81dd1cd` retargeted it at the runtime that exists — a text scanner's verdict, never a provider's. This layer is CONFIGURED at best and not MEASURED: nothing was applied, and no process has ever been shown to run on either runtime.
- **[LAYER 7/7 — Security, observability, delivery, reliability]** *Current:* three paper layers intact and re-verified by path this pass; two kill-switch wires at the cell since `ff86473` (§46.2), TESTED at the cell and the node in all three release directions since `6a515bb`, the `edge_halted` alert's text naming the polled source since `cd16f79` (`modules/observability/main.tf:200`), and not MEASURED; WIF only; CSI-projected secrets; Binary Authorization; telemetry emitted at the seams. *Keep.* *Change:* none. *Remove:* none. *Defer:* OpenTelemetry spans with cross-plane correlation (§47) — the current surface is a Prometheus-style metric registry, not spans. This row used to say policy-freshness, belief-calibration and reconciliation signals had nothing to emit; at `296e187` all three do — `qip_edge_policy_sequence` and `qip_edge_capability_freshness` at the cell, `qip_belief_brier_score` (`04738ee`) and `qip_central_reconciliation_breaks_total` (`de5d042`) at the centre, the kernel's fourteen newer series spelled in `qip-observability/src/metrics.rs` (`296e187`). What none of them has is a collector that has ingested one or an alert that names one. *Verification:* `security.rs`, `compliance_proof.rs`, `api_boundary.rs`; `egress.rs` and `infrastructure.rs`, both retargeted at `81dd1cd` (14 and 59 passed, per its message) after five commits in which they read manifest paths `7d79161` had deleted.

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

*Re-scored at `296e187`.* The owner's instruction — create the new
infrastructure while devouring the old — was taken by `808ca32` as approval
for the **code**: the blueprint runtime is wired into the root module and the
cluster's Terraform, chart, manifests and Argo CD stack are deleted from the
tree (`808ca32`, `67b3e92`, `7d79161`). What it was not taken as is approval
for an apply, and none has happened; no `terraform` binary exists here, so the
new modules have never been through `fmt`, `validate` or a plan. ADR 0020's
step 1 — evidence that any GKE workload ever ran — was never gathered and is
now moot, since there is no cluster in the tree to gather it from. ADR 0024,
which `b85684f` cites for the reconciler's retirement, was not in `docs/adr/`
at `296e187`; it landed five commits on, at `2b7e502`, quoting the
instruction as the authorisation for the code and stating that nothing was
applied.

### C3 — experience layer · direction settled, execution NOT authorised

Leptos over shared types (§40) is the target. `frontend/portal` and
`frontend/landing` are transitional and are **maintained**, not abandoned —
they are the only customer-facing surface there is. ADR 0001's browser
exception is superseded in direction.

No migration now. The sequencing stays in the backlog: identify contracts and
Playwright coverage, define the replacement boundary, and only then consider a
vertical slice — and only if it adds no dependency without an ADR.

### C4 — a stale factual claim in an instruction file · CLOSED, and reopened narrower

`.claude/rules/domains/observability.md` stated that nothing writes to
`Telemetry`. It was corrected at `232bc16` and now says both planes emit and
names the edge series; the row above is history.

What the rule file says at `296e187` that the code no longer does: that
`Platform::learn_from` "is called by nothing in the tree" and
`Platform::evaluate_alternatives` "is called only by `qip-kernel`'s own
tests". Both gained their production caller in LEARN (`04738ee`,
`platform.rs:4052`; `b9e2242`, `:5028`). Same rule as before: `.claude/rules/`
is the owner's, and this scorecard flags rather than edits it.

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

*Re-scored at `584c96b` — the aggregate half of §26/§33.* Reservation is
unchanged and this finding stands. What changed is what the central aggregate
is fed: `588335a` projects every instrument's sector, country, asset-class and
venue buckets from the universe at assembly and feeds them at both seams
(`platform.rs:1144-1150`, `:4486-4492`, `:4723`), and `98bc687` charges every
venue fill a cell reports under the cell's id (`central/plane.rs:1081`,
`platform.rs:1720`), so a leverage, gross or bucket limit can now fire on the
exposure seven cells carry —
`qip-kernel/tests/risk_aggregates.rs::a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused`,
`::a_cells_fills_are_charged_into_the_aggregate_and_the_next_desk_order_is_refused_on_leverage`.
Two things are open and said rather than smoothed: the share-of-gross
concentration caps refuse the first order into an empty book, which is what
the limit says and not what the fixtures assumed, so the fixtures drop them
and the semantics wait on the risk desk (plan D13); and the three roots
assemble `Universe::new()`, so no deployed process feeds a bucket. PARTIAL for
the aggregation; CONTRADICTS, still, for the placement.

### F7 — what the netting slice closed, and the two things it did not

**Status: recorded for the next reader, not a gap of its own.**

§27 and §27.1 are now implemented at the cell: intents, netting, contributor
attribution, self-trade prevention, internal crossing, the crossing cap and the
netting ratio.

The capability the blueprint numbers **31 in its capability list** — "Intent
Netting: aggregation, internal crossing, contributor attribution, self-trade
prevention" — is ALIGNED at the edge and absent at the centre. An earlier
version of this paragraph called that "§31 in the capability table", which was
wrong twice: the capability list is numbered independently of the sections, and
**§31 is "The Eight Execution Paths"**, as
[`blueprint-diagram-reconciliation.md`](blueprint-diagram-reconciliation.md)
correctly has it. Two documents on this branch used the same token for
different things. The sections that govern netting are §27, §27.1 and §27.2.

Two things it deliberately did not do, so that nobody reads the above as
covering them.

**Contributors reach the centre; nothing at the centre reads them yet.** The
uplink carries the vector and the schema bump refuses an old reader, but the
kernel does not consume cell deltas at all — `Platform::attribute` works from
its own `OrderManager`, and the two order paths never meet. Restoring
`Order::hypotheses` fixed the central plane's own attribution, which was
discarding a fact it already held; it did not join the edge's contributors to
it. That join is a real piece of work and it is not done.

*Closed at `7ef6063` and `7d79161`.* `CentralPlane::ingest`
(`central/plane.rs:855`) settles the interval's orders and crosses to
per-cell, per-strategy, per-instrument books (`:976`) before the halt step: a
fill pro rata to the contributors on its own side through `split_pro_rata`
(`qip-learning-engine/src/attribution.rs:266`, largest-remainder, asserting
the shares sum), a cross moving buyer up and seller down at the recorded mid,
and a decomposition that must close to the last unit or count
`qip_central_attribution_failures_total`. `qip-kernel/tests/attribution.rs::a_netted_orders_fill_is_attributed_to_its_contributors_with_zero_residual`,
`::an_internal_cross_moves_both_contributors_books_at_the_mid_and_the_close_out_is_exact`;
and the API sink carries the interval onto the report
(`qip-api/src/mesh.rs:1148-1149`), without which a deployed centre would have
settled nothing. This also answers the paragraph two below: a cross is now
settled at the *centre's* books; the cell's own journal entry is unchanged.

**A full cancellation is never crossed, by arithmetic.** The matched size is
`min(buy, sell)` over a denominator of `buy + sell`, so the ratio cannot exceed
one half and reaches it exactly when two strategies cancel completely. The
forty percent cap therefore refuses §27.1's flagship case — "strategies that
disagree cost nothing to run together" — every time. It is left that way
deliberately: §27.1 caps crossing "per instrument **per interval**" and never
defines the interval, and choosing one here to make the case reachable would be
inventing the parameter that decides when a safety control fires. The behaviour
is safe (nothing reaches a venue either way) and less than the blueprint asks
for. Setting the interval is an owner decision. *At `153e429`:* the interval
is a configuration, `CellConfig::crossing_interval` — `Passes(n)` or
`Span(d)`, refused at zero, negative or longer than the 1,024-sample history,
and `None` by default, which is this paragraph's arithmetic byte for byte.
Setting it is still the owner's (D3), and no root does.

**Crossing is booked, not settled.** A cross is recorded as having happened —
journal entry, both sides, price, size — and no position, cash balance or
utilisation moves as a result. Utilisation is charged pro-rata on what reached
the venue, which is correct for the venue-facing order and says nothing about
the crossed portion. §27.1's "both strategies receive their full intended fill
at the crossing price" is therefore **half implemented**: the price and the
record exist, the fills do not. Naming this here rather than in a comment
because a reader who sees `CrossedInternally` in the journal will otherwise
reasonably assume the books moved.

### F8 — the follow-on this slice makes easier, and the one footgun it adds

`Cell::work` now has a named seam between what the strategies want
(`Vec<Intent>`) and what is sent. The arbitrage scanner, the leg coordinator and
the path router all produce something an intent already is, so they append to
that vector rather than needing a second order path beside the netting one.
`NettingPolicy::NoNet { cycle_id }` already gives each leg its own group, so
legs cannot be silently combined with directional intents before a producer for
them exists.

The footgun is a **blocking precondition on that brief, not a note attached to
it.** A leg that forgets `as_cycle_leg` is netted against directional flow, and
the resulting order is well-formed, plausibly sized and wrong — no error, and
nothing in the journal to notice. `net()` refuses to combine a leg that
declares itself one, but `Intent` and `NetIntent` have public fields and are
built by literal, so the declaring is unenforced. Review judged this acceptable
only because `LegGroup` has zero call sites anywhere, including tests: the
guard currently protects a path nothing walks.

So the work that adds a leg producer must make the declaration impossible to
omit — a constructor that only yields no-net intents, or a leg type that cannot
become an `Intent` without carrying its cycle — **in the same change that adds
the producer**. Adding the producer first and the enforcement afterwards is the
ordering the guard exists to prevent.

*Closed, in the order this note asked for.* `3632932` made the leg a type
before the producer existed: `CycleLeg::new` is the only constructor that
yields `NoNet`, `Intent`'s policy is a private field with no setter, and a
producer's promise lives in its return type, `Vec<CycleLeg>`. Then `71f9465`
added the producer — the desk scans the cell's own books and its legs go out
by the one order path (`qip-edge/tests/arbitrage.rs::a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass`).
`LegGroup` still has no call site (`qip-contracts/src/intent.rs:104` says so):
the cell's `Placer` cannot cancel, so a cycle broken between legs trips the
switch and journals how far it got (`cell.rs:1628`,
`::a_cycle_that_breaks_between_legs_halts_the_cell_and_records_the_break`)
rather than coordinating an unwind it cannot perform. The crossing interval
(D3) is now a configuration (`CellConfig::crossing_interval`, `cell.rs:70`;
`153e429`) that defaults to the per-net arithmetic and that no root sets, so a
full cancellation still never crosses in any deployment.
