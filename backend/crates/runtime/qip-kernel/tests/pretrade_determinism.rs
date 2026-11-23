//! R12-RISK-3022: the order path asks the cost router for a deterministic
//! routing before any limit is read.
//!
//! `Determinism::Required` was constructed nowhere in production, so the
//! structural guarantee that a pre-trade check never routes to a model was
//! proven on the type and not on the call. `Platform::submit_order` now obtains
//! its routing from `Router::determine`; these tests fail if it stops.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_cost_router::{Horizon, IntelligenceTier};
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

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            ObjectId::from_string("obj-AAA"),
            "AAA",
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("XNYS")
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", start()))
        .build(start())?,
    )?;
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
}

fn order(platform: &mut Platform, quantity: Decimal) -> qip_execution_engine::order::Order {
    platform.order_from(
        ObjectId::from_string("obj-AAA"),
        Side::Buy,
        quantity,
        dec!("100"),
        "prop-determinism",
        vec!["hyp-determinism".to_string()],
        start(),
    )
}

#[test]
fn an_accepted_order_was_guarded_by_a_deterministic_routing_that_names_no_model() -> Result<()> {
    let mut platform = platform()?;
    assert!(
        platform.pretrade_routing().is_none(),
        "premise: nothing routed yet"
    );
    let order = order(&mut platform, dec!("10"));
    platform.submit_order(order, start())?;
    let routing = platform
        .pretrade_routing()
        .expect("submit_order must route");
    assert_eq!(routing.charge().tier, IntelligenceTier::DeterministicCode);
    assert_eq!(routing.charge().tier.model_tier(), None);
    assert_eq!(routing.conditions().horizon, Horizon::Microsecond);
    assert!(routing.rationale().contains("pre-trade limits for obj-AAA"));
    Ok(())
}

#[test]
fn a_refused_order_was_still_routed_deterministically_before_the_limits_spoke() -> Result<()> {
    let mut platform = platform()?;
    // Far past any conservative cap, so the limits refuse it.
    let order = order(&mut platform, dec!("1000000000"));
    let result = platform.submit_order(order, start());
    assert!(
        result.is_err(),
        "premise: the limits must refuse this order"
    );
    let routing = platform
        .pretrade_routing()
        .expect("routing precedes the limits");
    assert_eq!(routing.charge().tier, IntelligenceTier::DeterministicCode);
    Ok(())
}

/// The routing is not only obtained before the limits: its gate is what the
/// order manager ran the pre-trade check under. `OrderManager::submit`
/// demands the gate by signature and copies its rationale onto the result,
/// so a refusal the desk records names the determined decision it came
/// from. Fails if `submit_order` hands the manager a gate from any routing
/// other than the one it kept, or if the manager drops the rationale.
#[test]
fn the_order_manager_ran_the_pre_trade_check_under_the_routing_the_platform_kept() -> Result<()> {
    let mut platform = platform()?;
    assert!(
        platform.orders().refusals().is_empty(),
        "premise: nothing refused yet"
    );
    let order = order(&mut platform, dec!("1000000000"));
    assert!(
        platform.submit_order(order, start()).is_err(),
        "premise: the limits must refuse this order"
    );
    let routing = platform
        .pretrade_routing()
        .expect("routing precedes the limits");
    assert!(
        !routing.rationale().is_empty(),
        "premise: a rationale was given"
    );
    let refusals = platform.orders().refusals();
    assert_eq!(
        refusals.len(),
        1,
        "premise: the manager recorded the refusal"
    );
    assert_eq!(refusals[0].routed, routing.rationale());
    Ok(())
}

/// The router's opening words for a determined decision about `subject`,
/// written out here so the assertion does not read the subject back through
/// the kernel that routed it.
fn determined(subject: &str) -> String {
    format!(
        "'{subject}' must be a function of its inputs, so it is answered in deterministic code under "
    )
}

/// R12-RISK-3022 on the kill-switch path beyond the risk monitor: a cell
/// whose records do not reconcile is halted by `Platform::ingest_cell_report`
/// — the seam `qip-api`'s mesh hands every cell report to — under a gate the
/// cost router minted for that halt, and the trip the operator reads carries
/// the router's rationale. Fails if the central plane trips the cell's scope
/// through the ungated `KillSwitch::trip_scope`, or if the platform hands
/// the plane a gate determined for another decision.
#[test]
fn a_cell_halted_for_a_reconciliation_break_was_halted_under_a_determined_routing() -> Result<()> {
    use qip_compliance::incident::HaltScope;
    use qip_kernel::central::{BreakOrigin, CellReport, ReconciliationBreak};
    use qip_kernel::platform::HardLimitDecision;

    let mut platform = platform()?;
    assert!(
        platform.autonomy().kill_switch().history().is_empty(),
        "premise: nothing has tripped"
    );
    let broken = CellReport::new("cell-lon-1", start()).with_break(ReconciliationBreak {
        instrument: "AAA".to_string(),
        cell_quantity: dec!("10"),
        external_quantity: dec!("4"),
        detail: "six lots the venue has no record of".to_string(),
        origin: BreakOrigin::Book,
    });
    let ingestion = platform.ingest_cell_report(broken, start())?;
    assert_eq!(
        ingestion.halted,
        Some(HaltScope::Cell("cell-lon-1".to_string())),
        "premise: a break halts the cell"
    );
    let trip = platform
        .autonomy()
        .kill_switch()
        .scoped_trip("cell-lon-1")
        .expect("premise: the cell's scope is tripped");
    let routed = trip
        .routed
        .as_deref()
        .expect("the halt was tripped under no deterministic gate");
    assert!(
        routed.starts_with(&determined("reconciliation halt for cell-lon-1")),
        "the halt was tripped under a gate for another decision: {routed}"
    );
    assert_eq!(
        platform.hard_limit_routing(HardLimitDecision::ReconciliationHalt),
        Some(routed),
        "the platform kept a different routing from the one the plane tripped under"
    );
    Ok(())
}

/// R12-RISK-3022 on the survival waterfall: the ceiling the survival limit
/// refuses a gross-raising order against is seeded at assembly — on
/// `PlatformConfig::default`, as every composition root builds the platform
/// — under a gate the cost router minted for it. Fails if the seed stops
/// recording the gate it ran under.
#[test]
fn the_survival_ceiling_is_seeded_at_assembly_under_a_determined_routing() -> Result<()> {
    use qip_kernel::platform::HardLimitDecision;

    let platform = platform()?;
    let rationale = platform
        .hard_limit_routing(HardLimitDecision::SurvivalCeiling)
        .expect("the survival ceiling was seeded under no deterministic gate");
    assert!(
        rationale.starts_with(&determined("survival ceiling for the book")),
        "the survival ceiling was seeded under a gate for another decision: {rationale}"
    );
    Ok(())
}
