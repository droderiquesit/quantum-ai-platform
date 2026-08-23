//! A market-data adapter that opens a socket.
//!
//! Every other source in this crate invents its records or reads them back from
//! a file. This one asks a vendor over HTTP and turns the answer into
//! [`SensedRecord`]s, which makes it the first place in the platform where a
//! number can arrive that nobody here computed.
//!
//! # Why this is a REST poll and not a stream
//!
//! [`DataAdapter::poll`] hands the caller the clock, and a polling client is
//! the only shape that fits it: the same adapter drives a live run and a
//! backtest because each call names the instant it is allowed to know about.
//! A streaming feed would own the clock instead, and a backtest driven by one
//! is a backtest that reads the future. Transport is
//! [`qip_transport::HttpClient`] — blocking, one connection per request, every
//! limit explicit.
//!
//! # What this adapter refuses to do
//!
//! It does not fall back to synthetic data. An adapter with no endpoint and no
//! credential reports what is missing through
//! [`SourceDescriptor::production_requirement`] and fails
//! [`DataAdapter::start`] and [`DataAdapter::poll`] with
//! [`qip_core::error::Error::Unavailable`]. The reasoning is the one
//! `qip_training::vertex` sets out: a stand-in that returns plausible numbers
//! is worse than one that returns none, because the plausible numbers reach a
//! model card, a backtest and eventually a position, and nothing downstream can
//! tell them from the real thing.
//!
//! It does not deduplicate. Each poll re-asks for a fixed window
//! ([`RestFeedConfig::window`]), so a poll whose response was lost is covered by
//! the next one; the overlap means a vendor record can be handed over twice.
//! That is deliberate — the bus deduplicates on
//! [`qip_events::EventBody::idempotency_key`], and losing a trade is worse than
//! publishing one twice.
//!
//! It does not re-validate what the vendor sent. A bar whose high is below its
//! low is passed on, so that [`crate::service::IngestionService`] rejects it at
//! the validation gate and publishes a
//! [`qip_financial::intelligence::DataQualityFailure`]. An adapter that quietly
//! dropped it would make bad vendor data invisible, which is the failure
//! charter section 21 exists to prevent. What is refused here is what cannot
//! become a record at all: a body that is not JSON, a field of the wrong type,
//! an interval or a trade condition this decoder cannot name, a symbol with no
//! instrument behind it.
//!
//! # The peer is untrusted
//!
//! A vendor is a third party having a bad day. Every response is bounded before
//! it is buffered ([`ClientLimits::max_body`]), every wait is bounded
//! ([`ClientLimits::read_timeout`], [`ClientLimits::connect_timeout`]), and the
//! number of records one response may carry is capped
//! ([`RestFeedConfig::max_records`]) so a well-formed 900 kB answer still cannot
//! turn into a million allocations downstream. A body claiming to be
//! `text/html` is parsed as JSON anyway: trusting the peer's content type would
//! let the peer choose which code path runs.
//!
//! # Point in time
//!
//! Two instants are not the same and this module keeps them apart. A record's
//! event time is whatever the vendor said it was, parsed from the payload and
//! never taken from the local clock. Its *knowable* time is the earliest
//! instant the platform could have acted on it: for a bar, the close of its
//! bucket plus the vendor's dissemination delay — a bar is not knowable while
//! it is still forming. A record whose knowable time is after the poll's
//! `until` is withheld and counted in [`FetchStats::withheld`], not handed over
//! early. Where the record type carries a [`Provenance`], its `ingestion_time`
//! is the caller's `until` rather than the wall clock, so the same fetch
//! replayed in a backtest produces the same record it produced live.

use qip_core::error::{Error, Result};
use qip_core::{Duration, ObjectId, Timestamp};
use qip_events::Topic;
use qip_financial::intelligence::ReferenceDataUpdate;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_market::bar::{Bar, Interval};
use qip_market::book::Side;
use qip_market::quote::{Quote, Trade, TradeCondition};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use crate::adapter::{DataAdapter, MarketDataAdapter, SensedRecord, SourceDescriptor};

