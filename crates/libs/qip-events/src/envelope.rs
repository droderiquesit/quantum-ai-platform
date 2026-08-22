//! Event envelopes.
//!
//! Every event carries the same metadata regardless of payload: identity,
//! topic, schema version, both timestamps, lineage and an idempotency key.
//! The distinction between `occurred_at` and `recorded_at` is load-bearing —
//! point-in-time correctness depends on knowing when the platform *learned*
//! something, not just when it was true.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{EventId, Lineage, Timestamp};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::topic::Topic;

/// A payload that can travel on the bus.
///
/// Implementors declare their topic and schema version as associated
/// constants, so the pairing is fixed at compile time.
pub trait EventBody:
    Serialize + DeserializeOwned + std::fmt::Debug + Clone + Send + Sync + 'static
{
    const TOPIC: Topic;
    const SCHEMA_VERSION: u32;

    /// Key used to suppress duplicates. Defaults to no deduplication.
    ///
    /// Adapters that may re-deliver (a reconnecting feed, a retried webhook)
    /// override this with a stable natural key.
    fn idempotency_key(&self) -> Option<String> {
        None
    }
}

/// A typed event with its metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub event_id: EventId,
    pub topic: Topic,
    pub schema_version: u32,
    /// When the fact was true in the world.
    pub occurred_at: Timestamp,
    /// When the platform recorded it.
    pub recorded_at: Timestamp,
    /// Position in the log. Assigned on append; zero before that.
    #[serde(default)]
    pub sequence: u64,
    pub lineage: Lineage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub body: T,
}

impl<T: EventBody> Envelope<T> {
    /// Build an envelope for `body`.
    pub fn new(
        event_id: EventId,
        occurred_at: Timestamp,
        recorded_at: Timestamp,
        lineage: Lineage,
        body: T,
    ) -> Self {
        let idempotency_key = body.idempotency_key();
        Self {
            event_id,
            topic: T::TOPIC,
            schema_version: T::SCHEMA_VERSION,
            occurred_at,
            recorded_at,
            sequence: 0,
            lineage,
            idempotency_key,
            body,
        }
    }

    /// Erase the payload type for storage and transport.
    pub fn erase(&self) -> Result<AnyEvent> {
        let payload = serde_json::to_value(&self.body)?;
        let payload_hash = sha256_hex(canonical_json(&payload).as_bytes());
        Ok(AnyEvent {
            event_id: self.event_id.clone(),
            topic: self.topic,
            schema_version: self.schema_version,
            occurred_at: self.occurred_at,
            recorded_at: self.recorded_at,
            sequence: self.sequence,
            lineage: self.lineage.clone(),
            idempotency_key: self.idempotency_key.clone(),
            payload,
            payload_hash,
        })
    }
}

/// A type-erased event, as stored in the log and carried on the bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnyEvent {
    pub event_id: EventId,
    pub topic: Topic,
    pub schema_version: u32,
    pub occurred_at: Timestamp,
    pub recorded_at: Timestamp,
    #[serde(default)]
    pub sequence: u64,
    pub lineage: Lineage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub payload: serde_json::Value,
    /// Content hash of the canonicalised payload, for deduplication and audit.
    pub payload_hash: String,
}

impl AnyEvent {
    /// Recover the typed payload.
    ///
    /// Rejects a topic mismatch outright, and refuses to read a payload written
    /// by a *newer* schema than this build understands — silently ignoring
    /// unknown fields there would mean acting on a partially understood event.
    pub fn decode<T: EventBody>(&self) -> Result<Envelope<T>> {
        if self.topic != T::TOPIC {
            return Err(Error::schema(format!(
                "cannot decode {} as {}",
                self.topic,
                T::TOPIC
            )));
        }
        if self.schema_version > T::SCHEMA_VERSION {
            return Err(Error::schema(format!(
                "{} schema version {} is newer than the supported {}",
                self.topic,
                self.schema_version,
                T::SCHEMA_VERSION
            )));
        }
        let body: T = serde_json::from_value(self.payload.clone())?;
        Ok(Envelope {
            event_id: self.event_id.clone(),
            topic: self.topic,
            schema_version: self.schema_version,
            occurred_at: self.occurred_at,
            recorded_at: self.recorded_at,
            sequence: self.sequence,
            lineage: self.lineage.clone(),
            idempotency_key: self.idempotency_key.clone(),
            body,
        })
    }

    /// Deduplication key: the explicit idempotency key if present, otherwise
    /// the topic and payload hash.
    pub fn dedup_key(&self) -> String {
        match &self.idempotency_key {
            Some(key) => format!("{}:{key}", self.topic.name()),
            None => format!("{}:{}", self.topic.name(), self.payload_hash),
        }
    }

    /// Latency between the fact and the platform recording it.
    pub fn ingestion_latency(&self) -> qip_core::Duration {
        self.recorded_at.since(self.occurred_at)
    }

    /// A short human-readable description for logs and the UI.
    pub fn summary(&self) -> String {
        format!(
            "{} #{} at {} [{}]",
            self.topic.name(),
            self.sequence,
            self.occurred_at.to_rfc3339(),
            self.lineage.correlation_id
        )
    }
}

/// Serialise a JSON value with object keys in sorted order.
///
/// `serde_json::Value` uses a map that already preserves insertion order, but
/// two logically identical payloads built in different field orders would hash
/// differently. Canonicalising first makes the content hash a genuine identity.
pub fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String((*k).clone()),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}
