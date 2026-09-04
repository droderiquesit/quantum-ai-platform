//! What the envelope type guarantees rather than documents.
//!
//! Three claims are under test here, and each is a claim about what *cannot*
//! be constructed: an envelope whose hash disagrees with its payload, an
//! envelope whose ingest time precedes its event time, and a stored freshness
//! that has drifted from the timestamps it came from.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, instrument, to_wire, warm_note};
use qip_core::Rng;
use qip_core::error::{Error, Result};
use qip_core::testing::{Property, approx_eq};
use qip_core::{Decimal, Duration, Timestamp};
use qip_events::Topic;
use qip_streaming::{Confidence, StreamEnvelope};

#[test]
fn an_envelope_whose_payload_hash_disagrees_with_its_payload_cannot_be_built() -> Result<()> {
    let envelope = hot_tick(1, at(0), at(1))?;
    let mut wire = to_wire(&envelope)?;

    // Edit the payload without touching the hash — the exact tampering the
    // hash exists to detect, and the only way to attempt it, because the
    // envelope's own event is private and has no setter.
    let payload = wire
        .get_mut("event")
        .and_then(|event| event.get_mut("payload"))
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| Error::invalid("the wire form has no payload object"))?;
    payload.insert("price".into(), serde_json::json!("999999.0"));

    let outcome: std::result::Result<StreamEnvelope, _> = serde_json::from_value(wire);
    let Err(error) = outcome else {
        panic!("a payload edited out from under its hash was accepted");
    };
    assert!(
        error.to_string().contains("payload hash mismatch"),
        "the refusal must name what disagreed: {error}"
    );
    Ok(())
}

#[test]
fn editing_the_hash_to_match_a_forged_payload_is_still_refused_by_the_real_hash() -> Result<()> {
    let envelope = hot_tick(1, at(0), at(1))?;
    let mut wire = to_wire(&envelope)?;
    let event = wire
        .get_mut("event")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| Error::invalid("the wire form has no event object"))?;
    // A hash that is well-formed and simply wrong: the check recomputes rather
    // than compares shapes, so a plausible-looking hash is no better than an
    // absent one.
    event.insert("payload_hash".into(), serde_json::json!("0".repeat(64)));

    let outcome: std::result::Result<StreamEnvelope, _> = serde_json::from_value(wire);
    assert!(outcome.is_err(), "a forged hash was accepted");
    Ok(())
}

#[test]
fn a_sealed_envelope_round_trips_through_json_unchanged() -> Result<()> {
    for envelope in [hot_tick(7, at(0), at(3))?, warm_note("N1", at(0), at(9))?] {
        let wire = to_wire(&envelope)?;
        let recovered: StreamEnvelope = serde_json::from_value(wire)?;
        assert_eq!(
            recovered, envelope,
            "the wire form is not a faithful representation of the envelope"
        );
        recovered.verify_payload_hash()?;
    }
    Ok(())
}

#[test]
fn ingest_before_the_event_is_clamped_forward_and_the_clamp_is_visible() -> Result<()> {
    // A source whose clock runs 250ms ahead: it dates the delivery before the
    // thing it is delivering, which is physically impossible.
    let event_at = at(1_000);
    let reported = at(750);
    let envelope = hot_tick(1, event_at, reported)?;

    assert_eq!(
        envelope.ingest_timestamp(),
        event_at,
        "known-time must be clamped forward to valid-time"
    );
    assert_eq!(envelope.reported_ingest_timestamp(), reported);
    assert!(envelope.was_clamped(), "the clamp must be visible");
    assert_eq!(
        envelope.clock_correction(),
        Duration::from_millis(250),
        "the correction must say how far the source's clock was out"
    );
    // The clamp survives the wire, because the reported value is carried
    // alongside the corrected one rather than being thrown away.
    let recovered: StreamEnvelope = serde_json::from_value(to_wire(&envelope)?)?;
    assert!(recovered.was_clamped());
    assert_eq!(recovered.clock_correction(), Duration::from_millis(250));
    Ok(())
}