/// Default endpoint path, overridden per vendor.
const DEFAULT_PATH: &str = "/v1/market-data";
/// Default header the credential travels in. Never a query parameter: a URL is
/// written to every access log on the path, and a credential in one is a
/// credential in all of them.
const DEFAULT_KEY_HEADER: &str = "x-api-key";
/// The delay a standard non-realtime entitlement carries. The default is
/// deliberately the conservative one: a deployment that has not stated its
/// vendor's dissemination delay withholds records it could have published,
/// which is visible in [`FetchStats::withheld`], rather than publishing records
/// before they existed, which is visible in nothing until a backtest is wrong.
const DEFAULT_PUBLICATION_DELAY: Duration = Duration::from_mins(15);
/// Headers `qip_transport::HttpRequest` writes itself and refuses to take from
/// a caller. Naming them here is what turns "the credential quietly vanished"
/// into a configuration error.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];

/// One instrument this adapter is allowed to receive.
///
/// The mapping is configuration, not inference: a vendor's `MSFT` and another
/// vendor's `MSFT` are not necessarily the same instrument, and an adapter that
/// minted an [`ObjectId`] from a ticker would silently merge two of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestInstrument {
    pub object_id: ObjectId,
    /// The symbol as the vendor writes it, which is what the response is keyed
    /// by and what goes in the request.
    pub symbol: String,
    /// Venue code stamped on every record for this instrument.
    pub venue: String,
}

impl RestInstrument {
    pub fn new(object_id: ObjectId, symbol: impl Into<String>, venue: impl Into<String>) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            venue: venue.into(),
        }
    }
}

