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

/// ADR 0151 decision 2: a pack is never built over zero events. Without the
/// refusal an interval in which nothing was drained yields a pack with no
/// integrity flag, which a consumer reads as a clean tape that nobody read.
///
/// Mutation: delete the `events.is_empty()` refusal in `MarketStatePack::new`
/// — the empty pack builds with no flag and the last assertion fails.
#[test]
fn a_pack_over_no_events_is_refused_rather_than_reported_as_a_clean_tape() -> Result<()> {
    let one = [event("itch-a", 1, t(10))?];
    let clean = pack(&one)?;
    assert!(
        clean.integrity().is_empty(),
        "premise: a pack over one in-order event is clean"
    );
    let empty = pack(&[]);
    let Err(refusal) = empty else {
        panic!("a pack over zero events was admitted");
    };
    assert!(
        refusal.message().contains("at least one event"),
        "the refusal names the missing events: {}",
        refusal.message()
    );
    Ok(())
}

/// The pack encoded as a JSON object, for tests that tamper with its fields.
fn encoded(pack: &MarketStatePack) -> Result<serde_json::Map<String, serde_json::Value>> {
    let serde_json::Value::Object(fields) = serde_json::to_value(pack)? else {
        panic!("a market state pack encodes as a JSON object");
    };
    Ok(fields)
}

fn decode(
    fields: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Result<MarketStatePack> {
    serde_json::from_value(serde_json::Value::Object(fields))
}

/// R12-CONTRACT-7072: a pack that crosses between brains keeps every field —
/// the interval, the horizon, the lineage, and above all the integrity flag,
/// because a pack that lost its gap flag on the wire would arrive claiming a
/// clean tape.
///
/// Mutation: drop `integrity` from `MarketStatePackWire`'s `From` impl (encode
/// `Vec::new()`) — the decoded pack is then clean and `assert_eq!` fails.
#[test]
fn a_pack_round_trips_through_its_wire_form_keeping_every_field() -> Result<()> {
    let gapped = pack(&[event("itch-a", 1, t(10))?, event("itch-a", 4, t(20))?])?;
    assert!(
        !gapped.integrity().is_empty(),
        "premise: the pack carries a flag the round trip could lose"
    );
    let line = serde_json::to_string(&gapped)?;
    let back: MarketStatePack = serde_json::from_str(&line)?;
    assert_eq!(back, gapped);
    assert_eq!(back.kind(), StateKind::Book);
    assert_eq!((back.from(), back.to()), (t(0), t(100)));
    assert_eq!(back.horizon(), Duration::from_millis(500));
    assert_eq!(back.feature_lineage(), lineage().as_slice());
    assert_eq!(
        back.integrity(),
        [IntegrityFlag::SequenceGap {
            stream: "XNYS/itch-a/0".to_string(),
            missing: 2
        }]
    );
    Ok(())
}

/// R12-CONTRACT-7072: every one of the six fields is required on decode. An
/// absent `integrity` defaulted to `[]` would be a clean tape claimed by
/// omission; an absent `horizon` defaulted to zero would be a state with no
/// expiry.
///
/// Mutation: put `#[serde(default)]` on `integrity` in `MarketStatePackWire`
/// — the line without it then decodes and the loop's assertion fails naming
/// `integrity`.
#[test]
fn a_pack_line_missing_any_required_field_is_refused_on_decode() -> Result<()> {
    let whole = encoded(&pack(&[event("itch-a", 1, t(10))?])?)?;
    assert!(
        decode(whole.clone()).is_ok(),
        "premise: the untampered line decodes"
    );
    let required = [
        "kind",
        "from",
        "to",
        "horizon",
        "feature_lineage",
        "integrity",
    ];
    assert_eq!(
        whole.keys().map(String::as_str).collect::<Vec<_>>().len(),
        required.len(),
        "premise: the wire form has exactly the six fields this test removes"
    );
    for field in required {
        let mut fields = whole.clone();
        assert!(
            fields.remove(field).is_some(),
            "premise: {field} is on the wire"
        );
        assert!(
            decode(fields).is_err(),
            "a pack line without `{field}` was admitted"
        );
    }
    Ok(())
}

/// The constructor's refusals are re-run on decode: a line naming an inverted
/// interval, a zero horizon or a blank lineage is refused even though it
/// parses. Before 2026-11-12 the derived `Deserialize` admitted all three.
///
/// Mutation: make `TryFrom<MarketStatePackWire>` skip `check_shape` — the
/// inverted interval decodes and the first refusal assertion fails.
#[test]
fn a_pack_line_the_constructor_would_refuse_is_refused_on_decode() -> Result<()> {
    let whole = encoded(&pack(&[event("itch-a", 1, t(10))?])?)?;
    assert!(
        decode(whole.clone()).is_ok(),
        "premise: the untampered line decodes"
    );

    let with = |field: &str, value: serde_json::Value| {
        let mut fields = whole.clone();
        fields.insert(field.to_string(), value);
        decode(fields)
    };
    let inverted = with("from", whole["to"].clone());
    let Err(refusal) = inverted else {
        panic!("a pack line with `from` not before `to` was admitted");
    };
    assert!(
        refusal.to_string().contains("strictly before"),
        "the refusal names the interval: {refusal}"
    );
    assert!(
        with("horizon", serde_json::to_value(Duration::ZERO)?).is_err(),
        "zero horizon"
    );
    assert!(
        with("feature_lineage", serde_json::json!([])).is_err(),
        "no lineage"
    );
    assert!(
        with("feature_lineage", serde_json::json!([" "])).is_err(),
        "blank lineage"
    );
    Ok(())
}

/// A flag no gap check could have computed — zero missing messages, or a
/// blank stream — is refused on decode; and an unknown field is refused
/// rather than dropped, so a sender's extra claim is not silently lost.
///
/// Mutation: delete the `*missing == 0` condition in
/// `TryFrom<MarketStatePackWire>` — the zero-gap line decodes and the first
/// assertion fails.
#[test]
fn a_pack_line_with_an_impossible_flag_or_an_unknown_field_is_refused() -> Result<()> {
    let whole = encoded(&pack(&[event("itch-a", 1, t(10))?])?)?;
    assert!(
        decode(whole.clone()).is_ok(),
        "premise: the untampered line decodes"
    );
    let flag = |stream: &str, missing: u64| {
        let mut fields = whole.clone();
        fields.insert(
            "integrity".to_string(),
            serde_json::json!([{"sequence_gap": {"stream": stream, "missing": missing}}]),
        );
        decode(fields)
    };
    assert!(
        flag("XNYS/itch-a/0", 1).is_ok(),
        "premise: a real flag decodes"
    );
    assert!(flag("XNYS/itch-a/0", 0).is_err(), "a gap of zero messages");
    assert!(flag("  ", 1).is_err(), "a gap on a blank stream");
    let mut extra = whole.clone();
    extra.insert("clean".to_string(), serde_json::json!(true));
    assert!(decode(extra).is_err(), "an unknown field");
    Ok(())
}
