//! Book gamma and vega reach the Risk Gate as measured figures with limits
//! that can fire (ADR 0492, R12-RISK-4095).
//!
//! Before this, `BookGreeks` reported gamma and vega as `None` for any book
//! holding an option, because the kernel ignored the Greeks the catalogue's
//! option records carry, and no limit read either figure. These tests drive
//! desk orders through `Platform::submit_order` — the governed path every
//! order takes — and show that the gamma and vega limits refuse an order that
//! would carry the book past its bound, admit one inside it, and refuse an
//! option whose catalogue record states no usable Greeks rather than reading
//! its convexity as flat.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::extensions::{
    ExerciseStyle, Extension, OptionDetails, OptionType, SettlementStyle,
};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::risk_profile::Greeks;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::AggregateFigures;
use qip_risk::limits::{Limit, LimitKind, LimitSet, RiskState};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

const STOCK: &str = "obj-AAA";
/// An option whose record states finite Greeks.
const PRICED: &str = "obj-AAA-C";
/// An option whose record states a gamma and vega that are not numbers.
const UNPRICED: &str = "obj-AAA-P";

/// Per unit of the underlying: gamma 0.02, vega 0.30 (value per volatility
/// point). A contract is a hundred units, so a thousand contracts carry a
/// gamma of 2,000 and a vega of 30,000. The desk books a contract's notional
/// as its price, 5, with no multiplier.
const GAMMA: f64 = 0.02;
const VEGA: f64 = 0.30;

fn liquidity() -> qip_financial::costs::LiquidityProfile {
    qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(50_000_000), 3.0)
}

fn option(id: &str, gamma: f64, vega: f64) -> FinancialObject {
    FinancialObject::builder(
        ObjectId::from_string(id),
        id,
        InstrumentType::Option,
        liquidity(),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .underlying(ObjectId::from_string(STOCK))
    .price(dec!("5"))
    .contract_multiplier(Decimal::from_int(100))
    .extension(Extension::Option(OptionDetails {
        underlying_object_id: STOCK.into(),
        strike: dec!("100"),
        expiry: Timestamp::from_civil(2027, 1, 15),
        option_type: OptionType::Call,
        exercise_style: ExerciseStyle::European,
        contract_multiplier: Decimal::from_int(100),
        settlement: SettlementStyle::Cash,
        implied_volatility: 0.25,
        greeks: Greeks {
            delta: 0.5,
            gamma,
            vega,
            ..Greeks::default()
        },
        open_interest: 10_000,
    }))
    .provenance(Provenance::synthetic("test", start()))
    .build(start())
    .expect("valid option")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                ObjectId::from_string(STOCK),
                "AAA",
                InstrumentType::CommonStock,
                liquidity(),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("valid stock"),
        )
        .expect("insertable");
    universe
        .insert(option(PRICED, GAMMA, VEGA))
        .expect("insertable");
    universe
        .insert(option(UNPRICED, f64::NAN, f64::NAN))
        .expect("insertable");
    universe
}

/// One wide order cap beside the limit under test, so the only control that
/// can refuse the orders below is the convexity limit.
fn limits(convexity: Limit) -> LimitSet {
    LimitSet::new("convexity")
        .with(
            Limit::new(
                "order-notional",
                LimitKind::MaxOrderNotional {
                    limit: Decimal::from_int(100_000_000),
                },
            )
            .with_rationale("wide enough that only the convexity limit binds"),
        )
        .with(convexity)
}

fn platform(convexity: Limit) -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe(),
        limits(convexity),
    )?;
    let mut bars = Vec::new();
    for (id, price) in [
        (STOCK, dec!("100")),
        (PRICED, dec!("5")),
        (UNPRICED, dec!("5")),
    ] {
        for i in 0..30 {
            let at = start().saturating_sub(Duration::from_days(30 - i));
            bars.push(SensedRecord::Bar(Box::new(Bar {
                object_id: ObjectId::from_string(id),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
                open: price,
                high: price,
                low: price,
                close: price,
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: Some(price),
                quality: DataQuality::default(),
            })));
        }
    }
    platform.observe(bars);
    Ok(platform)
}