/// Everything a deployment has to decide before this adapter can fetch.
///
/// Not `Serialize`, and its [`std::fmt::Debug`] redacts the credential: a
/// config struct that round-trips through JSON ends up in a log line, a crash
/// dump or a support ticket, and the API key would go with it.
#[derive(Clone)]
pub struct RestFeedConfig {
    /// Stable adapter name. Recorded as the provenance source, so changing it
    /// renames the origin of every record already published under it.
    pub name: String,
    /// Human-readable vendor description, for the source listing.
    pub provider: String,
    /// `http://host[:port]` of the vendor. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    pub base_url: Option<String>,
    /// Path of the market-data endpoint under `base_url`. May not carry a
    /// query string: this adapter owns the query, and a second `?` would put
    /// the symbols somewhere the vendor does not read them.
    pub path: String,
    /// The credential. `None` is unconfigured, not "this vendor is open".
    pub api_key: Option<String>,
    /// Header the credential travels in, since vendors disagree.
    pub api_key_header: String,
    /// Licensing class of what the vendor sends, which decides whether records
    /// may drive a capital decision and whether they may be displayed raw.
    pub licensing: LicensingClass,
    /// How long after an event the vendor publishes it. Used to decide when a
    /// record became knowable, and reported as the descriptor's expected
    /// latency.
    pub publication_delay: Duration,
    /// How far back each poll asks. Larger than the polling interval on
    /// purpose: the overlap is what covers a poll whose response was lost.
    pub window: Duration,
    /// Most records one response may carry, across all four kinds.
    pub max_records: usize,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for RestFeedConfig {
    fn default() -> Self {
        Self {
            name: "rest-market-data".into(),
            provider: "unconfigured REST market-data vendor".into(),
            base_url: None,
            path: DEFAULT_PATH.into(),
            api_key: None,
            api_key_header: DEFAULT_KEY_HEADER.into(),
            licensing: LicensingClass::Licensed,
            publication_delay: DEFAULT_PUBLICATION_DELAY,
            window: Duration::from_mins(5),
            max_records: 10_000,
            http: ClientLimits {
                // A market-data page is tens of kilobytes. A megabyte is a
                // generous ceiling and still small enough that refusing one
                // costs nothing.
                max_body: 1024 * 1024,
                max_headers: 32,
                connect_timeout: StdDuration::from_secs(2),
                read_timeout: StdDuration::from_secs(5),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

impl std::fmt::Debug for RestFeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestFeedConfig")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("path", &self.path)
            // Present or absent is worth knowing; the value never is.
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("api_key_header", &self.api_key_header)
            .field("licensing", &self.licensing)
            .field("publication_delay", &self.publication_delay)
            .field("window", &self.window)
            .field("max_records", &self.max_records)
            .field("http", &self.http)
            .finish()
    }
}

/// What this adapter has done, for metrics and for tests that assert a fetch
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FetchStats {
    /// Responses successfully read and decoded.
    pub fetches: u64,
    /// Records handed to the caller.
    pub emitted: u64,
    /// Records the vendor sent that were not yet knowable at the poll's
    /// `until`. Not an error and not a loss: the next poll's window covers
    /// them again.
    pub withheld: u64,
}

/// Polls a vendor's REST endpoint and produces market records.
#[derive(Debug)]
pub struct RestMarketDataAdapter {
    config: RestFeedConfig,
    /// Prebuilt endpoint, parsed once at construction so a malformed vendor
    /// address fails where it was configured rather than on the first fetch.
    endpoint: Option<Url>,
    /// Keyed by vendor symbol, which is what arrives in the response.
    instruments: BTreeMap<String, RestInstrument>,
    client: HttpClient,
    stats: FetchStats,
}

impl RestMarketDataAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why the descriptor's
    /// requirement is never `None`. A configured adapter is not by itself a
    /// production feed.
    pub const REQUIREMENTS: [&'static str; 4] = [
        "a TLS-terminating egress proxy in front of this adapter, or a vendor reachable over the \
         cluster network: `qip_transport::http` has no TLS stack and refuses `https` by name \
         rather than downgrading it, so a credential sent straight to a public vendor would \
         cross the internet in clear text",
        "a licence with the vendor that permits this use, and a `licensing` class on the config \
         that matches it — the class decides whether a record may drive a capital decision and \
         whether its raw value may be displayed",
        "the vendor's contractual dissemination delay in `publication_delay`, because that is \
         what decides when a record became knowable, and a delay set to zero on a delayed feed \
         publishes records before the deployment was entitled to see them",
        "an alert on the withheld and refusal counts, which are what a vendor that has started \
         sending records this decoder cannot read looks like from the outside",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot fetch still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong — an unparseable
    /// address, a symbol that cannot go in a request line, a credential
    /// carrying a newline, two instruments claiming one symbol.
    pub fn new(config: RestFeedConfig, instruments: Vec<RestInstrument>) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::invalid(
                "a REST feed needs a name: it is recorded as the provenance source of every \
                 record it produces",
            ));
        }
        if config.max_records == 0 {
            return Err(Error::invalid(
                "max_records is zero, which would refuse every response the vendor sends",
            ));
        }
        let header = config.api_key_header.trim().to_ascii_lowercase();
        if header.is_empty() {
            return Err(Error::invalid(
                "the credential needs a header to travel in; it is never put in the URL",
            ));
        }
        // The client writes these four itself and silently drops a caller's
        // copy, which is right for framing headers and fatal for this one: the
        // request would go out unauthenticated and the vendor's 401 would look
        // like a bad key rather than a header name nobody could use.
        if CLIENT_OWNED_HEADERS.contains(&header.as_str()) {
            return Err(Error::invalid(format!(
                "the credential cannot travel in the `{header}` header: the transport writes that \
                 one itself and drops a caller's copy, so the request would leave without a \
                 credential at all"
            )));
        }
        if !header.chars().all(|c| c.is_ascii_graphic() && c != ':') {
            return Err(Error::invalid(format!(
                "{header:?} is not a usable header name: a space, a colon or a control character \
                 in one would end the header and let the rest be read as another"
            )));
        }
        if let Some(key) = &config.api_key {
            validate_credential(key)?;
        }
        if config.path.contains('?') || config.path.contains('#') {
            return Err(Error::invalid(format!(
                "the endpoint path {:?} carries a query or a fragment; this adapter builds the \
                 query itself, and a second one would put the symbols where the vendor does not \
                 read them",
                config.path
            )));
        }

        let endpoint = match &config.base_url {
            Some(base) => {
                let url = Url::parse(base).map_err(Error::from)?;
                Some(url.with_path(&config.path).map_err(Error::from)?)
            }
            None => None,
        };

        let mut mapped: BTreeMap<String, RestInstrument> = BTreeMap::new();
        for instrument in instruments {
            validate_symbol(&instrument.symbol)?;
            if instrument.venue.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "instrument {} has no venue; the venue is part of every record's identity",
                    instrument.symbol
                )));
            }
            if let Some(existing) = mapped.insert(instrument.symbol.clone(), instrument) {
                return Err(Error::invalid(format!(
                    "two instruments claim the vendor symbol {}: a response keyed by it could \
                     not be resolved to one of them",
                    existing.symbol
                )));
            }
        }

        let client = HttpClient::new(config.http);
        Ok(Self {
            config,
            endpoint,
            instruments: mapped,
            client,
            stats: FetchStats::default(),
        })
    }

    pub fn stats(&self) -> FetchStats {
        self.stats
    }

    pub fn config(&self) -> &RestFeedConfig {
        &self.config
    }

    /// Whether this adapter can fetch at all.
    pub fn is_available(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// Configuration a deployment has not supplied, each named on its own.
    ///
    /// Separately rather than as one "not configured" so an operator with two
    /// of the three learns which one is left, instead of re-checking all of
    /// them.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.endpoint.is_none() {
            missing.push(format!(
                "no endpoint: set `base_url` to the vendor's REST base address, which this \
                 adapter requests `{}` under",
                self.config.path
            ));
        }
        if self.config.api_key.is_none() {
            missing.push(format!(
                "no credential: set `api_key`, which is sent in the `{}` header. A market feed \
                 that needs no credential is one nobody can attribute or rate-limit, and this \
                 adapter will not treat an anonymous endpoint as a licensed one",
                self.config.api_key_header
            ));
        }
        if self.instruments.is_empty() {
            missing.push(
                "no instruments: supply the vendor symbol, venue and platform ObjectId of every \
                 instrument this feed covers. The mapping is configuration because an id \
                 invented from a ticker merges two vendors' instruments into one"
                    .into(),
            );
        }
        missing
    }

    /// The full text of what production must supply: what is missing now,
    /// followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the adapter cannot fetch.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "{} cannot fetch and will not substitute generated data: {}",
            self.config.name,
            self.requirement()
        ))
    }

    /// Vendor symbols, in the order they go into the request.
    pub fn symbols(&self) -> Vec<String> {
        self.instruments.keys().cloned().collect()
    }

    /// Fetch once and decode, without the knowable-time gate.
    ///
    /// Separate from [`DataAdapter::poll`] so an operator can exercise the
    /// connection and the credential — the two things a deployment gets wrong —
    /// without the result depending on where the clock is.
    pub fn fetch(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        Ok(self
            .fetch_dated(until)?
            .into_iter()
            .map(|(r, _)| r)
            .collect())
    }

    /// One request, decoded into records paired with the instant each became
    /// knowable.
    fn fetch_dated(&mut self, until: Timestamp) -> Result<Vec<(SensedRecord, Timestamp)>> {
        let Some(endpoint) = &self.endpoint else {
            return Err(self.unavailable());
        };
        let Some(key) = &self.config.api_key else {
            return Err(self.unavailable());
        };
        if self.instruments.is_empty() {
            return Err(self.unavailable());
        }

        let since = until.saturating_sub(self.config.window);
        let target = format!(
            "{endpoint}?symbols={}&since={}&until={}",
            self.symbols().join(","),
            since.to_rfc3339(),
            until.to_rfc3339()
        );
        let request = HttpRequest::new(Method::Get, &target)
            .map_err(Error::from)?
            .with_header("accept", "application/json")
            .with_header(&self.config.api_key_header, key);

        let response = self.client.send(&request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        let feed: WireFeed = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "{} sent a body this decoder cannot read: {error}. The first bytes of it were: {}",
                self.config.name,
                response.body_excerpt()
            ))
        })?;

        let count = feed.bars.len() + feed.quotes.len() + feed.trades.len() + feed.reference.len();
        if count > self.config.max_records {
            return Err(Error::guard(format!(
                "{} sent {count} records and the cap is {}: a response small enough to read is \
                 not automatically a response worth expanding",
                self.config.name, self.config.max_records
            )));
        }

        let mut dated = Vec::with_capacity(count);
        for bar in feed.bars {
            dated.push(self.decode_bar(bar)?);
        }
        for quote in feed.quotes {
            dated.push(self.decode_quote(quote)?);
        }
        for trade in feed.trades {
            dated.push(self.decode_trade(trade)?);
        }
        for update in feed.reference {
            dated.push(self.decode_reference(update, until)?);
        }
        self.stats.fetches += 1;
        Ok(dated)
    }

    /// What a non-2xx status means here.
    ///
    /// Separated by class because the operator action differs: a rejected
    /// credential is a deployment to fix, a 404 is a path to fix, a 429 or a
    /// 5xx is a vendor to wait for. One error type for all three would put
    /// every one of them on the same runbook page.
    fn status_refusal(&self, status: u16, excerpt: &str) -> Error {
        let name = &self.config.name;
        match status {
            401 | 403 => Error::denied(format!(
                "{name} rejected this deployment's credential with HTTP {status}. The credential \
                 itself is not quoted here, and is not written to any log by this adapter"
            )),
            404 => Error::not_found(format!(
                "{name} has no endpoint at the configured path (HTTP 404): {excerpt}"
            )),
            408 | 429 => Error::unavailable(format!(
                "{name} is rate-limiting or timing out this deployment (HTTP {status}): {excerpt}"
            )),
            500..=599 => Error::unavailable(format!(
                "{name} failed to serve the request (HTTP {status}): {excerpt}"
            )),
            other => Error::invalid(format!(
                "{name} answered HTTP {other}, which this adapter does not know how to read: \
                 {excerpt}"
            )),
        }
    }

    fn resolve(&self, symbol: &str) -> Result<&RestInstrument> {
        self.instruments.get(symbol).ok_or_else(|| {
            Error::not_found(format!(
                "{} returned the symbol {symbol:?}, which is not in this adapter's instrument \
                 map ({}). A record whose subject cannot be identified is refused rather than \
                 published under an invented id",
                self.config.name,
                self.symbols().join(", ")
            ))
        })
    }

    /// A bar becomes knowable when its bucket closes, not when it opens: until
    /// then it is a partial bar, and treating one as final is the cheapest way
    /// to write a backtest that reads the future.
    fn decode_bar(&self, wire: WireBar) -> Result<(SensedRecord, Timestamp)> {
        let instrument = self.resolve(&wire.symbol)?;
        let interval = interval_from_code(&wire.interval)?;
        let bar = Bar {
            object_id: instrument.object_id.clone(),
            venue: instrument.venue.clone(),
            interval,
            open_time: wire.open_time,
            open: wire.open,
            high: wire.high,
            low: wire.low,
            close: wire.close,
            volume: wire.volume,
            vwap: wire.vwap,
            trade_count: wire.trade_count.unwrap_or(0),
            quality: DataQuality::clean(),
        };
        let knowable = bar
            .close_time()
            .saturating_add(self.config.publication_delay);
        Ok((SensedRecord::Bar(Box::new(bar)), knowable))
    }

    fn decode_quote(&self, wire: WireQuote) -> Result<(SensedRecord, Timestamp)> {
        let instrument = self.resolve(&wire.symbol)?;
        let quote = Quote {
            object_id: instrument.object_id.clone(),
            venue: instrument.venue.clone(),
            at: wire.at,
            bid: wire.bid,
            ask: wire.ask,
            bid_size: wire.bid_size,
            ask_size: wire.ask_size,
            quality: DataQuality::clean(),
        };
        let knowable = quote.at.saturating_add(self.config.publication_delay);
        Ok((SensedRecord::Quote(quote), knowable))
    }

    fn decode_trade(&self, wire: WireTrade) -> Result<(SensedRecord, Timestamp)> {
        let instrument = self.resolve(&wire.symbol)?;
        let trade = Trade {
            object_id: instrument.object_id.clone(),
            venue: instrument.venue.clone(),
            at: wire.at,
            price: wire.price,
            size: wire.size,
            aggressor: wire.aggressor.as_deref().map(side_from_code).transpose()?,
            condition: match wire.condition.as_deref() {
                Some(code) => condition_from_code(code)?,
                None => TradeCondition::Regular,
            },
            // The vendor's own identity for the print, kept so a reconciliation
            // against the vendor has something to join on and so the bus can
            // recognise a redelivery.
            trade_id: wire.trade_id,
            quality: DataQuality::clean(),
        };
        let knowable = trade.at.saturating_add(self.config.publication_delay);
        Ok((SensedRecord::Trade(trade), knowable))
    }

    /// A reference change is the clearest case for keeping the two instants
    /// apart: `effective_from` is when the new value is true, which is often in
    /// the future, while what decides whether this adapter may hand the record
    /// over is when the vendor announced it.
    fn decode_reference(
        &self,
        wire: WireReference,
        until: Timestamp,
    ) -> Result<(SensedRecord, Timestamp)> {
        let instrument = self.resolve(&wire.symbol)?;
        if wire.field.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} sent a reference update for {} with no field name",
                self.config.name, wire.symbol
            )));
        }
        let knowable = wire
            .announced_at
            .saturating_add(self.config.publication_delay);
        let mut provenance = Provenance::new(
            self.config.name.clone(),
            wire.announced_at,
            // The caller's clock, not the wall clock: a backtest that fetches
            // this same response must produce this same record.
            until,
        )
        .with_licensing(self.config.licensing);
        if let Some(id) = wire.update_id {
            provenance = provenance.with_upstream_id(id);
        }
        let update = ReferenceDataUpdate {
            object_id: instrument.object_id.to_string(),
            field: wire.field,
            previous_value: wire.previous_value,
            new_value: wire.new_value,
            effective_from: wire.effective_from,
            provenance,
        };
        Ok((SensedRecord::ReferenceData(Box::new(update)), knowable))
    }
}

