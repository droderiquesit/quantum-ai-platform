//! What the platform did, what it declined, and what both cost it.
//!
//! Three composed capabilities are observed here, and each is observed through
//! an effect a public [`Platform`] call actually produced: the outcome capture
//! that records a refusal on the same hash chain as a fill, the demand
//! forecaster fitted from the fills the loop has seen, and the compute meter
//! that charges every cycle for having been run.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_capital_fabric::{DemandKind, RealisedDemand};
use qip_contracts::edge::DeductionKind;
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_cost_router::IntelligenceTier;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_simulation_engine::costs::CostModel;
use qip_twin::asof::TwinMarket;
use qip_twin::capture::Action;

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(object(symbol), symbol, InstrumentType::CommonStock)
                    .venue("XNYS")
                    .sector(Sector::InformationTechnology)
                    .price(dec!("100"))
                    .provenance(Provenance::synthetic("test", start()))
                    .build(start())
                    .expect("valid object"),
            )
            .expect("insertable");
    }
    universe
}

fn limits() -> LimitSet {
    LimitSet::new("kernel-test")
        .with(
            Limit::new(
                "max-position-weight",
                LimitKind::MaxPositionWeight { limit: 0.10 },
            )
            .with_rationale("no single name may dominate the book"),
        )
        .with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        )
}

/// A limit the book cannot satisfy: twice equity held in cash.
///
/// Deliberately unsatisfiable, because the property under test is what the
/// platform records when a control rules against it, and a control that never
/// rules cannot be observed doing so.
fn unsatisfiable_limits() -> LimitSet {
    LimitSet::new("kernel-test-breaching").with(
        Limit::new("min-cash-buffer", LimitKind::MinCashBuffer { limit: 2.0 })
            .with_rationale("a floor no book of this size can be over"),
    )
}

fn platform_with(config: PlatformConfig, limits: LimitSet) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits)
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    platform_with(config, limits())
}

/// Daily bars either side of the decision instant, so a counterfactual has
/// somewhere to enter and somewhere to exit.
fn twin_bars(symbol: &str, before: i64, after: i64) -> Vec<Bar> {
    let mut price = 100.0_f64;
    (0..(before + after))
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let open = price;
            price *= 1.0 + noise;
            let at = start().saturating_sub(Duration::from_days(before - i));
            Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: Decimal::from_f64(open).unwrap(),
                high: Decimal::from_f64(open.max(price) * 1.002).unwrap(),
                low: Decimal::from_f64(open.min(price) * 0.998).unwrap(),
                close: Decimal::from_f64(price).unwrap(),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Decimal::from_f64((open + price) / 2.0),
                quality: DataQuality::default(),
            }
        })
        .collect()
}

fn observations(symbol: &str, count: usize) -> Vec<SensedRecord> {
    twin_bars(symbol, count as i64, 0)
        .into_iter()
        .map(|bar| SensedRecord::Bar(Box::new(bar)))
        .collect()
}

/// Send one order that the controls accept, and hand back its id.
fn fill_one(platform: &mut Platform, at: Timestamp) -> Result<qip_core::ids::OrderId> {
    let order = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-1",
        vec!["hyp-1".to_string()],
        at,
    );
    let order_id = order.order_id.clone();
    platform.submit_order(order, at)?;
    Ok(order_id)
}

// --- the outcome capture ----------------------------------------------------

