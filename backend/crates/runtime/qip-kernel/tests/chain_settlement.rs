//! Digital-asset proceeds settle on paper through a simulated chain
//! (ADR 0396, R12-COVERAGE-5103).
//!
//! A coin's sale proceeds were released on the venue's settlement calendar,
//! as an equity's are. These tests drive desk orders through
//! `Platform::submit_order` and show that, once an operator declares a
//! venue's chain terms, a digital-asset sale there waits on the declared
//! finality depth and bridge delay, that the bridge's fee is never released
//! to survival sizing, that an equity at the same venue still settles on the
//! calendar, and that nothing changes for a venue with no declared terms.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_chain::{Confirmations, FeeBps};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{AssetClass, InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::chain_settlement::{ChainSettlementTerms, DeclaredBridge};
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const STOCK: &str = "obj-AAA";
const COIN: &str = "obj-BTC";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn instrument(id: &str, kind: InstrumentType) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        kind,
        qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(50_000_000), 3.0),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("100"))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
    .expect("valid object")
}

fn wide() -> LimitSet {
    LimitSet::new("wide").with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: Decimal::from_int(100_000_000),
            },
        )
        .with_rationale("wide enough that no limit refuses the orders below"),
    )
}

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(instrument(STOCK, InstrumentType::CommonStock))?;
    universe.insert(instrument(COIN, InstrumentType::Cryptocurrency))?;
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe, wide())?;
    let bars: Vec<SensedRecord> = [STOCK, COIN]
        .into_iter()
        .flat_map(|id| {
            (0..30).map(move |i| {
                SensedRecord::Bar(Box::new(Bar {
                    object_id: ObjectId::from_string(id),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: start().saturating_sub(Duration::from_days(30 - i)),
                    open: dec!("100"),
                    high: dec!("100.5"),
                    low: dec!("99.5"),
                    close: dec!("100"),
                    volume: dec!("1000000"),
                    trade_count: 5_000,
                    vwap: Some(dec!("100")),
                    quality: DataQuality::default(),
                }))
            })
        })
        .collect();
    platform.observe(bars);
    Ok(platform)
}

