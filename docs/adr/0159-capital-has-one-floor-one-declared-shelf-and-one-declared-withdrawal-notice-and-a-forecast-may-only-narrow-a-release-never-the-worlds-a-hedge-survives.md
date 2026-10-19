# ADR 0159: Capital has one floor, one declared shelf and one declared withdrawal notice, and a forecast may only narrow a release, never the worlds a hedge survives

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises the structural choices below. It does
  not authorise any reserve fraction, any shelf capacity, any share of cash
  routed, any freshness or sample threshold, any billed data source, or any
  edit to a protected document; those are listed under "Not decided here" and
  are untouched. Nothing here moves money: ADR 0021 and ADR 0023 still refuse
  the withdrawal half of treasury.
- **Date**: 2026-10-19
- **Extends**: ADR 0125 (an advisory input subtracts and never enlarges), ADR
  0140 (a number that reaches a size is read off a `Forecast`), ADR 0130 (the
  simulated venue is the custodian and its statement is custody truth).
- **Keeps**: ADR 0003, ADR 0005 (confidence is arithmetic), ADR 0006, ADR 0021,
  ADR 0109 (no dependency is added), the survival waterfall's rule that it may
  only ever contract a budget.

## Context

Eleven register rows are PARTIAL on the capital side for the same reason the
cell side was (ADR 0155): a control or a router wants an input, and the tree
holds either no producer or two. Four things in the code decide how the rows
should be read, and one of them contradicts a row.

1. **The floor is already stated, once, by the survival waterfall.**
   `SurvivalGovernor::size` (`qip-kernel/src/survival.rs`) withholds margin,
   collateral, settlement, funding maturity, an emergency reserve, hedge
   capacity and a stressed-liquidity reserve before it releases opportunity
   capital, and it refuses a construction whose waterfall and the ledger's
   `deployable_capital` disagree. `qip_capital::treasury::CapitalBook` carries
   its own `floor`, its own four `Use`s (`Grant`, `Placement`, `Collateral`,
   `Yield`) and its own total. Each `Use` except `Yield` restates a fact an
   authority already holds: envelopes are issued by the allocator, collateral
   arrives through `LedgerObligations`, placements are region allocations. Two
   claims about one balance disagree eventually, and the waterfall's own
   module doc says where that disagreement should become a refusal.
2. **A hold cannot be a long hold.** `ReservationLedger` is the one ledger of
   holds against free cash, `deployable_capital` reads it, and
   `MAXIMUM_RESERVATION_VALIDITY` is a day, refused rather than truncated,
   because an unbounded hold pinned capital for a century in the case the
   module documents. Any routing of cash out of the free balance must be held
   there or `deployable_capital` and the router will disagree about the same
   cash.
3. **The hedge brain already separates weight from protection.** `hedge.rs`
   says the worlds are weighted equally and that equal weight "affects only the
   reported edge given up, never which worlds must be protected — every world
   past the floor is protected whatever its weight." And `tail_response` takes
   the *worst* world, a probability-free rule. Both are the shape a
   forecast-driven change has to preserve.
4. **Row RISK-4108's second half is stale.** "No binary declares candidates":
   `qip-api`, `qip-fastbrain` and `qip-deepbrain` each call
   `with_hedge_candidates(paper_hedge_candidates(..))`
   (`grep -rn 'with_hedge_candidates' backend/crates/apps --include=*.rs`).
   Only the first half stands: the worlds carry no calibrated weights.

Locate them with
`grep -n 'pub fn size\|pub struct SurvivalPolicy\|pub fn tail_response' backend/crates/runtime/qip-kernel/src/survival.rs`,
`grep -n 'pub struct CapitalBook\|pub enum Use' backend/crates/services/qip-capital/src/treasury.rs`,
`grep -n 'pub fn route_idle_cash\|pub struct YieldInstrument' backend/crates/services/qip-capital/src/bank.rs`,
`grep -n 'MAXIMUM_RESERVATION_VALIDITY\|pub fn reserve' backend/crates/services/qip-capital/src/reservation.rs` and
`grep -n 'pub struct DatedLadder\|pub fn available_by' backend/crates/services/qip-capital/src/dated_ladder.rs`.

## Decision

