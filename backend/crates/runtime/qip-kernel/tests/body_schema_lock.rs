//! The hash-chained bodies the kernel writes keep the shape their version
//! says (R12-CICD-10203: "schemas versioned and locked").
//!
//! `qip-events/schemas.lock.json` locks the fabric-bound bodies and qip-api's
//! `COMMITTED_SCHEMAS` the bodies it streams; neither covers the records the
//! kernel seals into its own log — a fill booked, an attribution pass — and a
//! field added to one of those changed every later record's bytes under an
//! unchanged `SCHEMA_VERSION`, so a replay of an old log decoded it as a shape
//! it never had. Each body here is locked by its full recursive shape's
//! `SchemaId` and its version: changing the shape without bumping the version
//! fails, naming the new id to commit beside the bumped version.
//!
//! Every sample fills each collection, so an element type is part of the id.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_core::{Decimal, Timestamp};
use qip_events::EventBody;
use qip_events::event_fabric::schema_id::{SchemaId, Shape};
use qip_execution_engine::order::Side;
use qip_kernel::attribution::AttributionRecorded;
use qip_kernel::platform::FillBooked;
use std::collections::BTreeMap;

/// (type, version, schema id) as committed. Bump the version with the id.
const LOCKED: &[(&str, u32, &str)] = &[
    (
        "FillBooked",
        1,
        "84b85533a0dcdb70d33be1b2fd50e6070223b227daac7abfccd09c2e5ac44dce",
    ),
    (
        "AttributionRecorded",
        1,
        "6042323ea2faef4eb74d72bc7f9a99d336054379b1b52763a528dfe804970422",
    ),
];

fn id_of<T: EventBody>(sample: &T) -> String {
    let shape = Shape::of(sample).expect("a sample serialises to a shape");
    SchemaId::new(T::TOPIC.name(), T::SCHEMA_VERSION, &shape)
        .as_str()
        .to_string()
}

fn fill() -> FillBooked {
    FillBooked {
        order_id: "ord-1".to_string(),
        fill_id: "fill-1".to_string(),
        object_id: "obj-AAA".to_string(),
        venue: "XNYS".to_string(),
        side: Side::Buy,
        quantity: Decimal::from_int(10),
        price: Decimal::from_int(100),
        costs: Decimal::from_int(1),
        at: Timestamp::from_secs(1_760_000_000),
        simulated: true,
    }
}

fn attribution() -> AttributionRecorded {
    AttributionRecorded {
        cycle: 7,
        at: Timestamp::from_secs(1_760_000_000),
        fills: 1,
        total_pnl: Decimal::from_int(5),
        book_total: Decimal::from_int(5),
        unmarked: 0,
        unkeyed: 0,
        by_source: BTreeMap::from([("market".to_string(), Decimal::from_int(5))]),
        by_dimension: BTreeMap::from([(
            "strategy".to_string(),
            BTreeMap::from([("s-1".to_string(), Decimal::from_int(5))]),
        )]),
    }
}

fn computed() -> Vec<(&'static str, u32, String)> {
    vec![
        ("FillBooked", FillBooked::SCHEMA_VERSION, id_of(&fill())),
        (
            "AttributionRecorded",
            AttributionRecorded::SCHEMA_VERSION,
            id_of(&attribution()),
        ),
    ]
}

#[test]
fn each_hash_chained_kernel_body_has_the_shape_its_version_locks() {
    let computed = computed();
    assert_eq!(
        computed.len(),
        LOCKED.len(),
        "premise: every locked body is computed"
    );
    for ((name, version, id), (locked_name, locked_version, locked_id)) in
        computed.iter().zip(LOCKED)
    {
        assert_eq!(name, locked_name);
        assert!(
            version == locked_version && id == locked_id,
            "{name} is now version {version} with schema id {id}; the lock says version \
             {locked_version} with {locked_id}. A changed shape needs a bumped SCHEMA_VERSION \
             and both committed here together"
        );
    }
}
