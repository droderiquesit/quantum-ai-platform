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
| §2.2 | Risk reads aggregates, never strategy lists | Risk state is aggregate counters; since `b9e9e7d` the rule is structural — `qip_risk::aggregate::RiskAggregates` moves running counters on each fill and `LimitSet::check_aggregates` reads book-level figures through the `AggregateFigures` trait, so a test can wrap the book in a probe that counts every figure consulted | ALIGNED — re-scored at `296e187`; was PARTIAL | `libs/qip-risk/src/aggregate.rs`; `qip-risk/tests/aggregate.rs::the_aggregate_check_reads_the_same_fixed_figures_at_eight_strategies_and_at_five_hundred_and_twelve`; the tail figures a limit reads derive from the limit's own confidence through `RiskState::with_tail_risk` (`d94b156`, `990032a`; `platform.rs:4270`) and every `LimitKind` arm has a fixture it admits and one it refuses (`160c4e8`, `qip-risk/tests/limit_fixtures.rs`); at `88eb1e2`, five commits after this re-score, the kernel feeds every desk fill into the aggregate and reads limits from it, so the property is held in production and not only by the lib's own test. Re-scored at `584c96b`: the aggregate is now fed on the axes the limits read — sector, country, asset-class and venue buckets projected from the universe at assembly (`588335a`; `platform.rs:1144-1150`, `exposure_axes_for` at `:4760`, `aggregate_fill` at `:4723`, the pre-trade projection at `:4486-4492`; `qip-kernel/tests/risk_aggregates.rs::a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused`, `::an_order_that_keeps_its_sector_bucket_under_the_cap_is_admitted`) and every venue fill a cell reports, under the cell's id as the strategy axis (`98bc687`; `central/plane.rs:1081`, `charge_cell_fills` at `platform.rs:1720`; `::a_cells_fills_are_charged_into_the_aggregate_and_the_next_desk_order_is_refused_on_leverage`). Two limits that could never fire now can, and the first thing they did was refuse the first order into an empty book — share-of-gross concentration is 100% on any first position — so the two kernel fixtures drop `MaxConcentration` and say why (`tests/capital.rs:57-69`), and the semantics are the risk desk's (plan D13). Re-scored at `e04815e`: the three central roots assemble from the committed catalogue rather than `Universe::new()` — `data/datasets/universe.json`, read from `QIP_UNIVERSE_PATH` and refused unset (`8224509`; `qip-api/src/main.rs:346`, `qip-fastbrain/src/main.rs:282`, `qip-deepbrain/src/main.rs:367`; `qip-financial/src/catalogue.rs::load` at `:154`, the manifest recorded under its hash by `record_manifest` at `:239`), mounted at `/etc/qip/universe.json` on all three Cloud Run workloads (`e40335d`; three `file_name = "universe.json"` blocks in `catalogue.tf`) — so a deployed process would feed every bucket, and the first thing a fed bucket does is what D13 predicted: `qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_refused_by_the_default_concentration_cap_until_adr_0027_is_decided` (`:491`) pins the refusal until ADR 0027, proposed at `360cfd8`, is decided. The deep brain's replay branch keeps the empty universe on purpose (`qip-deepbrain/src/main.rs:230`), because a tape carries bars and not listings. **Re-scored at `eca7ebb`: D13 is closed and the pinned refusal is gone.** ADR 0027 is accepted on option (a) — a bucket cap is a share of equity, not of gross — and `LimitKind::MaxAxisWeight` (`qip-risk/src/limits.rs:77`, evaluated at `:636-660` through `RiskState::ratio` at `:310-315`) replaces the two `MaxConcentration` entries in `LimitSet::conservative_default` on the same axes at the same numbers (`:811-833`). The test the sentence above names — `::the_first_order_into_a_catalogued_universe_is_refused_by_the_default_concentration_cap_until_adr_0027_is_decided` — no longer exists; do not grep for it. It was replaced in place by `qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_admitted_and_a_sector_past_the_cap_is_still_refused` (`:519`), which asserts both halves against the **unmodified** default set: ten shares of the first decision-grade instrument in `data/datasets/universe.json` are admitted, and half of equity in the same sector is refused by a message naming the delimited token `sector-concentration:`. The guard against the class rather than the instance is `qip-risk/tests/risk.rs::no_limit_in_the_default_set_divides_by_a_number_the_order_itself_moves` (`:578`), reading `LimitKind::denominator_moves_with_the_order` (`limits.rs:148`), a wildcard-free match so a seventeenth kind cannot be added without someone answering the question. Two things this re-score does **not** claim: the fixture helpers in `qip-kernel/tests/capital.rs:64` and `tests/risk_aggregates.rs:142` still `retain` against `MaxConcentration` and still carry doc comments saying the default set holds two share-of-gross caps — the filters are now no-ops and the comments are false, which is open work; and no gate output is quoted here, because this row was re-scored by reading the tree and not by running the suite | Open: the two kernel fixture helpers' dead `retain` and false doc comment | — | 10 | `aggregate.rs` |
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
| 7 Ledger, wallet, treasury | Authoritative money state per user and per strategy; reconcile every holding; move capital in signed corridors | `qip-capital`, `qip-capital-fabric` (`transfer.rs`, `settlement.rs`), hash-chained event log; since `7ef6063` the centre's per-cell, per-strategy, per-instrument books, settled from each cell's report, and since `5290bb9` settled from the report's venue-confirmed `fills` and never from its `orders` (`CentralPlane::settle`, `central/plane.rs:1161`) | PARTIAL | Capital allocation, envelopes and exposure exist; a fill is booked as the cell's own shares, refused if they do not sum, and a cross moves both books at the recorded mid, closing to the last unit or counting `qip_central_attribution_failures_total` (`qip-kernel/tests/attribution.rs`). What was sent is registered, not booked: `SentOrders` holds 4,096 per cell (`:1526`), counted under `qip_central_orders_sent_total` (`:1187`), and a fill for an order the centre never saw sent, or beyond its unfilled remainder, is a `BreakOrigin::UnsentFill` break (`:1214`) that halts the cell — `tests/attribution.rs::a_report_from_a_cell_older_than_the_fill_record_is_counted_sent_and_settles_nothing` (`:441`), `::a_fill_on_an_order_the_centre_never_saw_sent_halts_the_cell_and_books_nothing` (`:499`), `::a_fill_beyond_the_quantity_sent_is_the_same_break` (`:567`), `::a_fill_whose_shares_do_not_sum_to_it_is_refused_rather_than_booked_short` (`:635`); `tests/risk_aggregates.rs::a_sent_order_the_venue_has_not_filled_charges_nothing_to_the_aggregate` (`:506`), `::the_same_order_filled_in_the_next_report_charges_exactly_the_fill` (`:562`) Re-scored 2026-09-05: **the books are per user as well as per strategy.** `qip_capital::ledger` (`backend/crates/services/qip-capital/src/ledger/`) holds `UserId` and `Jurisdiction` as validated newtypes, `Mandate` as the §43.3 terms validated by field (shares in `[0, 1]`, floor at most capital, families non-empty, refused by name), `Entitlement::evaluate` from jurisdiction, product eligibility, role and mandate on every request, `CashBalance` per `(user, strategy, currency)` whose `available()` excludes every `ExpectedInflow` until `post_inflow` says it arrived, and `UserLedger` keyed `(UserId, StrategyId)` in a `BTreeMap`. The kernel holds one (`Platform::user_ledger`) under a single desk mandate (`DESK_USER`, `Mandate::desk`) and `Platform::journal_to_desk` books every position of a settlement's exact attribution to it from `ingest_cell_report`, so the §43.4 chain now runs `Fill → contributor vector → Strategy → Mandate → User` for the desk. A fill split across users is refused whole unless the `UserShare`s sum to it exactly (`qip-capital/tests/user_ledger.rs::a_fill_split_across_users_that_does_not_sum_to_the_fill_is_refused_and_no_book_moves`), an expected inflow cannot be spent or reserved (`::an_expected_inflow_cannot_be_spent_until_the_ledger_posts_it`), and the withdrawal arm is `WithdrawalEntitlement`, a type with one variant, `Refused`, and no `Deserialize` — the ADR 0021 line held by a type rather than a sentence (`::a_withdrawal_is_refused_for_every_role_and_the_refusal_names_the_adr`); `qip-kernel/src/platform.rs::user_ledger_tests::a_fill_the_centre_settles_is_booked_to_the_desk_users_per_strategy_balance` proves a settled round trip reaches the desk's book at the strategy. Not built: no user but the desk is enrolled (no mandate registry, no `InvestmentRequest`, no per-user split of a fill in production — every fill is booked to the desk whole); the user books carry attributed P&L, not cash flow per fill, because the settlement's `PositionAttribution` carries no traded quantity; the desk's own broker fills reach the strategy attributor and not this ledger; `StrategyFamily` is absent from the chain because nothing maps a `StrategyId` to a family at the seam; and the ledger is in-process state, journalled to no event-log record yet. | **No wallet, no corridor, no transfer gate, no destination registry, no custody engine** — `grep` for each returns nothing. Phase 12, and bounded by ADR 0021. The books are per strategy, not per user Re-scored 2026-09-05: the books are per user for one user; see the evidence column for what the ledger holds and what it does not | 12 |

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

  **Re-scored 2026-09-05 — the self-model exists and is used.** "No
  self-model at all" above stopped being true with
  `qip-learning-engine/src/self_model.rs`: a `SelfModel` is a `BTreeMap` of
  `CapabilityEstimate`s keyed by `ComponentKey` (`detector:<class>`,
  `analyst:<manifest id>`, and the `rung` and `strategy` kinds, typed but not
  yet fed), each a window of at most 128 graded outcomes, the model capped at
  512 components evicting the least recently updated. `estimate()` is the hit
  rate shrunk toward one half by four pseudo-counts — `(h + 2) / (n + 4)`,
  stated in the doc comment — and is *refused* below ten outcomes rather than
  reported as `0.5`. Fed from LEARN: `Platform::learn_from` charges every
  informative evaluation to the detector named by the hypothesis class and to
  each roster analyst whose `run-<id>-<n>` is among the contributors
  (`Platform::components_of`), then hands
  `SelfModel::origin_factors()` to the reasoning engine whole
  (`ReasoningEngine::set_origin_factors`). Consumed in exactly one place:
  `Hypothesis::form_with_factors` multiplies each evidence item's signed
  weight by its origin's factor before the log-odds update, records the
  factors applied in `Hypothesis::origin_factors` so a replay re-forms the
  same confidence, and leaves an unmeasured origin at full weight; the prior,
  the review bar and the action bar are untouched. Evidence:
  `qip-learning-engine/tests/self_model.rs` (refusal below the minimum,
  always-wrong near 0 and always-right near 1, bounded window, bounded model),
  `qip-kernel/tests/self_model.rs::grading_a_resolved_thesis_moves_the_self_model_for_the_components_that_produced_it`
  and `::the_reason_factor_scales_an_origin_only_with_a_sufficient_sample_and_is_recorded_on_the_hypothesis`,
  and `platform.rs::self_model_tests` for the LEARN→REASON handover, every
  test mutation-verified. `Platform::self_model()` is exposed for the API and
  not yet served by a route — *history as of 2026-09-05: it is served at
  `GET /api/v1/cognition/self-model`; see "Re-score 2026-09-05" at the foot of
  this file*. Still open on this row: the rung and strategy
  kinds are unfed — the routing record is per cycle, not per hypothesis, and
  a strategy session has no stated confidence to grade — and §13.2's
  exploration budget reads nothing from the model.

  **Re-scored 2026-09-05 — episodic memory is the §10 episode vector with
  bounded, bitemporal approximate retrieval, consumed as precedent only.**
  "Episodic memory holds one agent's research conclusion" described
  `qip-agents/src/memory.rs`, which still does that and is untouched; the
  blueprint's episode now lives in `qip-ai/src/memory/` (`episode.rs`,
  `store.rs`). An `Episode` carries the instrument, the regime label in
  force (market and volatility, as the cost router's closed enums print
  them), a `FindingsSummary` (runs, findings, coverage, contested), the
  panel's `AnalystStance`s in agent-id order, the `ClaimRecord` (class,
  claim label, implied direction, effective confidence after review), the
  `DecisionTaken` (approved, rejected on review, not sizeable), the
  `EpisodeOutcome` once resolved (realised move in bps, realised P&L), and
  `at`/`known_at`. The embedding is a stated 32-dimensional encoding with no
  learned weight — eight instrument signs from `sha256`, one-hot regime,
  volatility and claim blocks, direction, confidence, coverage, mean
  conviction, stance shares and a log-scaled horizon, laid out index by
  index in the doc comment on `EPISODE_DIMENSIONS`. `EpisodicMemory` is
  capacity-bounded (4,096 by default) with oldest-`known_at`-first eviction,
  and recall is locality-sensitive hashing in four six-bit tables of random
  hyperplanes drawn by splitmix64 from the constant `LSH_SEED`, buckets in
  `BTreeMap`, home buckets then one-bit neighbours probed in a fixed order,
  at most 256 candidates gathered and re-ranked by exact cosine —
  deterministic across constructions and bounded whatever the memory holds.
  `recall(query, now, k)` returns only episodes whose `known_at` is strictly
  before `now`; the boundary is refused rather than admitted because the
  deterministic clock can hand two cycles one reading. In the kernel, REASON
  recalls the five nearest before the panel convenes and records them on the
  hypothesis as a `HypothesisPrecedent` (`Platform::precedents()`): the
  entries, their outcomes, and a `PrecedentDigest` — the share of the nearest
  resolved episodes whose realised move went the claim's way, `None` where
  nothing resolved rather than zero. LEARN's resolve path
  (`Platform::remember_resolved`, from `calibrate_resolved`) moves each
  resolved thesis's pending episode into memory with its outcome, stamped
  knowable at the resolution instant, so memory fills from real cycles. **The
  precedent does not touch the confidence arithmetic in this slice**, and the
  proof is a control rather than a sentence:
  `qip-kernel/tests/episodic.rs::the_kernel_records_precedents_on_a_hypothesis_once_prior_episodes_resolved_and_leaves_the_confidence_alone`
  drives two platforms through the same tape and the same three REASONs, one
  whose resolution is known before the third question and one whose
  resolution shares the third question's clock reading, and asserts the two
  confidences are bit-identical while the digests differ; the mutation that
  adds the recall count to the synthesis prior fails it. The precedent is
  also deliberately *not* written into the `AgentBrief` context, because
  `brief.context` is the string the reviewer's lesson matcher
  substring-matches against, so a block there could change which objections
  are raised and, through them, the confidence; the language-model context
  block the blueprint describes waits on a channel that is not also a
  matching key — *history as of 2026-09-05: that channel is
  `AgentBrief::precedent`, a typed field; see "Re-score 2026-09-05" at the
  foot of this file*. The route by which precedent could later bear on confidence
  is ADR 0005's evidence-weighted update — a digest entering as an
  `Evidence` item with a stated diagnosticity, subject to the same origin
  factors and review as every other item — and never a multiplier applied
  after review. Library evidence: `qip-ai/tests/episodic.rs` (not recalled
  before or at `known_at`; a record knowable before it was true is refused;
  capacity evicts the oldest-known; the bound binds and re-ranking is exact;
  two constructions bucket and recall identically; the digest excludes
  unresolved and directionless cases), every test mutation-verified. Still
  open: the outcome's realised move is on the precedent's own observable, so
  an agreement between a volatility episode and a price claim is a weak
  analogy the entry exposes by naming its claim; no digest is shipped to a
  cell; and the encoder is fixed rather than learned.