### A. One floor (CAPITAL-4075)

1. **The floor is what the waterfall withholds, and nothing else states it.**
   It is the sum of the tiers in `SizingBudget.survival` that are neither held
   cash nor opportunity: margin, collateral, settlement, funding maturity,
   emergency reserve, hedge capacity and stressed-liquidity reserve, computed
   every construction and reconciled to the ledger or refused. No other
   component carries a configured floor for the book. The mandate's
   `liquidity_floor` (what is investable for one user) and the exit-ladder
   liquidity limit are different questions and keep their names; neither is
   restated as this one.
2. **`CapitalBook` gets no kernel caller.** A kernel-side `CapitalBook` could
   only be a second ledger of grants, collateral and placements beside the
   three that exist, with a floor typed in beside the waterfall's. The type is
   not deleted here: deleting it removes its tests, which is a reviewed act
   that no row needs. If a future record gives it a kernel use, its floor is
   constructed from the waterfall and never configured, and this record says
   so first.
3. **The floor's values stay where they are**: the `SHIPPED_*` fractions and
   `SurvivalPolicy::from_settings`, whose rule is that a reserve is raised by
   configuration and never lowered. Unchanged and not decided (RISK).

### B. One shelf and one router (CAPITAL-4078, 1163, 1161)

4. **The yield shelf is an operator declaration in a committed file** beside
   `data/datasets/universe.json`, read by the composition roots (the only
   place configuration is read) and handed to the platform, validated at load.
   There is no licensed source and no custodian feed: a custodian statement or
   vendor shelf is a data subscription (BILLED, below). An absent shelf means
   nothing is routed; there is no default shelf. `InstrumentClass::Other` can be
   listed and is never routed, and there is no credit arm, so financing and
   credit cannot be expressed as a home for idle cash (ADR 0003, ADR 0021).
5. **The router's input is derived, not supplied.** `route_idle_cash` stops
   taking a free-standing `CapitalBook` and takes an `IdleCash` that can be
   built only from a waterfall report: the opportunity capital the cycle did
   **not** release, which is by the confidence governor's own decision capital
   no thesis may take this cycle. The router never touches `released`. A
   placement is a hold in the `ReservationLedger` with the instrument's
   realisation as its validity, so `deployable_capital` sees it with no
   reconciliation, and a `DatedLadder` entry under the same id. An instrument
   whose realisation exceeds `MAXIMUM_RESERVATION_VALIDITY` is refused at shelf
   load, naming the ceiling: it cannot be held honestly, and truncating its
   realisation would date cash earlier than it is available.
6. **The ladder is a view, rebuilt on use and never stored**, from the
   commitment book's dated calls, the active yield holds and declared notices
   (section C). The horizons are configuration with no default; without them
   the placement cannot be dated and is not made, which is what the router's own
   rollback already does when the ladder refuses.
7. **The routed share is a limit and has no default.** Today's router routes
   everything above the floor up to capacity. A maximum share of the
   unreleased capital that may be routed is an input the router requires; unset,
   routing is not armed. The call is made once per cycle at the end of DECIDE,
   from the budget just built, and journals a labelled paper record. It is
   still `NotAnOrder` for `qip-acceptance`'s `order_placement_paths` suite: the
   `.place(` inside `route_idle_cash` must stay the single such call there.
8. **CAPITAL-1161 and CAPITAL-1163 ride this section.** They close when 4078
   and 4079 close; credit stays excluded and the register records it as such
   once routing exists.

### C. One withdrawal signal (CAPITAL-4079)

9. **The signal is a dated notice declared by an authenticated operator**
   (`RedemptionNotice`: id, amount, effective instant, declarer, declared-at),
   journaled and replayable. The identity is of the kind
   `AutonomyController::request_change` takes, fresh within the same window,
   never an agent, a model output or a configuration value. The platform never
   processes a withdrawal (`WithdrawalEntitlement` has one variant, by ADR
   0021), so there is no event to observe; a notice is a record of an expected
   claim, an intent in ADR 0021's permitted column, and moves nothing. Mandate
   terms gain no field: a persisted record format is not changed to carry a
   fact only an operator holds. A later declaration with the same id supersedes
   an earlier one; nothing is inferred or aged out silently.