fn buy(platform: &mut Platform, id: &str, quantity: Decimal, price: Decimal) -> Result<()> {
    let order = platform.order_from(
        ObjectId::from_string(id),
        Side::Buy,
        quantity,
        price,
        "prop-convexity",
        vec!["hyp-convexity".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// The notional the gate's book holds in `id`.
fn held(platform: &Platform, id: &str) -> Decimal {
    platform
        .risk_figures()
        .position_notionals()
        .get(id)
        .copied()
        .unwrap_or(Decimal::ZERO)
}

/// The risk state the pre-trade gate reads.
fn gate_state(platform: &Platform) -> RiskState {
    platform.risk_state_from(platform.risk_figures())
}

/// Mutations that make it fail: report `None` from `record_convexity` for
/// every non-linear record (the admitted order is refused as unmeasured);
/// drop the `MaxBookGamma` arm's `Measured` record (the breaching order is
/// admitted); leave `RiskState::convexity` empty in `risk_state_from` (the
/// admitted order is refused).
#[test]
fn the_gamma_limit_admits_an_option_inside_its_bound_and_refuses_one_past_it() -> Result<()> {
    let mut platform = platform(
        Limit::new("book-gamma", LimitKind::MaxBookGamma { limit: 5_000.0 })
            .with_rationale("test bound"),
    )?;
    // The premise: the gate's own state measures the book, and an empty
    // book's gamma is a measured zero rather than an absence.
    let state = gate_state(&platform);
    let empty = state.book_convexity().expect("an empty book is measured");
    assert!(empty.gamma.abs() < 1e-12, "{}", empty.gamma);
    assert!(
        state.convexity.contains_key(PRICED),
        "the priced option's Greeks did not reach the risk state"
    );
    assert!(
        !state.convexity.contains_key(UNPRICED),
        "an option whose Greeks are not numbers must not be filed as measured"
    );

    // A thousand contracts: gamma 2,000, inside 5,000.
    buy(&mut platform, PRICED, dec!("1000"), dec!("5"))?;
    let filled = held(&platform, PRICED);
    assert!(filled.is_positive(), "premise: the admitted order filled");
    let gamma = gate_state(&platform)
        .book_convexity()
        .expect("a book of a priced option is measured")
        .gamma;
    // Contracts held are the notional over the price of 5; each carries a
    // hundred units of the underlying's gamma.
    let expected = filled.to_f64() / 5.0 * 100.0 * GAMMA;
    assert!(
        expected > 0.0 && (gamma - expected).abs() < 1e-9,
        "book gamma {gamma} is not the contracts in {filled} of notional times {GAMMA}"
    );

    // Five thousand more: 10,000 of further gamma on top of what is held,
    // past the bound.
    let refusal = buy(&mut platform, PRICED, dec!("5000"), dec!("5"))
        .expect_err("an order past the gamma bound");
    assert!(
        refusal.message().contains("book-gamma"),
        "the refusal does not name the gamma limit: {}",
        refusal.message()
    );
    Ok(())
}

/// Mutations that make it fail: divide by `FinancialObject::notional(1)` in
/// `record_convexity` (the multiplier cancels and the breaching order is
/// admitted), or drop the `MaxBookVega` arm's `Measured` record.
#[test]
fn the_vega_limit_refuses_an_order_whose_vega_per_point_exceeds_its_share_of_equity() -> Result<()>
{
    // Ten million of equity; a bound of one per cent is 100,000 of value per
    // volatility point. A thousand contracts carry 30,000; five thousand
    // more carry 150,000.
    let mut platform = platform(
        Limit::new("book-vega", LimitKind::MaxBookVega { limit: 0.01 })
            .with_rationale("test bound"),
    )?;
    assert_eq!(gate_state(&platform).equity, Decimal::from_int(10_000_000));
    buy(&mut platform, PRICED, dec!("1000"), dec!("5"))?;
    assert!(held(&platform, PRICED).is_positive(), "premise");
    let refusal = buy(&mut platform, PRICED, dec!("5000"), dec!("5"))
        .expect_err("an order past the vega bound");
    assert!(
        refusal.message().contains("book-vega"),
        "the refusal does not name the vega limit: {}",
        refusal.message()
    );
    Ok(())
}

/// Mutation that makes it fail: read an absent `convexity` entry as zero in
/// `RiskState::book_convexity` — the unpriced option is admitted.
#[test]
fn an_option_whose_catalogue_states_no_greeks_is_refused_and_a_stock_is_not() -> Result<()> {
    let mut platform = platform(
        Limit::new("book-gamma", LimitKind::MaxBookGamma { limit: 5_000.0 })
            .with_rationale("test bound"),
    )?;
    // A stock is linear: its gamma is a measured zero, so the limit admits
    // it — the limit is not a blanket refusal of everything.
    buy(&mut platform, STOCK, dec!("100"), dec!("100"))?;
    assert!(held(&platform, STOCK).is_positive(), "premise");

    let refusal = buy(&mut platform, UNPRICED, dec!("1"), dec!("5"))
        .expect_err("an option with no usable Greeks");
    assert!(
        refusal.message().contains("cannot be measured") && refusal.message().contains(UNPRICED),
        "the refusal does not say the unpriced option's gamma is unmeasured: {}",
        refusal.message()
    );
    assert_eq!(held(&platform, UNPRICED), Decimal::ZERO);
    Ok(())
}
