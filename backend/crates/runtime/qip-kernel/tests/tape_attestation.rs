//! ADR 0428 decision 5: a flagged or unattested tape may remove a trace's
//! weight from fusion and never add one.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::market_state::{MarketStatePack, StateKind};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp, dec};
use qip_kernel::fusion::{MarketTrace, TracePoint};
use qip_kernel::tape_attestation::TapeAttestation;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn event(venue: &str, sequence: u64, at: Timestamp) -> Result<MarketEvent> {
    let payload = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new(venue), "itch", 0, sequence),
        MessageBody::Trade {
            price: dec!("100.25"),
            quantity: dec!("100"),
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Bid),
        },
        at,
        at,
    );
    let hash = MarketEvent::hash_payload(&payload)?;
    MarketEvent::new(
        payload,
        at,
        at,
        at,
        Duration::ZERO,
        hash,
        Entitlement::Granted {
            dataset: "d".to_string(),
            usage: Usage::Trade,
            expires_at: t(10_000),
        },
    )
}

fn pack(events: &[MarketEvent]) -> Result<MarketStatePack> {
    MarketStatePack::new(
        StateKind::Book,
        t(0),
        t(100),
        Duration::from_millis(500),
        vec!["feature:mid@1".to_string()],
        events,
    )
}

fn trace(instrument: &str, venue: &str) -> MarketTrace {
    MarketTrace {
        instrument: instrument.to_string(),
        venue: venue.to_string(),
        points: vec![TracePoint {
            at: t(1),
            mid: 100.0,
            spread_bps: 1.0,
            depth: None,
            imbalance: None,
        }],
    }
}

fn traces() -> Vec<MarketTrace> {
    vec![
        trace("ACME", "XNYS"),
        trace("ACME", "XNYSX"),
        trace("BETA", "XLON"),
    ]
}

fn venues(of: &[MarketTrace]) -> Vec<&str> {
    of.iter().map(|x| x.venue.as_str()).collect()
}

#[test]
fn a_venue_with_a_sequence_gap_loses_its_traces_and_only_its_own() -> Result<()> {
    let events = [event("XNYS", 1, t(1))?, event("XNYS", 4, t(2))?];
    let flagged = pack(&events)?;
    assert!(
        !flagged.integrity().is_empty(),
        "premise: the pack is flagged"
    );
    let attestation = TapeAttestation::of(2, Some(&flagged));
    let kept = attestation.admit(traces());
    // XNYSX shares a prefix with XNYS and is a different venue: delimited match.
    assert_eq!(venues(&kept), ["XNYSX", "XLON"]);
    Ok(())
}

#[test]
fn applied_events_with_no_pack_are_unattested_and_admit_nothing() {
    let attestation = TapeAttestation::of(5, None);
    assert_eq!(attestation, TapeAttestation::Unattested);
    assert!(attestation.admit(traces()).is_empty());
}

#[test]
fn a_clean_pack_and_a_pass_with_no_tape_remove_nothing_and_add_nothing() -> Result<()> {
    let clean = pack(&[event("XNYS", 1, t(1))?, event("XNYS", 2, t(2))?])?;
    assert!(clean.integrity().is_empty(), "premise: the pack is clean");
    for attestation in [
        TapeAttestation::of(2, Some(&clean)),
        TapeAttestation::of(0, None),
    ] {
        let before = traces();
        let after = attestation.admit(before.clone());
        assert_eq!(venues(&after), venues(&before));
        assert_eq!(after.len(), before.len(), "never more traces than given");
    }
    Ok(())
}
