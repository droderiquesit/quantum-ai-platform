//! Version skew in both directions.
//!
//! The property is that a rolling upgrade is possible: the older half of a
//! fleet must keep reading what the newer half publishes, and the newer half
//! must not read an older publisher's silence as a value it never sent.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, to_wire};
use qip_core::error::{Error, Result};
use qip_streaming::schema::{self, SchemaCompatibility};
use qip_streaming::{Confidence, ENVELOPE_SCHEMA_VERSION, StreamEnvelope};

/// Mutate the wire object, or fail loudly if it is not one.
fn edit(
    envelope: &StreamEnvelope,
    mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> Result<serde_json::Value> {
    let mut wire = to_wire(envelope)?;
    let object = wire
        .as_object_mut()
        .ok_or_else(|| Error::invalid("the wire form is not an object"))?;
    mutate(object);
    Ok(wire)
}

#[test]
fn this_build_writes_its_own_envelope_version_and_reads_it_back_exactly() -> Result<()> {
    let envelope = hot_tick(1, at(0), at(1))?;
    let wire = to_wire(&envelope)?;
    assert_eq!(
        wire.get("envelope_version")
            .and_then(serde_json::Value::as_u64),
        Some(u64::from(ENVELOPE_SCHEMA_VERSION))
    );

    let (decoded, compatibility) = schema::decode(&wire)?;
    assert_eq!(decoded, envelope);
    assert_eq!(compatibility, SchemaCompatibility::Exact);
    assert!(!compatibility.is_lossy());
    Ok(())
}

#[test]
fn a_newer_publisher_with_extra_trailing_fields_decodes_identically() -> Result<()> {
    let envelope = hot_tick(4, at(0), at(2))?;
    let baseline = to_wire(&envelope)?;

    let ahead = edit(&envelope, |object| {
        object.insert(
            "envelope_version".into(),
            serde_json::json!(ENVELOPE_SCHEMA_VERSION + 2),
        );
        // Two fields this build has never heard of, in the shape a later
        // version would plausibly add them.
        object.insert("priority_hint".into(), serde_json::json!("urgent"));
        object.insert(
            "retention".into(),
            serde_json::json!({"class": "seven_years"}),
        );
    })?;

    let (from_ahead, compatibility) = schema::decode(&ahead)?;
    let (from_baseline, _) = schema::decode(&baseline)?;

    assert_eq!(
        from_ahead, from_baseline,
        "a newer publisher's envelope must decode to exactly what this build would have written"
    );
    match compatibility {
        SchemaCompatibility::Forward {
            publisher_version,
            ignored_fields,
        } => {
            assert_eq!(publisher_version, ENVELOPE_SCHEMA_VERSION + 2);
            assert_eq!(ignored_fields, vec!["priority_hint", "retention"]);
        }
        other => panic!("expected forward compatibility, got {other:?}"),
    }
    Ok(())
}

#[test]
fn an_older_publisher_yields_an_absent_field_rather_than_a_zero() -> Result<()> {
    let envelope = hot_tick(5, at(0), at(2))?;
    assert!(
        envelope.confidence().is_some() && envelope.cost().is_some(),
        "the fixture must actually carry the fields whose absence is under test"
    );

    // Envelope version 1 predates both `confidence` and `cost`, so an older
    // publisher simply does not write them.
    let behind = edit(&envelope, |object| {
        object.insert("envelope_version".into(), serde_json::json!(1));
        object.remove("confidence");
        object.remove("cost");
    })?;

    let (decoded, compatibility) = schema::decode(&behind)?;

    assert_eq!(
        decoded.confidence(),
        None,
        "an absent confidence must stay absent; zero would mean the source called its own \
         record worthless"
    );
    assert_ne!(
        decoded.confidence(),
        Some(Confidence::new(0.0)?),
        "absent and zero must not be the same value"
    );
    assert_eq!(
        decoded.cost(),
        None,
        "an absent cost must stay absent; zero would make an expensive vendor look free"
    );

    match compatibility {
        SchemaCompatibility::Backward {
            publisher_version,
            absent_fields,
        } => {
            assert_eq!(publisher_version, 1);
            assert_eq!(absent_fields, vec!["confidence", "cost"]);
        }
        other => panic!("expected backward compatibility, got {other:?}"),
    }
    // Everything else the older publisher did send is intact.
    assert_eq!(decoded.event_id(), envelope.event_id());
    assert_eq!(decoded.payload_hash(), envelope.payload_hash());
    assert_eq!(decoded.sequence_number(), envelope.sequence_number());
    Ok(())
}

#[test]
fn a_wire_form_predating_the_version_field_is_read_as_the_first_version() -> Result<()> {
    let envelope = hot_tick(6, at(0), at(2))?;
    let ancient = edit(&envelope, |object| {
        object.remove("envelope_version");
        object.remove("confidence");
        object.remove("cost");
    })?;

    let (_, compatibility) = schema::decode(&ancient)?;
    assert!(
        matches!(
            compatibility,
            SchemaCompatibility::Backward {
                publisher_version: 1,
                ..
            }
        ),
        "an absent version field means the publisher predates it: {compatibility:?}"
    );
    Ok(())
}

#[test]
fn a_missing_required_field_is_refused_rather_than_filled_in() -> Result<()> {
    let envelope = hot_tick(7, at(0), at(2))?;
    for required in ["source", "routing_class", "event"] {
        let broken = edit(&envelope, |object| {
            object.remove(required);
        })?;
        let outcome = schema::decode(&broken);
        assert!(
            outcome.is_err(),
            "an envelope with no {required} must not decode"
        );
    }
    Ok(())
}

#[test]
fn the_envelope_version_and_the_body_version_have_opposite_policies() -> Result<()> {
    // The envelope tolerates a newer publisher: the extra fields are routing
    // metadata this build has no use for.
    let envelope = hot_tick(8, at(0), at(2))?;
    let newer_envelope = edit(&envelope, |object| {
        object.insert("envelope_version".into(), serde_json::json!(99));
    })?;
    let (decoded, _) = schema::decode(&newer_envelope)?;
    assert_eq!(decoded.event_id(), envelope.event_id());

    // The body does not: acting on a payload written by a newer schema means
    // acting on an event this build only partly understands.
    let newer_body = edit(&envelope, |object| {
        if let Some(event) = object
            .get_mut("event")
            .and_then(serde_json::Value::as_object_mut)
        {
            event.insert("schema_version".into(), serde_json::json!(99));
        }
    })?;
    let (decoded, _) = schema::decode(&newer_body)?;
    let outcome = decoded.decode::<qip_market::quote::Tick>();
    assert!(
        outcome.is_err(),
        "a payload written by a newer body schema must not be decoded"
    );
    Ok(())
}
