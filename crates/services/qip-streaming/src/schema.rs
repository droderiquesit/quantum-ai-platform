//! The envelope's on-the-wire shape, and how it survives a version skew.
//!
//! # Two schemas, two policies
//!
//! An event has two independent versions and they are governed differently.
//!
//! The **body** schema is `qip_events::EventBody::SCHEMA_VERSION`, and
//! `qip_events::AnyEvent::decode` refuses a payload written by a newer one.
//! That is the right policy there: the body is what a decision is computed
//! from, and quietly ignoring fields a newer producer added means acting on an
//! event this build only partly understands.
//!
//! The **envelope** schema is [`ENVELOPE_SCHEMA_VERSION`], and it does the
//! opposite: a newer publisher's extra fields are ignored and the envelope
//! decodes. The envelope is routing and provenance metadata, so an unknown
//! field is a hint this build has no use for — refusing it would take a rolling
//! upgrade and turn it into an outage, with the older half of the fleet
//! rejecting everything the newer half publishes. The asymmetry is deliberate
//! and it is the reason the two versions are separate numbers.
//!
//! # Absent is not zero
//!
//! [`EnvelopeWire::confidence`] and [`EnvelopeWire::cost`] arrived in envelope
//! version 2 and are `Option`. An older publisher omits them, and they decode
//! as `None` rather than as `0`. That distinction is load-bearing in both
//! directions: a confidence of zero means the source says the record is
//! worthless and every consumer should discard it, while an absent confidence
//! means the source never spoke. A cost of zero means the data is free, and a
//! cost router that read an absent cost as free would route every flow to the
//! most expensive vendor on the list.

use qip_core::error::Result;
use qip_events::AnyEvent;
use serde::{Deserialize, Serialize};

use crate::envelope::StreamEnvelope;
use crate::provenance::{Confidence, CostMetadata, SourceIdentity, Subject};
use crate::routing::RoutingClass;

/// The envelope schema this build writes.
pub const ENVELOPE_SCHEMA_VERSION: u32 = 2;

/// The version assumed when a wire form predates the version field itself.
pub const IMPLIED_ENVELOPE_VERSION: u32 = 1;

/// Every field name this build knows about, so an unknown one can be reported
/// rather than silently dropped.
pub const KNOWN_FIELDS: [&str; 7] = [
    "envelope_version",
    "source",
    "subject",
    "routing_class",
    "confidence",
    "cost",
    "reported_ingest_timestamp",
];

/// Fields that did not exist in every envelope version, and the version that
/// introduced each.
///
/// Consulted only to *describe* a decode. Nothing branches on it, because a
/// field's absence is already represented by `None` and a second
/// representation of the same fact is a second thing to keep in step.
const INTRODUCED_IN: [(&str, u32); 2] = [("confidence", 2), ("cost", 2)];

/// The envelope exactly as it is written and read.
///
/// The event is nested under `event` rather than flattened into the top level
/// so that `qip_events::AnyEvent` stays the single definition of those fields.
/// A flatten would fork the definition, and the fork would be discovered the
/// first time `AnyEvent` gained a field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeWire {
    /// Absent in the first version of the envelope, which predates the field.
    #[serde(default = "implied_version")]
    pub envelope_version: u32,
    pub source: SourceIdentity,
    #[serde(default)]
    pub subject: Subject,
    pub routing_class: RoutingClass,
    /// Absent means the source never stated one. It does not mean zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// Absent means nobody metered this event. It does not mean free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostMetadata>,
    /// The ingest time as the source reported it, before the forward clamp.
    ///
    /// Kept alongside the clamped value so the clamp is visible after the fact:
    /// a source whose clock runs ahead is a fault to be found, and the only
    /// evidence of it is the difference between these two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_ingest_timestamp: Option<qip_core::Timestamp>,
    pub event: AnyEvent,
}

fn implied_version() -> u32 {
    IMPLIED_ENVELOPE_VERSION
}

/// What happened when a wire form met this build's schema.
///
/// Returned alongside the envelope rather than logged, because "this decoded,
/// but from a publisher two versions ahead" is something a consumer may want to
/// act on and a metric cannot be acted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaCompatibility {
    /// The publisher writes the same envelope version this build does.
    Exact,
    /// The publisher is ahead. The named fields were present and ignored.
    Forward {
        publisher_version: u32,
        ignored_fields: Vec<String>,
    },
    /// The publisher is behind. The named fields are absent — not zero.
    Backward {
        publisher_version: u32,
        absent_fields: Vec<&'static str>,
    },
}

impl SchemaCompatibility {
    /// Whether the decode lost information this build would have used.
    pub fn is_lossy(&self) -> bool {
        matches!(self, Self::Forward { ignored_fields, .. } if !ignored_fields.is_empty())
    }

    /// A line an operator can read.
    pub fn describe(&self) -> String {
        match self {
            Self::Exact => format!("envelope version {ENVELOPE_SCHEMA_VERSION}, exact"),
            Self::Forward {
                publisher_version,
                ignored_fields,
            } => format!(
                "publisher writes envelope version {publisher_version}, this build reads \
                 {ENVELOPE_SCHEMA_VERSION}; ignored: [{}]",
                ignored_fields.join(", ")
            ),
            Self::Backward {
                publisher_version,
                absent_fields,
            } => format!(
                "publisher writes envelope version {publisher_version}, this build reads \
                 {ENVELOPE_SCHEMA_VERSION}; absent (not zero): [{}]",
                absent_fields.join(", ")
            ),
        }
    }
}

/// Decode a wire form and say what the version skew cost.
///
/// Takes a `serde_json::Value` rather than bytes because the unknown-field
/// report needs the raw object: serde discards unknown keys before any typed
/// value exists, so by the time there is an [`EnvelopeWire`] the evidence is
/// gone.
pub fn decode(value: &serde_json::Value) -> Result<(StreamEnvelope, SchemaCompatibility)> {
    let wire: EnvelopeWire = serde_json::from_value(value.clone())?;
    let compatibility = compatibility_of(value, wire.envelope_version);
    let envelope = StreamEnvelope::try_from(wire)?;
    Ok((envelope, compatibility))
}

/// Classify a raw wire object against this build's schema.
pub fn compatibility_of(value: &serde_json::Value, publisher_version: u32) -> SchemaCompatibility {
    if publisher_version > ENVELOPE_SCHEMA_VERSION {
        let mut ignored_fields: Vec<String> = value
            .as_object()
            .map(|map| {
                map.keys()
                    .filter(|key| key.as_str() != "event" && !KNOWN_FIELDS.contains(&key.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        ignored_fields.sort();
        return SchemaCompatibility::Forward {
            publisher_version,
            ignored_fields,
        };
    }
    if publisher_version < ENVELOPE_SCHEMA_VERSION {
        let present = value.as_object();
        let absent_fields: Vec<&'static str> = INTRODUCED_IN
            .iter()
            .filter(|(field, introduced)| {
                *introduced > publisher_version
                    && present.is_none_or(|map| !map.contains_key(*field))
            })
            .map(|(field, _)| *field)
            .collect();
        return SchemaCompatibility::Backward {
            publisher_version,
            absent_fields,
        };
    }
    SchemaCompatibility::Exact
}
