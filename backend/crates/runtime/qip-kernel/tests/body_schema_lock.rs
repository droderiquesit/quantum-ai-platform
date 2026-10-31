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
//!
//! `LedgerEntry` is a tagged enum, so each variant is locked by a sample of
//! its own (and `Booked` once per `BookingBasis`): a field added to one
//! variant changes only that variant's id. `CycleJournalEntry` is locked at
//! its top level: its optional sub-journals are skipped when absent, so a
//! change inside one of them is not caught here, and that is stated rather
//! than implied.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_capital::ledger::{UserId, UserShare};
use qip_contracts::signal::StrategyId;
use qip_core::Currency;
use qip_core::{Decimal, Timestamp};
use qip_events::EventBody;
use qip_events::event_fabric::schema_id::{SchemaId, Shape};
use qip_execution_engine::order::Side;
use qip_kernel::attribution::AttributionRecorded;
use qip_kernel::platform::{BookingBasis, CycleJournalEntry, FillBooked, LedgerEntry, SensedRef};
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
    (
        "LedgerEntry::Funded",
        1,
        "20bc012b756ab5eb602d17b086b88d13c8d17ea9a66f575d94ce474c5e9a8cc5",
    ),
    (
        "LedgerEntry::Booked/DeskWhole",
        1,
        "2a990ab839d7c8edfb4743736bcb3188e56652d2e1b541faba733217d0ae63cb",
    ),
    (
        "LedgerEntry::Booked/ProRata",
        1,
        "224be58043d1c6323d218c9206feffef44c3e12b69303c2dbf91043b09d62fc2",
    ),
    (
        "LedgerEntry::Booked/Refused",
        1,
        "ba6dcc13301570b39677f61f5c6a2a4cf0702af01e4d0c7736b233e86acc81fa",
    ),
    (
        "LedgerEntry::FundingRefused",
        1,
        "bf1a1edaf7c2974b2558d6d448f7844d70fbff4fa62de7b3b0502d2d49c2179b",
    ),
    (
        "LedgerEntry::InflowExpected",
        1,
        "de2fd7fdfe3318024f86f80b4e67171378c0651bff824a6ebdf0b3e0ccc240ea",
    ),
    (
        "LedgerEntry::InflowCancelled",
        1,
        "92c985ea45062861752244ad4d36dcaa81332f332d121b7bd22cb9bcf000a591",
    ),
    (
        "CycleJournalEntry",
        1,
        "538ac8a14ab46b404fa858fa5eff105a5cfd088ae496f48e85b47bc127c26104",
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

fn user() -> UserId {
    UserId::new("user-1").unwrap()
}

fn strategy() -> StrategyId {
    StrategyId::new("s-1")
}

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn booked(basis: BookingBasis) -> LedgerEntry {
    LedgerEntry::Booked {
        strategy: strategy(),
        source: "market".to_string(),
        currency: Currency::USD,
        amount: Decimal::from_int(5),
        basis,
    }
}

fn ledger_entries() -> Vec<(&'static str, LedgerEntry)> {
    vec![
        (
            "LedgerEntry::Funded",
            LedgerEntry::Funded {
                user: user(),
                strategy: strategy(),
                currency: Currency::USD,
                amount: Decimal::from_int(100),
            },
        ),
        (
            "LedgerEntry::Booked/DeskWhole",
            booked(BookingBasis::DeskWhole {
                user: user(),
                reason: "no user at work".to_string(),
            }),
        ),
        (
            "LedgerEntry::Booked/ProRata",
            booked(BookingBasis::ProRata {
                shares: vec![UserShare {
                    user: user(),
                    amount: Decimal::from_int(5),
                }],
                entitlement_total: Decimal::from_int(100),
                remainder: Decimal::ZERO,
                remainder_to: user(),
            }),
        ),
        (
            "LedgerEntry::Booked/Refused",
            booked(BookingBasis::Refused {
                reason: "no entitlement".to_string(),
            }),
        ),
        (
            "LedgerEntry::FundingRefused",
            LedgerEntry::FundingRefused {
                user: user(),
                strategy: strategy(),
                amount: Decimal::from_int(100),
                gate: "mandate".to_string(),
                reason: "no mandate".to_string(),
            },
        ),
        (
            "LedgerEntry::InflowExpected",
            LedgerEntry::InflowExpected {
                user: user(),
                strategy: strategy(),
                currency: Currency::USD,
                reference: "wire-1".to_string(),
                amount: Decimal::from_int(100),
                declared_at: at(),
                declared_by: "operator".to_string(),
            },
        ),
        (
            "LedgerEntry::InflowCancelled",
            LedgerEntry::InflowCancelled {
                user: user(),
                strategy: strategy(),
                reference: "wire-1".to_string(),
                amount: Decimal::from_int(100),
                cancelled_at: at(),
                cancelled_by: "operator".to_string(),
            },
        ),
    ]
}

fn cycle_journal() -> CycleJournalEntry {
    CycleJournalEntry {
        cycle: 7,
        correlation_id: "cor-1".to_string(),
        started_at: at(),
        finished_at: at(),
        stages_ran: 8,
        produced: 3,
        problems: vec!["a problem".to_string()],
        halted: false,
        compute_cost: Decimal::from_int(1),
        summary: "a summary".to_string(),
        calibration: None,
        counterfactuals: None,
        rule_review: None,
        strategy_review: None,
        family_structure: None,
        horizon_arming: None,
        pre_positioning: None,
        solver_routing: None,
        second_order: None,
        experience: None,
        sensed: vec![SensedRef {
            correlation_id: "cor-0".to_string(),
            event_id: "evt-1".to_string(),
            topic: "market.bar".to_string(),
        }],
        sensed_unlisted: 1,
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
    .into_iter()
    .chain(
        ledger_entries()
            .into_iter()
            .map(|(name, entry)| (name, LedgerEntry::SCHEMA_VERSION, id_of(&entry))),
    )
    .chain([(
        "CycleJournalEntry",
        CycleJournalEntry::SCHEMA_VERSION,
        id_of(&cycle_journal()),
    )])
    .collect()
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
