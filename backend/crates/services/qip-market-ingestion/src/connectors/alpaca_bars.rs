//! Alpaca Market Data's daily bars from the IEX feed.
//!
//! One request returns a map from symbol to that symbol's bars, prices as
//! JSON numbers:
//!
//! ```json
//! {"bars":{"AAPL":[{"c":190.1,"h":191.5,"l":188.75,"n":1204,"o":189.2,
//!   "t":"2026-09-03T04:00:00Z","v":52341,"vw":189.85}]},
//!  "next_page_token":null}
//! ```
//!
//! Authenticated: every request carries the account's key id and secret key
//! in two headers. [`FIXTURE`] is **not** a recording — see its note.
//!
//! # This is a candidate, not an admitted source
//!
//! ADR 0034 names Alpaca as the equities candidate and is explicit that its
//! terms have not been read against a contract. The manifest declares the
//! fail-closed licensing floor, `qip-data-finder`'s catalogue carries an
//! `Ambiguous` posture naming the terms to read, and `admission::admit`
//! refuses the source. The host is in neither the egress allowlist nor the
//! Envoy bootstrap; that is ADR 0034's separate step, once the terms are
//! read and Coinbase has proven the path.
//!
//! ADR 0034 also names the specific hazard: the same account is a paper
//! brokerage, and a data credential must never become an order credential.
//! This connector describes one `GET` of bars and nothing else, and the
//! secret it names is the market-data one.
//!
//! # The credential the schema can hold, and the one it cannot yet
//!
//! Alpaca wants two headers, `apca-api-key-id` and `apca-api-secret-key`.
//! [`crate::connector::manifest::AuthSpec`] holds one header and one
//! [`crate::connector::manifest::SecretRef`], by design — a shape with no
//! room for a value. The manifest names the secret key, which is the
//! credential; the key id has no field to be named in, and this connector
//! does not smuggle it in through a request, because a request that can
//! carry a credential is the thing the transport's design exists to make
//! impossible. Until `AuthSpec` grows a second named header the transport
//! sends one of the two and Alpaca answers 401 — which is a refusal at the
//! vendor, visible in the health check, rather than a plaintext key. The
//! schema change belongs beside the transport, not here.
//!
//! # Why a daily bar is knowable seventeen hours after it is stamped
//!
//! Alpaca stamps a daily bar `t` at the session's midnight in New York —
//! `04:00Z` in summer, `05:00Z` in winter. The session closes sixteen hours
//! later, and the bar's close does not exist until then. A record made
//! knowable at `t` would hand a backtest the day's close at the day's open,
//! which is the point-in-time leak `.claude/rules/domains/data-and-streaming.md`
//! forbids. The manifest's `publication_delay_ms` is seventeen hours: the
//! session plus one hour for the closing auction and the vendor's own
//! finalisation, and [`AlpacaBarsConnector::new`] refuses anything under
//! sixteen. A partial bar fetched during the session is withheld by the
//! runtime and not buffered; the poll after the knowable instant fetches the
//! bar as it then stands.
//!
//! # Exact prices from JSON numbers
//!
//! The vendor sends `189.2` as a JSON number, not as `"189.2"`. This is the
//! crossing point between the vendor's floating point and this platform's
//! [`Decimal`], and it is crossed by rendering the number to its shortest
//! round-trip text and parsing that: for any literal of fifteen or fewer
//! significant digits — every equity price — the text is the vendor's own
//! literal, so `189.2` becomes exactly `189.2` and never
//! `189.19999999999998863`. Anything that is not a number is refused.
//!
//! # What is refused, by name
//!
//! * a symbol the connector was not given an instrument for — a minted id
//!   would merge two venues' listings the first time a second equities
//!   source was added, the failure the Coinbase connector documents;
//! * an incoherent bar — a high below the low, or an open or close outside
//!   them — refused rather than clamped, because a clamped bar is a vendor
//!   fault turned into plausible data;
//! * a manifest whose `feed` is not `iex` — the venue every bar carries
//!   would then be a lie — or whose `timeframe` is not `1Day`, which would
//!   stamp minute bars as daily ones.

