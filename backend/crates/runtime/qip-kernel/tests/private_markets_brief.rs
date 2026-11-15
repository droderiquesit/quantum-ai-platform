//! R12-ASSET-3018: the kernel writes its commitment book and illiquid marks
//! into the slot the desk's Private Markets Brain reads, every REASON.
//!
//! Before this the brain read a slot nothing wrote, so on a desk with a
//! private fund it would report no data for ever. The premise is checked
//! first: the slot is unwritten before any cycle, and the book holds the
//! commitment the assertion is about.
#![allow(clippy::panic_in_result_fn)] // a test's assertion is the deliverable

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_financial::asset_class::InstrumentType;
use qip_financial::costs::LiquidityProfile;
use qip_financial::extensions::{Extension, PrivateAssetDetails};
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

/// 400,000 promised against 150,000 drawn: 250,000 the fund may call.
fn private_fund() -> Result<FinancialObject> {
    FinancialObject::builder(
        ObjectId::from_string("obj-FUND"),
        "FUND",
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
        lockup_years: 7.0,
        capital_call_notice_days: 10,
        call_schedule: None,
    }))
    .provenance(Provenance::synthetic("administrator", start()))
    .build(start())
}

/// Mutation: delete the `brief_the_desk_on_private_markets(now)` call in
/// `Platform::stage_reason` — the slot stays unwritten after the cycle and
/// the `observed` assertion fails.
#[test]
fn a_cycle_writes_the_commitment_book_into_the_slot_the_private_markets_brain_reads() -> Result<()>
{
    let mut universe = Universe::new();
    universe.insert(private_fund()?)?;
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        ..PlatformConfig::default()
    };
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )?;
    assert!(
        !platform.desk_private_markets_view().observed,
        "premise: nothing wrote the slot before the first cycle"
    );
    assert_eq!(
        platform.commitments().iter().count(),
        1,
        "premise: the book holds the fund's commitment"
    );

    platform.run_cycle(start());

    let view = platform.desk_private_markets_view();
    assert!(view.observed, "the cycle did not write the slot");
    assert_eq!(view.commitments.len(), 1);
    assert_eq!(view.commitments[0].unfunded, dec!("250000"));
    assert_eq!(view.commitments[0].calls_open, 0);
    Ok(())
}
