//! The survival kernel reaches the Risk Gate as a limit, in every platform
//! the kernel assembles (R12-RISK-7205, R12-CAPITAL-4089, R12-CAPITAL-7204).
//!
//! `LimitKind::MaxSurvivalGross` evaluated deterministically and was in no
//! limit set any process ran, so the survival ceiling the kernel computed was
//! a number nothing compared an order against. These tests drive desk orders
//! through `Platform::submit_order` — the governed path every order takes —
//! against a platform assembled on a limit set that does **not** name the
//! survival limit, and show the kernel installed it, that it vetoes an order
//! past the ceiling, admits one inside it, and that the configured survival
//! policy moves the ceiling the gate reads.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_kernel::survival::{SURVIVAL_LIMIT_NAME, SurvivalPolicy};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-AAA")
}

fn universe() -> Universe {
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object(),
                "AAA",
                InstrumentType::CommonStock,
                qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(50_000_000), 3.0),
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

/// A set with one wide order cap and no survival limit, so the only control
/// that can refuse the orders below is the one the kernel installs.
fn wide() -> LimitSet {
    LimitSet::new("wide").with(
        Limit::new(
            "order-notional",
            LimitKind::MaxOrderNotional {
                limit: Decimal::from_int(100_000_000),
            },
        )
        .with_rationale("wide enough that only the survival ceiling binds"),
    )
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), wide())?;
    let bars: Vec<SensedRecord> = (0..30)
        .map(|i| {
            let at = start().saturating_sub(Duration::from_days(30 - i));
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at,
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
        .collect();
    platform.observe(bars);
    Ok(platform)
}

fn buy(platform: &mut Platform, quantity: Decimal) -> Result<()> {
    let order = platform.order_from(
        object(),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-survival",
        vec!["hyp-survival".to_string()],
        start(),
    );
    platform.submit_order(order, start())
}

/// Mutations that make it fail: return `Ok(limits)` unchanged from
/// `install_survival_limit` (the second order is admitted, and the premise
/// on the installed set fails first); write `None` for the ceiling in
/// `Platform::risk_state_from` (the first order is refused too).
#[test]
fn the_kernel_installs_the_survival_limit_and_it_vetoes_an_order_past_the_ceiling() -> Result<()> {
    let mut platform = platform()?;
    assert!(
        !wide()
            .limits
            .iter()
            .any(|limit| matches!(limit.kind, LimitKind::MaxSurvivalGross { .. })),
        "premise: the root's set does not name the survival limit"
    );
    assert!(
        platform
            .limits()
            .limits
            .iter()
            .any(|limit| limit.name == SURVIVAL_LIMIT_NAME
                && matches!(limit.kind, LimitKind::MaxSurvivalGross { .. })),
        "the kernel did not install the survival limit"
    );

    // Withhold nine tenths of equity: 45% as hedge capacity and 45% as the
    // emergency reserve. On ten million of equity and an empty book the
    // ceiling is the hedge capacity plus the one million released.
    let policy = SurvivalPolicy::new(dec!("0.45"), dec!("0.45"))?;
    let open = platform
        .survival_gross_ceiling()
        .expect("premise: assembly seeded a ceiling");
    platform.set_survival_policy(policy);
    let ceiling = platform
        .survival_gross_ceiling()
        .expect("the policy re-derived the ceiling");
    assert_eq!(ceiling, dec!("5500000"));
    assert!(
        ceiling < open,
        "withholding reserves must lower the ceiling the gate reads: {ceiling} against {open}"
    );

    // Five million of gross is inside it, so the gate admits the order.
    buy(&mut platform, dec!("50000"))?;
    // The gate reads the book as filled, and the simulated venue fills six
    // tenths of a market order immediately and works the rest
    // (`SimulationSettings::immediate_fill_fraction`). This test assumed a
    // full fill and was never run before merge; on the real fill the book
    // held three million, so its "further million" reached four and was
    // rightly admitted. The premise is asserted rather than assumed, and the
    // breaching order is sized from the book the gate actually reads.
    let filled: Decimal = platform
        .orders()
        .fills()
        .iter()
        .map(|fill| fill.quantity)
        .sum();
    assert_eq!(
        filled,
        dec!("30000"),
        "premise: the venue filled six tenths of the admitted order"
    );
    // Three million held; three million more carries the book to six,
    // half a million past the ceiling.
    let refusal = buy(&mut platform, dec!("30000")).expect_err("an order past the ceiling");
    assert!(
        refusal.message().contains(SURVIVAL_LIMIT_NAME),
        "the refusal does not name the survival limit: {}",
        refusal.message()
    );
    Ok(())
}

/// The shipped policy a composition root applies withholds before release,
/// so it stands behind less gross than the library default — the binary-side
/// half of R12-CAPITAL-4089's "removing the reserve raises the budget".
/// Mutation: make `SurvivalPolicy::shipped` return zero fractions.
#[test]
fn the_shipped_survival_policy_lowers_the_ceiling_against_the_library_default() -> Result<()> {
    let mut platform = platform()?;
    let library = platform.survival_gross_ceiling().expect("seeded");
    platform.set_survival_policy(SurvivalPolicy::shipped());
    let shipped = platform.survival_gross_ceiling().expect("re-seeded");
    // Ten million of equity, five per cent emergency reserve withheld: the
    // hedge capacity stays inside the ceiling, the emergency reserve does not.
    assert_eq!(library, dec!("10000000"));
    assert_eq!(shipped, dec!("9500000"));
    Ok(())
}
