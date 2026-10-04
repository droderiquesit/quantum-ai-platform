//! `qip_contracts::market_state::MarketStatePack` (CONTRACT-019): a state
//! rebuilt over a tape with a hole in it must say so, and a pack without
//! bounds, lineage or horizon is refused.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::market_state::{IntegrityFlag, MarketStatePack, StateKind};
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp, dec};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn event(feed: &str, sequence: u64, at: Timestamp) -> Result<MarketEvent> {
    let payload = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XNYS"), feed, 0, sequence),
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
            dataset: "xnys-itch".to_string(),
            usage: Usage::Trade,
            expires_at: t(10_000),
        },
    )
}

fn lineage() -> Vec<String> {
    vec!["feature:mid-price@1".to_string()]
}

fn pack(events: &[MarketEvent]) -> Result<MarketStatePack> {
    MarketStatePack::new(
        StateKind::Book,
        t(0),
        t(100),
        Duration::from_millis(500),
        lineage(),
        events,
    )
}

/// Mutation: make `MarketStatePack::new` skip the `sequence_gap` check — the
/// gap case below then carries no flag and the second assertion fails.
#[test]
fn a_pack_built_over_a_sequence_gap_carries_a_flag_and_a_clean_tape_carries_none() -> Result<()> {
    let clean = pack(&[
        event("itch-a", 1, t(10))?,
        event("itch-a", 2, t(20))?,
        event("itch-b", 7, t(25))?,
    ])?;
    assert_eq!(
        clean.integrity(),
        [],
        "a contiguous tape must not be flagged"
    );

    // Sequence 3 and 4 are missing; time says nothing was lost.
    let gapped = pack(&[
        event("itch-a", 1, t(10))?,
        event("itch-a", 2, t(20))?,
        event("itch-a", 5, t(21))?,
    ])?;
    assert_eq!(
        gapped.integrity(),
        [IntegrityFlag::SequenceGap {
            stream: "XNYS/itch-a/0".to_string(),
            missing: 2
        }]
    );
    Ok(())
}

/// Mutation: drop the lineage, horizon or interval check in
/// `MarketStatePack::new` — the matching refusal below is then admitted.
#[test]
fn a_pack_missing_its_time_bounds_lineage_or_horizon_is_refused() -> Result<()> {
    let events = [event("itch-a", 1, t(10))?];
    assert!(
        pack(&events).is_ok(),
        "premise: the well-formed pack builds"
    );

    let build = |from, to, horizon, lineage: Vec<String>| {
        MarketStatePack::new(StateKind::Flow, from, to, horizon, lineage, &events)
    };
    let h = Duration::from_millis(500);
    assert!(
        build(t(100), t(0), h, lineage()).is_err(),
        "inverted bounds"
    );
    assert!(build(t(0), t(0), h, lineage()).is_err(), "empty interval");
    assert!(
        build(t(0), t(100), Duration::ZERO, lineage()).is_err(),
        "zero horizon"
    );
    assert!(build(t(0), t(100), h, vec![]).is_err(), "no lineage");
    assert!(
        build(t(0), t(100), h, vec!["  ".to_string()]).is_err(),
        "blank lineage"
    );
    assert!(
        MarketStatePack::new(StateKind::Flow, t(0), t(5), h, lineage(), &events).is_err(),
        "an event after the interval's end"
    );
    Ok(())
}