#[test]
fn a_fill_and_a_refusal_land_on_the_same_chain() -> Result<()> {
    // The audit's finding: a platform that records only its trades can say what
    // it earned and not what it declined. Both go on one chain, counted by one
    // tally, or the second number does not exist.
    let mut platform = platform(PlatformConfig::default())?;
    assert!(platform.outcomes().is_empty());

    fill_one(&mut platform, start())?;

    // An order tracing to no hypothesis is refused by the control path.
    let untraceable = platform.order_from(
        object("AAA"),
        Side::Buy,
        dec!("1000"),
        dec!("100"),
        "prop-2",
        Vec::new(),
        start(),
    );
    assert!(platform.submit_order(untraceable, start()).is_err());

    let capture = platform.outcomes();
    capture.verify()?;
    assert!(
        !capture.taken().is_empty(),
        "the accepted order must be on the chain: {:?}",
        capture.tally()
    );
    assert!(
        !capture.refusals().is_empty(),
        "and so must the refused one: {:?}",
        capture.tally()
    );

    // The refusal names the control that produced it, so refusals can be
    // counted by cause rather than by however the message was worded.
    let refusal = capture
        .refusals()
        .into_iter()
        .find(|entry| matches!(entry.decision.action, Action::Rejected { .. }))
        .expect("a rejected order");
    let Action::Rejected { gate, .. } = &refusal.decision.action else {
        unreachable!("filtered above")
    };
    assert!(
        !gate.is_empty(),
        "a refusal with no named gate cannot be tallied"
    );

    // Nothing moved on a refusal, and the row exists anyway: a refusal with no
    // outcome row is indistinguishable from a decision nobody made.
    assert_eq!(refusal.outcome.realised_pnl(), Decimal::ZERO);
    assert_eq!(refusal.outcome.filled_quantity(), Decimal::ZERO);
    Ok(())
}

#[test]
fn the_capture_chain_verifies_across_every_kind_of_record() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.observe(observations("AAA", 120));
    for step in 0..3 {
        let at = start().saturating_add(Duration::from_mins(5 * step));
        fill_one(&mut platform, at)?;
        platform.run_cycle(at);
    }
    platform.outcomes().verify()?;
    assert!(
        platform.outcomes().len() >= 6,
        "{:?}",
        platform.outcomes().tally()
    );
    // Everything on the chain is traceable, which is what makes it
    // reconstructable after the fact.
    for entry in platform.outcomes().entries() {
        assert!(!entry.trace().as_str().is_empty());
        assert!(!entry.digest.is_empty());
    }
    Ok(())
}

#[test]
fn a_risk_control_that_rules_against_the_book_is_recorded_during_a_cycle() -> Result<()> {
    // The refusal path inside `run_cycle`. A control that ruled and left no
    // record is a refusal nobody can count.
    let mut platform = platform_with(PlatformConfig::default(), unsatisfiable_limits())?;
    let report = platform.run_cycle(start());

    let act = report.stage(Stage::Act).expect("act ran");
    assert!(
        !act.problems.is_empty(),
        "a blocked book must say so: {}",
        act.detail
    );
    let ruling = platform
        .outcomes()
        .refusals()
        .into_iter()
        .find(|entry| {
            matches!(
                entry.decision.action,
                Action::RiskDecision { allowed: false, .. }
            )
        })
        .map(|entry| entry.decision.clone());
    let ruling = ruling.expect("the monitor's ruling must be on the chain");
    assert_eq!(ruling.correlation, report.correlation_id);
    assert!(!ruling.rationale.is_empty());
    platform.outcomes().verify()?;
    Ok(())
}

#[test]
fn the_learn_stage_reports_what_was_captured() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let quiet = platform.run_cycle(start());
    assert!(
        !quiet
            .stage(Stage::Learn)
            .expect("learn ran")
            .detail
            .contains("captured"),
        "an empty capture is not worth a clause"
    );

    fill_one(&mut platform, start())?;
    let report = platform.run_cycle(start());
    let detail = &report.stage(Stage::Learn).expect("learn ran").detail;
    assert!(detail.contains("outcome(s) captured"), "{detail}");
    assert!(detail.contains("taken"), "{detail}");
    Ok(())
}

#[test]
fn the_alternatives_to_a_trade_are_priced_and_stay_simulated() -> Result<()> {
    // Everything a counterfactual produces is `Simulated`, and there is no
    // conversion out of it. The realised total is therefore unchanged by
    // evaluating any number of alternatives — which is the property that stops
    // a twin from becoming a machine for flattering the P&L.
    let mut platform = platform(PlatformConfig::default())?;
    let order_id = fill_one(&mut platform, start())?;
    let realised_before = platform.outcomes().realised_pnl();

    let mut market = TwinMarket::new(twin_bars("AAA", 90, 30), CostModel::liquid_equity(), 20)?;
    let set = platform.evaluate_alternatives(&order_id, &mut market)?;

    assert!(!set.is_empty(), "the menu produces alternatives");
    assert_eq!(set.decided_at, start());
    assert!(
        set.by_kind("do_not_trade").is_some(),
        "not trading is always an alternative"
    );
    assert_eq!(
        platform.outcomes().realised_pnl(),
        realised_before,
        "pricing an alternative cannot move realised P&L"
    );
    // The forgone total is carried in the type that cannot join the realised
    // line, and it is still nil here because nothing was recorded as missed.
    assert!(
        platform.outcomes().forgone().as_f64_for_statistics().abs() < f64::EPSILON,
        "nothing was recorded as declined, so nothing was forgone"
    );
    Ok(())
}

