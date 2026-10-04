//! One conformance suite, run over every adapter in this crate (EXEC-011).
//!
//! The three checks the requirement names: the adapter declares its
//! capabilities, an order needing a capability it does not declare is refused
//! before it is sent, and orders and fills round-trip through the common types.

#![allow(clippy::panic_in_result_fn)]

#[allow(dead_code)] // the harness is shared with tests that use more of it
mod server;

use qip_brokers::VenueCredential;
use qip_brokers::adapter::{VenueAdapter, VenueOrderState};
use qip_brokers::credential::{
    RequirementKind, Secret, requirements_of_kind, standard_requirements,
};
use qip_brokers::exchange::{ExchangeSettings, SimulatedExchange};
use qip_brokers::rest::{RestOrderEntryAdapter, RestVenueConfig};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_core::{Decimal, dec};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::InstrumentType;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use server::{Action, Route, TestServer};

const OBJECT: &str = "OBJ00000000000000000000AAA";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn object() -> ObjectId {
    ObjectId::from_string(OBJECT)
}

fn order(label: &str, order_type: OrderType) -> Order {
    Order::new(
        OrderId::from_string(label),
        object(),
        Side::Buy,
        Decimal::from_int(10),
        order_type,
        dec!("100"),
        "proposal-under-test",
        vec!["hypothesis-under-test".to_string()],
        "scope-under-test",
        start(),
    )
}

fn credential(venue: &VenueId, with_secret: bool) -> VenueCredential {
    let enforced = requirements_of_kind(
        &standard_requirements(venue),
        &[RequirementKind::Account, RequirementKind::SessionCredential],
    );
    let credential = VenueCredential::satisfying(venue.as_str(), "book-under-test", &enforced)
        .expect("a named venue and account");
    if !with_secret {
        return credential;
    }
    let name = standard_requirements(venue)
        .into_iter()
        .find(|r| r.kind == RequirementKind::SessionCredential)
        .map(|r| r.name)
        .expect("a session credential is always named");
    credential.with_secret(
        name,
        "QIP_CONF_CREDENTIAL",
        Secret::new("conformance-secret"),
    )
}

fn simulated() -> Result<SimulatedExchange> {
    let venue = VenueId::new("XSIM");
    let mut exchange =
        SimulatedExchange::new(venue.clone(), ExchangeSettings::default(), 7, start());
    exchange.list(
        FinancialObject::builder(
            object(),
            "AAA",
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .name("Instrument A")
        .venue("XSIM")
        .price(dec!("100"))
        .lot_size(Decimal::ONE)
        .tick_size(dec!("0.01"))
        .provenance(Provenance::synthetic("conformance", start()))
        .build(start())?,
    );
    exchange.seed_liquidity(
        &object(),
        Side::Sell,
        dec!("100.02"),
        Decimal::from_int(50),
        start(),
    )?;
    exchange.seed_liquidity(
        &object(),
        Side::Buy,
        dec!("99.98"),
        Decimal::from_int(50),
        start(),
    )?;
    exchange.bring_up(&credential(&venue, false), start())?;
    Ok(exchange)
}

/// The suite, over any adapter through the common contract only.
fn conforms(adapter: &mut dyn VenueAdapter) -> Result<()> {
    let capabilities = adapter.capabilities();
    // Declares its capabilities: a non-empty list naming the common types.
    assert!(!capabilities.supported_types.is_empty());
    assert!(capabilities.supported_types.iter().any(|t| t == "limit"));

    let ticket = adapter.ready(start())?;

    // An undeclared type is refused by the declared-capability check itself.
    assert!(!capabilities.supported_types.iter().any(|t| t == "twap"));
    let refused = adapter
        .submit_declared(
            &ticket,
            &order("ORD-TWAP", OrderType::TimeWeighted { minutes: 30 }),
            start(),
        )
        .expect_err("an undeclared order type is refused");
    assert_eq!(refused.code(), "denied");
    assert!(
        refused.to_string().contains("does not declare"),
        "refused by the capability check, not by the venue: {refused}"
    );
    assert!(
        adapter.query_fills(None)?.is_empty(),
        "nothing was sent, so nothing filled"
    );

    // A declared type round-trips: ack, fill, simulated stamp.
    let ack = adapter.submit_declared(
        &ticket,
        &order(
            "ORD-1",
            OrderType::Limit {
                price: dec!("100.02"),
            },
        ),
        start(),
    )?;
    assert_eq!(ack.state, VenueOrderState::Filled);
    assert!(!ack.fills.is_empty());
    assert!(ack.fills.iter().all(|fill| fill.simulated));
    Ok(())
}

#[test]
fn every_adapter_declares_refuses_undeclared_and_round_trips_through_the_common_contract()
-> Result<()> {
    let mut exchange = simulated()?;
    conforms(&mut exchange)?;

    let record = r#"{"client_order_id":"ORD-1","venue_order_id":"V-1","state":"filled",
        "instrument":"OBJ00000000000000000000AAA","side":"buy","quantity":"10","filled":"10",
        "fills":[{"fill_id":"F-1","quantity":"10","price":"100.01","costs":"0.10",
        "at":"2026-08-24T00:00:01Z"}]}"#;
    let server = TestServer::routed(vec![
        Route::new("GET", "/v1/health", Action::json(200, "{}")),
        Route::new("POST", "/v1/orders", Action::json(200, record)),
    ]);
    let venue = VenueId::new("XSBX");
    let mut rest = RestOrderEntryAdapter::new(
        venue.clone(),
        RestVenueConfig {
            base_url: Some(server.url()),
            ..RestVenueConfig::default()
        },
        start(),
    )?;
    rest.bring_up(&credential(&venue, true), start())?;
    conforms(&mut rest)?;
    assert_eq!(
        server.hits("POST", "/v1/orders"),
        1,
        "only the declared order was sent"
    );
    Ok(())
}