- **[PLANE 3/7 — Valuation]** *Ownership:* **none.** *Placement:* n/a.
  *Authority:* would be informs-only per §39 layer 4. *State:* n/a.
  *Degradation:* n/a. *Tests:* none. MISSING-CURRENT, blueprint Phase 14.
  **Deliberately not scaffolded** — six engines named by §16.1 with no consumer
  would be six empty crates.

  **Re-scored 2026-09-04.** The gap-map's verdict on §16.1's credit engine —
  "`default_probability`/`recovery_rate` fields... a data holder, not an
  engine. No spread decomposition" — is now half true rather than wholly
  true. `RiskCharacteristics::spread_decomposition`
  (`qip-financial/src/risk_profile.rs`) turns the two fields already on the
  type into the named identity `spread ≈ default_probability ×
  (1 − recovery_rate)` in exact `Decimal` arithmetic, refusing (naming the
  field and the offending value, via `Error::invalid`) a `default_probability`
  or `recovery_rate` outside `[0, 1]` rather than clamping either — proved by
  `qip-financial/tests/object_model.rs::the_spread_decomposition_identity_is_exact_in_decimal`,
  `::a_recovery_rate_above_one_is_refused_not_clamped`,
  `::a_recovery_rate_of_exactly_one_is_the_admitted_boundary`, and
  `::a_negative_default_probability_is_refused_not_clamped`, each
  mutation-verified. This does not promote the plane out of MISSING-CURRENT:
  no engine, consumer, credit-spread valuation flow, or entry in the six
  named by §16.1 exists yet — one method on an existing struct now computes
  a documented identity instead of leaving two fields unrelated, and the doc
  comment states what the identity assumes (risk-neutral, single-period, no
  liquidity premium) so it is not mistaken for a market spread. The six
  engines remain unscaffolded for the reason given above.

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

**Re-score at 2026-09-04, on the §43.3/§43.4 attribution gap named above and
in `docs/plan/blueprint-v10.1-gap-map.md`'s "Explanation object" row.**
`qip-compliance::model_risk::Explanation` (`src/model_risk.rs:373`) now
carries an optional `upstream: Option<HypothesisId>` — the claim or
hypothesis whose belief produced the explained output's inputs — stated
through `Explanation::reconciled`'s constructor rather than defaulted, so a
caller must write `None` explicitly to record that no hypothesis drove an
output. It is serialised with `serde` and preserved exactly by a round trip,
and it is folded into the same private-field, one-constructor discipline
that already refuses an explanation whose contributions do not reconcile to
its output in exact `Decimal` arithmetic — carrying the reference does not
relax that check (`tests/model_risk.rs::an_explanation_with_an_upstream_reference_that_does_not_reconcile_is_still_refused`).
This is one additive hop toward the blueprint's full attribution chain (fill
→ strategy → family → mandate; intent → belief → causal edge → world event →
entity), not the chain itself: `Explanation` still explains one model's
numeric output, not a position, and the gap-map's finding stands unchanged —
`grep -rln "model_risk::Explanation"` still finds no caller outside this
crate's own `lib.rs` re-export and its tests, so the field is real and
round-trips but is wired to nothing that would populate it from a live
hypothesis. The per-user, per-strategy ledger §43.3 also names is still
absent, as stated above; this hop touches only the explanation half of the
gap-map row, not the ledger half.

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
| Self-model stale | Exploration budget reverts to flat | PLANNED-FUTURE — Phase 9 | Was: "No self-model exists (`grep -rln "SelfModel"` returns nothing). Deliberately not represented". Re-scored 2026-09-05: a `SelfModel` exists (`qip-learning-engine/src/self_model.rs`; the Plane 2 re-score above), so the first sentence is history. The verdict does not move: `grep -n -i self_model backend/crates/libs/qip-contracts/src/degradation.rs` still returns nothing, and §13.2's exploration budget reads nothing from the model, so there is no budget to revert to flat. Still deliberately not represented |
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

**Re-scored at `2fd254f`: PARTIAL, was CONTRADICTS.** The slice this row asked
for was written at `0ca4b92` and this paragraph was never updated, so the
document went on calling absent a thing the tree had carried for days — the
exact drift the re-score convention exists to prevent, in the row whose whole
subject is not inferring a property from a document.

What exists now, verified by reading rather than from the commit message:
`qip-edge/src/reservation.rs` holds `RegionAllocation` with a `reserve` that
refuses against a balance already spent; `Cell::with_region_allocation`
(`cell.rs:598`) takes the allocation; and `hold_region_capital`
(`cell.rs:3422`) is consulted on both paths that can commit capital,
`cell.rs:1723` and `:2668`. `qip-edge/tests/reservation.rs` carries the
property tests, among them
`a_second_strategy_is_refused_once_the_region_allocation_is_spent_even_though_its_own_envelope_would_admit_it`
— which is precisely the disconnected-cell case the paragraph above says
nothing covers.