impl DataAdapter for RestMarketDataAdapter {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: self.config.name.clone(),
            provider: self.config.provider.clone(),
            licensing: self.config.licensing,
            topics: vec![
                Topic::MarketBar,
                Topic::MarketQuote,
                Topic::MarketTrade,
                Topic::ReferenceDataUpdated,
            ],
            expected_latency: self.config.publication_delay,
            // Never `None`, even fully configured: see [`Self::REQUIREMENTS`].
            production_requirement: Some(self.requirement()),
        }
    }

    /// Refuse at startup rather than at the first tick.
    ///
    /// A deployment that is missing a credential should fail while somebody is
    /// watching the rollout, not an hour later inside a poll loop.
    fn start(&mut self, _at: Timestamp) -> Result<()> {
        if self.is_available() {
            Ok(())
        } else {
            Err(self.unavailable())
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let dated = self.fetch_dated(until)?;
        let mut records = Vec::with_capacity(dated.len());
        for (record, knowable) in dated {
            if knowable > until {
                self.stats.withheld += 1;
                continue;
            }
            records.push(record);
        }
        // Event order, so a consumer that assumes a monotone stream is not
        // reading the vendor's arbitrary array order instead.
        records.sort_by_key(|r| r.occurred_at().as_nanos());
        self.stats.emitted += records.len() as u64;
        Ok(records)
    }
}

