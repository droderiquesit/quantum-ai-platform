//! R12-TICK-2053: a sensed record reaches the platform under a header that
//! names its source and its licence, and a record that cannot state either
//! cannot be built into one.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::{Entitlement, Usage};
use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Decimal, ObjectId, dec};
use qip_financial::quality::DataQuality;
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::admit::{admit, record_digest};

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn tick(late_by: Duration, price: &str) -> SensedRecord {
    SensedRecord::Tick(Tick {
        object_id: ObjectId::from_string("obj-AAA"),
        venue: "XNYS".to_string(),
        at: at(),
        capture_time: at().saturating_add(late_by),
        sequence: None,
        price: Decimal::parse(price).expect("a price"),
        volume: dec!("100"),
        quality: DataQuality::clean(),
    })
}

fn granted(until: Timestamp) -> Entitlement {
    Entitlement::Granted {
        dataset: "xnys-ticks".to_string(),
        usage: Usage::Derive,
        expires_at: until,
    }
}

fn open() -> Entitlement {
    granted(at().saturating_add(Duration::from_secs(3600)))
}

#[test]
fn an_admitted_tick_carries_its_own_instants_its_source_and_its_licence() -> Result<()> {
    let record = tick(Duration::from_secs(2), "123");
    let hash = record_digest(&record)?;
    let admitted = admit(record.clone(), hash.clone(), open(), Duration::ZERO)?;
    let header = admitted.admission();
    assert_eq!(header.event_time(), at());
    assert_eq!(
        header.receive_time(),
        at().saturating_add(Duration::from_secs(2))
    );
    assert_eq!(header.source_hash(), hash);
    assert_eq!(header.entitlement().dataset(), "xnys-ticks");
    assert_eq!(admitted.record(), &record);
    assert_ne!(
        record_digest(&tick(Duration::from_secs(2), "124"))?,
        hash,
        "premise: a different record has a different digest"
    );
    Ok(())
}

#[test]
fn a_record_with_no_source_no_licence_or_an_impossible_clock_is_not_built() -> Result<()> {
    let ok = tick(Duration::from_secs(2), "123");
    assert!(
        admit(ok.clone(), "h", open(), Duration::ZERO).is_ok(),
        "premise: the control admits"
    );
    assert!(
        admit(ok.clone(), " ", open(), Duration::ZERO).is_err(),
        "no source hash"
    );
    let denied = Entitlement::Denied {
        dataset: "xnys-ticks".to_string(),
        usage: Usage::Derive,
        reason: "unlicensed".to_string(),
    };
    assert!(
        admit(ok.clone(), "h", denied, Duration::ZERO).is_err(),
        "denied licence"
    );
    assert!(
        admit(ok.clone(), "h", granted(at()), Duration::ZERO).is_err(),
        "lapsed at event"
    );
    let early = tick(Duration::from_secs(-5), "123");
    assert!(
        admit(early.clone(), "h", open(), Duration::ZERO).is_err(),
        "received early"
    );
    assert!(
        admit(early, "h", open(), Duration::from_secs(5)).is_ok(),
        "inside the clock's stated doubt the two instants are one"
    );
    assert!(
        admit(ok, "h", open(), Duration::from_secs(-1)).is_err(),
        "negative doubt"
    );
    Ok(())
}
