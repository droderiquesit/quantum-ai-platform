//! The ECB's daily euro foreign-exchange reference rates, via Frankfurter.
//!
//! One request returns a whole rate table for one reference date:
//!
//! ```json
//! {"amount":1.0,"base":"EUR","date":"2026-09-04",
//!  "rates":{"GBP":0.85898,"JPY":181.59,"USD":1.1622}}
//! ```
//!
//! Free, unauthenticated, no signup. The recorded body above is
//! `fixtures/frankfurter-ecb-reference-rates.json`.
//!
//! # The host moved once, and the transport was right to notice
//!
//! The source was first wired as `api.frankfurter.app/latest`. On
//! 2026-09-04 that host answered every request with a `301` to
//! `https://api.frankfurter.dev/v1/latest`, and the old path on the new host
//! answered `404`. `qip_transport::http` never follows a redirect — a
//! redirect is a second destination nobody reviewed, and following one out
//! of a proxy whose whole design is one host per listener would have defeated
//! the listener — so the connector could not be opened at all, which is the
//! correct outcome and the reason [`UPSTREAM_HOST`] exists: the host is named
//! once in code, the manifest's `provider` must name the same one, and the
//! acceptance suite holds the Envoy cluster and the Terraform allowlist to
//! it. ADR 0034 requires the three to move in one commit; the constant is
//! what makes forgetting one of them a compile-time or test-time failure
//! rather than a `301` in production.
//!
//! # This is the connector that shows why three instants are not one
//!
//! The ECB fixes these rates against the euro at 14:15 CET and publishes them
//! shortly after 16:00 CET, for a reference *date*. So:
//!
//! * the **event time** is the reference date, at midnight UTC — that is what
//!   the observation is about, and it is what a time series is indexed by;
//! * the **knowable time** is sixteen hours later, from the manifest's
//!   `publication_delay_ms` — nobody could have acted on this rate at 00:01
//!   on its own reference date, because it did not exist yet;
//! * the **ingest time** is when this platform fetched it.
//!
//! A backtest that filtered on event time would have used the day's closing
//! reference rate to trade that day's open. The runtime withholds each record
//! until its knowable time, and [`crate::connector::MarketEventEnvelope`]
//! carries all three so nothing downstream has to re-derive them.
//!
//! # Why one payload becomes several events
//!
//! Each currency pair is a separate observation with its own series, its own
//! value and its own fingerprint. Publishing the table as one record would
//! make a source that dropped one currency indistinguishable from one that
//! changed every rate.
//!
//! # Why floating point here and exact decimals in the Coinbase connector
//!
//! A reference rate is a published statistic, not a price anything settles at,
//! and [`MacroObservation::value`] is `f64` because that is what the rest of
//! the platform's statistics are. A traded price is money and is
//! [`qip_core::Decimal`]. The distinction is `qip-core`'s second rule and this
//! pair of connectors is where it is visible.

use crate::adapter::SensedRecord;
use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::intelligence::MacroObservation;
use qip_financial::quality::{DataQuality, Provenance};
use serde_json::Value;

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/frankfurter-ecb-reference-rates.json");

/// A body recorded from the live endpoint, for tests and for the harness.
///
/// Provenance: fetched on 2026-09-04 at 22:55 UTC from
/// `https://api.frankfurter.dev/v1/latest?base=EUR&symbols=USD,GBP,JPY`,
/// over a TLS connection verified against the session's CA bundle, and
/// recorded byte for byte — the `date` is the ECB reference date the vendor
/// stamped, and the `latency_ms` is that fetch's wall time. The fixture's
/// own JSON refuses unknown fields, which is why the note is here and not in
/// it. The earlier recording, from `api.frankfurter.app/latest` on
/// 2026-08-24, was re-taken when that host began redirecting; a fixture
/// recorded from a host the connector no longer names is a contract test
/// against a source that no longer exists.
pub const FIXTURE: &str = include_str!("fixtures/frankfurter-ecb-reference-rates.json");