impl MarketDataAdapter for RestMarketDataAdapter {
    fn instruments(&self) -> Vec<ObjectId> {
        self.instruments
            .values()
            .map(|i| i.object_id.clone())
            .collect()
    }
}

/// Reject a credential that would break the request or leak into a log.
fn validate_credential(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(Error::invalid(
            "the API key is blank; an unconfigured credential is `None`, not an empty string, so \
             that the adapter reports itself unavailable instead of sending an empty header",
        ));
    }
    if key
        .chars()
        .any(|c| c.is_control() || c == '\r' || c == '\n')
    {
        return Err(Error::invalid(
            "the API key contains a control character; sent as a header value it would end the \
             header and let the rest be read as another one",
        ));
    }
    Ok(())
}

/// Symbols go into the request line, so what may be in one is decided here
/// rather than discovered when a vendor's symbol splits a request.
fn validate_symbol(symbol: &str) -> Result<()> {
    if symbol.trim().is_empty() {
        return Err(Error::invalid("an instrument with an empty symbol"));
    }
    if !symbol
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(Error::invalid(format!(
            "the symbol {symbol:?} contains a character this adapter will not put in a request \
             line: only ASCII letters, digits and . - _ : are accepted, since the symbols are \
             joined by commas into a query this adapter builds by hand"
        )));
    }
    Ok(())
}