#[test]
fn a_sane_pair_of_timestamps_is_left_alone_and_reports_no_clamp() -> Result<()> {
    let envelope = hot_tick(1, at(1_000), at(1_400))?;
    assert!(!envelope.was_clamped());
    assert_eq!(envelope.clock_correction(), Duration::ZERO);
    assert_eq!(envelope.ingest_timestamp(), at(1_400));
    Ok(())
}

#[test]
fn the_clamp_holds_for_every_ordering_of_the_two_timestamps() {
    Property::new("ingest never precedes the event")
        .cases(256)
        .for_all(
            |rng| {
                let event_ms = rng.below(10_000) as i64;
                let ingest_ms = rng.below(10_000) as i64;
                (event_ms, ingest_ms)
            },
            |(event_ms, ingest_ms)| {
                let envelope = hot_tick(1, at(*event_ms), at(*ingest_ms))
                    .map_err(|error| format!("seal failed: {error}"))?;
                if envelope.ingest_timestamp() < envelope.event_timestamp() {
                    return Err("ingest precedes the event after clamping".to_string());
                }
                let clamped_expected = ingest_ms < event_ms;
                if envelope.was_clamped() != clamped_expected {
                    return Err(format!(
                        "clamp visibility is wrong: reported {} for ingest {ingest_ms} vs event \
                     {event_ms}",
                        envelope.was_clamped()
                    ));
                }
                Ok(())
            },
        );
}

#[test]
fn freshness_is_derived_from_the_timestamps_and_an_as_of() -> Result<()> {
    let envelope = hot_tick(1, at(1_000), at(1_200))?;
    let freshness = envelope.freshness(at(1_500));

    assert_eq!(freshness.age, Duration::from_millis(500));
    assert_eq!(freshness.ingest_lag, Duration::from_millis(200));
    assert_eq!(freshness.delivery_lag, Duration::from_millis(300));
    assert_eq!(
        freshness.ingest_lag + freshness.delivery_lag,
        freshness.age,
        "the two lags must account for the whole age"
    );

    // Derived, not stored: a later as-of gives a larger age from the same
    // envelope, and there is no field anywhere that could disagree.
    assert!(envelope.freshness(at(2_000)).age > freshness.age);
    assert!(freshness.is_within(Duration::from_secs(1)));
    assert!(!freshness.is_within(Duration::from_millis(100)));

    let wire = to_wire(&envelope)?;
    let object = wire
        .as_object()
        .ok_or_else(|| qip_core::error::Error::invalid("the wire form is not an object"))?;
    assert!(
        !object.contains_key("freshness"),
        "freshness must not be a stored field; it would drift from the timestamps"
    );
    Ok(())
}

#[test]
fn the_envelope_reuses_the_fields_qip_events_already_defines() -> Result<()> {
    let envelope = hot_tick(42, at(0), at(5))?;

    // These are not parallel copies: each accessor reads the inner AnyEvent.
    let inner = envelope.event();
    assert_eq!(envelope.event_id(), &inner.event_id);
    assert_eq!(envelope.event_type(), inner.topic);
    assert_eq!(envelope.event_type(), Topic::MarketTick);
    assert_eq!(envelope.event_timestamp(), inner.occurred_at);
    assert_eq!(envelope.ingest_timestamp(), inner.recorded_at);
    assert_eq!(envelope.schema_version(), inner.schema_version);
    assert_eq!(envelope.payload_hash(), inner.payload_hash);
    assert_eq!(envelope.lineage(), &inner.lineage);
    assert_eq!(envelope.idempotency_key(), inner.dedup_key());

    // The source sequence is the publisher's, not the log's position.
    assert_eq!(envelope.sequence_number(), Some(42));
    assert_eq!(inner.sequence, 0, "the log assigns its own position");
    assert_eq!(envelope.instrument(), Some(&instrument()));
    assert_eq!(
        envelope.venue().map(qip_contracts::VenueId::as_str),
        Some(common::VENUE)
    );
    Ok(())
}