/// The ECB's reference rates as a fan-out of macro observations.
#[derive(Clone, Debug)]
pub struct FrankfurterRatesConnector {
    manifest: SourceManifest,
}

impl FrankfurterRatesConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and any licensing catalogue that
    /// admits this source can refer to it without retyping a string that
    /// would silently drift from the manifest if the two were ever edited
    /// apart — the same discipline [`crate::connectors::CoinbaseTickerConnector::SOURCE_ID`]
    /// already keeps.
    pub const SOURCE_ID: &str = "frankfurter-ecb-reference-rates";

    /// The vendor host the egress proxy dials for this source.
    ///
    /// Not a field the connector reads — the transport is pointed at the
    /// proxy, never at the vendor — but the one place in code the host is
    /// written, so that the manifest's `provider`, the Envoy cluster and the
    /// Terraform allowlist can each be held to it by a test instead of by a
    /// reviewer's memory. When the vendor moved from `api.frankfurter.app`
    /// there were three files to change and nothing that named the
    /// disagreement; see the module documentation.
    pub const UPSTREAM_HOST: &str = "api.frankfurter.dev";

    /// The region every observation carries.
    ///
    /// The euro area, because that is who publishes the series — not the
    /// quote currency's region. A GBP rate in this table is the ECB's view of
    /// sterling against the euro, and filing it under the United Kingdom would
    /// make it look like a British statistic that the Bank of England would
    /// disagree with.
    pub const REGION: &'static str = "EA";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    pub fn new(manifest: SourceManifest) -> Result<Self> {
        manifest.validate()?;
        if manifest.publication_delay().is_zero() {
            return Err(Error::invalid(format!(
                "`{}` declares no dissemination delay. These rates are published sixteen hours \
                 after the reference date they are stamped with, and a delay of zero would make \
                 every one of them knowable at midnight on its own date — which is a backtest \
                 that trades the open on the close",
                manifest.source_id
            )));
        }
        Ok(Self { manifest })
    }

    /// `2026-08-24` as midnight UTC on that date.
    fn reference_date(payload: &Value) -> Result<Timestamp> {
        let date = payload.get("date").and_then(Value::as_str).ok_or_else(|| {
            Error::schema("the rate table has no `date`, so nothing says what it is a table of")
        })?;
        Timestamp::parse_rfc3339(date).ok_or_else(|| {
            Error::schema(format!(
                "the rate table's `date` is {date:?}, which is not a date this platform can read"
            ))
        })
    }

    fn base(payload: &Value) -> Result<&str> {
        payload.get("base").and_then(Value::as_str).ok_or_else(|| {
            Error::schema(
                "the rate table has no `base`, so every rate in it is a number with no direction",
            )
        })
    }

    /// `FX.EUR.USD` — publisher-neutral, and stable across a change of quote
    /// set, so a series does not get renamed when a currency is added.
    pub fn series_id(base: &str, quote: &str) -> String {
        format!("FX.{base}.{quote}")
    }
}