#[test]
fn an_order_the_platform_never_sent_has_no_alternatives_to_price() -> Result<()> {
    let platform = platform(PlatformConfig::default())?;
    let mut market = TwinMarket::new(twin_bars("AAA", 90, 30), CostModel::liquid_equity(), 20)?;
    let error = platform
        .evaluate_alternatives(
            &qip_core::ids::OrderId::from_string("ord-never"),
            &mut market,
        )
        .unwrap_err();
    assert!(
        error.message().contains("nothing to counterfact"),
        "{error}"
    );
    Ok(())
}

// --- the capital fabric -----------------------------------------------------

#[test]
fn a_fill_is_the_demand_observation_the_forecaster_is_fitted_on() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    assert!(platform.demand_lanes().is_empty());
    assert!(
        platform
            .forecast_capital_demand(start(), Duration::from_days(1))
            .is_empty(),
        "a lane the fabric has never observed gets no forecast rather than a default one"
    );

    fill_one(&mut platform, start())?;
    let lanes = platform.demand_lanes();
    assert_eq!(lanes.len(), 1, "{lanes:?}");
    assert_eq!(lanes[0].1, DemandKind::Cash);

    // Recorded at `fill.at`, not the order's submission instant: the
    // simulated broker's default latency (50ms) lands the fill after
    // `start()`. Forecasting from `start()` itself would be asking about an
    // instant before the observation existed — exactly the leakage
    // DemandForecaster::forecast now refuses — so this reads from a moment
    // that has actually seen the fill, the same guarantee the real DECIDE
    // stage has structurally: its own fills always land in ACT, later in the
    // same cycle whose `now` it already fixed.
    let after_the_fill = start().saturating_add(Duration::from_millis(50));
    let forecasts = platform.forecast_capital_demand(after_the_fill, Duration::from_days(1));
    assert_eq!(forecasts.len(), 1);
    let interval = forecasts[0].interval();
    assert!(
        interval.width().is_positive(),
        "a point forecast wearing an interval's clothes is the failure this floor prevents"
    );
    assert!(interval.lower() <= interval.point() && interval.point() <= interval.upper());
    assert!(forecasts[0].needed_by() > after_the_fill);
    Ok(())
}

#[test]
fn the_decide_stage_reports_where_capital_will_be_needed() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    let quiet = platform.run_cycle(start());
    assert!(
        !quiet
            .stage(Stage::Decide)
            .expect("decide ran")
            .detail
            .contains("funding lane"),
        "an unobserved book forecasts nothing"
    );

    fill_one(&mut platform, start())?;
    // Same reasoning as the forecaster test above: the fill lands 50ms after
    // `start()` (the simulated broker's default latency), so the cycle that
    // is meant to see it in DECIDE has to run at or after that instant.
    let after_the_fill = start().saturating_add(Duration::from_millis(50));
    let report = platform.run_cycle(after_the_fill);
    let detail = &report.stage(Stage::Decide).expect("decide ran").detail;
    assert!(detail.contains("funding lane(s) forecast"), "{detail}");
    Ok(())
}

