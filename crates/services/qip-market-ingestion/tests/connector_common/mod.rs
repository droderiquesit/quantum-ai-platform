//! A minimal connector, so the runtime's behaviours can be exercised without
//! any one source's quirks standing in for the general case.
//!
//! `allow(dead_code)` per item: each integration-test binary compiles this
//! module on its own, so a helper only one binary needs is genuinely unused in
//! the others. The attribute sits on the items rather than the module so that
//! a helper no binary uses is still reported.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::quote::Tick;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::connector::{Cursor, RawEvent, SourceConnector, SourceManifest};
use serde_json::Value;

/// A manifest whose every interesting field a test can move.
///
/// Written as JSON rather than built with struct literals because that is how
/// a real source arrives, and because it is what proves the parse path — a
/// manifest that only ever existed as Rust would not catch a field this crate
/// renamed.
#[allow(dead_code)]
pub(crate) fn manifest_json(
    source_id: &str,
    publication_delay_ms: i64,
    schema_version: &str,
) -> String {
    format!(
        r#"{{
  "source_id": "{source_id}",
  "provider": "a source that exists to be tested",
  "asset_class": "crypto",
  "region": "global",
  "protocol": "rest",
  "schema": {{
    "version": "{schema_version}",
    "required_fields": [
      {{ "path": "events", "kind": "array" }}
    ],
    "unknown_fields": "ignore"
  }},
  "auth": {{ "scheme": "none" }},
  "endpoint": {{
    "base_url": "http://source.test:8080",
    "path": "/v1/events",
    "health_path": "/v1/health"
  }},
  "rate_limit": {{ "requests": 10, "per_ms": 1000, "burst": 10 }},
  "retry": {{
    "max_attempts": 3,
    "initial_backoff_ms": 100,
    "max_backoff_ms": 2000,
    "multiplier": 4,
    "jitter_basis_points": 2500
  }},
  "poll_interval_ms": 1000,
  "freshness_sla_ms": 60000,
  "publication_delay_ms": {publication_delay_ms},
  "licensing": "public",
  "max_events_per_batch": 16
}}"#
    )
}

/// The manifest most tests use: no dissemination delay, schema 1.0.
#[allow(dead_code)]
pub(crate) fn manifest() -> SourceManifest {
    SourceManifest::from_json(&manifest_json("test-source", 0, "1.0"))
        .expect("the fixture manifest is valid")
}

/// A source that publishes fifteen minutes after the fact.
#[allow(dead_code)]
pub(crate) fn delayed_manifest(delay_ms: i64) -> SourceManifest {
    SourceManifest::from_json(&manifest_json("test-source", delay_ms, "1.0"))
        .expect("the delayed fixture manifest is valid")
}

/// A connector over `{"events":[{"key":…,"at":…,"price":…}]}`.
#[derive(Clone, Debug)]
pub(crate) struct TestConnector {
    manifest: SourceManifest,
    object_id: ObjectId,
}

impl TestConnector {
    #[allow(dead_code)]
    pub(crate) fn new(manifest: SourceManifest) -> Self {
        Self {
            manifest,
            object_id: ObjectId::from_string("OBJ00000000000000000TEST"),
        }
    }
}

impl SourceConnector for TestConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// A source that stamps its own version in the body, so the version gate
    /// has something to read.
    fn declared_version(
        &self,
        payload: &Value,
    ) -> Option<qip_market_ingestion::connector::SchemaVersion> {
        payload
            .get("schema_version")
            .and_then(Value::as_str)
            .and_then(|text| qip_market_ingestion::connector::SchemaVersion::parse(text).ok())
    }

    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let items = payload
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::schema("the payload has no `events` array"))?;
        let mut events = Vec::with_capacity(items.len());
        for item in items {
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::schema("an event with no `key`"))?;
            let at = item
                .get("at")
                .and_then(Value::as_str)
                .and_then(Timestamp::parse_rfc3339)
                .ok_or_else(|| Error::schema("an event with no readable `at`"))?;
            events.push(RawEvent::new(key, at, item.clone()));
        }
        Ok(events)
    }

    fn map(&self, event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
        let price = event
            .body
            .get("price")
            .and_then(Value::as_str)
            .and_then(Decimal::parse)
            .ok_or_else(|| {
                Error::schema(format!(
                    "the event {} has no exact `price` this platform can hold",
                    event.key
                ))
            })?;
        Ok(SensedRecord::Tick(Tick {
            object_id: self.object_id.clone(),
            venue: "TEST".to_string(),
            at: event.event_time,
            price,
            volume: Decimal::parse("1").unwrap_or_default(),
            quality: DataQuality::clean(),
        }))
    }
}

/// A body carrying the given events, as the source would send it.
#[allow(dead_code)]
pub(crate) fn body(events: &[(&str, &str, &str)]) -> String {
    let items: Vec<String> = events
        .iter()
        .map(|(key, at, price)| format!(r#"{{"key":"{key}","at":"{at}","price":"{price}"}}"#))
        .collect();
    format!(r#"{{"events":[{}]}}"#, items.join(","))
}

#[allow(dead_code)]
pub(crate) fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}
