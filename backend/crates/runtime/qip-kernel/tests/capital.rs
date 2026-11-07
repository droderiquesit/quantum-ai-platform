//! The capital state the control functions watch.
//!
//! Until this change the risk monitor ran every cycle against a hardcoded
//! `equity = cash = 10M` — real-time in cadence, constant in content — and
//! the decide stage sized against the same literal. These tests pin the new
//! truth: equity starts at the configured book, moves with realised fills and
//! nothing else, and is what the decide stage reads. Realised-only is the
//! stated contract — the platform holds no marks, so unrealised P&L is
//! deliberately excluded rather than fabricated.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

/// The liquidity every fixture in this file states, because nothing states it
/// for them any more.
///
/// [`qip_financial::costs::LiquidityProfile`] has no `Default`: the one it had
/// asserted a 10bp quote and a one-session exit for any instrument at all, and
/// `MinLiquidity` and `MaxDaysToLiquidate` — controls whose job is to veto
/// trading — read exactly those two figures. A fixture may state its own
/// premise; it may not inherit one nobody wrote down. A liquid listed name on
/// five million units a day, quoted at three basis points.
fn fixture_liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(qip_core::Decimal::from_int(5_000_000), 3.0)
}

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object("AAA"),
                "AAA",
                InstrumentType::CommonStock,
                fixture_liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid object"),
        )
        .expect("insertable");
    universe
}

/// The conservative default, exactly as it ships.
///
/// This fixture used to strip `MaxConcentration` from the set, because these
/// tests need the first order into an empty book admitted and a
/// share-of-gross cap cannot grant that — the first position in any book is
/// the whole of gross, so the cap read 100% and refused it at every size.
///
/// ADR 0027 settled that by making the default caps a share of *equity*, so
/// the exemption is no longer needed. It is removed rather than left as a
/// harmless no-op: `conservative_default` holds no `MaxConcentration` today,
/// so the `retain` stripped nothing while still telling a reader that these
/// tests run against a reduced set. A fixture that removes a default limit
/// to pass is a fixture testing a set nobody deploys, and one that only
/// appears to is worse — it survives the limit coming back.
fn limits() -> LimitSet {
    LimitSet::conservative_default()
}

fn platform(config: PlatformConfig) -> Result<Platform> {
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe(), limits())
}