**What has NOT changed, and is why this is PARTIAL rather than ALIGNED: no
composition root constructs it.** `grep -rn "with_region_allocation\|
RegionAllocation" backend/crates/apps/` returns nothing. The type is
opt-in by construction — a cell records into an allocation it is *given*,
never one it reaches for — so a `Cell` built by `qip-edge-node` today has no
region allocation and behaves exactly as this row originally described. The
control exists and is tested; it is not installed. A reader must not take
"the slice landed" for "the property holds in a deployment", which is the
same distinction the row was written to protect and the reason the verdict
moves one step and not two.

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

*Re-scored at `eca7ebb` — both of the two open things above are closed, and
the placement finding is not.* The roots stopped assembling `Universe::new()`
at `8224509`, and the concentration semantics stopped being the risk desk's
open question at `eca7ebb`: ADR 0027 is accepted on a share of **equity**, so
`LimitKind::MaxAxisWeight` (`qip-risk/src/limits.rs:77`) carries the two
default entries and the first order into a fed book is admitted while a sector
past 0.35 of equity is still refused
(`qip-api/src/main.rs::the_first_order_into_a_catalogued_universe_is_admitted_and_a_sector_past_the_cap_is_still_refused`).
The denominator is equity because it is knowable before an order exists and
the order under check does not move it; a share of gross is the position under
test divided by itself on a one-name book, which measured the book's size and
called it composition. PARTIAL stands for the aggregation on the placement
finding alone, which nothing here touched.

*Re-scored 2026-09-05, in the working tree after `997cad8` — the placement
finding, at last.* **PARTIAL, closer; the shape is built, installed and
proven, and one thing about the deployment is said rather than assumed.**
Verified by reading and by running, not from any commit message. The root
installs the bound (`63e4556`, `qip-edge-node/src/lib.rs::assemble`, refused
at start when `QIP_REGION_ALLOCATION` is absent), so the "no composition root
constructs it" half of the earlier verdict is closed. The table is now the
blueprint's shape rather than one ledger per cell: `qip-edge/src/reservation.rs`
holds `RegionTable`, a `Send + Sync` handle on one `RegionAllocation` that a
root gives to every cell of a region through `Cell::with_region_table`
(`with_region_allocation` opens a private one and is what the node still
calls); holds are filed under the owning cell so two cells on the same pass
running the same strategy do not collide and one cell's pass-scoped sweep
cannot return a hold its sibling is mid-pass on. Three properties the row
named as untested are now tests in `qip-edge/tests/region_table.rs`, each
driven through `Cell::work` and each mutation-verified:
`a_disconnected_cells_second_proposal_is_refused_against_what_its_first_still_holds_until_that_order_expires`
(no mesh, grant fixed; refused under the literal `region_reservation`, the
refusals series moves, and the capital returns when the venue withdraws the
first order whole and unfilled — before this a rested order that expired
stayed billed forever, so a partitioned cell starved on orders that never
ran); `two_cells_under_one_region_table_cannot_each_spend_the_whole_grant`
(with the contrast that the same two cells over two separate tables both
send, so it is the sharing and not the amount that refuses);
`a_committed_reservation_survives_the_cells_halt_and_a_halted_pass_neither_sweeps_nor_returns_it`
and `an_expired_orders_capital_returns_once_and_no_later_pass_returns_it_again`.
What remains, and why this is not ALIGNED: a `RegionTable` is shared in
memory, and the node runs one cell per process on one execution node
(ADR 0024), so in a deployment the table each cell consults is its own
process's, opened over that node's `QIP_REGION_ALLOCATION`. Two cells of one
region on two nodes are bounded by two operator-given amounts, and nothing
checks that those amounts sum to the region's grant — that is operator
discipline, not a structural guarantee, and the mesh carries no per-region
amount that could make it one. The in-process property is proven; the
cross-process one is a signed field on the wire and a producer at the centre
away, in crates this slice did not touch.

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

**Re-scored 2026-09-04: a booked cross now settles at the cell.**
`Cell::settle_cross` calls `Cell::book_cross` (`qip-edge/src/cell.rs`)
before it seals the record, and `book_cross` reads *only* the
`InternalCross` record the journal entry is written from — the one buyer,
the one seller, the size and the mid — so the buyer's lot rises and its cash
falls by `quantity × price`, the seller's the reverse, and the two cash legs
sum to zero. Read back through `Cell::strategy_position` and
`Cell::strategy_cash`; the venue-facing `Cell::position` moves by nothing,
because the venue saw nothing and the drop-copy reconciler must not. The
forty percent cap is untouched and the settlement sits behind it. Two
consequences worth stating. First, the record must be able to settle
itself: a net whose crossable portion names two buyers or two sellers
carries one size and no per-strategy split, so `cross_internally` now
refuses it under `internal_cross_attribution` before any record exists —
the same record `CentralPlane::ingest` already refused to settle, for the
same reason, so cell and centre can no longer disagree about a cross one of
them booked and the other could not settle; those intents still net exactly
as before, and nothing extra reaches a venue. Second, this is the crossed
portion only: a venue fill is attributed at the centre and is not booked to
these per-strategy books, so `strategy_position` is what never reached a
venue and not a strategy's whole position. Proved by
`qip-edge/tests/crossing.rs::a_booked_cross_moves_both_contributors_lots_and_cash_at_the_journaled_mid_and_the_cash_legs_cancel`
(price read back from the chain, not the report),
`::a_cross_above_the_cap_is_still_refused_and_moves_no_lot_or_cash` and
`::a_cross_with_two_strategies_on_one_side_is_refused_rather_than_settled_by_a_guess`,
each mutation-verified (settlement call removed; settled at the journaled
price plus one; attribution gate disabled; cap comparison inverted). §27.1's
"both strategies receive their full intended fill at the crossing price" is
therefore implemented at the cell for the crosses the cell books; the
sentence above that says the fills do not exist is history from this date.

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

## Rows added after the full source text became available

`docs/architecture/algorik-blueprint-v10.1-source.md`, committed this session,
is the first time the ~232-section prose of the blueprint (as opposed to the
shorter, unnumbered HTML companion) has been available to score against. The
five rows below were read directly from that text, checked against a numbered
section or subsection with no prior row anywhere in this file — not a
citation, not a paraphrase — and then checked against the tree. They are
placed here rather than folded into the tables above so the "read against the
full text for the first time" provenance stays visible; a future re-score may
move one into its natural table without losing that history.