10. **A notice feeds the ladder as an outflow, and its one consumer is the
    router.** An instrument is routable only if its availability instant is at
    or before the earliest declared outflow; otherwise the placement is
    refused, naming the notice. This is restrictive, needs no number, and
    stops the specific failure the dated ladder exists for: cash parked where it
    cannot be had back before it is owed.
11. **A notice does not feed the survival reserve.** Raising the funding tier
    or the emergency reserve from a declared outflow is a change to a capital
    reserve and is not decided here (RISK).

### D. The emergency reserve is not a ledger account (CAPITAL-4087)

12. **The reserve stays a policy fraction of equity, derived by the
    waterfall.** A reserve nobody moved is not an observed fact, and the ledger
    books only observed facts (ADR 0153). Opening an account for it would
    assert a movement that never happened and invite a second claim about the
    same balance. What the ledger contributes is what it observes, margin,
    collateral and settlement through `LedgerObligations`. The remaining gap in
    that row, that no binary feeds `Platform::observe_ledger_obligations`, is
    the obligations-route row's work and is not decided here.

### E. Custody (RES-7099, COVERAGE-7168)

13. **Nobody asserts custody unavailable.** On a paper platform the venue is
    the custodian (ADR 0130 decision 4), so there is no third party to be down.
    The only honest source is the freshness of the position statement the
    simulated venue publishes; that reading, when ADR 0130's statement slice
    exists, is made by the node (which hosts the gateway) and handed to the cell
    as a `Capability::Custody` observation, narrowing only. An operator who
    wants trading stopped has the halt, which is stronger. No operator
    assertion route is built.
14. **No `CustodyAvailability` input is added to `TransferGate::assess`.** The
    gate is veto-only, takes every input from its caller and has no production
    caller; an argument with no producer is a gate that always passes. Until
    the statement reading exists the Custody row stays unobserved and
    contributes one to `dependency_multiplier`, as the degradation module
    already says on purpose.
15. **The Clock half of COVERAGE-7168 is done and the register text is stale.**
    `Cell::narrowing` observes `Capability::Clock`, `Feed` and `VenueSession`
    and the pass compounds the clock row; only Custody is unobserved. The
    chart series for the new rows wait on the protected observability
    document and are not decided here.

### F. A forecast narrows; it never shrinks the set of worlds (CAPITAL-10097, RISK-4108)

16. **No mapping from panel beliefs to a distribution over worlds is built.**
    A panel belief is a claim about an instrument with a point confidence (ADR
    0088 is not built); a `StressReport` world is a scenario with a shock and
    no probability. No function relates them, and writing one is assigning
    confidence (ADR 0005).
17. **A forecast influences capital only if it is calibrated.** Calibrated
    means its `Forecast.calibration` is `Calibration::Measured` over at least a
    stated number of resolved forecasts. The number has no default; unset, no
    forecast influences anything, which is today's behaviour (RISK).
18. **A forecast reaches a world only through a committed binding table**, world
    id to forecast target and outcome or threshold, as ADR 0149 does for rules.
    An unbound world keeps equal weight and stays protected.
19. **Three direction rules bind any consumer**, and each is a test any future
    slice must add with its mutation. (a) The set of worlds a hedge must survive
    (every world past the floor) is never reduced by a weight, however small.
    (b) A calibrated forecast may increase what is withheld, by adding a
    forecast-derived world to the stress report so that `worst_world` can only
    get worse; it never reweights the worst-world rule and so never lowers the
    stressed-liquidity reserve. (c) Weights may reorder hedge plans each of
    which already protects every world, changing the reported edge given up and
    nothing else.
20. **Equal weights stay until a binding and a calibrated source both exist.**
    Nothing is built now. The stale half of RISK-4108 is corrected in the
    register.

### G. The forecast contract states what it cannot measure (CONTRACT-3117)

21. **`DemandForecast` measures its split.** Its interval is a prediction
    interval from `linear_fit`: residual dispersion, fit uncertainty and a
    leverage term. Aleatoric is the residual component and epistemic the fit
    and leverage components, so the pair sums to the prediction variance the
    interval already encodes. Tail mass is the interval's own tail, at
    `(1 - coverage) / 2` below its lower bound. One model is consulted and its
    disagreement is zero, the lattice's convention for one model.
