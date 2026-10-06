//! DATA-030: User-authorized source entitlements.
//!
//! Verifies that entitlements flow through from a RegisteredSource into its
//! DataReference, and that the entitlement is available for inspection when
//! the source is ingested.

use qip_contracts::governance::{Entitlement, Usage};
use qip_core::Timestamp;
use qip_data_finder::reference::{DataPeriod, DataReference};
use qip_data_finder::schema::SourceSchema;

mod common;
use common::*;

#[test]
fn an_entitled_discovered_source_carries_the_entitlement_on_its_reference() {
    let start = Timestamp::from_millis(1_700_000_000_000);
    let end = Timestamp::from_millis(1_700_086_400_000);
    let range = DataPeriod::new(start, end).expect("valid range");

    let source = tradeable_source("tradeable-with-entitlement", start).expect("source built");

    let entitlement = Entitlement::Granted {
        dataset: "tradeable-with-entitlement".into(),
        usage: Usage::Trade,
        expires_at: Timestamp::from_millis(2_000_000_000_000),
    };
    let expected_entitlements = vec![entitlement.clone()];

    let bytes = b"symbol,open,high,low,close\nEUR/USD,1.10,1.12,1.09,1.11\n";
    let reference = DataReference::of(
        &source,
        "https://example.com/data",
        vec!["EUR/USD".to_string()],
        range,
        SourceSchema::from_fields([]),
        bytes,
        start,
        qip_core::Decimal::from_int(10),
        0.95,
    )
    .expect("reference created");

    assert_eq!(reference.source_id(), source.id());
    assert_eq!(reference.entitlements(), &expected_entitlements);
}

#[test]
fn an_unentitled_discovered_source_carries_no_entitlements_on_its_reference() {
    let start = Timestamp::from_millis(1_700_000_000_000);
    let end = Timestamp::from_millis(1_700_086_400_000);
    let range = DataPeriod::new(start, end).expect("valid range");

    let source = unentitled_source("unentitled", start).expect("source built");

    let bytes = b"symbol,open,high,low,close\nEUR/USD,1.10,1.12,1.09,1.11\n";
    let reference = DataReference::of(
        &source,
        "https://example.com/data",
        vec!["EUR/USD".to_string()],
        range,
        SourceSchema::from_fields([]),
        bytes,
        start,
        qip_core::Decimal::from_int(10),
        0.95,
    )
    .expect("reference created");

    assert_eq!(reference.source_id(), source.id());
    assert!(reference.entitlements().is_empty());
}

#[test]
fn entitlements_are_preserved_when_a_reference_is_serialized_and_deserialized() {
    let start = Timestamp::from_millis(1_700_000_000_000);
    let end = Timestamp::from_millis(1_700_086_400_000);
    let range = DataPeriod::new(start, end).expect("valid range");

    let source = tradeable_source("tradeable-roundtrip", start).expect("source built");

    let bytes = b"symbol,open,high,low,close\nEUR/USD,1.10,1.12,1.09,1.11\n";
    let original = DataReference::of(
        &source,
        "https://example.com/data",
        vec!["EUR/USD".to_string()],
        range,
        SourceSchema::from_fields([]),
        bytes,
        start,
        qip_core::Decimal::from_int(10),
        0.95,
    )
    .expect("reference created");

    // Serialize and deserialize
    let json = serde_json::to_string(&original).expect("serialization succeeds");
    let restored: DataReference = serde_json::from_str(&json).expect("deserialization succeeds");

    assert_eq!(original.entitlements(), restored.entitlements());
    assert_eq!(original.source_id(), restored.source_id());
}