| Blueprint element | Required invariant | Implementation | Status | Evidence | Minimal action | Risk / blast radius | Phase | Validation |
|---|---|---|---|---|---|---|---|---|
| §20.3 Decay and Retirement | "Retirement is as automated as promotion" — six signals trigger a response, and sustained underperformance beyond threshold retires a strategy, archiving its state and freeing its slot, without a human in the loop | `qip_lifecycle::demotion::DemotionMonitor::enforce` (`backend/crates/services/qip-lifecycle/src/demotion.rs:368`) is wired to production: `CentralPlane::learn` calls `self.factory_mut().review(...)` (`backend/crates/runtime/qip-kernel/src/central/learning.rs:237`), reached from `stage_learn`. But `enforce` only ever demotes to a lower rung or a rung holding no capital — it never calls `retire`. `StrategyFactory::retire` (`central/factory.rs:406-414`) exists and forwards to `LifecycleLedger::retire`, but its only callers in the whole workspace are a doc-comment example (`qip-lifecycle/src/lib.rs:62`) and the crate's own test (`qip-lifecycle/tests/lifecycle.rs::retirement_is_terminal_and_a_retired_strategy_must_be_re_proposed_as_a_new_candidate`, which calls `ledger.retire(...)` directly, not through the automatic monitor) | PARTIAL | `grep -rn "\.retire(" backend/crates` returns three sites: the definition, the doc example, and the manual test above — no production call site. `an_automatic_demotion_for_decayed_performance_is_counted` (`qip-lifecycle/tests/lifecycle.rs:976`) proves decay demotes automatically; nothing proves sustained decay ever retires automatically | Give `DemotionMonitor::enforce` (or a caller above it, since retirement also has to free the strategy's slot and address §35.2's "never orphaned" question for its open positions) a sustained-underperformance trigger that calls `retire`, not just `demote`, and a test that drives it there without a human call | Medium — a strategy that should have been retired keeps consuming its evaluation slot and, per §20.3's own reasoning, "evaluation budget, message rate and capital while contributing nothing" | 2 | `qip-lifecycle/tests/lifecycle.rs` |
| §25.3 The Risk Envelope | Ten enforced levels: per user, per strategy, per family, per instrument, per asset class, per venue, per factor, per causal driver, per region, and a global ceiling | `qip_risk::limits::LimitKind` (`backend/crates/libs/qip-risk/src/limits.rs:46-83`) has sixteen kinds, none named by level; the kernel's aggregate is fed on four axes projected from the universe — sector, country, asset-class and venue (`platform.rs:4760` `exposure_axes_for`, cited above at line 67) — plus a per-cell (region-adjacent) axis via `charge_cell_fills`. There is no per-family cluster limit, no per-factor decomposition at fill time, and no per-causal-driver limit: a grep for `causal_driver` or `CausalDriver` under `backend/crates/libs/qip-risk` returns nothing | PARTIAL | `backend/crates/libs/qip-risk/src/limits.rs:46-83`; `backend/crates/runtime/qip-kernel/src/platform.rs:4760`; absence confirmed by the grep above | The per-causal-driver level is the one the blueprint calls out by name as catching "the concentration that ends firms" (§25.3, §25.6) — it needs a `RiskState` input sourced from `qip-world-model`'s causal graph before a causal-driver limit kind can mean anything. Family and factor levels are smaller, additive slices of the existing aggregate-axis work (D13/ADR 0027) | Medium — a concentration a notional or bucket view cannot see is undetected, not merely unlimited | 8 | `qip-risk/tests/aggregate.rs`, `qip-kernel/tests/risk_aggregates.rs` |
| §25.6 The Cross-Margin Model | A collateral graph — what backs what, portfolio vs. isolated margin per venue, rehypothecation, correlated collapse, cross-venue collateral, liquidation cascade — feeding the risk envelope and the liquidity ladder | Absent entirely. A grep for `cross_margin`, `CrossMargin`, `collateral_graph` or `rehypothecation` (any case) across `backend/crates` returns nothing | MISSING-CURRENT | Grep above; no type, module or test anywhere in the tree | Backlog. Needs an owner decision before code: a collateral graph is a new data model that several existing engines (`qip-risk`, `qip-capital`, `qip-portfolio-engine`) would all read, so it is a crate-boundary question — where the graph lives and who is allowed to write it — not an addition to one crate. Likely needs an ADR under `.claude/rules/architecture/00-boundaries.md`'s "Recording a decision" bar if it becomes its own service rather than a library type the risk engine consumes | Low while every venue relationship is simulated and paper-only, but the blueprint frames it as where "the classic margin spiral" is caught | 8 | — |
| §29 The Quote Loop, Quote Rate Management and Market Creation | Two-sided quoting with inventory skew, adverse-selection and volatility terms, a requote threshold, per-venue quote-rate budgets, and a gate (valuation, causal explanation, adverse-selection model, hard-coded max exposure, human approval per instrument class) before any market-creation activity | No market-making strategy exists anywhere in `backend/crates/edge/qip-strategy` or elsewhere — no fair-value/inventory-skew/half-spread/adverse-selection arithmetic and no market-making strategy type. The adjacent mechanism — cancel-and-replace repricing of a resting child order, which could carry the requote-threshold half of §29.1/§29.2 — exists in `qip-routing/src/reprice.rs` but its own module doc says so explicitly: "Nothing here sends anything, and nothing is wired yet — deliberately" (`backend/crates/edge/qip-routing/src/reprice.rs:19`); no caller exists in `qip-edge-node` or `qip-edge` | MISSING-CURRENT | `backend/crates/edge/qip-routing/src/reprice.rs:1-33`; absence of any market-making strategy confirmed by inspection of `backend/crates/edge/qip-strategy` | Two separable pieces of backlog, not one: (1) wiring `Repricer` into `qip-edge-node`'s gateway loop is scoped and stated already in the module's own doc comment; (2) an actual quoting/market-making strategy family (fair value, skew, size, quote-rate budget, the §29.3 market-creation gate) does not exist at any layer and is a new strategy family, not a wiring gap | Low now (nothing quotes); the blueprint frames market creation itself as "the most dangerous" activity in the platform, gated on a causal explanation and human approval per instrument class — a reason to scope any future work narrowly | 4 | `qip-routing/tests/reprice.rs` (mechanism only; no market-making test exists) |
| §35 A Position Has a Lifecycle / §35.2 The Three Questions Version 9 Left Open / §35.3 Unwind Ordering | An explicit state machine (Opened, Held, Flagged, Unwinding, Orphaned, Closed); a retired strategy's positions are reassigned or scheduled for unwinding, never left ownerless; a thesis-expiry sweep flags and unwinds positions held past their horizon; unwind order is a ranked policy (failed thesis first, tax efficiency, liquidity-ladder cost, hedge preservation, cross-margin respect) | `qip_portfolio::position::Position` (`backend/crates/libs/qip-portfolio/src/position.rs:38-54`) carries `lots`, `realised_pnl`, `opened_at`/`updated_at` and a `PositionSide` (Long/Short/Flat) derived from quantity — there is no lifecycle-state field and no Flagged/Unwinding/Orphaned variant anywhere in the workspace. No code path runs when a strategy retires to reassign or schedule-unwind its open positions — `StrategyFactory::retire` (see the §20.3 row above) touches only the lifecycle ledger, never `qip-portfolio` or `qip-capital`. No thesis-expiry sweep and no ranked unwind-ordering policy exist anywhere in the tree | MISSING-CURRENT | `backend/crates/libs/qip-portfolio/src/position.rs:12-54`; `StrategyFactory::retire` reviewed at `central/factory.rs:406-414` has no call into `qip-portfolio` or `qip-capital` | Backlog, and sequenced after §20.3's retirement wiring is closed — retirement without a position-disposition step is exactly the orphan case §35.2 names. This crosses the `qip-lifecycle` / `qip-portfolio` / `qip-capital` boundary (a retirement decision in one service has to reach position state owned by another), so per `.claude/rules/architecture/00-boundaries.md` the composition belongs in `qip-kernel`, not a new dependency edge between the two services. It does not by itself need a new crate; it would need an ADR only if the eventual design promotes a shared position-lifecycle type into a lib both services read, which is a decision for whoever scopes the slice, not a default | Medium — an orphaned position after a retirement is, in the blueprint's own words, "a reconciliation break, not a normal state"; today nothing distinguishes the two | 10 | — |

## Re-score at `2fd254f` — the environment gained a Terraform binary, and four gaps closed

Appended rather than folded in, per this file's convention. Every claim below
was checked at HEAD in the session that wrote it, not carried forward.

**The `no terraform binary exists in this environment` verdict is withdrawn.**
It appears twice above — in the LAYER 6/7 row and in the verification note at
the foot of the runtime section — and in four other documents. It was true
when written and is not now: `/usr/local/bin/terraform` exists, and both gates
were run against the tree.

```
terraform fmt -check -recursive .   exit 0
terraform validate                  Success! The configuration is valid.
```

This matters more than a corrected sentence. That verdict was the stated
reason "every precondition in the new modules is asserted and unexercised",
and it was load-bearing in the worst way: because nothing here parsed HCL, a
`variable "source"` (a reserved name) and a conditional `ignore_changes`
(which must be a static list) shipped together, `terraform validate` failed on
every commit carrying them, and 3,741 Rust tests passed straight through.
`terraform_contract.rs` was written against that failure. **What is still
true, and is the half a reader must not lose: `validate` is not a plan.**
Preconditions are plan-time, so ADR 0030's pairing rules and ADR 0031's
`secret_env` refusal are still asserted and unexercised. Only a real plan
exercises them.

**The frontend gates ran for the first time and pass.** They had never been
run in any session that scored this file, so the LAYER 1 row's verdict rested
on reading. `npm ci` in `frontend/` and separately in `frontend/landing/` —
the landing keeps its own dependency tree deliberately (ADR 0015, a different
React major), so it is not a workspace member and a root install does not
reach it, which is how a first attempt produced five `Can't resolve 'swiper/*'`
errors that were the runner's mistake and not a defect:

```
portal   npm run lint       clean     npm run typecheck   clean     npm run build   36 routes
landing  npm run lint       clean — 41 files, 12 routes   npm run build   13 routes
```

Both frontend absolutes were then verified by reading, not assumed from the
rule that requires them. **No control that could submit an order exists**: a
grep for `submitOrder|placeOrder|sendOrder|createOrder|POST.*order` across
both apps' `app/`, `components/` and `lib/` returns exactly one hit, and it is
prose in `landing/components/layout/footer/Footer1.js:59` stating that no
control submits a live order. **`PAPER TRADING` renders wherever posture is
shown**: `portal/src/components/chrome/PaperTradingBanner.tsx:27`, the admin
autonomy page, both risk pages, both portfolio pages, the system page and the
portal root, with the live-capable arm spelled as its own label rather than as
the absence of this one.