impl SourceConnector for FrankfurterRatesConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One event per currency pair.
    ///
    /// The body of each event carries the pair and the rate only. It
    /// deliberately does *not* carry the whole table: the fingerprint is taken
    /// over the body, and a body containing every other currency would change
    /// whenever any of them moved — so an unchanged USD rate would fingerprint
    /// differently on consecutive days and be published as new.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let base = Self::base(payload)?.to_string();
        let reference_date = Self::reference_date(payload)?;
        let rates = payload
            .get("rates")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::schema("the rate table's `rates` is not an object"))?;
        let mut events = Vec::with_capacity(rates.len());
        for (quote, value) in rates {
            let rate = value.as_f64().ok_or_else(|| {
                Error::schema(format!(
                    "the rate for {base}/{quote} is {value}, which is not a number"
                ))
            })?;
            if !rate.is_finite() || rate <= 0.0 {
                return Err(Error::schema(format!(
                    "the rate for {base}/{quote} is {rate}, and a non-positive exchange rate is \
                     not a rate this platform will invert or publish"
                )));
            }
            events.push(RawEvent::new(
                format!("{base}/{quote}@{}", reference_date.to_date_string()),
                reference_date,
                serde_json::json!({
                    "base": base,
                    "quote": quote,
                    "rate": rate,
                    "date": reference_date.to_date_string(),
                }),
            ));
        }
        Ok(events)
    }

    fn map(&self, event: &RawEvent, ingest_time: Timestamp) -> Result<SensedRecord> {
        let base = Self::base(&event.body)?;
        let quote = event
            .body
            .get("quote")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::schema("a decoded rate event lost its quote currency between decode and map")
            })?;
        let rate = event
            .body
            .get("rate")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                Error::schema("a decoded rate event lost its rate between decode and map")
            })?;
        let provenance = Provenance::new(
            self.manifest.source_id.clone(),
            event.event_time,
            // The caller's horizon, not the wall clock: the same fetch
            // replayed in a backtest must produce the same record.
            ingest_time,
        )
        .with_licensing(self.manifest.licensing)
        .with_upstream_id(event.key.clone());
        Ok(SensedRecord::Macro(Box::new(MacroObservation {
            series_id: Self::series_id(base, quote),
            region: Self::REGION.to_string(),
            value: rate,
            unit: format!("{quote} per {base}"),
            reference_date: event.event_time,
            // The ECB publishes no consensus and no prior value on this feed,
            // and inventing either would put a surprise into a signal that
            // nobody forecast.
            consensus: None,
            previous: None,
            // These rates are fixed once and not revised. A source that began
            // revising them would show up as a second event with the same key
            // and a different body, which is a new fingerprint — visible as a
            // duplicate key rather than a silent overwrite.
            is_revision: false,
            provenance,
            quality: DataQuality::clean(),
        })))
    }
}

// The workspace denies `panic_in_result_fn` for production code; a test that
// returns `Result` so it can use `?` on the manifest loader still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// The manifest is the document a reviewer and the acceptance suite
    /// read; the constant is what the code says. When the vendor moved
    /// hosts, the manifest's `provider` was the only place in this crate the
    /// old host appeared, and nothing compared it with anything. The token is
    /// matched delimited — inside the parentheses the provider convention
    /// uses — because `api.frankfurter.dev` is a substring of a longer name
    /// that would pass a `contains`.
    #[test]
    fn the_shipped_manifest_names_the_same_upstream_host_as_the_code() -> Result<()> {
        let manifest = FrankfurterRatesConnector::shipped_manifest()?;
        // Premise: the constant is a bare hostname, not a URL or a path; a
        // constant that carried a scheme or a slash would never appear inside
        // the manifest's parentheses and the assertion below would fail for
        // the wrong reason.
        assert!(
            !FrankfurterRatesConnector::UPSTREAM_HOST.contains('/')
                && FrankfurterRatesConnector::UPSTREAM_HOST.contains('.'),
            "UPSTREAM_HOST is {:?}, which is not a bare hostname",
            FrankfurterRatesConnector::UPSTREAM_HOST
        );
        let named = format!("({})", FrankfurterRatesConnector::UPSTREAM_HOST);
        assert!(
            manifest.provider.contains(&named),
            "the shipped manifest's provider is {:?} and does not name {named}; the host the \
             proxy dials and the host the manifest documents have drifted apart",
            manifest.provider
        );
        // And the endpoint is the versioned path the new host serves. The old
        // `/latest` answers 404 there, which a health probe would report as
        // an unreachable source rather than a wrong path.
        assert_eq!(manifest.endpoint.path, "/v1/latest");
        assert_eq!(manifest.endpoint.health_path(), "/v1/latest");
        Ok(())
    }
}