/// Vendor interval codes. Guessing at an unknown one would misplace the bar's
/// close, which is the instant that decides when the bar became knowable.
fn interval_from_code(code: &str) -> Result<Interval> {
    match code {
        "1s" => Ok(Interval::Second),
        "1m" => Ok(Interval::Minute),
        "5m" => Ok(Interval::FiveMinutes),
        "15m" => Ok(Interval::FifteenMinutes),
        "1h" => Ok(Interval::Hour),
        "1d" => Ok(Interval::Day),
        "1w" => Ok(Interval::Week),
        "1mo" => Ok(Interval::Month),
        other => Err(Error::schema(format!(
            "unknown bar interval {other:?}: this decoder accepts 1s, 1m, 5m, 15m, 1h, 1d, 1w \
             and 1mo"
        ))),
    }
}

fn side_from_code(code: &str) -> Result<Side> {
    match code {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(Error::schema(format!(
            "unknown aggressor side {other:?}: this decoder accepts buy and sell. A guessed side \
             becomes signed order flow, which is an input to the microstructure signal"
        ))),
    }
}

/// Trade conditions decide whether a print contributes to price discovery, so
/// an unreadable one is refused rather than defaulted: defaulting to `regular`
/// would let a late or corrected print into a VWAP.
fn condition_from_code(code: &str) -> Result<TradeCondition> {
    match code {
        "regular" => Ok(TradeCondition::Regular),
        "auction" => Ok(TradeCondition::Auction),
        "late_report" => Ok(TradeCondition::LateReport),
        "off_exchange" => Ok(TradeCondition::OffExchange),
        "corrected" => Ok(TradeCondition::Corrected),
        other => Err(Error::schema(format!(
            "unknown trade condition {other:?}: this decoder accepts regular, auction, \
             late_report, off_exchange and corrected"
        ))),
    }
}