fn order(platform: &mut Platform, id: &str, side: Side, quantity: Decimal) -> Result<()> {
    let order = platform.order_from(
        ObjectId::from_string(id),
        side,
        quantity,
        dec!("100"),
        "prop-chain",
        vec!["hyp-chain".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// Buy `id` and return what filled and the venue that filled it: the name
/// the desk's sales settle under, read from the fill and not assumed.
fn buy(platform: &mut Platform, id: &str) -> Result<(Decimal, String)> {
    let before = platform.orders().fills().len();
    order(platform, id, Side::Buy, dec!("1000"))?;
    let fills = &platform.orders().fills()[before..];
    let bought: Decimal = fills.iter().map(|fill| fill.quantity).sum();
    assert!(bought.is_positive(), "premise: the buy of {id} filled");
    Ok((bought, fills[0].venue.clone()))
}

/// Sell `quantity` of `id`, and return the sale's proceeds net of its costs
/// and the instant the venue filled it.
fn sell(platform: &mut Platform, id: &str, quantity: Decimal) -> Result<(Decimal, Timestamp)> {
    let before = platform.orders().fills().len();
    order(platform, id, Side::Sell, quantity)?;
    let fills = &platform.orders().fills()[before..];
    assert!(
        fills.iter().all(|fill| fill.simulated),
        "a fill of {id} was not stamped simulated"
    );
    let proceeds: Decimal = fills
        .iter()
        .map(|fill| fill.price * fill.quantity - fill.costs)
        .sum();
    assert!(proceeds.is_positive(), "premise: the sale of {id} filled");
    // Premise: one fill, so one settlement clock starts and the seconds the
    // tests count are counted from it.
    assert_eq!(fills.len(), 1, "the sale of {id} filled in pieces");
    Ok((proceeds, fills[0].at))
}

/// Twelve blocks of twelve seconds, then a ten-minute bridge keeping 30bp:
/// 144s + 600s = 744s from the fill.
fn terms() -> Result<ChainSettlementTerms> {
    ChainSettlementTerms::declare(
        Confirmations::exactly(12),
        Duration::from_secs(12),
        Some(DeclaredBridge::declare(
            Duration::from_mins(10),
            FeeBps::new(30)?,
        )?),
    )
}

/// Mutations that fired: dropping the depth from
/// `ChainSettlementTerms::available_at` releases the proceeds 144 seconds
/// early; dropping the bridge delay releases them 600 seconds early; reading
/// the terms for every class in `Platform::register_settlement` releases the
/// equity on the chain's clock; never adding the fee to the withheld total
/// releases the bridge's cut; ignoring declared terms leaves the coin on the
/// calendar.
#[test]
fn a_coins_proceeds_wait_on_the_declared_finality_and_bridge_and_the_fee_is_never_released()
-> Result<()> {
    let mut platform = platform()?;
    // Premise: the coin is the class the terms apply to and the stock is not.
    assert_eq!(
        instrument(COIN, InstrumentType::Cryptocurrency).asset_class,
        AssetClass::DigitalAsset
    );
    let (coins, venue) = buy(&mut platform, COIN)?;
    platform.declare_chain_settlement(&venue, terms()?);
    assert!(platform.chain_settlement_terms(&venue).is_some());

    let (proceeds, filled_at) = sell(&mut platform, COIN, coins)?;
    let fee = proceeds * dec!("0.003");
    let at = |seconds: i64| filled_at.saturating_add(Duration::from_secs(seconds));
    // One second short of finality plus the bridge, everything is in flight.
    assert_eq!(platform.unsettled_sale_proceeds(at(743))?, proceeds);
    // At 744s the proceeds are final and delivered; the bridge's cut is not.
    assert_eq!(platform.unsettled_sale_proceeds(at(744))?, fee);
    assert_eq!(platform.chain_bridge_fees(), fee);
    // A day on it is still withheld: a fee a bridge kept never comes back.
    assert_eq!(
        platform.unsettled_sale_proceeds(at(86_400 * 30))?,
        fee,
        "the bridge fee was released"
    );

    // The equity at the same venue is not a chain asset: T+1, in flight an
    // hour on, where the coin had been final for most of that hour.
    let (shares, stock_venue) = buy(&mut platform, STOCK)?;
    assert_eq!(stock_venue, venue, "premise: both trade at the one venue");
    let (stock_proceeds, _) = sell(&mut platform, STOCK, shares)?;
    assert_eq!(
        platform.unsettled_sale_proceeds(at(3_600))?,
        fee + stock_proceeds
    );
    assert_eq!(platform.chain_bridge_fees(), fee, "an equity paid a bridge");
    Ok(())
}

/// The other half: with no terms declared, nothing is invented. The coin
/// settles on the venue's calendar exactly as it did before, and no bridge
/// fee exists.
///
/// Mutation that fired: giving an undeclared venue a default of one block
/// and no bridge releases the coin within the hour.
#[test]
fn a_venue_with_no_declared_chain_terms_settles_a_coin_on_its_calendar_as_before() -> Result<()> {
    let mut platform = platform()?;
    let (coins, venue) = buy(&mut platform, COIN)?;
    assert!(platform.chain_settlement_terms(&venue).is_none());
    let (proceeds, filled_at) = sell(&mut platform, COIN, coins)?;
    let an_hour_on = filled_at.saturating_add(Duration::from_hours(1));
    assert_eq!(platform.unsettled_sale_proceeds(an_hour_on)?, proceeds);
    assert_eq!(platform.chain_bridge_fees(), Decimal::ZERO);
    Ok(())
}

/// Mutations that fired: admitting a depth of zero; admitting a block
/// interval of zero; admitting a negative bridge delay. Each lets the
/// matching `expect_err` below fail.
#[test]
fn chain_terms_that_would_release_proceeds_at_once_are_refused_not_defaulted() -> Result<()> {
    let head = ChainSettlementTerms::declare(Confirmations::AT_RISK, Duration::from_secs(12), None)
        .expect_err("a finality depth of zero was admitted");
    assert!(head.message().contains("finality depth of zero"), "{head}");
    let instant =
        ChainSettlementTerms::declare(Confirmations::exactly(12), Duration::from_secs(0), None)
            .expect_err("a block interval of zero was admitted");
    assert!(instant.message().contains("block interval"), "{instant}");
    let early = DeclaredBridge::declare(Duration::from_secs(-1), FeeBps::new(30)?)
        .expect_err("a negative bridge delay was admitted");
    assert!(early.message().contains("bridge delay"), "{early}");
    // Premise for the three refusals: the same shapes with lawful figures
    // are admitted, so the refusals are of the figures and not of the call.
    let terms = terms()?;
    assert_eq!(terms.finality(), Confirmations::exactly(12));
    assert!(terms.is_bridged());
    let settled = terms.settle(dec!("1000"), start())?;
    assert_eq!(settled.bridge_fee, dec!("3"));
    assert_eq!(settled.proceeds, dec!("997"));
    assert_eq!(
        settled.available_at,
        start().saturating_add(Duration::from_secs(744))
    );
    Ok(())
}