use crate::adapter::SensedRecord;
use crate::connector::SourceConnector;
use crate::connector::checkpoint::{Cursor, CursorPosition};
use crate::connector::envelope::RawEvent;
use crate::connector::manifest::SourceManifest;
use crate::connector::transport::SourceRequest;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::bar::{Bar, Interval};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The manifest this connector was written against.
pub const MANIFEST: &str = include_str!("manifests/alpaca-daily-bars.json");

/// A body in the documented shape, for tests and for the harness.
///
/// PLACEHOLDER-RECORD-LIVE. This is **not** a recording: the endpoint needs
/// an account credential, none is held in this session, and the source is
/// refused by the licensing gate in any case. The shape — `bars` keyed by
/// symbol, each bar `c h l n o t v vw` with prices as JSON numbers and
/// `t` at the New York midnight, `next_page_token` beside it — is taken
/// from the vendor's published documentation, and the values are round
/// numbers chosen to be obviously not prices anyone traded at. Re-record
/// from the live endpoint, through the egress proxy, once the terms are
/// read and a credential is mounted, and replace this note with the
/// provenance the Frankfurter fixture carries.
pub const FIXTURE: &str = include_str!("fixtures/alpaca-daily-bars.json");

/// Alpaca's daily bars as one [`Bar`] per symbol per session.
#[derive(Clone, Debug)]
pub struct AlpacaBarsConnector {
    manifest: SourceManifest,
    instruments: BTreeMap<String, ObjectId>,
}

impl AlpacaBarsConnector {
    /// The manifest's own `source_id`, named as a constant so
    /// [`crate::connector_feed`]'s bridge and the licensing catalogue can refer
    /// to it without retyping a string that would drift from the manifest.
    pub const SOURCE_ID: &str = "alpaca-daily-bars";

    /// The vendor host the egress proxy would dial for this source. Held to
    /// the manifest's `provider` by a test; in neither the allowlist nor the
    /// bootstrap today, by ADR 0034's ordering.
    pub const UPSTREAM_HOST: &str = "data.alpaca.markets";

    /// The venue code every bar carries: IEX's MIC, because the manifest's
    /// `feed=iex` is what makes every print in these bars an IEX print.
    /// [`Self::new`] refuses a manifest whose feed says otherwise.
    pub const VENUE: &'static str = "IEXG";

    /// The header Alpaca reads the key id from. Named so the gap the module
    /// documentation describes is a constant a reviewer can grep for, and so
    /// [`Self::new`] can refuse a manifest that put the key id where the
    /// secret belongs.
    pub const KEY_ID_HEADER: &'static str = "apca-api-key-id";

    /// The header the manifest's one secret travels in.
    pub const SECRET_KEY_HEADER: &'static str = "apca-api-secret-key";

    /// The shortest delay under which a daily bar could be final: the
    /// sixteen hours from the New York midnight to the close.
    pub const MIN_PUBLICATION_DELAY: Duration = Duration::from_hours(16);

    pub fn shipped_manifest() -> Result<SourceManifest> {
        SourceManifest::from_json(MANIFEST)
    }

