//! Kalshi's open binary markets, via the public market list.
//!
//! One request returns a page of markets, each carrying the top of its yes
//! book, its sizes, and the no side derived from them:
//!
//! ```json
//! {"cursor":"…","markets":[{"ticker":"KXUSL1HTOTAL-26SEP05MONPHO-2",
//!   "market_type":"binary","status":"active",
//!   "yes_bid_dollars":"0.3400","yes_ask_dollars":"0.4000",
//!   "yes_bid_size_fp":"564.00","yes_ask_size_fp":"50.00",
//!   "no_bid_dollars":"0.6000","no_ask_dollars":"0.6600",
//!   "updated_time":"2026-09-05T02:07:00.686274Z", …}]}
//! ```
//!
//! Unauthenticated on this endpoint. The recorded body is
//! `fixtures/kalshi-markets.json`; its provenance is on [`FIXTURE`].
//!
//! # This is a candidate, not an admitted source
//!
//! ADR 0034 names Kalshi as the prediction candidate and is explicit that its
//! terms have not been read against a contract. The manifest therefore
//! declares the fail-closed licensing floor, `qip-data-finder`'s catalogue
//! carries an `Ambiguous` posture naming the terms to read, and
//! `admission::admit` refuses the source. Nothing here reaches the egress
//! allowlist or the Envoy bootstrap: that is ADR 0034's separate step, taken
//! once the terms are read, and a listener for a source the gate refuses
//! would be a widening without the use.
//!
//! # Why this produces a quote and not a probability
//!
//! A yes price on a binary contract *is* the market's probability, and the
//! temptation is to publish a `MacroObservation` holding the mid as an `f64`.
//! That would discard the spread and the depth, and a market quoted
//! `0.0100 / 0.9900` — nine of the twenty in the recording — would arrive
//! downstream as a confident `0.50`. The payload carries a bid, an ask and
//! two sizes, which is exactly a [`Quote`], so that is what it becomes; the
//! spread survives, and [`Quote::mid`] already answers `None` for a
//! one-sided book rather than inventing a centre. Prices stay
//! [`qip_core::Decimal`] throughout: a yes price is what a contract settles
//! against, which makes it money.
//!
//! # What is refused, by name
//!
//! * a `market_type` other than `binary` — a scalar market's price does not
//!   settle at zero or one, so it is not a probability and not a price on the
//!   contract this connector describes;
//! * a price outside `[0, 1]` — nothing on a binary contract trades there;
//! * a crossed book, bid above ask with both present;
//! * a side whose price and size disagree about whether it exists — a price
//!   with no size, or size with no price;
//! * the no side disagreeing with the yes side. Kalshi's books are one book
//!   seen twice: `no_bid + yes_ask = 1` and `no_ask + yes_bid = 1`, exactly.
//!   A payload where that fails is not the shape this connector understands,
//!   and the exact decimal arithmetic is what lets the check be an equality;
//! * a market with no resting order on either side. There is no quote to
//!   publish, and a `0.0000 / 0.0000` record would be one.
//!
//! Each refusal is raised in [`SourceConnector::map`], so the runtime
//! quarantines that market with its reason and the other nineteen are
//! published. Raising them in `decode` would drop the page.
//!
//! # Why the body carries the quote and nothing else
//!
//! A market object carries forty fields, most of them metadata that changes
//! independently of the quote — open interest, 24-hour volume, the rules
//! text. The fingerprint is taken over the event body, so a body holding all
//! of that would republish an unchanged quote every time the volume ticked.
//! The body holds the ten fields the record is made from.

use crate::adapter::SensedRecord;
use crate::connector::SourceConnector;
use crate::connector::checkpoint::Cursor;
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::quote::Quote;
use serde_json::Value;

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/kalshi-markets.json");

/// Bodies recorded from the live endpoints, for tests and for the harness.
///
/// Provenance: fetched on 2026-09-05 at 02:12:49 UTC from
/// `https://api.elections.kalshi.com/trade-api/v2/markets?limit=20&mve_filter=exclude&status=open`
/// (the market page; SHA-256 of the body
/// `9c593eeac42c4ba1e704a2edd096648d79c04e46f48b71e373cf0e95127d224f`),
/// and at 02:14:31 UTC from
/// `https://api.elections.kalshi.com/trade-api/v2/exchange/status` (the
/// health path), both over a TLS connection verified against the session's
/// CA bundle and recorded byte for byte, trailing newline included; each
/// `latency_ms` is that fetch's wall time. The fixture's own JSON refuses
/// unknown fields, which is why the note is here and not in it.
///
/// `mve_filter=exclude` is in the query because the first recording, with
/// `limit=20&status=open` alone, returned twenty multivariate shard markets
/// with an empty yes book on every one — a fixture from which this connector
/// would publish nothing, and which would therefore prove nothing about it.
/// The manifest's query is what was recorded, so the harness exercises the
/// request a deployment would make.
pub const FIXTURE: &str = include_str!("fixtures/kalshi-markets.json");