#[test]
fn a_pre_positioning_plan_is_scored_against_what_the_world_needed() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    fill_one(&mut platform, start())?;
    fill_one(
        &mut platform,
        start().saturating_add(Duration::from_days(1)),
    )?;

    let horizon = Duration::from_days(1);
    let at = start().saturating_add(Duration::from_days(2));
    let plan = platform.pre_position(at, horizon)?;
    assert!(
        plan.is_within_budget(),
        "a plan is checked against the allocator's live limits: {}",
        plan.describe()
    );

    // Score it against a world where nothing was needed after all. A plan that
    // moved nothing scores exactly zero; one that moved capital nobody wanted
    // scores below it. Either is a real answer, and "unscored" is not.
    let score = platform.evaluate_pre_positioning(&RealisedDemand::new(), at, horizon)?;
    assert_eq!(score.at, at);
    assert_eq!(score.lanes.len(), plan.lanes.len());
    assert!(score.net_value <= Decimal::ZERO, "{}", score.describe());

    // And against a world that needed exactly what was forecast.
    let forecasts = platform.forecast_capital_demand(at, horizon);
    let mut realised = RealisedDemand::new();
    for forecast in &forecasts {
        realised = realised.with(
            forecast.location.clone(),
            forecast.kind,
            forecast.interval().point(),
        );
    }
    let scored = platform.evaluate_pre_positioning(&realised, at, horizon)?;
    assert!(
        scored.net_value >= score.net_value,
        "meeting the demand cannot score worse than missing it: {} against {}",
        scored.describe(),
        score.describe()
    );
    Ok(())
}

// --- what the platform charges itself ---------------------------------------

#[test]
fn a_cycle_charges_what_it_consumed_and_the_total_only_grows() -> Result<()> {
    // An opportunity that earns less than it cost to find is not an
    // opportunity. That statement is only checkable if somebody is metering.
    let mut platform = platform(PlatformConfig::default())?;
    assert_eq!(platform.compute_spend(), Decimal::ZERO);
    assert!(platform.cycle_ledger().is_none());
    assert!(platform.cost_deductions().is_err());

    platform.run_cycle(start());
    let first = platform.compute_spend();
    assert!(first.is_positive(), "eight stages are not free");
    assert_eq!(platform.last_cycle_cost(), first);

    let ledger = platform.cycle_ledger().expect("a cycle has run");
    assert_eq!(
        ledger.count(IntelligenceTier::DeterministicCode),
        8,
        "one deterministic pass per stage that ran: {}",
        ledger.basis()
    );

    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    assert!(
        platform.compute_spend() > first,
        "the running total is monotone"
    );
    Ok(())
}

#[test]
fn a_busier_cycle_costs_more_than_a_quiet_one() -> Result<()> {
    let quiet = {
        let mut platform = platform(PlatformConfig::default())?;
        platform.run_cycle(start());
        platform.last_cycle_cost()
    };
    let busy = {
        let mut platform = platform(PlatformConfig::default())?;
        platform.observe(observations("AAA", 120));
        platform.run_cycle(start());
        platform.last_cycle_cost()
    };
    assert!(
        busy > quiet,
        "a cycle that convened the organisation and resampled a path cost {busy}, \
         a blind one cost {quiet}"
    );
    Ok(())
}

#[test]
fn the_cycle_produces_the_two_deductions_the_platform_charges_itself() -> Result<()> {
    // Seven of a `NetEdge`'s nine deductions are charged by the market. These
    // two are charged by the platform to itself, and this is what fills them.
    let mut platform = platform(PlatformConfig::default())?;
    platform.run_cycle(start());

    let (compute, data) = platform.cost_deductions()?;
    assert_eq!(compute.kind, DeductionKind::ComputeCost);
    assert_eq!(data.kind, DeductionKind::DataCost);
    assert_eq!(compute.amount, platform.last_cycle_cost());
    assert!(
        !compute.basis.is_empty() && !data.basis.is_empty(),
        "a zero deduction with a basis is a claim somebody can argue with; \
         an absent one is indistinguishable from a cost nobody thought about"
    );
    assert!(compute.amount.is_positive());
    Ok(())
}

#[test]
fn the_journal_records_what_the_cycle_cost() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    platform.run_cycle(start());
    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));

    let entries = platform.journal_entries()?;
    let total: Decimal = entries
        .iter()
        .map(|entry| entry.compute_cost)
        .fold(Decimal::ZERO, |sum, cost| sum + cost);
    assert_eq!(
        total,
        platform.compute_spend(),
        "the journal and the running total must agree"
    );
    Ok(())
}
