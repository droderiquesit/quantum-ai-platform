//! A desk position walks discover to attribute, and nothing reaches
//! attribute without settling (blueprint R12-ASSET-4071).
//!
//! `qip_portfolio::LifeStage` held the checked edge table and its unit tests
//! for a release before anything in the kernel used it for more than a
//! display string, so "exit cannot reach attribute without settle" was true
//! of a table no position ever walked. These tests drive the desk's real
//! booking seam — an order through `submit_order`, filled by the simulated
//! venue — and read the stage the kernel recorded.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_execution_engine::order::Side;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_observability::Telemetry;
use qip_portfolio::LifeStage;
use qip_risk::limits::LimitSet;

const SYMBOL: &str = "LIFE";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            object(),
            SYMBOL,
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    let config = PlatformConfig::default().with_initial_equity(dec!("1000000"));
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

fn held(platform: &Platform) -> Decimal {
    platform
        .positions_at_cost()
        .get(object().as_str())
        .map_or(Decimal::ZERO, |(quantity, _)| *quantity)
}

fn order(platform: &mut Platform, side: Side, shares: Decimal, tag: &str) -> Result<()> {
    let order = platform.order_from(
        object(),
        side,
        shares,
        dec!("100"),
        &format!("prop-{tag}"),
        vec![format!("hyp-{tag}")],
        start(),
    );
    platform.submit_order(order, start())
}

#[test]
fn an_exited_unsettled_position_is_not_attributed() -> Result<()> {
    let mut platform = platform()?;
    // Premise: a fresh desk holds nothing and has recorded no life.
    assert!(held(&platform).is_zero());
    assert_eq!(platform.position_life_stage(object().as_str()), None);

    // The simulated venue fills a fixed share of each order (three of five
    // here) and refuses a size off the one-unit lot grid, so the sizes below
    // are chosen to land on whole lots and to take the book exactly flat.
    order(&mut platform, Side::Buy, dec!("5"), "open")?;
    let opened = held(&platform);
    // Premise: the venue filled the opening order, so a position exists.
    assert!(opened.is_positive(), "the simulated venue filled nothing");
    assert_eq!(
        platform.position_life_stage(object().as_str()),
        Some(LifeStage::Hold),
        "an opening fill must walk discover, evaluate and acquire to hold"
    );

    // A second purchase resizes the held position and returns it to hold.
    order(&mut platform, Side::Buy, dec!("5"), "resize")?;
    assert!(
        held(&platform) > opened,
        "the resizing order filled nothing"
    );
    assert_eq!(
        platform.position_life_stage(object().as_str()),
        Some(LifeStage::Hold)
    );

    // Six held, and the venue fills six tenths of a sale: only an order for
    // ten fills exactly six, and an order for ten projects the book four
    // short, which the borrow gate refuses unless borrow has been observed.
    // Without this a desk position on the shipped simulator cannot reach
    // flat at all — a sale no larger than the holding always leaves four
    // tenths of it, off the lot grid for the next order.
    assert_eq!(
        held(&platform),
        dec!("6"),
        "premise: the venue fills 3 of 5"
    );
    platform.observe_borrow(object().as_str(), dec!("10"), start(), start())?;
    order(&mut platform, Side::Sell, dec!("10"), "close")?;
    // Premise: the fills really took the position flat.
    assert!(held(&platform).is_zero(), "the desk never went flat");
    assert_eq!(
        platform.position_life_stage(object().as_str()),
        Some(LifeStage::Exit)
    );

    let (_, life) = platform
        .position_lives()
        .lives()
        .find(|(held, _)| *held == object().as_str())
        .expect("an exited life is kept until it is attributed");
    let exited_at = life.exited_at().expect("an exited life records its exit");
    let settles_at = life
        .settles_at()
        .expect("the desk venue's calendar schedules the exit's settlement");
    // Premise: the venue settles later than the fill, so there is a LEARN
    // before settlement to run. A T+0 calendar would make the first half of
    // this test pass vacuously.
    assert!(
        settles_at > exited_at,
        "the exit settles at {settles_at:?}, not after its fill at {exited_at:?}"
    );

    // LEARN before the calendar's instant: the position has exited and not
    // settled, so it must not be attributed.
    let report = platform.run_cycle(start());
    assert!(
        report.stage(Stage::Learn).is_some_and(|learn| learn.ran),
        "LEARN did not run"
    );
    assert!(
        !report
            .problems()
            .iter()
            .any(|(_, problem)| problem.contains("position in obj-LIFE")),
        "the life register refused a transition: {:?}",
        report.problems()
    );
    assert_eq!(
        platform.position_life_stage(object().as_str()),
        Some(LifeStage::Exit),
        "an exited, unsettled position was moved on by LEARN"
    );
    assert_eq!(platform.position_lives().attributed().count(), 0);

    // LEARN after settlement: settled, then attributed, in that order.
    let after = Timestamp::from_secs(settles_at.as_secs() + 60);
    let report = platform.run_cycle(after);
    assert!(
        report.stage(Stage::Learn).is_some_and(|learn| learn.ran),
        "LEARN did not run"
    );
    assert!(
        !report
            .problems()
            .iter()
            .any(|(_, problem)| problem.contains("position in obj-LIFE")),
        "the life register refused a transition: {:?}",
        report.problems()
    );
    assert_eq!(
        platform.position_life_stage(object().as_str()),
        Some(LifeStage::Attribute),
        "a settled position was not attributed by the next LEARN"
    );
    let (_, attributed) = platform
        .position_lives()
        .attributed()
        .last()
        .expect("the attributed life is kept for reading");
    assert_eq!(attributed.attributed_at(), Some(after));
    let learn = report.stage(Stage::Learn).expect("LEARN ran");
    assert!(
        learn.detail.contains("; position lives: 0 open, 0 exited awaiting settlement, 1 settled and 1 attributed this cycle"),
        "LEARN did not say what it attributed: {}",
        learn.detail
    );
    Ok(())
}