#[test]
fn the_typed_body_survives_the_envelope() -> Result<()> {
    let envelope = hot_tick(3, at(0), at(1))?;
    let decoded = envelope.decode::<qip_market::quote::Tick>()?;
    assert_eq!(decoded.body.price, Decimal::from_int(103));
    assert_eq!(decoded.occurred_at, at(0));
    Ok(())
}

#[test]
fn confidence_outside_the_unit_interval_is_refused_on_the_wire() -> Result<()> {
    assert!(Confidence::new(1.5).is_err());
    assert!(Confidence::new(f64::NAN).is_err());
    assert!(approx_eq(Confidence::new(0.25)?.value(), 0.25, 1e-12));

    let envelope = hot_tick(1, at(0), at(1))?;
    let mut wire = to_wire(&envelope)?;
    let object = wire
        .as_object_mut()
        .ok_or_else(|| Error::invalid("the wire form is not an object"))?;
    object.insert("confidence".into(), serde_json::json!(2.0));
    let outcome: std::result::Result<StreamEnvelope, _> = serde_json::from_value(wire);
    assert!(
        outcome.is_err(),
        "a confidence of 2.0 must not decode as certainty"
    );
    Ok(())
}

#[test]
fn an_event_is_knowable_only_from_its_ingest_time_onwards() -> Result<()> {
    let envelope = hot_tick(1, at(100), at(400))?;
    assert!(!envelope.was_known_by(at(399)));
    assert!(envelope.was_known_by(at(400)));
    // Valid-time is earlier and must not be what a point-in-time read uses.
    assert!(envelope.event_timestamp() < Timestamp::from_nanos(at(400).as_nanos()));
    Ok(())
}

#[test]
fn one_fact_has_one_fingerprint_at_the_envelope_and_at_its_frame() -> Result<()> {
    // Both arms of `AnyEvent::dedup_key`, because they were wrong in
    // different ways: a body with its own key, and one without.
    let keyed = hot_tick(7, at(0), at(5))?;
    let unkeyed = warm_note("N", at(0), at(5))?;
    assert!(
        keyed.event().idempotency_key.is_some(),
        "the premise: this body supplies its own key"
    );
    assert!(
        unkeyed.event().idempotency_key.is_none(),
        "the premise: this body supplies none, so the payload hash must serve"
    );

    for envelope in [&keyed, &unkeyed] {
        let frame = envelope.to_frame()?;
        assert_eq!(
            frame.dedup_key(),
            envelope.idempotency_key(),
            "the mesh window keys on the frame and this side keys on the envelope; a fact whose \
             two keys differ is deduplicated on neither wire"
        );
    }
    Ok(())
}

#[test]
fn the_contract_stamp_reports_the_clamp_the_envelope_applied() -> Result<()> {
    // Delivered at t=200 a fact that was true at t=400: physically impossible,
    // so a clock or a parsing fault at the source.
    let clamped = hot_tick(1, at(400), at(200))?;
    assert!(
        clamped.was_clamped(),
        "the premise: this envelope really was clamped"
    );
    assert_eq!(clamped.clock_correction(), Duration::from_millis(200));

    let stamped = clamped.stamped();
    assert!(
        stamped.was_clamped(),
        "the envelope and the contract type must not disagree about whether a source's clock was \
         wrong; a flag that cannot be true reads as protection and is none"
    );
    assert_eq!(stamped.known_at(), clamped.ingest_timestamp());
    assert_eq!(stamped.valid_at(), clamped.event_timestamp());

    // The other half: an envelope whose clock was sane must not be reported as
    // corrected, or the flag is true of everything and says nothing.
    let sane = hot_tick(2, at(100), at(300))?;
    assert!(!sane.was_clamped());
    assert!(!sane.stamped().was_clamped());
    assert_eq!(sane.stamped().known_at(), at(300));
    Ok(())
}
