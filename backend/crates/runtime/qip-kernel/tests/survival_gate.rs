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

/// Ledger balances in the shape `qip_ledgerd::LedgerStore::balances` returns.
fn ledger_balances(
    rows: &[(qip_contracts::ledger::Account, Decimal)],
) -> std::collections::BTreeMap<(qip_contracts::ledger::Account, String), Decimal> {
    rows.iter()
        .map(|(account, amount)| ((account.clone(), "USD".to_string()), *amount))
        .collect()
}

/// R12-CAPITAL-4087 through the platform: a ledger reading with cash owed
/// and not yet settled, and collateral pledged beyond its margin, lowers the
/// ceiling the Risk Gate reads by exactly what it withholds. Before the
/// reading the platform's only source was the cross-margin review, which
/// derives both as zero, so the same capital was released to opportunity.
///
/// Mutations, each fired and restored: drop the `ledger` argument from
/// `survival_figures` (pass `None`) and the ceiling stays at ten million;
/// remove the `seed_survival_ceiling` call from `observe_ledger_obligations`
/// and the ceiling stays at ten million until the next construction; drop
/// the stale-reading check and the older reading is accepted.
#[test]
fn unsettled_cash_and_pledged_collateral_on_the_ledger_lower_what_survival_releases() -> Result<()>
{
    use qip_contracts::ledger::Account;
    let mut platform = platform()?;
    let open = platform.survival_gross_ceiling().expect("seeded");
    assert_eq!(open, dec!("10000000"), "premise: nothing is withheld yet");

    // Settlement alone: two hundred and fifty thousand owed on a funding
    // accrual, and an interest receivable that must not offset it.
    let owed = qip_capital::LedgerObligations::from_balances(
        &ledger_balances(&[
            (
                Account::Accrual {
                    accrual_type: "funding".to_string(),
                },
                dec!("-250000"),
            ),
            (
                Account::Accrual {
                    accrual_type: "interest".to_string(),
                },
                dec!("90000"),
            ),
        ]),
        "USD",
        start(),
    )?;
    platform.observe_ledger_obligations(owed)?;
    assert_eq!(
        platform.survival_gross_ceiling().expect("re-seeded"),
        dec!("9750000"),
        "unsettled cash is withheld and a receivable does not release it"
    );

    // A later reading: the accrual has settled, and three hundred thousand
    // is pledged against a hundred thousand of booked margin. Margin and
    // the excess pledge are withheld; the covered part once, not twice.
    let pledged = qip_capital::LedgerObligations::from_balances(
        &ledger_balances(&[
            (
                Account::Margin {
                    venue: "XNYS".to_string(),
                },
                dec!("100000"),
            ),
            (
                Account::Collateral {
                    counterparty: "prime-broker".to_string(),
                },
                dec!("300000"),
            ),
        ]),
        "USD",
        start().saturating_add(Duration::from_secs(60)),
    )?;
    platform.observe_ledger_obligations(pledged)?;
    assert_eq!(
        platform.survival_gross_ceiling().expect("re-seeded"),
        dec!("9700000")
    );

    // An older reading delivered late is refused, and the held one stands.
    let stale =
        qip_capital::LedgerObligations::from_balances(&ledger_balances(&[]), "USD", start())?;
    let refused = platform
        .observe_ledger_obligations(stale)
        .expect_err("a reading older than the one held");
    assert!(refused.message().contains("older"), "{refused}");
    assert_eq!(
        platform.survival_gross_ceiling().expect("unchanged"),
        dec!("9700000")
    );

    // A reading in a unit the waterfall does not size in is refused.
    let euros = qip_capital::LedgerObligations::from_balances(
        &std::collections::BTreeMap::new(),
        "EUR",
        start().saturating_add(Duration::from_secs(120)),
    )?;
    assert!(platform.observe_ledger_obligations(euros).is_err());
    Ok(())
}

/// R12-CAPITAL-4087's settlement tier from the platform's own state: the
/// desk venue is declared T+1 in the settlement book at assembly, and a sale
/// there books its cash at the fill. Until the calendar's settlement
/// instant the proceeds are withheld from survival sizing — the ceiling the
/// gate reads is lower by exactly them — and once it passes they are
/// released. Before this the waterfall read settlement as zero while the
/// pre-positioning planner quoted the same venue T+1.
///
/// Mutations, each fired and restored: return early from
/// `register_settlement` (nothing is withheld and the first assertion
/// fails); pass `Decimal::ZERO` as `unsettled_sales` in `survival_figures`
/// (the ceiling does not fall); treat every sale as available at the fill
/// (nothing is in flight an hour on).
#[test]
fn a_sale_on_the_desk_venues_t_plus_one_calendar_is_withheld_until_it_settles() -> Result<()> {
    let config = PlatformConfig::default();
    let (context, clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(config, context, Telemetry::silent(), universe(), wide())?;
    platform.observe(
        (0..30)
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
            .collect(),
    );
    buy(&mut platform, dec!("1000"))?;
    assert_eq!(
        platform.unsettled_sale_proceeds(start())?,
        Decimal::ZERO,
        "premise: a purchase owes nothing later — the broker debited it"
    );
    let bought: Decimal = platform
        .orders()
        .fills()
        .iter()
        .map(|fill| fill.quantity)
        .sum();
    assert!(bought.is_positive(), "premise: the buy filled");

    // Sell what filled, at 08:53 UTC on a Thursday: inside the 16:00
    // cut-off, so T+1 settles on Friday at 09:00.
    let sell = platform.order_from(
        object(),
        Side::Sell,
        bought,
        dec!("100"),
        "prop-survival",
        vec!["hyp-survival".to_string()],
        start(),
    );
    let bought_fills = platform.orders().fills().len();
    platform.submit_order(sell, start())?;
    let proceeds: Decimal = platform.orders().fills()[bought_fills..]
        .iter()
        .map(|fill| fill.price * fill.quantity - fill.costs)
        .sum();
    assert!(proceeds.is_positive(), "premise: the sale filled");
    // An hour on — after the fill, still the day before settlement — the
    // proceeds are in flight.
    let an_hour_on = start().saturating_add(Duration::from_hours(1));
    assert_eq!(platform.unsettled_sale_proceeds(an_hour_on)?, proceeds);

    clock.set(an_hour_on);
    let policy = *platform.survival_policy();
    platform.set_survival_policy(policy);
    let in_flight = platform.survival_gross_ceiling().expect("seeded");

    // Friday 09:00 has passed: the venue's calendar has made them usable.
    clock.set(start().saturating_add(Duration::from_days(2)));
    assert_eq!(
        platform.unsettled_sale_proceeds(start().saturating_add(Duration::from_days(2)))?,
        Decimal::ZERO
    );
    platform.set_survival_policy(policy);
    let settled = platform.survival_gross_ceiling().expect("re-seeded");
    assert_eq!(
        settled - in_flight,
        proceeds,
        "the ceiling did not fall by the proceeds in settlement and rise by them once settled"
    );
    Ok(())
}