// --- the wire schema --------------------------------------------------------
//
// The shape this decoder accepts, and the whole of what it promises to read. No
// vendor is obliged to speak it: a deployment whose vendor does not either
// points this adapter at a translating endpoint or writes a second decoder
// beside this one. Naming one schema and refusing everything else is the reason
// a malformed response is an error here rather than a partially-populated
// record downstream.
//
// Unknown fields are ignored rather than refused, because a vendor adding a
// field is not a fault and must not stop the feed. Unknown *values* in a field
// this decoder reads are refused, because those change what the record means.

#[derive(Debug, Default, Deserialize)]
struct WireFeed {
    #[serde(default)]
    bars: Vec<WireBar>,
    #[serde(default)]
    quotes: Vec<WireQuote>,
    #[serde(default)]
    trades: Vec<WireTrade>,
    #[serde(default)]
    reference: Vec<WireReference>,
}

#[derive(Debug, Deserialize)]
struct WireBar {
    symbol: String,
    /// `1m`, `1d`, … See [`interval_from_code`].
    interval: String,
    /// Start of the bucket. The close, and so the knowable time, is derived
    /// from it and the interval rather than taken from a second vendor field
    /// that could disagree with both.
    open_time: Timestamp,
    open: qip_core::Decimal,
    high: qip_core::Decimal,
    low: qip_core::Decimal,
    close: qip_core::Decimal,
    volume: qip_core::Decimal,
    #[serde(default)]
    vwap: Option<qip_core::Decimal>,
    #[serde(default)]
    trade_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WireQuote {
    symbol: String,
    at: Timestamp,
    bid: qip_core::Decimal,
    ask: qip_core::Decimal,
    bid_size: qip_core::Decimal,
    ask_size: qip_core::Decimal,
}

#[derive(Debug, Deserialize)]
struct WireTrade {
    symbol: String,
    at: Timestamp,
    price: qip_core::Decimal,
    size: qip_core::Decimal,
    #[serde(default)]
    aggressor: Option<String>,
    #[serde(default)]
    condition: Option<String>,
    #[serde(default)]
    trade_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireReference {
    symbol: String,
    /// e.g. `lot_size`, `sector`, `venue`.
    field: String,
    #[serde(default)]
    previous_value: Option<String>,
    new_value: String,
    /// When the new value starts being true, which may be in the future.
    effective_from: Timestamp,
    /// When the vendor published the change, which is when it became knowable.
    announced_at: Timestamp,
    #[serde(default)]
    update_id: Option<String>,
}
