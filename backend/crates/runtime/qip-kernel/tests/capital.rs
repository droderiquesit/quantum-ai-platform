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
            FinancialObject::builder(object("AAA"), "AAA", InstrumentType::CommonStock)
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