    /// Build the connector for the symbols its manifest asks for, each with
    /// the instrument it maps to.
    ///
    /// The manifest's `symbols` and the map's keys must be the same set. A
    /// symbol fetched but not mapped would be refused at every poll; a symbol
    /// mapped but not fetched is an instrument the operator believes is fed
    /// and is not.
    pub fn new(manifest: SourceManifest, instruments: BTreeMap<String, ObjectId>) -> Result<Self> {
        manifest.validate()?;
        let query = &manifest.endpoint.query;
        match query.get("feed").map(String::as_str) {
            Some("iex") => {}
            other => {
                return Err(Error::invalid(format!(
                    "`{}` fetches feed {other:?} and every bar this connector produces is \
                     stamped venue {}: a consolidated or a different feed under that venue \
                     code would attribute prints to an exchange they did not happen on",
                    manifest.source_id,
                    Self::VENUE
                )));
            }
        }
        match query.get("timeframe").map(String::as_str) {
            Some("1Day") => {}
            other => {
                return Err(Error::invalid(format!(
                    "`{}` fetches timeframe {other:?} and this connector stamps every bar as a \
                     daily one; a minute bar published under a daily interval closes a day \
                     after it opened",
                    manifest.source_id
                )));
            }
        }
        if manifest.publication_delay() < Self::MIN_PUBLICATION_DELAY {
            return Err(Error::invalid(format!(
                "`{}` declares a dissemination delay of {:?}. A daily bar is stamped at the \
                 session's midnight and its close does not exist until sixteen hours later; a \
                 shorter delay hands a backtest the close at the open",
                manifest.source_id,
                manifest.publication_delay()
            )));
        }
        if manifest.auth.header.as_deref() == Some(Self::KEY_ID_HEADER) {
            return Err(Error::invalid(format!(
                "`{}` names `{}` as the header its secret travels in. That header carries the \
                 key id; the secret key travels in `{}`, and sending the secret under the key \
                 id's name sends it to a header the vendor logs as an identifier",
                manifest.source_id,
                Self::KEY_ID_HEADER,
                Self::SECRET_KEY_HEADER
            )));
        }
        if instruments.is_empty() {
            return Err(Error::invalid(
                "an Alpaca connector needs at least one symbol mapped to an instrument",
            ));
        }
        let asked: BTreeSet<&str> = query
            .get("symbols")
            .map(|list| list.split(',').map(str::trim).collect())
            .unwrap_or_default();
        let mapped: BTreeSet<&str> = instruments.keys().map(String::as_str).collect();
        if asked != mapped {
            return Err(Error::invalid(format!(
                "the manifest fetches symbols {asked:?} and this connector maps {mapped:?}. A \
                 symbol fetched and not mapped is refused on every poll, and one mapped and not \
                 fetched is an instrument nobody is feeding"
            )));
        }
        Ok(Self {
            manifest,
            instruments,
        })
    }

    fn text<'a>(bar: &'a Value, field: &str) -> Result<&'a str> {
        bar.get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::schema(format!("an Alpaca bar has no string `{field}`")))
    }

    /// The vendor's floating point as this platform's exact decimal — the
    /// crossing the module documentation describes.
    fn decimal(bar: &Value, field: &str) -> Result<Decimal> {
        let number = bar.get(field).and_then(Value::as_number).ok_or_else(|| {
            Error::schema(format!(
                "an Alpaca bar has no numeric `{field}`; a price sent as a string or null is \
                 not coerced into one"
            ))
        })?;
        Decimal::parse(&number.to_string()).ok_or_else(|| {
            Error::schema(format!(
                "an Alpaca bar sent `{field}` as {number}, which is not a decimal this platform \
                 can hold exactly"
            ))
        })
    }

    fn count(bar: &Value, field: &str) -> Result<u64> {
        bar.get(field).and_then(Value::as_u64).ok_or_else(|| {
            Error::schema(format!(
                "an Alpaca bar has no non-negative integer `{field}`"
            ))
        })
    }
}

impl SourceConnector for AlpacaBarsConnector {
    fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    /// The manifest's request, with the cursor as `start`.
    ///
    /// Alpaca's `start` is inclusive, so the newest bar already read is
    /// re-served on the next poll and the fingerprint absorbs it; without a
    /// cursor the vendor's default window applies and the beginning cursor
    /// sends none.
    fn fetch_request(&self, cursor: &Cursor) -> Result<SourceRequest> {
        let endpoint = &self.manifest.endpoint;
        let mut request = SourceRequest::get(&endpoint.path);
        request.query = endpoint.query.clone();
        if let CursorPosition::EventTime { at } = cursor.position {
            request.query.insert("start".to_string(), at.to_rfc3339());
        }
        Ok(request)
    }