22. **The contract gains an explicit stated-unmeasurable arm for exactly two
    fields**, `regime_confidence_bp` and `expected_impact`, each carrying a
    required reason, at a bumped schema version that still decodes the old one.
    A demand forecast has no regime and no profit-and-loss of its own; stating
    that is honest, and a zero is a claim. The split, the tail mass and the
    distribution have no such arm: a producer that cannot state them produces no
    `Forecast`. A consumer never reads a size off an unmeasurable field and
    refuses it, naming the field (ADR 0125 decision 5's shape).
23. **`CashflowForecast` does not become a `Forecast`.** It is a schedule of
    dated, probability-weighted flows; its uncertainty is the flows' own
    probabilities, not a predictive distribution over a target. ADR 0140's
    closing caution applies: a quantity that is not a forecast needs its own
    record, not a distribution with zero variance. It stays a schedule and the
    register says so.

## Consequences

- The platform states its floor once and its router cannot be handed a number
  the waterfall did not release. The cost of any placement is visible in the
  next construction's free cash with no reconciliation step.
- Code slices: `IdleCash` and the re-based router with the reservation hold
  and the shelf loader (4078); the notice record, its declaration route and the
  router's refusal (4079); the contract arm and the `DemandForecast` split
  (3117); register wording for the rest. Every new test needs its mutation.
- `qip-capital/tests/treasury.rs` tests of `route_idle_cash` migrate to the new
  signature with the same assertions; no assertion is dropped.

## What it costs

- Routing is restricted to instruments realisable within a day. The longer
  ShortDuration instruments the blueprint names are not routable until a
  longer-lived hold has its own record. That is a real loss of yield on a
  paper book, accepted for one ledger of holds.
- A routed placement reduces free cash for up to a day. The share limit is
  the only lever on how much liquidity a cycle gives up, and it has no default.
- The notice path adds an operator route and a journaled record for a fact
  that moves nothing. Its only effect is refusing placements, so most days it
  does nothing visible.
- Two fields of `Forecast` become sum types. Every producer and reader of the
  contract compiles against the change, and the old schema must keep decoding.
- Waiting on a binding table for forecast-driven capital means no forecast
  changes a reserve for now. That is the cost of refusing to invent a mapping.

## What would make this wrong

- If the router starves DECIDE in practice, measured as cycles whose released
  capital was smaller than the thesis' need while a yield hold covering it was
  active, decision 5's use of unreleased capital is too generous and the
  share limit must be set low, or routing removed.
- If operators never declare a notice, decision 10 is inert and the ladder holds
  only commitments; the notice record should then be dropped rather than kept
  as a ritual.
- If a reserve ever has to be shown to an auditor as a ledger account, decision
  12 fails, and the answer is a derived report of the waterfall's tier, not a
  posting.
- If a calibrated forecast is found to lower a reserve through an interaction
  (for example by displacing the worst world), rule 19(b) was broken in code
  and the fix is the code.

## Dissent

**Let a calibrated forecast relax, not only tighten.** The strongest form: a
hedge that must survive every world past the floor, whatever its probability,
pays forever for protection against worlds a calibrated forecast says are
negligible. A platform that cannot spend that saving is systematically
over-hedged, and "only narrow" discards the information in the cheap direction.
It loses because the loss is asymmetric. A forecast wrong in the tightening
direction costs trades; wrong in the relaxing direction costs the survival
floor, and the calibration that would justify relaxing is exactly what a thin
sample cannot give in the tail. It would be revived by a long, scored record of
a forecast's tail probabilities against realised worlds, a new ADR naming the
loss bound, and a limit set by the RISK lane.

**Do not record a withdrawal notice for a platform that never pays one.** A
record of an outflow nothing will honour is theatre, and the survival reserve
is where a real withdrawal would matter. It loses because the record has a
mechanical consumer (the router refuses a placement that cannot be had back
before it is owed) and the claim it records exists in the real world even
though the platform will not act on it. It would be revived by a month in which
no notice is declared, which is decision 10's reversal condition above.

## Code it constrains

- `qip-kernel/src/survival.rs`: `SurvivalGovernor::size`, `SurvivalPolicy`,
  `tail_response`, `SizingBudget`.
- `qip-capital/src/bank.rs`: `route_idle_cash`, `YieldInstrument`,
  `InstrumentClass`; `qip-capital/src/treasury.rs`: `CapitalBook` (unwired);
  `qip-capital/src/reservation.rs`: `ReservationLedger`,
  `MAXIMUM_RESERVATION_VALIDITY`; `qip-capital/src/dated_ladder.rs`: `DatedLadder`.
- `qip-kernel/src/platform.rs`: `Platform::deployable_capital`,
  `observe_ledger_obligations`, `stage_decide`
  (`grep -n 'pub fn deployable_capital\|pub fn observe_ledger_obligations\|fn stage_decide' backend/crates/runtime/qip-kernel/src/platform.rs`).
- `qip-acceptance/tests/order_placement_paths.rs`: the `route_idle_cash` entry.
- `qip-kernel/src/hedge.rs`: the world weighting and protection rule;
  `qip-risk/src/hedge_search.rs`: `HedgeSearch`.
- `qip-contracts/src/forecast.rs`: `ForecastFields`, `Calibration`;
  `qip-capital-fabric/src/forecast.rs`: `DemandForecast`, `Interval`;
  `qip-world-model/src/lattice.rs` and `federation.rs` (the producers that
  compile against the amended contract).
- `qip-capital-fabric/src/gate.rs`: `TransferGate::assess` (unchanged);
  `qip-contracts/src/degradation.rs`: `Capability::Custody`.

## Rows unblocked

- R12-CAPITAL-4075: the floor is the waterfall's withheld tiers, stated once;
  `CapitalBook` has no kernel caller.
- R12-CAPITAL-4078: the shelf is a committed operator declaration; the router is
  re-based on a derived `IdleCash` and a reservation hold; unset share means not
  armed.
- R12-CAPITAL-4079: the signal is an operator-declared dated notice; it feeds the
  ladder and the router's refusal, not the reserve.
- R12-CAPITAL-1161: rides 4075, 4078 and 4079; closes with them.
- R12-CAPITAL-1163: rides the shelf; credit has no arm and is recorded excluded.
- R12-CAPITAL-4087: the emergency reserve stays a derived policy fraction, not a
  ledger account.
- R12-RES-7099: nobody asserts custody down; no gate input is added.
- R12-COVERAGE-7168: Clock is done; Custody awaits the venue statement reading;
  the chart series wait on a protected document.
- R12-CAPITAL-10097: no panel-to-world mapping; calibrated forecasts only add
  worlds and never shrink the protected set.
- R12-RISK-4108: worlds stay equally weighted until a binding and a calibrated
  source exist; candidates are already declared in three binaries.
- R12-CONTRACT-3117: `DemandForecast` measures its split; two fields gain a
  stated-unmeasurable arm; `CashflowForecast` stays a schedule.

## Not decided here

- **RISK**: the survival fractions and any change to them; the maximum share of
  unreleased capital that may be routed; every shelf capacity and realisation
  declared; the ladder horizons only if the RISK lane treats them as limits;
  whether a declared notice raises the funding tier or the emergency reserve and
  by how much; the minimum resolved-forecast count for a forecast to influence
  anything; the freshness threshold that turns an old custody statement into
  `Stale` or `Unavailable`; any reserve increase a forecast-derived world
  causes. (rows: R12-CAPITAL-4075, R12-CAPITAL-4078, R12-CAPITAL-4079,
  R12-CAPITAL-10097, R12-RISK-4108, R12-COVERAGE-7168)
- **BILLED**: a custodian statement feed or a licensed yield-shelf data source;
  both are subscriptions, so the shelf is hand-declared. (row: R12-CAPITAL-4078)
- **PROTECTED**: the chart series for the Custody and Clock rows, which wait on
  the observability rule document under `.claude/**`; the need is described, the
  edit is not decided. (row: R12-COVERAGE-7168)
- **paper-boundary**: any withdrawal, redemption payment or custodian call. A
  notice records a claim and moves nothing; ADR 0021 and ADR 0023 stand.
  (rows: R12-CAPITAL-4079, R12-RES-7099)