/// Submit a traceable order through the full control path.
fn submit(platform: &mut Platform, side: Side, quantity: Decimal, price: Decimal) -> Result<()> {
    let order = platform.order_from(
        object("AAA"),
        side,
        quantity,
        price,
        "prop-capital",
        vec!["hyp-capital".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// Total quantity filled so far, from the order manager's own record.
fn filled_quantity(platform: &Platform) -> Decimal {
    platform
        .orders()
        .fills()
        .iter()
        .fold(Decimal::ZERO, |sum, fill| sum + fill.quantity)
}

#[test]
fn equity_starts_at_the_configured_book_and_an_opening_fill_costs_exactly_its_costs() -> Result<()>
{
    let mut platform = platform(PlatformConfig::default())?;
    // The premise: before any fill, equity is the configured book — the same
    // ten million the old constant claimed, now as a starting point rather
    // than a permanent truth.
    assert_eq!(platform.equity(), Decimal::from_int(10_000_000));
    assert_eq!(platform.trading_costs(), Decimal::ZERO);

    submit(&mut platform, Side::Buy, dec!("1000"), dec!("100"))?;

    // Opening a position realises nothing; the only money that moved is what
    // the fills cost. The simulated venue charges commission, so the premise
    // that something moved is itself asserted.
    let costs = platform.trading_costs();
    assert!(
        costs.is_positive(),
        "the simulated venue charged no commission, so this test cannot see equity move"
    );
    assert_eq!(platform.realised_pnl(), Decimal::ZERO);
    assert_eq!(
        platform.equity(),
        Decimal::from_int(10_000_000) - costs,
        "an opening fill must cost exactly its costs — anything else is an invented mark"
    );
    Ok(())
}

#[test]
fn closing_a_position_above_its_entry_realises_a_gain_the_monitor_can_see() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    submit(&mut platform, Side::Buy, dec!("1000"), dec!("100"))?;
    let bought = filled_quantity(&platform);
    assert!(
        bought.is_positive(),
        "the premise: the buy actually filled something to close"
    );

    // Sell exactly what filled, 10% higher. The venue's slippage moves the
    // realised prices a few basis points; a 10% gap dwarfs it, so the sign of
    // the realised P&L is a property, not a coincidence.
    submit(&mut platform, Side::Sell, bought, dec!("110"))?;

    assert!(
        platform.realised_pnl().is_positive(),
        "selling above entry realised {}",
        platform.realised_pnl()
    );
    // The accounting identity the tracking promises: equity is the initial
    // book plus realised P&L minus costs, exactly — positions at cost mean
    // no other term exists.
    assert_eq!(
        platform.equity(),
        Decimal::from_int(10_000_000) + platform.realised_pnl() - platform.trading_costs(),
        "equity drifted from its own definition"
    );
    Ok(())
}

#[test]
fn the_same_fills_produce_the_same_equity() -> Result<()> {
    // Determinism is the property that makes the risk numbers auditable: a
    // replay of the same orders must watch the same equity, with no clock and
    // no ambient randomness anywhere in the tracking.
    let run = || -> Result<(Decimal, Decimal)> {
        let mut platform = platform(PlatformConfig::default())?;
        submit(&mut platform, Side::Buy, dec!("1000"), dec!("100"))?;
        let bought = filled_quantity(&platform);
        submit(&mut platform, Side::Sell, bought, dec!("110"))?;
        Ok((platform.equity(), platform.realised_pnl()))
    };
    assert_eq!(run()?, run()?);
    Ok(())
}

#[test]
fn the_decide_stage_sizes_against_the_tracked_equity_not_a_constant() -> Result<()> {
    let mut platform = platform(PlatformConfig::default())?;
    submit(&mut platform, Side::Buy, dec!("1000"), dec!("100"))?;
    let bought = filled_quantity(&platform);
    submit(&mut platform, Side::Sell, bought, dec!("110"))?;
    let tracked = platform.equity();
    // The premise: the fills moved equity off the old constant, so the
    // assertion below can tell the tracked number from the hardcoded one.
    assert_ne!(
        tracked,
        Decimal::from_int(10_000_000),
        "the fills left equity exactly at the old constant; this run proves nothing"
    );

    platform.run_cycle(start().saturating_add(Duration::from_mins(5)));
    let proposal = platform
        .proposals()
        .last()
        .ok_or_else(|| Error::not_found("the cycle's proposal"))?;
    assert_eq!(
        proposal.equity.amount, tracked,
        "the decide stage sized against something other than the book the fills produced"
    );
    Ok(())
}

#[test]
fn a_configured_book_size_reaches_the_tracked_equity_and_the_decide_stage() -> Result<()> {
    let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(5_000_000));
    let mut platform = platform(config)?;
    assert_eq!(platform.equity(), Decimal::from_int(5_000_000));

    platform.run_cycle(start());
    let proposal = platform
        .proposals()
        .last()
        .ok_or_else(|| Error::not_found("the cycle's proposal"))?;
    assert_eq!(proposal.equity.amount, Decimal::from_int(5_000_000));
    Ok(())
}

// --- idle cash and redemption notices (ADR 0159) -----------------------------
//
// A whole cycle, not a stage called directly: the router is reached from the end
// of DECIDE and these tests drive it from `run_cycle`. The confidence governor
// is given a poor calibration through the same `ConfidenceSignals` LEARN writes,
// so the cycle releases half of what the waterfall left and the other half is
// idle. Without that the shipped schedule releases everything on a fresh tape
// and there is nothing for the router to move, which is what the first test
// asserts as its own premise.

mod idle {
    use super::*;
    use qip_financial::quality::DataQuality;
    use qip_kernel::yield_shelf;
    use qip_market::bar::{Bar, Interval};
    use qip_market_ingestion::adapter::SensedRecord;
    use qip_reasoning_engine::redteam::ReviewPolicy;
    use qip_risk::limits::{Limit, LimitKind};
    use qip_risk_engine::autonomy::OperatorIdentity;

    /// A Brier score the shipped schedule answers with a multiplier of one half.
    const POOR_BRIER: f64 = 0.28;

    fn book() -> Decimal {
        dec!("1000000")
    }

    fn bar(at: Timestamp, open: f64, close: f64) -> SensedRecord {
        SensedRecord::Bar(Box::new(Bar {
            object_id: object("AAA"),
            venue: "XNYS".to_string(),
            interval: Interval::Day,
            open_time: at,
            open: Decimal::from_f64(open).expect("a price"),
            high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
            low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
            close: Decimal::from_f64(close).expect("a price"),
            volume: dec!("1000000"),
            trade_count: 5_000,
            vwap: Decimal::from_f64((open + close) / 2.0),
            quality: DataQuality::default(),
        }))
    }

    /// A quiet series that drops nine percent on its last bar, which the panel
    /// reads as an undervalued long the mandate permits.
    fn tape() -> Vec<SensedRecord> {
        const BARS: usize = 120;
        let mut price = 100.0_f64;
        (0..BARS)
            .map(|i| {
                let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
                let drop = if i == BARS - 1 { -0.09 } else { 0.0 };
                let open = price;
                price *= 1.0 + noise + drop;
                bar(
                    start().saturating_sub(Duration::from_days((BARS - i) as i64)),
                    open,
                    price,
                )
            })
            .collect()
    }

    /// 25% of what the cycle declined to release may be routed, in three
    /// instruments of which the best yield is realised in six hours.
    const DECLARATION: &str = r#"{
        "schema": 1,
        "instruments": [
            {"id": "overnight", "class": "cash", "realisation_hours": 6, "capacity": "1000000", "yield_rate": "0.030"},
            {"id": "stable-fund", "class": "stable_value", "realisation_hours": 12, "capacity": "1000000", "yield_rate": "0.041"},
            {"id": "bills-1d", "class": "short_duration", "realisation_hours": 24, "capacity": "1000000", "yield_rate": "0.048"},
            {"id": "credit-note", "class": "other", "realisation_hours": 120, "capacity": "1000000", "yield_rate": "0.090"}
        ],
        "routing": {"max_share": "0.25", "ladder_horizons_hours": [24, 168]}
    }"#;

    fn platform_with(declaration: Option<&str>) -> Result<Platform> {
        let mut config = PlatformConfig {
            initial_equity: book(),
            review: ReviewPolicy {
                minimum_surviving_confidence: 0.10,
                ..ReviewPolicy::default()
            },
            ..PlatformConfig::default()
        };
        if let Some(text) = declaration {
            config = config.with_yield_shelf(yield_shelf::load(text)?);
        }
        let (context, _clock) = Context::deterministic(start(), config.seed);
        let limits = LimitSet::new("kernel-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        );
        let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), limits)?;
        platform
            .confidence_signals_mut()
            .observe_calibration(POOR_BRIER, 50, []);
        platform.observe(tape());
        Ok(platform)
    }

    fn operator(at: Timestamp) -> OperatorIdentity {
        OperatorIdentity::verified("desk-operator-1", "hardware-token", at)
    }

    /// One cycle, asserted to have constructed, and the budget it sized against.
    fn cycle(platform: &mut Platform) -> qip_kernel::survival::SizingBudget {
        let report = platform.run_cycle(start());
        assert!(report.traversed_every_stage(), "{}", report.summarise());
        platform
            .sizing_budget()
            .cloned()
            .expect("the cycle constructed, so the governor holds this cycle's budget")
    }

    #[test]
    fn idle_cash_routed_to_a_shelf_instrument_is_held_in_the_reservation_ledger_so_the_next_construction_has_exactly_that_much_less_free_cash()
    -> Result<()> {
        // The reference: the same cycle with no shelf declared.
        let mut bare = platform_with(None)?;
        let bare_budget = cycle(&mut bare);
        // Premise: the governor held half of the opportunity capital back, so
        // there is idle cash to route, and with no shelf nothing was.
        let unreleased = bare_budget.survival.opportunity_capital - bare_budget.released;
        assert!(unreleased.is_positive(), "{bare_budget:?}");
        assert_eq!(bare.parked_capital(start()), Decimal::ZERO);
        assert!(bare.replay_yield_routing()?.is_empty());

        let mut routed = platform_with(Some(DECLARATION))?;
        let budget = cycle(&mut routed);

        // The router never touches what the cycle released.
        assert_eq!(budget.released, bare_budget.released);
        assert_eq!(
            budget.survival.opportunity_capital,
            bare_budget.survival.opportunity_capital
        );
        // A quarter of the unreleased capital is routed, best yield first: the
        // 24-hour bills take all of it because their capacity covers it.
        let parked = routed.parked_capital(start());
        assert_eq!(parked, unreleased * dec!("0.25"));
        // And the next construction has exactly that much less free cash.
        let after = start().saturating_add(Duration::from_mins(1));
        assert_eq!(
            bare.deployable_capital(after)? - routed.deployable_capital(after)?,
            parked
        );
        // The record says what it is.
        let journal = routed.replay_yield_routing()?;
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].label, qip_kernel::platform::YIELD_ROUTING_LABEL);
        assert_eq!(journal[0].unreleased, unreleased);
        assert_eq!(journal[0].outcome.placements.len(), 1);
        assert_eq!(journal[0].outcome.placements[0].instrument, "bills-1d");
        assert_eq!(journal[0].outcome.placements[0].amount, parked);
        Ok(())
    }

    #[test]
    fn a_fresh_tape_with_nothing_declined_routes_nothing_whatever_is_declared() -> Result<()> {
        // The same shelf, without the poor calibration: the governor releases
        // everything and the router has nothing it may move.
        let mut config_platform = platform_with(Some(DECLARATION))?;
        *config_platform.confidence_signals_mut() =
            qip_kernel::confidence::ConfidenceSignals::default();
        let budget = cycle(&mut config_platform);
        assert_eq!(
            budget.released, budget.survival.opportunity_capital,
            "premise: nothing was declined"
        );
        assert_eq!(config_platform.parked_capital(start()), Decimal::ZERO);
        assert!(config_platform.replay_yield_routing()?.is_empty());
        Ok(())
    }

    #[test]
    fn a_shelf_that_is_listed_but_not_armed_routes_nothing() -> Result<()> {
        let unarmed = r#"{"schema": 1, "instruments": [
            {"id": "overnight", "class": "cash", "realisation_hours": 6, "capacity": "100000", "yield_rate": "0.030"}
        ]}"#;
        let mut platform = platform_with(Some(unarmed))?;
        let budget = cycle(&mut platform);
        assert!(
            (budget.survival.opportunity_capital - budget.released).is_positive(),
            "premise: there is idle cash"
        );
        assert_eq!(platform.parked_capital(start()), Decimal::ZERO);
        Ok(())
    }

    #[test]
    fn an_instrument_of_class_other_is_never_routed_however_well_it_yields() -> Result<()> {
        let only_other = r#"{"schema": 1, "instruments": [
            {"id": "credit-note", "class": "other", "realisation_hours": 6, "capacity": "100000", "yield_rate": "0.090"}
        ], "routing": {"max_share": "1", "ladder_horizons_hours": [24]}}"#;
        let mut platform = platform_with(Some(only_other))?;
        let budget = cycle(&mut platform);
        assert!((budget.survival.opportunity_capital - budget.released).is_positive());
        assert_eq!(platform.parked_capital(start()), Decimal::ZERO);
        Ok(())
    }

    #[test]
    fn a_declared_outflow_the_instruments_cannot_beat_stops_the_cycles_routing_and_a_later_one_does_not()
    -> Result<()> {
        // Premise carried by the first test: with no notice the cycle parks.
        // An outflow in 8 hours leaves only the 6-hour instrument eligible,
        // and it is the lowest yield, so it is the one that is placed.
        let mut early = platform_with(Some(DECLARATION))?;
        early.declare_redemption_notice(
            "n-1",
            dec!("500"),
            start().saturating_add(Duration::from_hours(8)),
            &operator(start()),
            start(),
        )?;
        cycle(&mut early);
        let journal = early.replay_yield_routing()?;
        assert_eq!(journal.len(), 1);
        let placed: Vec<&str> = journal[0]
            .outcome
            .placements
            .iter()
            .map(|p| p.instrument.as_str())
            .collect();
        assert_eq!(placed, vec!["overnight"]);
        assert_eq!(journal[0].outcome.refused.len(), 2);
        assert!(
            journal[0]
                .outcome
                .refused
                .iter()
                .all(|r| r.reason.contains("notice:n-1")),
            "{:?}",
            journal[0].outcome.refused
        );

        let mut late = platform_with(Some(DECLARATION))?;
        late.declare_redemption_notice(
            "n-1",
            dec!("500"),
            start().saturating_add(Duration::from_hours(36)),
            &operator(start()),
            start(),
        )?;
        cycle(&mut late);
        let journal = late.replay_yield_routing()?;
        assert_eq!(journal[0].outcome.placements[0].instrument, "bills-1d");
        assert!(journal[0].outcome.refused.is_empty());
        Ok(())
    }

    #[test]
    fn a_redemption_notice_is_journaled_superseded_by_id_and_rebuilt_from_the_log() -> Result<()> {
        let mut platform = platform_with(None)?;
        let due = start().saturating_add(Duration::from_days(3));
        platform.declare_redemption_notice("n-1", dec!("100"), due, &operator(start()), start())?;
        platform.declare_redemption_notice("n-2", dec!("50"), due, &operator(start()), start())?;
        // A later declaration under the same id replaces the earlier one.
        platform.declare_redemption_notice("n-1", dec!("250"), due, &operator(start()), start())?;
        assert_eq!(platform.redemption_notices().len(), 2);
        assert_eq!(platform.redemption_notices()["n-1"].amount, dec!("250"));
        assert_eq!(
            platform.redemption_notices()["n-1"].declarer,
            "desk-operator-1"
        );
        // The log alone says the same.
        assert_eq!(
            platform.replay_redemption_notices()?,
            *platform.redemption_notices()
        );
        Ok(())
    }

    #[test]
    fn a_redemption_notice_needs_a_fresh_operator_and_a_valid_record() -> Result<()> {
        let mut platform = platform_with(None)?;
        let due = start().saturating_add(Duration::from_days(3));
        // Authenticated 16 minutes ago: past the 15-minute window.
        let stale = operator(start().saturating_sub(Duration::from_mins(16)));
        let refused = platform.declare_redemption_notice("n-1", dec!("100"), due, &stale, start());
        assert!(refused.unwrap_err().to_string().contains("re-authenticate"));
        assert!(
            platform
                .declare_redemption_notice("n-1", Decimal::ZERO, due, &operator(start()), start())
                .is_err()
        );
        assert!(
            platform
                .declare_redemption_notice(
                    "n-1",
                    dec!("1"),
                    start().saturating_sub(Duration::from_mins(1)),
                    &operator(start()),
                    start()
                )
                .is_err(),
            "a claim cannot fall due before it was declared"
        );
        // Nothing was adopted or journaled by any refusal.
        assert!(platform.redemption_notices().is_empty());
        assert!(platform.replay_redemption_notices()?.is_empty());
        Ok(())
    }

    #[test]
    fn a_yield_shelf_that_would_not_load_stops_assembly() {
        let past_the_ceiling = r#"{"schema": 1, "instruments": [
            {"id": "bills-30d", "class": "short_duration", "realisation_hours": 720, "capacity": "1", "yield_rate": "0.04"}
        ]}"#;
        assert!(yield_shelf::load(past_the_ceiling).is_err());
        // The loader is not the only door: a configuration assembled with the
        // declaration set directly is held to the same rule by `Platform::new`.
        let declaration =
            yield_shelf::YieldShelfDeclaration::parse(past_the_ceiling).expect("parses");
        let config = PlatformConfig::default().with_yield_shelf(declaration);
        let (context, _clock) = Context::deterministic(start(), config.seed);
        assert!(Platform::new(config, context, Telemetry::silent(), universe(), limits()).is_err());
        // Credit is not a class, and the refusal says why.
        let credit = r#"{"schema": 1, "instruments": [
            {"id": "loan", "class": "credit", "realisation_hours": 6, "capacity": "1", "yield_rate": "0.04"}
        ]}"#;
        assert!(
            yield_shelf::load(credit)
                .unwrap_err()
                .to_string()
                .contains("credit")
        );
        assert!(yield_shelf::load(r#"{"schema": 2}"#).is_err());
        assert!(yield_shelf::load(r#"{"schema": 1, "surprise": true}"#).is_err());
    }

    #[test]
    fn a_named_shelf_file_that_does_not_read_stops_the_process_rather_than_turning_routing_off() {
        let committed = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../data/datasets/yield-shelf.json"
        );
        assert!(
            yield_shelf::load_file(committed).is_ok(),
            "premise: the committed file reads"
        );
        let refused = yield_shelf::load_file("/nonexistent/yield-shelf.json").unwrap_err();
        assert!(
            refused.to_string().contains(yield_shelf::PATH_VARIABLE),
            "{refused}"
        );
    }

    #[test]
    fn the_committed_shelf_declaration_loads_and_declares_nothing_it_was_not_given() -> Result<()> {
        const COMMITTED: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../data/datasets/yield-shelf.json"
        ));
        let declaration = yield_shelf::load(COMMITTED)?;
        // The RISK lane has declared no shelf capacity, realisation, share or
        // horizon (ADR 0159), so the committed file routes nothing.
        assert!(declaration.instruments.is_empty());
        assert!(declaration.routing.is_none());
        Ok(())
    }
}