/// Kalshi's open binary markets as one quote per market.
#[derive(Clone, Debug)]
pub struct KalshiMarketsConnector {
    manifest: SourceManifest,
}

impl KalshiMarketsConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and the licensing catalogue can refer
    /// to it without retyping a string that would drift from the manifest.
    pub const SOURCE_ID: &str = "kalshi-markets";

    /// The vendor host the egress proxy would dial for this source.
    ///
    /// Not a field the connector reads — the transport is pointed at the
    /// proxy, never at the vendor — but the one place in code the host is
    /// written, so the manifest's `provider` is held to it by a test now, and
    /// the Envoy cluster and the Terraform allowlist can be held to it when
    /// ADR 0034's allowlist step is taken. It is deliberately in neither
    /// today.
    pub const UPSTREAM_HOST: &str = "api.elections.kalshi.com";

    /// The venue code every quote carries.
    ///
    /// Not an ISO 10383 MIC: none is asserted for Kalshi here, and a made-up
    /// four-letter code would look like one. The venue is part of every
    /// record's identity and of the [`ObjectId`] namespace below.
    pub const VENUE: &'static str = "KALSHI";

    /// The market type this connector reads. A `scalar` market's price does
    /// not settle at zero or one.
    const BINARY: &'static str = "binary";

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the page its manifest asks for.
    ///
    /// Refuses a manifest whose `limit` is above `max_events_per_batch`: the
    /// runtime quarantines any response carrying more events than the cap,
    /// so such a manifest would have every full page refused and would look,
    /// from outside, like a source that stopped answering.
    pub fn new(manifest: SourceManifest) -> Result<Self> {
        manifest.validate()?;
        let limit = manifest.endpoint.query.get("limit").ok_or_else(|| {
            Error::invalid(format!(
                "`{}` asks for no `limit`, so the page size is whatever the vendor's default \
                     is today, and a default larger than max_events_per_batch is a page the \
                     runtime refuses",
                manifest.source_id
            ))
        })?;
        let limit: usize = limit.parse().map_err(|_| {
            Error::invalid(format!(
                "`{}` asks for a `limit` of {limit:?}, which is not a page size",
                manifest.source_id
            ))
        })?;
        if limit > manifest.max_events_per_batch {
            return Err(Error::invalid(format!(
                "`{}` asks for pages of {limit} markets and bounds a batch at {}: every full page \
                 would be quarantined by the runtime rather than decoded. Lower the limit or \
                 raise the bound, together",
                manifest.source_id, manifest.max_events_per_batch
            )));
        }
        Ok(Self { manifest })
    }

    /// The instrument id for one market: the venue and the ticker.
    ///
    /// Minted rather than configured, unlike the Coinbase connector's, because
    /// a Kalshi ticker names one contract on one venue and nowhere else; the
    /// venue prefix is what keeps a second prediction venue's contract on the
    /// same event from merging with it.
    pub fn object_id(ticker: &str) -> ObjectId {
        ObjectId::from_string(format!("{}:{ticker}", Self::VENUE))
    }

    fn text<'a>(market: &'a Value, field: &str) -> Result<&'a str> {
        market.get(field).and_then(Value::as_str).ok_or_else(|| {
            Error::schema(format!(
                "a Kalshi market has no string `{field}`; the page is not the shape this \
                 connector was written against"
            ))
        })
    }

    /// A dollar price or a contract count, exactly.
    fn decimal(market: &Value, field: &str) -> Result<Decimal> {
        let text = Self::text(market, field)?;
        Decimal::parse(text).ok_or_else(|| {
            Error::schema(format!(
                "Kalshi sent `{field}` as {text:?}, which is not an exact decimal this platform \
                 can hold"
            ))
        })
    }

    fn price(market: &Value, field: &str) -> Result<Decimal> {
        let price = Self::decimal(market, field)?;
        if price.is_negative() || price > Decimal::ONE {
            return Err(Error::schema(format!(
                "`{field}` is {price}, and a binary contract settles at zero or one dollar, so \
                 nothing on it trades outside that range"
            )));
        }
        Ok(price)
    }

    fn size(market: &Value, field: &str) -> Result<Decimal> {
        let size = Self::decimal(market, field)?;
        if size.is_negative() {
            return Err(Error::schema(format!(
                "`{field}` is {size}, and a resting size cannot be negative"
            )));
        }
        Ok(size)
    }

    /// The failure this prevents is a side that half exists: a price with no
    /// contracts behind it would let [`Quote::mid`] centre on a level nobody
    /// is at, and contracts with no price is a size for nothing.
    fn side(price: Decimal, size: Decimal, label: &str) -> Result<()> {
        if price.is_positive() != size.is_positive() {
            return Err(Error::schema(format!(
                "the yes {label} is {price} for {size} contracts: a side is either priced and \
                 sized or absent, and this one is neither"
            )));
        }
        Ok(())
    }
}