    /// One event per symbol per bar, keyed `SYMBOL@t` and timed by `t`.
    ///
    /// The body carries the symbol beside the bar's own fields and nothing
    /// from any other symbol: the fingerprint is taken over the body, and a
    /// body holding the whole map would republish an unchanged AAPL bar
    /// whenever MSFT's moved.
    fn decode(&self, payload: &Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
        let by_symbol = payload
            .get("bars")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::schema("the Alpaca page's `bars` is not an object"))?;
        let mut events = Vec::new();
        for (symbol, bars) in by_symbol {
            let bars = bars
                .as_array()
                .ok_or_else(|| Error::schema(format!("the bars for {symbol} are not an array")))?;
            for bar in bars {
                let stamped = Self::text(bar, "t")?;
                let open_time = Timestamp::parse_rfc3339(stamped).ok_or_else(|| {
                    Error::schema(format!(
                        "a bar for {symbol} is stamped {stamped:?}, which is not an RFC 3339 \
                         instant. The event time is never taken from a local clock, so there \
                         is nothing to fall back to"
                    ))
                })?;
                let mut body = bar
                    .as_object()
                    .cloned()
                    .ok_or_else(|| Error::schema(format!("a bar for {symbol} is not an object")))?;
                body.insert("symbol".to_string(), Value::from(symbol.as_str()));
                events.push(RawEvent::new(
                    format!("{symbol}@{stamped}"),
                    open_time,
                    Value::Object(body),
                ));
            }
        }
        Ok(events)
    }

    fn map(&self, event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
        let bar = &event.body;
        let symbol = Self::text(bar, "symbol")?;
        let object_id = self.instruments.get(symbol).ok_or_else(|| {
            Error::schema(format!(
                "the page carries bars for {symbol}, which this connector has no instrument \
                 for; an id minted from the symbol would merge with another venue's listing"
            ))
        })?;
        let open = Self::decimal(bar, "o")?;
        let high = Self::decimal(bar, "h")?;
        let low = Self::decimal(bar, "l")?;
        let close = Self::decimal(bar, "c")?;
        let vwap = Self::decimal(bar, "vw")?;
        let volume = Decimal::from_int(i64::try_from(Self::count(bar, "v")?).map_err(|_| {
            Error::schema(format!(
                "the volume of a bar for {symbol} does not fit a Decimal"
            ))
        })?);
        let trade_count = Self::count(bar, "n")?;
        let record = Bar {
            object_id: object_id.clone(),
            venue: Self::VENUE.to_string(),
            interval: Interval::Day,
            open_time: event.event_time,
            open,
            high,
            low,
            close,
            volume,
            vwap: Some(vwap),
            trade_count,
            quality: DataQuality::clean(),
        };
        if !record.is_coherent() {
            return Err(Error::schema(format!(
                "the bar for {symbol} at {} is incoherent: open {open} high {high} low {low} \
                 close {close}. It is refused rather than clamped, because a clamped bar is a \
                 vendor fault turned into plausible data",
                event.event_time.to_rfc3339()
            )));
        }
        Ok(SensedRecord::Bar(Box::new(record)))
    }
}