**Four decisions were taken and recorded** — ADR 0032 (telemetry drains to a
collector on a private address, not to a public URL; the fast brain forces it,
having no egress proxy by design), ADR 0033 (OpenObserve becomes
authenticated before it holds telemetry, firing ADR 0030's own trigger),
ADR 0034 (Coinbase, then Alpaca, then Kalshi — candidates, with
`qip-data-finder`'s licensing gate deciding), ADR 0035 (one execution node,
`us-east4`, shadow mode, dev only). `docs/plan/gate-completion-plan.md` plans
the four gates against those decisions.

**A6 is refused rather than open.** The collector image was reviewed this
session: its digest still resolves, `modules/cloudrun` does publish the
`/etc/rungmp/config.yaml` it insists on, and Trivy failed it on
CVE-2026-56854, a CRITICAL authentication bypass in `golang.org/x/crypto`
v0.54.0 fixed in 0.55.0. The registry publishes nothing above 1.9.2, so there
is no patched release. No scanner exception was written.
`infrastructure/egress/vendored-images.txt` carries the finding beside the
still-commented line.

**What has not moved: the four gates are still 0 of 4**, and three of them
are blocked on an empirical fact — sustained streaming of real market data —
that none of the above supplies. The fourth cannot pass while paper trading
holds and should be read as structurally refused rather than outstanding.

**§20.3 re-scored after the retirement wiring (this commit): PARTIAL, closer.**
The row above says "no production call site" for `retire`. There is one now:
`DemotionMonitor::enforce` (`qip-lifecycle/src/demotion.rs`) calls
`ledger.retire(...)` when a strategy that holds no capital, whose last move
was downward, is still in decay `RetirementThreshold` after it was pushed off
capital — reached from `CentralPlane::learn` through the same
`factory_mut().review(...)` seam as demotion, so retirement is automated as
promotion is. The threshold is a **duration at the floor**, not the review
count first proposed, and deliberately: the monitor is `Copy` and copied out
per review, the ledger records moves only, and a counter kept anywhere in
memory would make a retirement irreproducible from the log — the one thing
`.claude/rules/10-product-direction.md` forbids. The floor instant is in the
ledger already, so "still decaying this long after it" is reproducible from
the ledger plus one observation. `LearningVerdict::for_review` reads
`Retired` first, so a retirement from shadow is reported as `Retire` and not
`Adapt`. Proven by `sustained_decay_at_the_floor_retires_the_strategy_without_a_human_call`
and four siblings in `qip-lifecycle/tests/lifecycle.rs`, nine mutations
fired. **What keeps this PARTIAL: §35.2.** A retired strategy's open
positions are not dispositioned — the code says so at both sites — and until
§35's row closes, an automatic retirement produces exactly the orphan the
blueprint calls "a reconciliation break, not a normal state".

**§35 re-scored after the retirement disposition (2026-09-04): PARTIAL, from
MISSING-CURRENT.** The row above says "no code path runs when a strategy
retires to reassign or schedule-unwind its open positions". There is one now,
for the second half of that sentence. `CentralPlane::learn`
(`backend/crates/runtime/qip-kernel/src/central/learning.rs`) follows every
review the ledger retired this tick with `disposition_for`, which reads the
attribution's strategy books — the join A3/B11 built, keyed cell, strategy,
instrument — and produces a `RetirementDisposition`: the strategy, the
ledger's own rationale, and every non-flat lot it held as `cell/instrument`
with signed quantity, average price and a `DispositionInstruction::Unwind {
flatten_by }` for the owning cell's own DECIDE/ACT path. No order is created
anywhere on that path. `Platform::learn_from_cells` writes the record to the
event log and the journal (`Topic::PositionUpdated`) in the same call that
retired the strategy, so the instruction is reproducible from the log alone;
`CentralPlane::scheduled_unwinds` reads the same schedule back from the
ledger and the books on every call rather than from a list kept beside them,
so a retired strategy still holding a lot is listed rather than discovered.
Where the centre holds two claims about the lots — the attribution and a book
the cell itself reported — and they disagree, nothing is scheduled: a
`DispositionRefused` record names each disagreeing lot with both quantities
and goes to the log under `Topic::ReconciliationCompleted`, which is the
"reconciliation break, not a normal state" §35.2 says an ownerless position
is. The delta a cell ships carries no positions (`qip-api/src/mesh.rs`,
`report_from`), so in a deployment the attribution is the one claim and the
refusal fires only where a cell has made a second one. Proven by
`an_automatic_retirement_schedules_every_lot_the_strategy_holds_for_unwinding_and_journals_it`,
`a_retirement_whose_lots_the_cells_book_and_the_attribution_disagree_on_is_refused_not_guessed`
and `a_retired_strategy_holding_no_lot_is_dispositioned_as_holding_nothing_and_that_is_journaled`
in `qip-kernel/tests/central.rs`, each driven through `learn_from_cells` with
no call to `retire`, and mutation-verified. **What keeps this PARTIAL, in
the row's own terms:** handover — reassignment to a funded strategy sharing
the thesis — is not produced, because the centre records no thesis shared
between two strategies and an owner picked on anything else would be a
guess; there is still no lifecycle-state field on `qip_portfolio::Position`
and no Flagged/Unwinding/Orphaned variant; no thesis-expiry sweep and no
ranked unwind ordering (§35.3) exist; and the flatten instruction reaches a
cell only when something ships it, which nothing yet does — the record is
the schedule, and a cell that never reads it leaves the lot listed in
`scheduled_unwinds` until a fill closes it. §20.3's "what keeps this
PARTIAL: §35.2" paragraph above is answered to that extent and no further.

**§20.3 and §35 corrected after the LEARN stage gained the call
(2026-09-04): the review is now reached from `stage_learn`, and was not
before.** The §20.3 row above and the two re-scoring paragraphs say the
review seam was "reached from `stage_learn`". That was never true. What was
true: `CentralPlane::learn` runs `factory_mut().review(...)`, which is what
demotes and, since `3deace8`, retires; `Platform::learn_from_cells` calls it
and, since the disposition slice, journals every `DispositionOutcome`; and
`grep -rn learn_from_cells backend/crates/apps backend/crates/runtime/qip-kernel/src/platform.rs`
returned only the definition — `stage_learn` did not call it, no composition
root did, and every retirement, demotion and disposition test in
`qip-kernel/tests/central.rs` drove `learn_from_cells` by hand with a
`CellOutcome` it assembled itself. In a deployed `qip-api` the automatic
retirement path reached no process, and the paragraphs above that said
"automated as promotion is" described a seam that only tests entered. What is
true now: `Platform::stage_learn` (`backend/crates/runtime/qip-kernel/src/platform.rs`,
`review_strategies`) calls `learn_from_cells` every cycle over the outcomes
`CentralPlane::live_outcomes` derives, so the path runs in `qip-api`'s cycle
(`routes.rs`, `platform.run_cycle`) with no change to the API. The outcomes
have one provenance: `CentralPlane::ingest` books each settlement's
attributed P&L — the same `Settlement::by_strategy` figure the centre bills —
into `central/realised.rs`'s per-cell, per-strategy sessions, one per UTC day
of the cell's report instant, bounded at `REALISED_SESSIONS` (252) and kept
only for strategies the factory holds a baseline for. The observation the
monitor reads is built at LEARN time from the closed sessions since the
baseline was established: the daily return is the day's attributed P&L over
the gross limit of the envelope the centre held for the pair, the realised
loss and losing-day run are read off the same P&L, drawdown is against the
grant-plus-P&L high-water mark, and the realised cost is stated as zero
because the wire carries none — so the cost kill condition still cannot
fire from this series, and the code says so rather than inventing a figure.
The cell's own `Utilisation::realised_loss` is deliberately not read: it is
a second claim about the same fact. The LEARN stage's outcome now records
"N strategy(ies) reviewed on realised sessions (D demoted, R retired, P
dispositioned, X disposition(s) refused, S skipped)", and the cycle's
`CycleJournalEntry` carries the same counts as `strategy_review:
Option<StrategyReviewJournal>` (absent on a cycle in which no cell had closed
a session, so a platform with no cells journals exactly as before —
`attaching_the_central_plane_leaves_a_cycle_exactly_as_it_was` still holds).
A retired strategy's sessions are dropped in the call that retired it, so
it is not reviewed again. Proven by
`the_learn_stage_retires_a_strategy_whose_cells_realised_sustained_decay_and_journals_its_disposition`
in `qip-kernel/tests/central.rs`, which drives sixty decayed sessions of
venue-filled round trips through `ingest_cell_report`, runs `run_cycle` twice
ninety days apart, and reads the demotion, the retirement, the
`RetirementDisposition` and both cycles' review counts back from the journal —
with no call to `learn_from_cells`, `learn`, `review` or `retire`; ten
mutations fired, including removing the new `stage_learn` call (the test then
fails with the strategy still at `Pilot` and the stage reading "no fills to
attribute"). What this does not change: the ledger's demotion and retirement
records are still not events in the log — the disposition is, and carries the
ledger's rationale — and a strategy pushed off capital produces no new
sessions, so its retirement ninety days later is judged, as
`retirement_due` already specifies, on time at the floor plus the series that
put it there.

**§35 re-scored (2026-09-04): the lifecycle-state field named as missing in
the paragraph above now exists.** `qip_portfolio::position::Position` carries
a `lifecycle: PositionLifecycle` field
(`backend/crates/libs/qip-portfolio/src/position.rs`), and
`PositionLifecycle` (`backend/crates/libs/qip-portfolio/src/lifecycle.rs`) is
the six-variant enum §35 names — `Opened`, `Held`, `Flagged`, `Unwinding`,
`Orphaned`, `Closed`. A new position starts `Opened`; `apply_fill` moves it to
`Held` on its first confirmed lot and to `Closed` when the last lot closes,
both through `PositionLifecycle::transition`, which refuses every pair not on
its own legal-edge table (`Closed` admits no further move, and `Flagged`
cannot be walked back to `Held` directly — only `Opened -> Held`,
`Opened/Held/Flagged/Unwinding/Orphaned -> Closed`, `Held -> Flagged`,
`Flagged -> Unwinding/Orphaned` and `Unwinding -> Orphaned` are legal).
`Flagged`, `Unwinding` and `Orphaned` are reachable only by calling
`transition` explicitly (via `Position::move_lifecycle`); nothing in
`qip-portfolio` assigns them directly. Proven by
`an_opened_position_moves_to_held_on_its_first_confirmed_lot`,
`a_flagged_position_cannot_be_reopened_without_a_new_lot`,
`closing_the_last_lot_moves_a_position_to_closed_and_the_closed_state_refuses_further_transitions`
and a full-cross-product table test, in
`qip-portfolio/tests/lifecycle.rs` and `qip-portfolio/src/lifecycle.rs`'s own
unit tests; two mutations verified (deleting the refusal arm, and adding
`Closed -> Held` to the legal table) both broke the intended tests for the
stated reason. **What this does not close, in the row's own terms:** the
field exists but nothing outside `qip-portfolio` writes `Flagged`,
`Unwinding` or `Orphaned` yet — the retirement disposition recorded in the
paragraph above still reasons about lots and a `DispositionInstruction`
without touching this field, so a lot scheduled for unwinding is not yet
reflected as `Unwinding` on the `Position` itself, and no thesis-expiry sweep
or ranked unwind-ordering policy (§35.3) exists. The row stays PARTIAL.

## Re-score at `d951ff4` (2026-09-04) — §2.2 "Strategies are compiled, not interpreted"

Appended rather than folded in, per this file's convention. The §2.2 row
above reads "`qip-strategy` evaluates; no shared compiled plan with CSE" and
scores PARTIAL. Read against the tree at HEAD, the implementation half of
that verdict was already stale when scored: `StrategyCompiler` lowers every
`Expr` into one shared `Program` arena, interning children before parents on
a structural key (`backend/crates/edge/qip-strategy/src/compile.rs`,
`intern` and `structural_key`), so two structurally identical subtrees —
within one strategy or across many compiled through the same compiler — are
one IR node; commutative operands are ordered so `a + b` and `b + a` are one
node; constant subtrees fold to a literal before interning; and the map is a
`BTreeMap`, with node numbers assigned in lowering order, so a recompile of
the same source numbers every node identically. The shared plan reaches
production through `qip-edge-node/src/strategies.rs:355`, which compiles
every loaded strategy through one compiler and hands `into_program()` to the
cell's `StrategyRuntime`. What was missing was the proof, and that is what
this pass adds, in `backend/crates/edge/qip-strategy/tests/strategy.rs`:
`two_clauses_sharing_a_ratio_compile_to_one_ratio_node_rather_than_two`
(premise first: the two `Expr::Ratio` subtrees are equal and distinct
allocations; then 10 unique nodes for 14 written, exactly one `Op::Ratio`,
both rules reading it);
`a_shared_plan_evaluates_exactly_as_the_unshared_expression_tree_does`
(premise: the fixture writes the ratio three times and the compiler shared
something; then six market states through the runtime agree with a
separate reference interpreter over the written tree on kind, quantity and
conviction, covering both rules and no rule);
`node_numbering_is_stable_across_independent_compiles` (two fresh compilers
over three strategies produce equal `Program`s and equal plans); and
`the_size_refusal_still_fires_on_a_program_that_is_oversized_after_sharing`
(a 1,023-node tree written twice — 2,051 nodes as written, 1,030 once
shared, both above the 512 ceiling — is refused with `guard` and "budget").
Five mutations to `compile.rs` were applied and each fired: the dedup pass
removed (count test: 14 unique for 14 written); `Ratio` keyed by the
numerator's address rather than structure (count test: 11 for 14, while the
differential test still passed, which is the right split); the `Feature` key
salted with wall-clock parity (numbering test, three runs of three);
the syntactic budget check in `measure` removed alone (the new size test
still passed — the post-sharing `cost_of` check refused on its own) and
then both checks removed (it failed); and statistic literals keyed without
their value (differential test: conviction 0.0005 against 0.6). `compile.rs`
was restored byte-for-byte after each, sha256
`0a6b5b64f36e765656aeddeab6af3f18657d41f007d1b876f86fe70b89deb8e9`.
Re-scored **ALIGNED** for the shared-plan-with-CSE half of the row. Two
honest limits: the compiler's first refusal is on the *written* node count,
so a strategy that would fit once shared but exceeds the ceiling as written
is still refused — conservative by design, and unchanged here; and the
reference interpreter in the test covers only the operators its fixture
uses, so it is a specification of that fixture, not of the whole language.