impl SourceConnector for KalshiMarketsConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// One event per market, keyed by ticker and timed by the vendor's own
    /// `updated_time`.
    ///
    /// The page has no cursor worth keeping: every poll re-serves the open
    /// markets as they stand, so — as with the Coinbase ticker — it is the
    /// fingerprint over the quote body that stops an unchanged market being
    /// republished on every poll.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let markets = payload
            .get("markets")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::schema("the Kalshi page's `markets` is not an array"))?;
        let mut events = Vec::with_capacity(markets.len());
        for market in markets {
            let ticker = Self::text(market, "ticker")?;
            let updated = Self::text(market, "updated_time")?;
            let event_time = Timestamp::parse_rfc3339(updated).ok_or_else(|| {
                Error::schema(format!(
                    "market {ticker} has `updated_time` {updated:?}, which is not an RFC 3339 \
                     instant. The event time is never taken from a local clock, so there is \
                     nothing to fall back to"
                ))
            })?;
            let mut body = serde_json::Map::new();
            for field in [
                "ticker",
                "market_type",
                "status",
                "yes_bid_dollars",
                "yes_ask_dollars",
                "yes_bid_size_fp",
                "yes_ask_size_fp",
                "no_bid_dollars",
                "no_ask_dollars",
                "updated_time",
            ] {
                if let Some(value) = market.get(field) {
                    body.insert(field.to_string(), value.clone());
                }
            }
            events.push(RawEvent::new(ticker, event_time, Value::Object(body)));
        }
        Ok(events)
    }

    fn map(&self, event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
        let market = &event.body;
        let ticker = Self::text(market, "ticker")?;
        let market_type = Self::text(market, "market_type")?;
        if market_type != Self::BINARY {
            return Err(Error::schema(format!(
                "market {ticker} is a {market_type:?} market, and this connector reads binary \
                 contracts only: a price that does not settle at zero or one is not a \
                 probability and not a price on the contract this record would describe"
            )));
        }
        let bid = Self::price(market, "yes_bid_dollars")?;
        let ask = Self::price(market, "yes_ask_dollars")?;
        let bid_size = Self::size(market, "yes_bid_size_fp")?;
        let ask_size = Self::size(market, "yes_ask_size_fp")?;
        let no_bid = Self::price(market, "no_bid_dollars")?;
        let no_ask = Self::price(market, "no_ask_dollars")?;

        Self::side(bid, bid_size, "bid")?;
        Self::side(ask, ask_size, "ask")?;
        if !bid.is_positive() && !ask.is_positive() {
            return Err(Error::schema(format!(
                "market {ticker} has no resting order on either side, so there is no quote to \
                 publish; a 0/0 record would read downstream as one"
            )));
        }
        if bid.is_positive() && ask.is_positive() && bid > ask {
            return Err(Error::schema(format!(
                "market {ticker} is quoted {bid} bid over {ask} ask, which is a crossed book \
                 and not a quote; it is refused rather than averaged"
            )));
        }
        // One book seen from both sides. Exact arithmetic makes this an
        // equality rather than a tolerance, and a tolerance is where a
        // one-cent disagreement would have hidden.
        if no_bid + ask != Decimal::ONE || no_ask + bid != Decimal::ONE {
            return Err(Error::schema(format!(
                "market {ticker} quotes yes {bid}/{ask} and no {no_bid}/{no_ask}, and on a \
                 binary contract the no side is one minus the yes side exactly; two sides that \
                 disagree are a page this connector does not understand"
            )));
        }

        Ok(SensedRecord::Quote(Quote {
            object_id: Self::object_id(ticker),
            venue: Self::VENUE.to_string(),
            at: event.event_time,
            bid,
            ask,
            bid_size,
            ask_size,
            quality: DataQuality::clean(),
        }))
    }
}