// The workspace denies `panic_in_result_fn` for production code; a test that
// returns `Result` so it can use `?` on the manifest loader still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn instruments() -> BTreeMap<String, ObjectId> {
        [("AAPL", "OBJ-AAPL"), ("MSFT", "OBJ-MSFT")]
            .into_iter()
            .map(|(symbol, id)| (symbol.to_string(), ObjectId::from_string(id)))
            .collect()
    }

    fn connector() -> Result<AlpacaBarsConnector> {
        AlpacaBarsConnector::new(AlpacaBarsConnector::shipped_manifest()?, instruments())
    }

    fn placeholder_page() -> Result<Value> {
        let script: Value = serde_json::from_str(FIXTURE)?;
        let body = script["exchanges"][0]["answers"][0]["body"]
            .as_str()
            .ok_or_else(|| Error::invalid("the fixture's first answer has no body"))?;
        Ok(serde_json::from_str(body)?)
    }

    fn map_one(symbol: &str, bar: Value) -> Result<SensedRecord> {
        let connector = connector()?;
        let events = connector.decode(
            &serde_json::json!({ "bars": { symbol: [bar] } }),
            &Cursor::beginning(),
        )?;
        assert_eq!(events.len(), 1);
        connector.map(
            &events[0],
            Timestamp::parse_rfc3339("2026-09-05T12:00:00Z").unwrap(),
        )
    }

    fn bar() -> Value {
        serde_json::json!({
            "c": 190.1, "h": 191.5, "l": 188.75, "n": 1204, "o": 189.2,
            "t": "2026-09-03T04:00:00Z", "v": 52341, "vw": 189.85
        })
    }

    #[test]
    fn the_shipped_manifest_names_the_same_upstream_host_as_the_code() -> Result<()> {
        let manifest = AlpacaBarsConnector::shipped_manifest()?;
        assert!(
            !AlpacaBarsConnector::UPSTREAM_HOST.contains('/')
                && AlpacaBarsConnector::UPSTREAM_HOST.contains('.'),
            "UPSTREAM_HOST is {:?}, which is not a bare hostname",
            AlpacaBarsConnector::UPSTREAM_HOST
        );
        let named = format!("({})", AlpacaBarsConnector::UPSTREAM_HOST);
        assert!(
            manifest.provider.contains(&named),
            "the shipped manifest's provider is {:?} and does not name {named}",
            manifest.provider
        );
        // The one header the schema can name is the secret key's, and the
        // key id is not it.
        assert_eq!(
            manifest.auth.header.as_deref(),
            Some(AlpacaBarsConnector::SECRET_KEY_HEADER)
        );
        assert_ne!(
            AlpacaBarsConnector::SECRET_KEY_HEADER,
            AlpacaBarsConnector::KEY_ID_HEADER
        );
        Ok(())
    }

    /// Two symbols with two sessions each: four events, each keyed by the
    /// symbol and the vendor's stamp, so nothing across symbols shares a key.
    #[test]
    fn the_placeholder_page_decodes_into_one_event_per_symbol_per_session() -> Result<()> {
        let page = placeholder_page()?;
        assert_eq!(page["bars"].as_object().map(|m| m.len()), Some(2));
        let events = connector()?.decode(&page, &Cursor::beginning())?;
        assert_eq!(events.len(), 4);
        let keys: Vec<&str> = events.iter().map(|event| event.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "AAPL@2026-09-03T04:00:00Z",
                "AAPL@2026-09-04T04:00:00Z",
                "MSFT@2026-09-03T04:00:00Z",
                "MSFT@2026-09-04T04:00:00Z",
            ]
        );
        assert!(
            events[0].body.get("symbol").is_some(),
            "the body must carry the symbol, or map cannot find the instrument"
        );
        Ok(())
    }

    /// `189.2` as a JSON number is exactly `189.2` as a Decimal, and the
    /// volume is an exact count. An `f64` round trip would give a bar whose
    /// open is `189.19999999999998863`.
    #[test]
    fn a_json_number_price_is_held_exactly_and_never_through_f64_rounding() -> Result<()> {
        match map_one("AAPL", bar())? {
            SensedRecord::Bar(bar) => {
                assert_eq!(bar.open, Decimal::parse("189.2").unwrap());
                assert_eq!(bar.high, Decimal::parse("191.5").unwrap());
                assert_eq!(bar.low, Decimal::parse("188.75").unwrap());
                assert_eq!(bar.close, Decimal::parse("190.1").unwrap());
                assert_eq!(bar.vwap, Some(Decimal::parse("189.85").unwrap()));
                assert_eq!(bar.volume, Decimal::from_int(52341));
                assert_eq!(bar.trade_count, 1204);
                assert_eq!(bar.interval, Interval::Day);
                assert_eq!(bar.venue, AlpacaBarsConnector::VENUE);
                assert_eq!(bar.object_id.as_str(), "OBJ-AAPL");
                assert_eq!(
                    bar.close_time(),
                    Timestamp::parse_rfc3339("2026-09-04T04:00:00Z").unwrap()
                );
            }
            other => panic!("a bar decoded into {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn an_incoherent_bar_is_refused_by_name_rather_than_clamped() -> Result<()> {
        let mut broken = bar();
        broken["l"] = Value::from(192.0);
        let error =
            map_one("AAPL", broken).expect_err("a bar with its low above its high was published");
        assert!(
            error.message().contains("AAPL")
                && error.message().contains("refused rather than clamped"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_price_sent_as_a_string_is_refused_rather_than_coerced() -> Result<()> {
        let mut broken = bar();
        broken["o"] = Value::from("189.2");
        let error = map_one("AAPL", broken).expect_err("a string price was coerced");
        assert!(
            error.message().contains("no numeric `o`"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_symbol_with_no_instrument_is_refused_rather_than_minted() -> Result<()> {
        let error = map_one("NVDA", bar()).expect_err("an unmapped symbol was published");
        assert!(
            error.message().contains("NVDA") && error.message().contains("no instrument"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_manifest_whose_symbols_differ_from_the_mapped_instruments_is_refused() -> Result<()> {
        let mut only_aapl = instruments();
        only_aapl.remove("MSFT");
        let error = AlpacaBarsConnector::new(AlpacaBarsConnector::shipped_manifest()?, only_aapl)
            .expect_err("a connector fetching MSFT with no instrument for it was built");
        assert!(error.message().contains("MSFT"), "{}", error.message());
        Ok(())
    }

    #[test]
    fn a_daily_bar_source_that_publishes_before_the_close_is_refused() -> Result<()> {
        let mut manifest = AlpacaBarsConnector::shipped_manifest()?;
        manifest.publication_delay_ms = Duration::from_hours(15).as_millis();
        let error = AlpacaBarsConnector::new(manifest, instruments())
            .expect_err("a delay under the session length was accepted");
        assert!(
            error
                .message()
                .contains("hands a backtest the close at the open"),
            "{}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_manifest_on_another_feed_or_timeframe_is_refused() -> Result<()> {
        let mut sip = AlpacaBarsConnector::shipped_manifest()?;
        sip.endpoint
            .query
            .insert("feed".to_string(), "sip".to_string());
        let error = AlpacaBarsConnector::new(sip, instruments())
            .expect_err("a consolidated feed was stamped as IEX");
        assert!(error.message().contains("IEXG"), "{}", error.message());

        let mut minutes = AlpacaBarsConnector::shipped_manifest()?;
        minutes
            .endpoint
            .query
            .insert("timeframe".to_string(), "1Min".to_string());
        let error = AlpacaBarsConnector::new(minutes, instruments())
            .expect_err("minute bars were stamped as daily");
        assert!(error.message().contains("1Min"), "{}", error.message());
        Ok(())
    }

    #[test]
    fn a_manifest_that_sends_the_secret_under_the_key_id_header_is_refused() -> Result<()> {
        let mut manifest = AlpacaBarsConnector::shipped_manifest()?;
        manifest.auth.header = Some(AlpacaBarsConnector::KEY_ID_HEADER.to_string());
        let error = AlpacaBarsConnector::new(manifest, instruments())
            .expect_err("the secret key was routed to the key id header");
        assert!(
            error.message().contains("logs as an identifier"),
            "{}",
            error.message()
        );
        Ok(())
    }

    /// The cursor becomes the vendor's `start`, and the beginning cursor
    /// sends none — a `start` of zero would ask for every bar since 1970.
    #[test]
    fn the_cursor_is_sent_as_the_inclusive_start_of_the_next_fetch() -> Result<()> {
        let connector = connector()?;
        let first = connector.fetch_request(&Cursor::beginning())?;
        assert!(!first.query.contains_key("start"), "{first:?}");
        let resumed = connector.fetch_request(&Cursor::at_event_time(
            Timestamp::parse_rfc3339("2026-09-04T04:00:00Z").unwrap(),
        ))?;
        assert_eq!(
            resumed.query.get("start").map(String::as_str),
            Some("2026-09-04T04:00:00.000Z")
        );
        assert!(resumed.target().starts_with("/v2/stocks/bars?"));
        Ok(())
    }
}