**§16.4 (the gap map's "market simulation with adaptive agents" row;
§15.3 in the numbered source text) re-scored after the counterparty agents
(2026-09-04, working tree above `5ae86ce`): PARTIAL, closer — and NOT
calibrated, by construction.** No row for this element existed in this file
before; the verdict lived only in `docs/plan/blueprint-v10.1-gap-map.md`
(paraphrased: `SyntheticMarket`, `MarketSimulator` and `SimulationRun`
existed, and nothing typed the five named counterparty behaviours). That
verdict is no longer true. `qip-simulation-engine/src/agents.rs` types the five the blueprint
names — `Behaviour::Passive`, `::Informed`, `::Momentum`, `::Competitor`,
`::Maker`, each constructed only through a validating constructor on
`CounterpartyAgent` and each with its flow rule and the failure it models in
its doc — as deterministic order-flow generators inside the existing
synthetic market: `MarketSimulator::with_agents` generates the whole flow
once from the run seed, the path and the condition schedule (regenerated by
`with_conditions`, since every agent withdraws under any injected
condition), holds the agents in a `BTreeMap` by name so declaration order
cannot reach the flow, and `build_book` puts each step's flow through the
book it is asked for — takers sweep it, quotes rest behind every calm order
and never inside the calm touch — so the depth a strategy finds is the depth
the agents left. An agent reads the path only through `PathWindow`, which
refuses any observation whose `known_at` lies beyond the instant its
declared horizon reaches; four of the five hold a horizon of zero, the
informed agent holds the one it was built with and is refused one step past
it, and `CounterpartyAgent::act` refuses a window wider than the agent's
own declaration before any read. **What the record says about
calibration:** every `SimulationRun` — agents or none — carries
`flow_calibration: FlowCalibration`, an enum with the single variant
`NotCalibrated`, serialised as the sentence
`NOT_CALIBRATED_STATEMENT` ("synthetic counterparty behaviour, not
calibrated against real fills: none exist") and refusing to decode from any
other sentence; the run's `agents` and `counterparty_flow` are in
`SimulationRun::digest` and its `summarise` repeats the statement. The
blueprint's own line — "uncalibrated, it is confident expensive error" —
is why the variant that would claim calibration does not exist: adding it
is the moment someone must produce the real fills, and ADR 0003 means there
are none. TESTED in `qip-simulation-engine/tests/agents.rs`, eleven tests,
among them
`::a_momentum_agents_flow_follows_the_trailing_return_and_a_passive_agents_does_not`
(premise asserted: the fixed path has ≥30 rising, ≥30 falling and ≥30 flat
lookback returns; momentum flow correlates >0.8 with the trailing return
and passive flow |r|<0.3),
`::a_momentum_agents_take_leaves_a_hole_in_the_book_the_strategy_trades_into`
(the flow reaches the book: ask depth is the calm depth less the clip),
`::an_informed_agent_leans_toward_the_planted_move_only_within_its_horizon`,
`::an_agent_reading_a_bar_before_its_known_at_is_refused`,
`::every_agent_withdraws_under_an_injected_condition` and
`::the_run_record_names_its_agents_and_states_that_the_flow_is_not_calibrated`
(a record forged to say "calibrated against real fills" does not decode).
Eleven mutations fired, among them momentum's rule swapped for passive's
coin flip, the leakage refusal removed, `apply_flow` dropped from
`build_book`, the calibration sentence altered, and the maker's skew put on
the wrong side — the last of which survived the first version of its test,
which counted widened sides without tying each to the inventory's sign, and
fires now that the test checks the side per quote. **What keeps this
PARTIAL:** the blueprint's agents are *adaptive* — they respond to the
platform's own orders — and these do not: the flow is a pure function of
the seed and the path, generated before the strategy places anything, so
the competitor races the signal and not the strategy's actual footprint,
and the maker's inventory is the other agents' flow, not the strategy's.
Reactive flow needs the book to stop being a function of the instant alone,
which is a design decision about `MarketSimulator::execute`, not an addition
to this module. Calibration against real fills is structurally refused
rather than outstanding, for the same reason as the End-of-Phase-2 row:
there are no real fills and, under ADR 0003, will not be. The gap map's
row is not edited here; it is outside this slice's owned paths.

## Re-score at `b3ebc7f` (2026-09-04) — §29 The Quote Loop, mechanism half only

Appended rather than folded in, per this file's convention. Every claim below
was checked at HEAD in the session that wrote it. This re-scores **only piece
(1)** of the §29 row's minimal action — wiring `Repricer` into
`qip-edge-node`'s pass loop — and leaves piece (2), the quoting strategy
family, exactly where the row put it: absent at every layer.

The row's evidence was the module's own sentence at
`backend/crates/edge/qip-routing/src/reprice.rs:19`: "Nothing here sends
anything, and nothing is wired yet — deliberately." The seam it documents
(lines 17-33: caller is the node's loop, once per book update, after the
gateway's events have been drained into the parent) now has that caller.
`backend/crates/apps/qip-edge-node/src/reprice.rs` (new) holds a `Requoter`
(`:239`) and a `RequotingPlacer` (`:585`); `run_pass` in
`qip-edge-node/src/pass.rs` builds the placer over the simulated gateway at
`:117`, drains the venue's reports through it, withdraws what has expired,
checks the halt at `:149`, and only then calls `reprice` at `:161` — before
`Cell::work`, so the pass judges staleness against the book the feed just
published and against fills already booked. The cell has no per-order cancel
and no replacement placement of its own, and `cell.rs` was not touched
(another agent held it): the wiring sits beneath the cell's `Placer` seam. A
repriced intention is a `ParentOrder` whose children are the venue-level
orders; the venue sees the cell's id for the original and a fresh
`<id>-cN` for each replacement (the simulated exchange refuses a reused
client id, and `reprice.rs` says why a real venue would dedupe one), and the
wrapper maps the fresh id back to the cell's on every channel the cell
reads — execution reports, cancels, the drop copy — so the cell keeps one id
per intention and the reconciler compares the same fill on both channels.
What is refused rather than guessed: a cancel the venue refuses leaves the
order standing (`Requote::CancelRefused`); a cancel whose acknowledged
remainder disagrees with what the drain booked mints no replacement
(`Requote::CancelDisagreed`) and leaves the fill for the drop copy to
surface; a replacement the venue rejects releases its quantity to the parent
and is reported, not retried (`Requote::ReplacementRefused`). The policy is
declared, not defaulted: `QIP_REPRICE=<tick>:<ticks>:<bps>`
(`reprice.rs:80`, `parse_reprice` at `:91`), validated by
`RepricePolicy::validate` at start-up, refused when set on a node with no
feed (`main.rs`, `NodeConfig::from_env`), and named in the production
requirements when unset. One series was added to `qip_edge::CellMetrics`:
`qip_edge_orders_repriced_total{venue}` (`qip-edge/src/telemetry.rs:58`,
recorded at `qip-edge-node/src/reprice.rs:558` on the seam where the
replacement reached the venue, and nowhere for a refused cancel or
replacement); the health JSON's `pass` block gained `repriced`.

Proof, in `backend/crates/apps/qip-edge-node/tests/pass.rs`, each premise
asserted first:
`a_node_pass_reprices_a_stale_resting_child_after_draining_gateway_events_first`
(`:811`; premise: one order the venue holds open, no fill pending, the
series at zero; then one `Replaced` outcome, the venue's own record no
longer holding the original and holding the replacement, the venue's
working count exactly the replacement plus what the pass itself sent, the
cell still holding one open order under its own id with the replacement's
id nowhere in its record, the series at one; then an aggressor through the
replacement and the next pass confirming that fill under the cell's id on
both channels with no break);
`a_fill_that_arrived_this_pass_is_booked_before_staleness_is_judged`
(`:1005`; premise: a partial fill of one share against a sized order of
3.75 and a bid past the threshold, both waiting for the next pass; then the
fill confirmed on that pass and the replacement carrying exactly the
remainder, 2.75);
`a_fresh_resting_child_is_not_repriced` (`:1095`; premise: the cell's own
book shows the bid two ticks above the resting price; then no outcome, the
venue still holding the same order, the series still at zero). A unit test
covers the configuration form. Five mutations were applied, each fired, and
each file was restored and hashed: the node skipping the venue cancel
(test 1 failed at "the venue still holds the stale order open beside its
replacement"); `reprice` moved before the drain in `run_pass` (test 2 failed
at the fill-confirmed premise, the cancel having disagreed with the unbooked
remainder); `if !stale_by_ticks && !stale_by_bps` replaced by `if false` in
`qip-routing/src/reprice.rs` (test 3 failed at "an order inside the drift
thresholds was touched"); the recording site removed (test 1 failed at the
series assertion); `parse_reprice` skipping `validate` (the unit test failed
on the zero-tick refusal). Restored hashes: `qip-edge-node/src/reprice.rs`
`cf3d0719048c1c48841240abcaf230645750cc98073dbfdcf2c4b3dd56b7a598`,
`qip-edge-node/src/pass.rs`
`52fa843d254eeec4ad072a2d66641945a3e2135846b3f40f090bd5c3628a7831`,
`qip-routing/src/reprice.rs`
`196381d71fe39675cf27db009cdde965322c4ed78fefa6ec37f92f5ba8baa903`.

Re-scored **PARTIAL** for the row as a whole: piece (1) is closed for the
resting-child case the node already had — an order sent under
`PricingPolicy::RestAtMid`, which is the only order the cell holds with a
time to live — and piece (2) is untouched. Honest limits, stated so nobody
reads more into the re-score than it says. The mechanism reaches a running
process only where `Cell::work` does, which is a node with
`QIP_VENUE_FEED=simulated` and `QIP_REPRICE` set, and no node is deployed
(`execution_nodes = {}` everywhere). `Repricer`'s per-order budget map in
`qip-routing` grows by one entry per order ever repriced and is pruned by
nothing; over a long session that is a bounded-retention question for the
routing crate's owner, not a defect this wiring introduced. A halted node
reprices nothing by the placement of the call after the halt check, and
that property is held by the code's order rather than by a test of its own.
Nothing here quotes two sides, skews for inventory, or creates a market;
the §29.3 gate has nothing to gate yet.

## Re-score 2026-09-05 — the working tree above `b42214b`

Appended rather than folded in, per this file's convention. Every claim below
was checked by reading the named file at the named line in the uncommitted
working tree of this date; the one gate run for this section is
`cargo test -p qip-acceptance --test documentation`, and no other test named
here was run by the session that wrote it. Where a paragraph says a test was
mutation-verified, that is the applying session's statement (in ADR 0039's
"Applied" section or the commit that landed the test), not this one's.

### The execution plane is MEASURED in-process — and nothing is deployed

Until this date every execution row above read "TESTED, MEASURED nowhere".
[`docs/ops/execution-measurements.md`](../ops/execution-measurements.md) is
the first set of figures: fourteen per-operation costs printed by the section
of `backend/crates/tests/qip-acceptance/tests/performance.rs` headed "the
execution capabilities" (`:1172-1807`), each behind a test that asserts its
premise before it reads a clock, and each pinned by
`performance.rs::the_execution_measurements_document_names_only_tests_this_file_holds_and_says_what_a_number_is_not`
(`:1898`), which refuses a row naming a test the file does not hold and an
edit that drops the document's caveats.

What the number is, in that document's own words, and repeated here because a
figure quoted without it is not a figure: **measured in-process, not in
deployment; nothing is deployed.** Release profile, a shared 4-core Linux
container, one thread, no venue, no network, no node, and
`execution_nodes = {}` in every environment. None of the rows says anything
about latency to a venue.

The rows this moves, from "TESTED, MEASURED nowhere" to **TESTED and MEASURED
in-process (not in deployment)**:

- **§2.2 Feasibility precedes profitability** (table row above): the edge
  gate at 0.31 µs/op over 200,000 intents, refusing exactly the off-lot half
  (`the_edge_feasibility_gate_costs_what_the_execution_measurements_say`,
  `:1476`); the central grid at 3.58 µs/op
  (`central_instrument_feasibility_costs_what_the_execution_measurements_say`,
  `:1230`). The row's other reason for PARTIAL — the central pre-trade path in
  `qip-execution-engine` has no feasibility step of its own beyond the
  instrument grid — is unchanged.
- **Plane 6 Execution** (table row and bullet): one `Cell::work` pass with its
  fill confirmed and its drop copy reconciled at 20.98 µs and 1.51 µs
  (`an_edge_work_pass_with_a_fill_and_its_drop_copy_costs_what_the_execution_measurements_say`,
  `:1294`); netting four intents to one order at 35.35 µs/pass (`:1353`); an
  internal cross booked and journaled at 27.50 µs/pass (`:1393`); a resting
  order's expiry through the venue cancel at 17.98 µs/pass (`:1436`);
  sequencing at 0.60 µs/message (`:1574`); line arbitration at 0.65 µs/unit
  (`:1615`); the journal chain at 2.62 µs/entry recorded, 1.90 verified,
  0.27 shipped (`:1752`); a two-leg group completing at 1.75 µs (`:1807`);
  central OMS submission through five pre-trade limits at 3.41 µs/op
  (`:1172`).
- **LAYER 4/7, §41.5**: verifying and applying a policy payload, sealing a
  chain entry each, at 23.26 µs/op
  (`verifying_and_applying_a_policy_payload_costs_what_the_execution_measurements_say`,
  `:1717`); verifying a capital envelope at 3.04 µs/op (`:1666`). The second
  halt wire (`ff86473`) is **not** among the fourteen and stays TESTED only.
- **F6**: a region reservation hold-and-commit pair at 0.14 µs/op through the
  shared mutex, one thread, one cell
  (`a_region_reservation_hold_and_commit_costs_what_the_execution_measurements_say`,
  `:1535`). Contention is not measured.

What stays "MEASURED nowhere", by that document's own "could not be measured"
list: `qip-routing` (`qip-acceptance` does not depend on it), the node's pass
loop (`qip-edge-node::run_pass`, an application crate), `Cell::scan_cycles`
(no whitelist producer feeds an installed desk in the suite),
`CentralPlane::ingest` as a seam of its own, and anything with a wire on it.
**LAYER 6/7's verdict does not move**: the node the module boots is still
TESTED in `qip-edge-node/tests/pass.rs` and measured on no machine, because
none exists.

### Plane 2 — the cognition read surface, and the typed precedent channel

**Served.** The Plane 2 sentence "exposed for the API and not yet served by a
route" is history. `qip-api/src/routes.rs` declares
`GET /cognition/self-model` (`:338`) and `GET /cognition/precedents` (`:347`)
at `Role::Viewer` and dispatches them at `:798` and `:809` through
`qip-api/src/self_model_views.rs` (`self_model` at `:82`, `precedents` at
`:132`), whose module comment states the two structural properties: the
application layer names no learning-engine type, and `accuracy` and
`calibrated` are both read off one call to the engine's own `estimate`, so
the body cannot show a number the engine declined to compute. The stated
`MINIMUM_SAMPLE` (`:39`) is a copy the route checks against every row and
answers 500 on drift rather than serving. The contract is
`backend/crates/apps/qip-api/ROUTES-COGNITION.md`. Proven in
`qip-api/tests/self_model_routes.rs`:
`every_cognition_route_is_a_viewer_get_that_answers_json_with_the_documented_keys`
(`:258`), `an_empty_platform_serves_empty_lists_and_still_states_the_minimum_sample`
(`:300`), `a_monitor_credential_is_refused_and_no_method_but_get_reaches_a_cognition_path`
(`:330`), `a_component_below_the_minimum_sample_reports_no_accuracy_and_is_not_calibrated`
(`:367`), `the_minimum_sample_the_body_states_is_the_count_at_which_the_engine_starts_reporting`
(`:430`), `a_precedent_reason_recorded_is_served_as_the_kernel_holds_it_and_in_its_order`
(`:479`). The console renders both, read-only, under a new "Cognition" nav
section (`frontend/portal/src/lib/nav.ts:149`): pages at
`frontend/portal/src/app/(portal)/cognition/{self-model,precedents}/page.tsx`
over `frontend/portal/src/lib/hooks/useCognition.ts` (`useSelfModel` at `:84`,
`usePrecedents` at `:92`), with Playwright coverage in
`frontend/portal/tests/cognition-self-model.spec.ts` and
`cognition-precedents.spec.ts` (two tests each, among them "the self-model
page renders rows as received, says a refused estimate is refused, and holds
no control that acts"). The frontend gates were not run by this session.

**The precedent reaches the panel through a type, not through `context`.**
The sentence "waits on a channel that is not also a matching key" is history:
`qip_agents::finding::BriefPrecedent` (`qip-agents/src/finding.rs:203`) holds
the `PrecedentDigest`, the nearest episode's cosine similarity, its outcome
against the claim and its age, with private fields; `BriefPrecedent::new`
refuses an age at or below zero (the point-in-time boundary the store already
refuses) and a similarity outside `[-1, 1]`; `AgentBrief::precedent` (`:314`)
is the field and `with_precedent` (`:353`) sets it and touches nothing else —
in particular not `context`. The kernel builds it in `brief_precedent`
(`qip-kernel/src/platform.rs:760`) and attaches it at `:3981`; a precedent the
brief refuses is reported as a stage problem and the panel is convened
without it. Proven by
`qip-agents/tests/agents.rs::a_precedent_attached_to_a_brief_leaves_the_context_the_lesson_matcher_reads_untouched`
(`:1061`) and `::a_precedent_knowable_at_or_after_the_question_is_refused_rather_than_briefed`
(`:1101`);
`qip-investment-agents/tests/organisation.rs::two_briefs_identical_except_for_the_precedent_produce_identical_convictions_and_confidence`
(`:1647`) — the control that says the panel can cite it and cannot count it;
and `qip-kernel/tests/episodic.rs::the_panel_is_briefed_on_the_recalled_precedent_through_the_typed_field_and_only_when_one_was_recalled`
(`:285`). What this does not change: the confidence arithmetic reads nothing
from the field, by the same argument as before, and the only text the type
yields is `BriefPrecedent::cite`, a sentence and not a record id.

### §41.5, F6 and the region grant — ADR 0039's first phase, by reference

ADR 0039's new "Applied" section is the record and this paragraph defers to
it on every point; what follows is the scorecard's reading of the same tree.

**What is applied.** `qip-kernel/src/central/regions.rs`: `RegionMembership`
(validated at construction — a non-positive grant, a cell filed under an
ungranted region and a blank name are each refused, `:70-98`), `RegionShare`
(`:125`, `manifest()` at `:160`), `RegionShares` (`:172`) and `partition`
(`:217`), which **refuses, never scales**, a plan whose cells' shares would
exceed a region's grant, and withholds a manifest — with the reason — from any
cell whose live grants already sum past its share; the entry point is
`CentralPlane::region_shares` (`central/plane.rs:977`). At the cell,
`qip-edge/src/reservation.rs` gives `RegionAllocation` (`:112`) and
`RegionTable` (`:465`) an `unfunded(ceiling)` constructor (`:176`, `:480`)
that opens at nothing, and `rebase(owner, share, sequence)` (`:234`, `:525`)
that bounds at `min(share, ceiling)`, refuses a sequence at or below the last
applied and a second owner, and reports a `Rebase` (`:141`) whose `deficit`
is non-zero rather than letting `free` go negative. `Cell::apply_policy`
calls `apply_region_share` (`cell.rs:1139`) after the swap; the share is the
sum of `gross_limit` over the verified, live envelopes the payload's
`capital_grants` manifest names — one fact from one source, and the deviation
from the ADR's explicit `region_share` field, which `qip-contracts` did not
gain. Every re-base is journaled as `Decision::RegionShareApplied`
(`qip-edge/src/journal.rs:171`). Tests, as the ADR names them:
`qip-kernel/tests/region_shares.rs` (`a_regions_shares_are_disjoint_and_sum_to_at_most_its_grant`
`:80`, `a_plan_whose_cells_exceed_a_regions_grant_is_refused_not_scaled` `:130`,
`a_cell_in_no_region_receives_no_share` `:165`,
`a_membership_that_files_a_cell_under_an_ungranted_region_is_refused` `:210`);
`qip-kernel/tests/central.rs::a_cells_manifest_names_only_grants_whose_gross_fits_its_share`
(`:2278`); `qip-edge/tests/region_table.rs`
(`two_cells_in_two_processes_under_disjoint_shares_cannot_together_exceed_the_regions_grant`
`:814`, `a_replayed_lower_sequence_cannot_widen_a_cells_share` `:928`,
`a_cell_absent_from_the_shares_books_nothing` `:977`,
`a_share_below_what_the_cell_already_committed_narrows_free_to_zero_and_journals_the_deficit`
`:1056`, `a_partitioned_cell_keeps_spending_within_its_last_share_until_its_envelopes_expire`
`:1104`).

**What is not, and why F6 stays PARTIAL.** `qip-api`'s `pending_policy`
still produces the `capital_grants` slot from every live grant's signature
(`qip-api/src/mesh.rs:667`) without calling `region_shares` — `grep -rn
region_shares backend/crates/apps` returns nothing — so no deployed centre
withholds a manifest or refuses a plan, and the partitioner has no production
caller; `qip-edge-node::assemble` still opens the table funded at the
operator's amount (`qip-edge-node/src/lib.rs:108`, `with_region_allocation`),
not `unfunded`; membership is an argument and nothing outside a test
constructs one (decision 3 of the ADR). The cross-process property this row
has wanted since `2fd254f` is therefore **built at both ends and joined at
neither in a deployment**. The §41.5 producer count is unchanged at three of
twelve — `capital_grants` (`mesh.rs:667`), `risk_envelope` (`:669`) and
`cycle_whitelist` (`:674`) — because the share travels inside a slot that was
already produced. The §6.2 table is unchanged by this; its self-model row is
corrected in place above.

### Plane 7 — the ledger plane as records and refusals under ADR 0021 (§37, §38, §43.3)

**A correction first.** The Plane 7 table row and bullet above say "no wallet,
no corridor, no transfer gate, no destination registry, no custody engine —
`grep` for each returns nothing". That was already history at `5546a24`,
before this wave: `qip-capital-fabric/src/{corridor,destination,gate,wallet,custody}.rs`
hold the §37.1 corridor lifecycle (`Corridor`, `CorridorCaps` `:125`,
`PermittedHours` `:64`), the §38.4 destination registry
(`DestinationRegistry` `:249`, `DestinationStatus` `:186`), the §37.3
transfer gate (`TransferIntent` `:90`, `StatedPurpose` `:48`,
`TransferHistory` `:158`), the §38.1/§38.3 wallet read model
(`HoldingObservation` `:158`, `LedgerView` `:206`, `TolerancePolicy` `:284`)
and the §37.4 custody policy (`CustodyPolicy` `:238`, `permits` at `:347`),
proven in `qip-capital-fabric/tests/corridor_and_gate.rs` (twenty-seven
tests, among them
`an_intent_that_satisfies_every_check_is_approved_naming_all_seven_in_order`,
`a_tripped_kill_switch_vetoes_everything`,
`the_proposer_cannot_review_their_own_corridor`,
`a_destination_is_unusable_until_twenty_four_hours_after_its_signature_and_usable_on_the_instant`)
and `tests/wallet.rs`
(`a_delta_at_tolerance_halts_exactly_that_venue_asset_and_no_other`). Every
one is a record or a refusal: the gate's admitted arm is
`qip_capital_fabric::gate::Approved`, which carries no way to execute, and
`security.rs::no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
(`:1014`) still holds the ADR 0021 line. The API serves them read-only under
`backend/crates/apps/qip-api/ROUTES-LEDGER.md` (`/ledger/users`, `/wallet`,
`/corridors`, `/transfer-gate`), and what `/wallet` and `/corridors` say today
is that nothing is held: `ledger_views::wallet` (`qip-api/src/ledger_views.rs:332`)
answers `assembled: false` with its reason and `corridors` (`:431`) answers
`held: false` for both registries, because the kernel constructs none of
these controls — `grep -rln 'TransferGate\|DestinationRegistry\|CustodyPolicy'
backend/crates/apps backend/crates/runtime` returns only `ledger_views.rs`.

**What this wave adds, in `qip-capital`'s ledger (§43.3).** `MandateRegistry`
(`qip-capital/src/ledger/registry.rs:59`): every user mandate admitted against
the desk's own mandate as a ceiling, term by term and in aggregate, each under
a `MandateId` of its own, an id seen twice refused with the holder named, and
a stored registry replayed through `register` on deserialisation so a record
that has gone bad is refused on the way back in. `InvestmentRequest`,
`InvestmentDecision` and `RefusedLimit` (`ledger/request.rs:33`, `:75`,
`:45`): a request admitted or refused by `UserLedger::admit` (`:119`) before
anything downstream exists, the refusal a variant a test can name, and the
decision serialisable but deliberately not deserialisable. `ProRataSplit`
(`ledger/book.rs:66`) from `UserLedger::pro_rata_shares` (`:378`): a fill
split across users by what each has at work, naming where the rounding
remainder went (`ledger/mod.rs`, the `pro_rata_shares` bullet), and
`journal_pro_rata` (`:452`) booking it. Proven in `qip-capital/tests/user_ledger.rs`:
`a_mandate_id_registered_twice_is_refused_naming_its_holder_and_nothing_is_recorded`,
`a_mandate_that_promises_more_than_the_desk_carries_is_refused_by_the_term_that_exceeds_it`,
`user_mandates_cannot_together_promise_more_capital_than_the_desk_holds`,
`a_stored_registry_that_has_gone_bad_is_refused_on_the_way_back_in`,
`an_investment_request_is_admitted_or_refused_by_the_named_limit_before_anything_is_funded`,
`the_same_request_against_the_same_books_gets_the_same_decision`,
`a_pro_rata_split_reconciles_to_the_fill_exactly_and_the_remainder_is_recorded_not_dropped`,
`a_pro_rata_split_follows_the_entitlements_and_the_largest_holder_takes_the_remainder`,
`a_fill_with_no_capital_at_work_behind_it_is_not_split_and_nothing_is_booked`.

**And in `qip-capital-fabric`: the control half is now reproducible from the
log alone.** `FabricJournal` (`qip-capital-fabric/src/journal.rs:682`;
`resume` at `:712`, `decide` at `:732`) writes every destination, corridor,
wallet and gate decision to the hash-chained event log as the `FabricCommand`
(`:376`) and its `Outcome` — refusals included — under
`Topic::ComplianceEvaluated` (`:442`) with producer `capital-fabric` (`:80`),
executing on a scratch copy and adopting the state only after the log has the
record. `replay` (`src/replay.rs:70`) rebuilds `FabricState` by re-running
each command and refuses, naming the position, a record out of sequence,
altered, undecodable, or whose recorded outcome disagrees with what the
control computes — a chain that verifies is not taken as a record that was
true. Proven in `qip-capital-fabric/tests/journal.rs`:
`a_state_rebuilt_from_the_journal_equals_the_live_state_after_a_mixed_sequence`
(`:365`), `a_tampered_record_is_refused_naming_its_position` (`:409`),
`a_record_out_of_sequence_is_refused_naming_its_position` (`:441`),
`a_record_whose_recorded_outcome_disagrees_with_the_control_is_refused_even_when_the_chain_verifies`
(`:465`), `replay_is_deterministic_across_two_runs_and_two_journals` (`:513`),
`the_journals_chain_rule_agrees_with_the_event_logs_own` (`:550`),
`a_journal_resumed_from_a_shared_log_rebuilds_its_state_and_chain_verifies_foreign_records`
(`:637`), `the_journal_adopts_a_decision_only_after_the_log_has_it` (`:725`).

**Verdict: PARTIAL, unchanged, and for reasons that are now narrower.** The
kernel still books every settlement's attribution to the desk whole —
`Platform::journal_to_desk` (`platform.rs:2194`, called from
`ingest_cell_report` at `:2177`) — and calls neither `pro_rata_shares` nor
`admit`; it constructs no `FabricJournal` and no `MandateRegistry` of its own
beyond the desk's (`grep -n 'MandateRegistry\|InvestmentRequest\|pro_rata'
backend/crates/runtime/qip-kernel/src/platform.rs` returns nothing). So the
§43.4 chain runs to one user, the §37/§38 controls decide only in tests, and
the fabric's journal has no writer in any binary. What moved is the shape of
the gap: every ledger-plane element the blueprint names now exists as a
record that can be replayed or a refusal that names its limit, and none of
them can move, sign or call out. That is the whole of what ADR 0021 permits,
and it is what the row now says.