// The workspace denies `panic_in_result_fn` for production code; a test that
// returns `Result` so it can use `?` on the manifest loader still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn connector() -> Result<KalshiMarketsConnector> {
        KalshiMarketsConnector::new(KalshiMarketsConnector::shipped_manifest()?)
    }

    fn recorded_page() -> Result<Value> {
        let script: Value = serde_json::from_str(FIXTURE)?;
        let body = script["exchanges"][0]["answers"][0]["body"]
            .as_str()
            .ok_or_else(|| Error::invalid("the fixture's first answer has no body"))?;
        Ok(serde_json::from_str(body)?)
    }

    /// A market as the recording has it, with one field replaced.
    fn market_with(field: &str, value: Value) -> Value {
        let mut market = serde_json::json!({
            "ticker": "KXTEST-1",
            "market_type": "binary",
            "status": "active",
            "yes_bid_dollars": "0.3400",
            "yes_ask_dollars": "0.4000",
            "yes_bid_size_fp": "564.00",
            "yes_ask_size_fp": "50.00",
            "no_bid_dollars": "0.6000",
            "no_ask_dollars": "0.6600",
            "updated_time": "2026-09-05T02:07:00.686274Z"
        });
        market[field] = value;
        market
    }

    fn map_one(market: Value) -> Result<SensedRecord> {
        let connector = connector()?;
        let events = connector.decode(
            &serde_json::json!({ "markets": [market] }),
            &Cursor::beginning(),
        )?;
        assert_eq!(events.len(), 1, "one market decodes to one event");
        connector.map(
            &events[0],
            Timestamp::parse_rfc3339("2026-09-05T03:00:00Z").unwrap(),
        )
    }

    /// The manifest is the document a reviewer reads; the constant is what
    /// the code says. Matched delimited, inside the parentheses the provider
    /// convention uses, because the host is a substring of longer names.
    #[test]
    fn the_shipped_manifest_names_the_same_upstream_host_as_the_code() -> Result<()> {
        let manifest = KalshiMarketsConnector::shipped_manifest()?;
        assert!(
            !KalshiMarketsConnector::UPSTREAM_HOST.contains('/')
                && KalshiMarketsConnector::UPSTREAM_HOST.contains('.'),
            "UPSTREAM_HOST is {:?}, which is not a bare hostname",
            KalshiMarketsConnector::UPSTREAM_HOST
        );
        let named = format!("({})", KalshiMarketsConnector::UPSTREAM_HOST);
        assert!(
            manifest.provider.contains(&named),
            "the shipped manifest's provider is {:?} and does not name {named}",
            manifest.provider
        );
        // The health probe is the cheap status endpoint, not the 40 KB page.
        assert_eq!(
            manifest.endpoint.health_path(),
            "/trade-api/v2/exchange/status"
        );
        Ok(())
    }

    /// The recording holds twenty markets and every one of them decodes,
    /// including the one with an empty book — `decode` extracts, `map`
    /// judges, so that one bad market costs one quarantine and not a page.
    #[test]
    fn the_recorded_page_decodes_into_one_event_per_market() -> Result<()> {
        let page = recorded_page()?;
        let markets = page["markets"]
            .as_array()
            .ok_or_else(|| Error::invalid("the recording has no markets"))?;
        assert_eq!(markets.len(), 20, "the recording was taken with limit=20");

        let events = connector()?.decode(&page, &Cursor::beginning())?;
        assert_eq!(events.len(), 20);
        let first = &events[0];
        assert_eq!(first.key, "KXBRASILEIROBSPREAD-26SEP04BRAAMG-BRA5");
        assert_eq!(
            first.event_time,
            Timestamp::parse_rfc3339("2026-09-05T02:05:16.61399Z").unwrap()
        );
        // The body is the quote, not the forty-field market object: a body
        // carrying open interest would republish an unchanged quote whenever
        // the open interest moved.
        let body = first.body.as_object().unwrap();
        assert_eq!(body.len(), 10, "{body:?}");
        assert!(!body.contains_key("open_interest_fp"));
        Ok(())
    }

    /// The prices and sizes are exact. `0.3900` is not `0.39000000000000001`,
    /// and the mid of `0.34` and `0.40` is exactly `0.37`.
    #[test]
    fn dollar_prices_and_contract_sizes_are_held_exactly() -> Result<()> {
        match map_one(market_with("ticker", Value::from("KXTEST-1")))? {
            SensedRecord::Quote(quote) => {
                assert_eq!(quote.bid, Decimal::parse("0.34").unwrap());
                assert_eq!(quote.ask, Decimal::parse("0.4").unwrap());
                assert_eq!(quote.bid_size, Decimal::from_int(564));
                assert_eq!(quote.ask_size, Decimal::from_int(50));
                assert_eq!(quote.mid(), Some(Decimal::parse("0.37").unwrap()));
                assert_eq!(quote.venue, KalshiMarketsConnector::VENUE);
                assert_eq!(quote.object_id.as_str(), "KALSHI:KXTEST-1");
                assert!(quote.validate().is_empty(), "{:?}", quote.validate());
            }
            other => panic!("a market decoded into {other:?} rather than a quote"),
        }
        Ok(())
    }

    /// A one-sided book is published — it is a real state of the market and
    /// the platform's own `Quote::mid` already declines to centre it.
    #[test]
    fn a_one_sided_book_is_a_quote_whose_mid_is_none() -> Result<()> {
        let mut market = market_with("yes_bid_dollars", Value::from("0.0000"));
        market["yes_bid_size_fp"] = Value::from("0.00");
        market["no_ask_dollars"] = Value::from("1.0000");
        match map_one(market)? {
            SensedRecord::Quote(quote) => {
                assert_eq!(quote.mid(), None);
                assert_eq!(quote.ask, Decimal::parse("0.4").unwrap());
            }
            other => panic!("{other:?}"),
        }
        Ok(())
    }

    #[test]
    fn a_market_with_no_order_on_either_side_is_refused_as_no_quote() -> Result<()> {
        let mut market = market_with("yes_bid_dollars", Value::from("0.0000"));
        market["yes_ask_dollars"] = Value::from("0.0000");
        market["yes_bid_size_fp"] = Value::from("0.00");
        market["yes_ask_size_fp"] = Value::from("0.00");
        market["no_bid_dollars"] = Value::from("1.0000");
        market["no_ask_dollars"] = Value::from("1.0000");
        let error = map_one(market).expect_err("an empty book was published as a quote");
        assert!(
            error.message().contains("no resting order on either side"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_crossed_book_is_refused_by_name_rather_than_averaged() -> Result<()> {
        // Bid 0.45 over ask 0.40; the no side kept consistent so that this
        // refusal, and not the disagreement one, is the one that fires.
        let mut market = market_with("yes_bid_dollars", Value::from("0.4500"));
        market["no_ask_dollars"] = Value::from("0.5500");
        let error = map_one(market).expect_err("a crossed book was published as a quote");
        assert!(
            error.message().contains("crossed book") && error.message().contains("KXTEST-1"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_price_outside_the_contracts_range_is_refused() -> Result<()> {
        let error = map_one(market_with("yes_ask_dollars", Value::from("1.2500")))
            .expect_err("a price above one dollar was accepted on a binary contract");
        assert!(
            error.message().contains("settles at zero or one dollar"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_no_side_that_disagrees_with_the_yes_side_is_refused() -> Result<()> {
        // no_bid should be 1 - 0.40 = 0.60; a one-cent disagreement is exactly
        // what a tolerance would have hidden.
        let error = map_one(market_with("no_bid_dollars", Value::from("0.5900")))
            .expect_err("a page whose two sides disagree was accepted");
        assert!(
            error.message().contains("one minus the yes side exactly"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_side_with_a_price_and_no_size_is_refused() -> Result<()> {
        let error = map_one(market_with("yes_bid_size_fp", Value::from("0.00")))
            .expect_err("a priced side with no contracts behind it was accepted");
        assert!(
            error
                .message()
                .contains("either priced and sized or absent"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_market_that_is_not_binary_is_refused() -> Result<()> {
        let error = map_one(market_with("market_type", Value::from("scalar")))
            .expect_err("a scalar market's price was published as a binary quote");
        assert!(
            error.message().contains("binary contracts only"),
            "{}",
            error.message()
        );
        Ok(())
    }

    /// A page the runtime would quarantine on every poll is refused at
    /// construction, where the operator sees it, rather than in production,
    /// where it looks like a source that stopped answering.
    #[test]
    fn a_manifest_whose_page_exceeds_its_own_batch_bound_is_refused() -> Result<()> {
        let mut manifest = KalshiMarketsConnector::shipped_manifest()?;
        manifest
            .endpoint
            .query
            .insert("limit".to_string(), "100".to_string());
        // Premise: the shipped bound is below the edited limit.
        assert!(manifest.max_events_per_batch < 100);
        let error = KalshiMarketsConnector::new(manifest)
            .expect_err("a page larger than the batch bound was accepted");
        assert!(
            error
                .message()
                .contains("every full page would be quarantined"),
            "{}",
            error.message()
        );
        Ok(())
    }
}
