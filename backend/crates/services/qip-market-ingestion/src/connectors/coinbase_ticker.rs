//! Coinbase Exchange's public spot ticker.
//!
//! One request returns the last trade and the top of book for one product:
//!
//! ```json
//! {"ask":"64231.55","bid":"64230.11","volume":"9184.42736519",
//!  "trade_id":712553481,"price":"64230.99","size":"0.00184300",
//!  "time":"2026-08-24T14:59:41.812734Z"}
//! ```
//!
//! Free, unauthenticated, no signup. The recorded body above is
//! `fixtures/coinbase-spot-ticker.json`, captured from the live endpoint.
//!
//! # Why this produces a tick and not a trade or a quote
//!
//! A [`qip_market::quote::Trade`] must state a [`TradeCondition`], and this
//! payload does not carry one. `rest.rs` sets out why an unstated condition is
//! refused rather than defaulted to `Regular`: `Regular` is the one condition
//! that forms a price, so a late report or an off-exchange cross defaulted to
//! it would move a mark it never traded at. This connector cannot tell "the
//! source says this printed normally" from "the source said nothing", so it
//! does not produce a trade.
//!
//! A [`qip_market::quote::Quote`] needs bid and ask *sizes*, and this endpoint
//! sends neither. Publishing a quote with invented sizes would put made-up
//! depth into the microstructure signal.
//!
//! A [`Tick`] is exactly what the payload is: a price, a size and the instant
//! it happened. So that is what it produces.
//!
//! # Why the instrument mapping is configuration
//!
//! The manifest names a product (`BTC-USD`); the [`ObjectId`] and venue behind
//! it are passed to [`CoinbaseTickerConnector::new`]. A connector that minted
//! an id from a product string would merge two venues' BTC into one instrument
//! the first time a second crypto source was added.

use crate::adapter::SensedRecord;
use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::quote::Tick;
use serde_json::Value;

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/coinbase-spot-ticker.json");

/// A body recorded from the live endpoint, for tests and for the harness.
pub const FIXTURE: &str = include_str!("fixtures/coinbase-spot-ticker.json");

/// Coinbase Exchange's spot ticker for one product.
#[derive(Clone, Debug)]
pub struct CoinbaseTickerConnector {
    manifest: SourceManifest,
    product: String,
    object_id: ObjectId,
    venue: String,
}

impl CoinbaseTickerConnector {
    /// The venue code every record carries. Coinbase Exchange's MIC.
    pub const VENUE: &'static str = "CBSE";

    /// The stable source identity, matching the manifest's `source_id`.
    /// Named once here so the catalogue, the configuration and the manifest
    /// cannot drift apart silently.
    pub const SOURCE_ID: &str = "coinbase-spot-ticker";

    /// The manifest as shipped, before a deployment sets its egress address.
    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the product its manifest asks for.
    ///
    /// Fails when the manifest's path does not name `product`. The two are
    /// separate strings and nothing else would notice them disagreeing: the
    /// connector would fetch ETH-USD and stamp every tick with the ObjectId of
    /// BTC-USD, which is a wrong price on a real instrument rather than an
    /// error anybody sees.
    pub fn new(
        manifest: SourceManifest,
        product: impl Into<String>,
        object_id: ObjectId,
        venue: impl Into<String>,
    ) -> Result<Self> {
        manifest.validate()?;
        let product = product.into();
        let venue = venue.into();
        if product.trim().is_empty() {
            return Err(Error::invalid("a Coinbase connector needs a product id"));
        }
        if !product
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::invalid(format!(
                "the product {product:?} is not a Coinbase product id: they are upper-case ASCII \
                 with a hyphen, and anything else would go into a request line this connector \
                 builds by hand"
            )));
        }
        if venue.trim().is_empty() {
            return Err(Error::invalid(
                "a venue is part of every record's identity and cannot be blank",
            ));
        }
        let expected = format!("/products/{product}/ticker");
        if manifest.endpoint.path != expected {
            return Err(Error::invalid(format!(
                "the manifest fetches {:?} and this connector maps the product {product:?}, whose \
                 path is {expected:?}. Left alone, every tick from one product would be published \
                 under the other's instrument id",
                manifest.endpoint.path
            )));
        }
        Ok(Self {
            manifest,
            product,
            object_id,
            venue,
        })
    }

    pub fn product(&self) -> &str {
        &self.product
    }

    fn text<'a>(payload: &'a Value, field: &str) -> Result<&'a str> {
        payload.get(field).and_then(Value::as_str).ok_or_else(|| {
            Error::schema(format!(
                "the Coinbase ticker payload has no string `{field}`; the schema gate should have \
                 refused this body before it reached the decoder"
            ))
        })
    }

    fn decimal(payload: &Value, field: &str) -> Result<Decimal> {
        let text = Self::text(payload, field)?;
        Decimal::parse(text).ok_or_else(|| {
            Error::schema(format!(
                "the Coinbase ticker sent `{field}` as {text:?}, which is not an exact decimal \
                 this platform can hold. A price rounded to fit would be a price nobody traded at"
            ))
        })
    }
}

impl SourceConnector for CoinbaseTickerConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One object, one event.
    ///
    /// The ticker has no cursor: every call returns the last trade, whatever
    /// it was last time. That is why this source's correctness depends on
    /// deduplication rather than on a position — the same trade is served for
    /// as long as no newer one happens, and the fingerprint is what keeps it
    /// from being published on every poll.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let time = Self::text(payload, "time")?;
        let event_time = Timestamp::parse_rfc3339(time).ok_or_else(|| {
            Error::schema(format!(
                "the Coinbase ticker sent `time` as {time:?}, which is not an RFC 3339 instant. \
                 The event time is never taken from a local clock, so there is nothing to fall \
                 back to"
            ))
        })?;
        let trade_id = payload
            .get("trade_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                Error::schema(
                    "the Coinbase ticker sent no integer `trade_id`; without the source's own key \
                     a redelivery cannot be told from a new print",
                )
            })?;
        Ok(vec![RawEvent::new(
            trade_id.to_string(),
            event_time,
            payload.clone(),
        )])
    }

    fn map(&self, event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
        Ok(SensedRecord::Tick(Tick {
            object_id: self.object_id.clone(),
            venue: self.venue.clone(),
            at: event.event_time,
            price: Self::decimal(&event.body, "price")?,
            // The size of the print this tick reports, not the product's
            // rolling 24-hour `volume` — that is a different quantity on the
            // same payload and reporting it here would make every tick look
            // like a nine-thousand-bitcoin trade.
            volume: Self::decimal(&event.body, "size")?,
            quality: DataQuality::clean(),
        }))
    }
}
