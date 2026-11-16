//! R12-ASSET-4072: a held private position gets a secondary-sale route
//! priced off its dated mark at a discount the caller evidences, and the
//! platform refuses where it has no mark, no exit plan or no stage to plan
//! from. The position is opened the way a deployment opens one: a tape, a
//! thesis, a construction, a fill; the premise is asserted first.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
use qip_financial::object::FinancialObject;
use qip_financial::position_record::ExitRoute;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::secondary_exit::DiscountToMark;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

const REVIEW_FLOOR: f64 = 0.10;
const TAPE: usize = 120;
const DROP: f64 = -0.09;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object(symbol: &str) -> ObjectId {
    ObjectId::from_string(format!("obj-{symbol}"))
}

/// A leverage cap and nothing else, so a position opened here is not also
/// being judged by a concentration or notional control.
fn leverage_only() -> LimitSet {
    LimitSet::new("kernel-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    )
}

fn config() -> PlatformConfig {
    PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: REVIEW_FLOOR,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
}

fn bar(symbol: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
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

/// A quiet series that drops nine percent on its final bar: a `PriceMove`
/// read as `Claim::Undervalued`, a long the mandate permits.
fn tape(symbol: &str) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..TAPE)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662) % 1.0 - 0.5) * 0.008;
            let drop = if i == TAPE - 1 { DROP } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + drop;
            let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
            bar(symbol, at, open, price)
        })
        .collect()
}

/// A private fund with an undrawn commitment the commitment book will hold,
/// whose one-year lockup expired before `start()` — a fund inside its lockup
/// cannot be sized on a twenty-day thesis (ADR 0183):
/// 400,000 promised against 150,000 drawn, and a residual the valuation
/// plane marks from.
fn private_fund(symbol: &str) -> Result<FinancialObject> {
    private_fund_locked_for(symbol, 1.0)
}

/// [`private_fund`] with a stated lockup term, in years from its vintage.
fn private_fund_locked_for(symbol: &str, lockup_years: f64) -> Result<FinancialObject> {
    FinancialObject::builder(
        object(symbol),
        symbol,
        InstrumentType::PrivateEquityFund,
        LiquidityProfile::illiquid(90.0, 250.0),
    )
    .venue("OTC")
    .geography("US")
    .price(dec!("100"))
    .extension(Extension::PrivateAsset(PrivateAssetDetails {
        vintage_year: 2024,
        committed_capital: dec!("400000"),
        called_capital: dec!("150000"),
        distributed_capital: Decimal::ZERO,
        residual_value: dec!("160000"),
        stage: "buyout".to_string(),
        lockup_years,
        capital_call_notice_days: 10,
        call_schedule: None,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())
}

/// A held private fund, planned a secondary sale at an evidenced discount:
/// priced off the dated mark, routed as a secondary sale, dated no earlier
/// than now. And the refusals: a position nobody holds has no plan.
///
/// Mutation: in `Platform::secondary_exit_plan`, pass `mark.value() * 2`
/// to `plan_secondary_exit` (or drop the discount) — the indicative-price
/// assertion fails.
#[test]
fn a_held_private_position_is_planned_a_secondary_sale_off_its_dated_mark_at_the_evidenced_discount()
-> Result<()> {
    let mut universe = Universe::new();
    universe.insert(private_fund("PRIV")?)?;
    let config = config();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        leverage_only(),
    )?;
    platform.observe(tape("PRIV"));
    let report = platform.run_cycle(start());
    let fund = "obj-PRIV";
    assert!(
        platform.positions_at_cost().contains_key(fund),
        "premise: the cycle opened a position in {fund}: {}",
        report.summarise()
    );
    let mark = platform
        .illiquid_mark(fund)
        .expect("premise: the fund carries a dated mark")
        .value();
    assert!(mark.is_positive(), "premise: the mark is a price");

    let discount = DiscountToMark::new(dec!("1500"), "comparable fund trade, administrator quote")?;
    let sale = platform
        .secondary_exit_plan(fund, discount, start())?
        .expect("no primary liquidity event is forecast, so a secondary route is planned");
    assert_eq!(sale.plan().route(), ExitRoute::SecondarySale);
    assert!(sale.plan().exit_at() >= start());
    assert_eq!(sale.mark(), mark);
    assert_eq!(sale.discount_bps(), dec!("1500"));
    assert_eq!(
        sale.indicative_price(),
        mark.checked_mul(dec!("0.85")).expect("arithmetic"),
        "the indicative price is not the mark less the evidenced discount"
    );
    assert!(!sale.discount_evidence().trim().is_empty());

    // A position nobody holds has no horizon to plan inside.
    let again = DiscountToMark::new(dec!("1500"), "comparable fund trade")?;
    assert!(
        platform
            .secondary_exit_plan("obj-NONE", again, start())
            .is_err()
    );
    Ok(())
}
