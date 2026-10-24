//! EXEC-027: Market creation is a governed workflow, never an unrestricted
//! model action — and, since R12-EXEC-5080, one that actually creates
//! something, on the simulated venue only.
//!
//! This suite used to build `ApprovedMarketCreation` directly from a struct
//! literal, in a test whose comment said the struct could not be built that
//! way, and assert that three functions which returned `Ok(())` without
//! doing anything returned `Ok(())`. Every one of its assertions passed with
//! the governance removed. The approval struct and the no-op functions are
//! gone; what replaces them is `CreationPermissions::admit` — the only mint
//! of a `CreationAdmission`, which has private fields and no `Deserialize` —
//! and `MarketFactory`, whose every step demands one. These tests drive both
//! and assert what is refused, not only what passes.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_core::{Decimal, dec};
use qip_execution_engine::broker::{ListingVenue, SimulatedBroker, SimulationSettings};
use qip_execution_engine::creation::{
    CreationAction, CreationAdmission, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::market_creation::{MarketFactory, PricingRule};
use qip_market::book::OrderBook;

const VENUE: &str = "simulated-venue";
const PRODUCT: &str = "TEST/USD";

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn rule() -> Result<PricingRule> {
    PricingRule::new(
        dec!("100"),
        dec!("0.5"),
        dec!("0.25"),
        dec!("10"),
        3,
        dec!("0.05"),
    )
}

fn request(action: CreationAction, venue: &str) -> CreationRequest {
    CreationRequest::new(action, venue, PRODUCT, "GB")
}

fn permit_all(permissions: &mut CreationPermissions, venue: &str) -> Result<()> {
    for action in CreationAction::ALL {
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request(action, venue), "recorded evidence")?;
        }
    }
    Ok(())
}

/// With nothing recorded there is no admission, so no step of the factory
/// can be reached; with all three authorities recorded the same request
/// creates a market. Mutation: make `CreationPermissions::admit` skip its
/// missing-authority check and the first assertion fails.
#[test]
fn a_market_is_created_only_with_an_admission_from_all_three_authorities() -> Result<()> {
    let empty = CreationPermissions::new();
    assert!(
        empty
            .admit(&request(CreationAction::Create, VENUE))
            .is_err()
    );

    let mut permissions = CreationPermissions::new();
    permit_all(&mut permissions, VENUE)?;
    let broker = SimulatedBroker::new(SimulationSettings::frictionless(), 1);
    let mut factory = MarketFactory::for_venue(&broker)?;
    factory.create(
        &permissions.admit(&request(CreationAction::Create, VENUE))?,
        rule()?,
        at(),
    )?;
    assert!(factory.market(PRODUCT).is_some());
    Ok(())
}

/// Permission to create is not permission to list or seed. Mutation: drop
/// the action check in `MarketFactory::admitted_for` and the create
/// admission lists the market.
#[test]
fn each_step_needs_an_admission_for_that_step() -> Result<()> {
    let mut permissions = CreationPermissions::new();
    permit_all(&mut permissions, VENUE)?;
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 1);
    let mut factory = MarketFactory::for_venue(&broker)?;
    let create = permissions.admit(&request(CreationAction::Create, VENUE))?;
    factory.create(&create, rule()?, at())?;
    let refused = factory
        .list(&create, &mut broker, at())
        .expect_err("a create admission listed the market");
    assert!(
        refused
            .message()
            .contains("each step needs its own admission")
    );
    assert!(broker.listed_book(PRODUCT).is_none());
    Ok(())
}

/// No market is created on a real venue: an admission for one is refused by
/// the factory, and a listing surface that is not simulated cannot have a
/// factory at all. Mutation: drop `refuse_live` from `for_venue` and the
/// live surface gets a factory.
#[test]
fn no_market_is_created_on_a_venue_that_is_not_the_simulated_one() -> Result<()> {
    let mut permissions = CreationPermissions::new();
    permit_all(&mut permissions, "XNYS")?;
    let broker = SimulatedBroker::new(SimulationSettings::frictionless(), 1);
    let mut factory = MarketFactory::for_venue(&broker)?;
    let refused = factory
        .create(
            &permissions.admit(&request(CreationAction::Create, "XNYS"))?,
            rule()?,
            at(),
        )
        .expect_err("a market admitted for a real venue was created");
    assert!(refused.message().contains("only at the simulated venue"));

    #[derive(Debug)]
    struct LiveSurface;
    impl ListingVenue for LiveSurface {
        fn venue_name(&self) -> &str {
            "XNYS"
        }
        fn is_simulated(&self) -> bool {
            false
        }
        fn listed_book(&self, _object_id: &str) -> Option<&OrderBook> {
            None
        }
        fn list_book(&mut self, _book: OrderBook, _admission: &CreationAdmission) -> Result<()> {
            Err(qip_core::error::Error::denied(
                "a live surface rests no book; the factory must never reach this",
            ))
        }
        fn withdraw_book(&mut self, _object_id: &str) -> Option<OrderBook> {
            None
        }
    }
    let refused =
        MarketFactory::for_venue(&LiveSurface).expect_err("a factory was built over a live venue");
    assert!(refused.message().contains("is not simulated"));
    Ok(())
}

/// The desk's broker is the only listing venue the port hands out. A
/// mutation that made `Broker::listing_venue`'s default answer would let any
/// broker be listed on; this asserts the simulator answers and keeps the
/// seeded book the factory rests. Mutation: drop `venue.list_book(book)`
/// from `seed` and the depth is zero.
#[test]
fn a_seeded_market_rests_its_rule_at_the_simulated_venue() -> Result<()> {
    use qip_execution_engine::broker::Broker;
    let mut permissions = CreationPermissions::new();
    permit_all(&mut permissions, VENUE)?;
    let mut broker = SimulatedBroker::new(SimulationSettings::frictionless(), 1);
    let mut factory = MarketFactory::for_venue(&broker)?;
    factory.create(
        &permissions.admit(&request(CreationAction::Create, VENUE))?,
        rule()?,
        at(),
    )?;
    let venue = broker
        .listing_venue()
        .expect("the simulated broker offers a listing venue");
    factory.list(
        &permissions.admit(&request(CreationAction::List, VENUE))?,
        venue,
        at(),
    )?;
    // The ladder's risk verdict, from the only mint there is: the desk's
    // order manager, here under a 2x leverage limit over a book of a million
    // that the ladder is far inside (R12-RISK-4093).
    let mut orders = qip_execution_engine::oms::OrderManager::new(
        qip_risk_engine::pretrade::PreTradeChecker::new(
            qip_risk::limits::LimitSet::new("governance").with(qip_risk::limits::Limit::new(
                "leverage",
                qip_risk::limits::LimitKind::MaxLeverage { limit: 2.0 },
            )),
        ),
    );
    let ladder = orders.admit_ladder(
        PRODUCT,
        VENUE,
        &rule()?,
        "market-factory",
        &qip_risk_engine::autonomy::AutonomyController::new(),
        &qip_risk::limits::RiskState {
            equity: dec!("1000000"),
            cash: dec!("1000000"),
            ..qip_risk::limits::RiskState::default()
        },
        at(),
    )?;
    let venue = broker.listing_venue().expect("still offered");
    factory.seed(
        &permissions.admit(&request(CreationAction::Seed, VENUE))?,
        &ladder,
        dec!("30"),
        venue,
        at(),
    )?;
    let depth = broker
        .listed_book(PRODUCT)
        .map_or(Decimal::ZERO, OrderBook::total_depth);
    assert_eq!(depth, dec!("60"));
    Ok(())
}
