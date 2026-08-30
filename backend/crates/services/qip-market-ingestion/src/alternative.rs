//! An alternative-data adapter that opens a socket.
//!
//! Satellite imagery, IoT telemetry, mobility counts, web-scraped panels. Until
//! this module existed, [`AlternativeDataPoint`] and
//! [`Topic::AlternativeDataReceived`] were produced by the synthetic generator
//! and by nothing else, which meant every consumer of alternative data in this
//! platform had only ever seen readings it wrote itself.
//!
//! Transport, credential handling, the untrusted-peer stance and the refusal to
//! substitute generated data are the ones [`crate::rest`] established, and this
//! module does not restate them. Two disciplines it *does* restate, because
//! alternative data is where both bite hardest, are [`crate::narrative`]'s: the
//! instant a record became knowable, and the licence it travels under.
//!
//! # Three instants, and which one this adapter takes
//!
//! A price has one instant. A document has two. A satellite reading has three,
//! and they are routinely weeks apart:
//!
//! | instant | what it is |
//! | --- | --- |
//! | captured | when the sensor saw the thing — the overpass, the ping, the crawl |
//! | processed | when the vendor finished turning pixels into a number |
//! | published | when the vendor released that number to this deployment |
//!
//! The one this adapter keys on is **published**, because it is the only one at
//! which a consumer could have acted. A parking-lot count from an image taken
//! on the 3rd, processed on the 9th and released on the 12th did not exist as
//! information on the 3rd; a record keyed on the capture instant claims nine
//! days of foresight, and no per-feature timing check downstream can see it —
//! [`crate::adapter::SensedRecord::occurred_at`] returns `observed_at` for an
//! alternative-data point precisely because that is its *valid* time, and
//! nothing downstream is in a position to know when it was released.
//!
//! So the capture instant is recorded as [`AlternativeDataPoint::observed_at`],
//! which is what that field means — when the underlying phenomenon was observed
//! — and the publication instant is recorded as [`Provenance::event_time`],
//! which is what gates the record and what an audit afterwards subtracts from
//! `observed_at` to see the lag. That is exactly [`crate::narrative`]'s
//! arrangement for a filing, whose `period_end` is its valid time and whose
//! `filed_at` is its knowable anchor.
//!
//! What this does **not** promise: the *processing* instant does not reach the
//! record. [`AlternativeDataPoint`] has no field that means it, and this module
//! will not write it into one that means something else — a processing instant
//! stored where a consumer reads a publication instant is worse than a
//! processing instant nobody kept, because the first cannot be told from the
//! truth and the second can. It is not ignored, though: the vendor must state
//! it, the three instants must be in order, and a vendor claiming a reading
//! processed before it was captured or published before it was processed is
//! refused. That ordering is the only claim about the processing instant this
//! module can make honestly, and making it is what turns a nonsense pipeline
//! into an error instead of a plausible number.
//!
//! # Licensing, which is stricter here than anywhere else
//!
//! Alternative data is commonly the most restrictively licensed thing a fund
//! buys: per-seat, non-displayable, derived-values-only, sometimes with the
//! vendor's own consent to the raw imagery layered on top. So every point
//! leaves here carrying a [`LicensingClass`] — the one the point states, or
//! failing that the one the feed is configured with — and where neither exists
//! the point is **refused**.
//!
//! That refusal is the whole reason the field is read at all.
//! [`LicensingClass`]'s `Default` is `Internal`, and
//! [`LicensingClass::allows_raw_display`] is true for `Internal`, so an unset
//! class does not read downstream as "unknown". It reads as **permission**.
//! This follows [`crate::narrative`], which refuses an unclassed document for
//! the same reason, and `qip_data_finder::legal`, where unknown is a third
//! value and is never a grant.
//!
//! What this does **not** promise: it labels, it does not redact. A
//! `Restricted` reading still arrives with its value, because a model's use of
//! it is a derived use and stripping it would blind the platform to comply with
//! a rule about *display*. The class travels with the record so that the
//! boundary which would display it can decide.
//!
//! # Quality, and why an unstated one is refused
//!
//! Alternative data is full of holes: cloud cover over the car park, a sensor
//! offline for a fortnight, a panel that lost a third of its contributors. The
//! vendors fill those holes, and they are right to — a series with gaps is hard
//! to use. What is not acceptable is the filling arriving as observation.
//!
//! [`DataQuality`] already models this: `is_imputed`, `completeness`,
//! `confidence`, `validation_failures` and the issues behind them. The trap is
//! that [`DataQuality::default`] is *completeness 1.0, confidence 1.0, not
//! imputed* — a perfect, directly observed reading. So a vendor that says
//! nothing about quality produces exactly the same record as a vendor that
//! measured everything perfectly, and [`DataQuality::score`] feeds
//! [`qip_financial::quality::DECISION_QUALITY_FLOOR`], which is what decides
//! whether the reading may drive a decision at all.
//!
//! Every point therefore has to state its quality, and every point has to state
//! whether its value was **observed** or **imputed** — and an imputed one has
//! to name the method, because "we filled this gap" and "we filled this gap by
//! carrying the last value forward for eleven days" are different facts. A
//! point that states nothing is refused. There is deliberately no third basis
//! for "the vendor did not say" to be pattern-matched into.
//!
//! What this does **not** promise: it cannot detect *undeclared* imputation. A
//! vendor that interpolates a series and ships it labelled observed is
//! indistinguishable from here from one that measured it. Only a vendor that
//! keeps its raw observations addressable can answer that, which is why it is
//! one of [`AlternativeFeedAdapter::REQUIREMENTS`] rather than a claim made in
//! this file.
//!
//! # Bounds
//!
//! A panel response is a long list of small readings rather than a short list
//! of large ones, so [`AlternativeFeedConfig::max_records`] is what actually
//! protects this process; the transport's body cap is the second line.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_events::Topic;
use qip_financial::intelligence::AlternativeDataPoint;
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration as StdDuration;

use crate::adapter::{AlternativeDataAdapter, DataAdapter, SensedRecord, SourceDescriptor};

/// Default endpoint path, overridden per vendor.
const DEFAULT_PATH: &str = "/v1/alternative";
/// Default header the credential travels in. Never a query parameter, for the
/// reason [`crate::rest`] gives: a URL reaches every access log on the path.
const DEFAULT_KEY_HEADER: &str = "x-api-key";
/// Delay between a vendor publishing a reading and this deployment being
/// entitled to it. Conservative by default: a deployment that has not stated
/// its terms withholds readings it could have published, which shows up in
/// [`AlternativeStats::withheld`], rather than publishing them early, which
/// shows up in nothing until a backtest is wrong.
const DEFAULT_PUBLICATION_DELAY: Duration = Duration::from_mins(15);
/// Headers `qip_transport::HttpRequest` writes itself and drops a caller's copy
/// of. Stated here as well as in [`crate::rest`] rather than shared from it:
/// that module is a landed, tested unit this one does not restructure.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];

/// One subject this feed is allowed to report on.
///
/// The mapping is configuration for the reason [`crate::narrative`]'s issuer
/// map is: [`AlternativeDataPoint::subject_id`] is the key every downstream
/// join uses, and one vendor's site code, region code or company key is not
/// another's. An id invented from a vendor key would merge two subjects, and
/// alternative data is where that is hardest to notice — nobody eyeballs a
/// parking-lot count the way they eyeball a share price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeSubject {
    /// The platform's canonical id for the entity or region, e.g.
    /// `ent-northwind` or `region-us-midwest`.
    pub subject_id: String,
    /// The key as the vendor writes it, which is what arrives in the response.
    pub vendor_key: String,
}

impl AlternativeSubject {
    pub fn new(subject_id: impl Into<String>, vendor_key: impl Into<String>) -> Self {
        Self {
            subject_id: subject_id.into(),
            vendor_key: vendor_key.into(),
        }
    }
}

/// Everything a deployment has to decide before this adapter can fetch.
///
/// Not `Serialize`, and its [`std::fmt::Debug`] redacts the credential: a
/// config that round-trips through JSON ends up in a log line or a support
/// ticket, and the API key would go with it.
#[derive(Clone)]
pub struct AlternativeFeedConfig {
    /// Stable adapter name. Recorded as the provenance source, so changing it
    /// renames the origin of every reading already published under it.
    pub name: String,
    /// Human-readable vendor description, for the source listing.
    pub provider: String,
    /// `http://host[:port]` of the vendor. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    pub base_url: Option<String>,
    /// Path of the readings endpoint under `base_url`. May not carry a query
    /// string: this adapter owns the query.
    pub path: String,
    /// The credential. `None` is unconfigured, not "this vendor is open".
    pub api_key: Option<String>,
    /// Header the credential travels in, since vendors disagree.
    pub api_key_header: String,
    /// Licensing class for readings that do not state their own.
    ///
    /// `None` is a legitimate configuration, not a missing one: a vendor whose
    /// datasets carry different terms states them per reading, and inventing a
    /// feed-wide default would override them. What `None` means is that a
    /// reading arriving without terms is refused rather than given the
    /// `Internal` default, which permits raw display.
    pub licensing: Option<LicensingClass>,
    /// How long after publication the vendor releases a reading to this
    /// deployment. Decides when a reading became knowable.
    pub publication_delay: Duration,
    /// How far back each poll asks, over *publication* time. Larger than the
    /// polling interval on purpose: the overlap covers a poll whose response
    /// was lost.
    pub window: Duration,
    /// Most readings one response may carry.
    pub max_records: usize,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for AlternativeFeedConfig {
    fn default() -> Self {
        Self {
            name: "alternative-data".into(),
            provider: "unconfigured alternative-data vendor".into(),
            base_url: None,
            path: DEFAULT_PATH.into(),
            api_key: None,
            api_key_header: DEFAULT_KEY_HEADER.into(),
            // Deliberately not `Some(LicensingClass::Internal)`: see the field.
            licensing: None,
            publication_delay: DEFAULT_PUBLICATION_DELAY,
            // Alternative data lands in daily or weekly batches rather than
            // continuously, so the window is wide: a poll lost on the morning a
            // weekly panel drops must be covered by the next one.
            window: Duration::from_hours(6),
            max_records: 20_000,
            http: ClientLimits {
                max_body: 4 * 1024 * 1024,
                max_headers: 32,
                connect_timeout: StdDuration::from_secs(2),
                // Longer than the price feed's: a vendor assembling a panel is
                // slower than one serving a quote cache, and a timeout that
                // fires on the normal case teaches operators to raise it
                // everywhere.
                read_timeout: StdDuration::from_secs(15),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

impl std::fmt::Debug for AlternativeFeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlternativeFeedConfig")
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
pub struct AlternativeStats {
    /// Responses successfully read and decoded.
    pub fetches: u64,
    /// Readings handed to the caller.
    pub emitted: u64,
    /// Readings not yet knowable at the poll's `until`. Not an error and not a
    /// loss: the next poll's window covers them again.
    pub withheld: u64,
    /// Readings whose value the vendor filled in rather than measured.
    ///
    /// Counted separately because it is the number that says how much of a
    /// series is actually data. A panel that quietly moves from 5% imputed to
    /// 60% imputed has stopped being the thing the model was fitted on, and
    /// nothing else says so.
    pub imputed: u64,
    /// Readings carrying at least one validation failure the vendor declared.
    pub with_declared_failures: u64,
}

/// Polls a vendor's alternative-data endpoint and produces readings.
#[derive(Debug)]
pub struct AlternativeFeedAdapter {
    config: AlternativeFeedConfig,
    /// Prebuilt endpoint, parsed once at construction so a malformed vendor
    /// address fails where it was configured rather than on the first fetch.
    endpoint: Option<Url>,
    /// Datasets this deployment ingests. A dataset outside the set is refused:
    /// coverage is configured, not discovered from whatever the vendor happens
    /// to send, because a dataset nobody configured is one nobody modelled and
    /// admitting it puts an unreviewed input in front of the signal layer.
    datasets: BTreeSet<String>,
    /// Keyed by vendor subject key, which is what arrives in the response.
    subjects: BTreeMap<String, AlternativeSubject>,
    client: HttpClient,
    stats: AlternativeStats,
}

impl AlternativeFeedAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why the descriptor's
    /// requirement is never `None`.
    pub const REQUIREMENTS: [&'static str; 5] = [
        "a TLS-terminating egress proxy in front of this adapter, or a vendor reachable over the \
         cluster network: `qip_transport::http` has no TLS stack and refuses `https` by name \
         rather than downgrading it, so a credential sent straight to a public vendor would \
         cross the internet in clear text",
        "a licence that covers this use, and a `licensing` class that matches it. Alternative \
         data is routinely the most restrictively licensed input a fund buys — per-seat, \
         derived-values-only, sometimes with a separate consent for the raw imagery — and the \
         class is what decides whether a reading may be shown or only modelled from. This \
         adapter refuses a reading that carries no class rather than defaulting to a permissive \
         one",
        "the vendor's dissemination delay in `publication_delay`, because that is what decides \
         when a reading became knowable; a delay set to zero on an embargoed panel publishes \
         readings before the deployment was entitled to them",
        "a vendor endpoint that states, per reading, when the observation was captured, when it \
         was processed and when it was published, and whether the value was measured or filled \
         in. This adapter records what a vendor declares and cannot see what it does not: a \
         series interpolated and shipped labelled observed is indistinguishable from here from \
         one that was measured, and that is exactly the input a backtest reads as history",
        "an alert on the imputation rate and the withheld count. A panel whose imputed share has \
         quietly risen is no longer the dataset any model was fitted on, and from the outside it \
         looks like a dataset that is still arriving",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot fetch still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong.
    pub fn new(
        config: AlternativeFeedConfig,
        datasets: Vec<String>,
        subjects: Vec<AlternativeSubject>,
    ) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::invalid(
                "an alternative-data feed needs a name: it is recorded as the provenance source \
                 of every reading it produces",
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
                 query itself, and a second one would put the datasets where the vendor does not \
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

        let mut covered: BTreeSet<String> = BTreeSet::new();
        for dataset in datasets {
            validate_query_token("dataset name", &dataset)?;
            covered.insert(dataset);
        }

        let mut mapped: BTreeMap<String, AlternativeSubject> = BTreeMap::new();
        for subject in subjects {
            validate_query_token("subject key", &subject.vendor_key)?;
            if subject.subject_id.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "the subject key {} maps to an empty subject id; that id is the join key \
                     every downstream consumer uses",
                    subject.vendor_key
                )));
            }
            if let Some(existing) = mapped.insert(subject.vendor_key.clone(), subject) {
                return Err(Error::invalid(format!(
                    "two subjects claim the vendor key {}: a reading keyed by it could not be \
                     resolved to one of them",
                    existing.vendor_key
                )));
            }
        }

        let client = HttpClient::new(config.http);
        Ok(Self {
            config,
            endpoint,
            datasets: covered,
            subjects: mapped,
            client,
            stats: AlternativeStats::default(),
        })
    }

    pub fn stats(&self) -> AlternativeStats {
        self.stats
    }

    pub fn config(&self) -> &AlternativeFeedConfig {
        &self.config
    }

    /// Vendor subject keys, in the order they go into the request.
    pub fn subject_keys(&self) -> Vec<String> {
        self.subjects.keys().cloned().collect()
    }

    /// Whether this adapter can fetch at all.
    pub fn is_available(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// Configuration a deployment has not supplied, each named on its own, so
    /// an operator with two of the three learns which one is left.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.endpoint.is_none() {
            missing.push(format!(
                "no endpoint: set `base_url` to the vendor's alternative-data base address, which \
                 this adapter requests `{}` under",
                self.config.path
            ));
        }
        if self.config.api_key.is_none() {
            missing.push(format!(
                "no credential: set `api_key`, which is sent in the `{}` header. An \
                 alternative-data feed that needs no credential is one whose licence terms nobody \
                 agreed to, and this adapter will not treat an anonymous endpoint as a licensed \
                 one",
                self.config.api_key_header
            ));
        }
        if self.datasets.is_empty() {
            missing.push(
                "no datasets: name the datasets this feed carries, e.g. \
                 `satellite.parking_lot_counts`. Coverage is configuration because a dataset \
                 nobody configured is one nobody modelled, and a proxy series admitted without \
                 review is an unreviewed input to a capital decision"
                    .into(),
            );
        }
        if self.subjects.is_empty() {
            missing.push(
                "no subjects: supply the vendor key and platform id of every entity or region \
                 this feed reports on. The mapping is configuration because a subject id invented \
                 from a vendor's site code merges two subjects, and nobody eyeballs a \
                 parking-lot count the way they eyeball a price"
                    .into(),
            );
        }
        missing
    }

    /// The full text of what production must supply: what is missing now,
    /// followed by what is required even when nothing is.
    pub fn requirement(&self) -> String {
        let mut parts = self.missing_configuration();
        if self.config.licensing.is_none() {
            parts.push(
                "no feed-wide licensing class is configured, so every reading must state its own; \
                 one that does not is refused rather than published under the `Internal` default, \
                 which would permit its raw value to be displayed"
                    .into(),
            );
        }
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the adapter cannot fetch.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "{} cannot fetch and will not substitute generated readings: {}",
            self.config.name,
            self.requirement()
        ))
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

    /// One request, decoded into readings paired with the instant each became
    /// knowable.
    fn fetch_dated(&mut self, until: Timestamp) -> Result<Vec<(SensedRecord, Timestamp)>> {
        let Some(endpoint) = &self.endpoint else {
            return Err(self.unavailable());
        };
        let Some(key) = &self.config.api_key else {
            return Err(self.unavailable());
        };
        if self.datasets.is_empty() || self.subjects.is_empty() {
            return Err(self.unavailable());
        }

        let since = until.saturating_sub(self.config.window);
        // `published_since`/`published_until`, not `since`/`until`: the window
        // is over publication time, and the parameter name is the only place
        // this adapter can say so to a vendor. A vendor filtering by capture
        // time would return last week's overpasses and miss the one processed
        // late — which is the one that has not been priced in.
        let target = format!(
            "{endpoint}?datasets={}&subjects={}&published_since={}&published_until={}",
            self.datasets.iter().cloned().collect::<Vec<_>>().join(","),
            self.subject_keys().join(","),
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

        if feed.readings.len() > self.config.max_records {
            return Err(Error::guard(format!(
                "{} sent {} readings and the cap is {}: a response small enough to read is not \
                 automatically a response worth expanding",
                self.config.name,
                feed.readings.len(),
                self.config.max_records
            )));
        }

        let mut dated = Vec::with_capacity(feed.readings.len());
        for reading in feed.readings {
            dated.push(self.decode(reading, until)?);
        }
        self.stats.fetches += 1;
        Ok(dated)
    }

    /// What a non-2xx status means here.
    ///
    /// Separated by class because the operator action differs: a rejected
    /// credential is a deployment to fix, a 404 is a path to fix, a 429 or a
    /// 5xx is a vendor to wait for. A 451 gets its own arm, because for a feed
    /// licensed this tightly it is the one status that means the agreement
    /// rather than the request.
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
            451 => Error::denied(format!(
                "{name} refused on legal grounds (HTTP 451): this deployment's licence does not \
                 cover what it asked for. Widening the request is not the fix; the agreement is: \
                 {excerpt}"
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

    /// The licensing class a reading travels under.
    ///
    /// Stated by the reading where the vendor states it, otherwise the feed's
    /// configured class, otherwise refused. The third branch is the reason this
    /// function exists rather than an `unwrap_or_default`.
    fn licensing_for(&self, stated: Option<&str>, observation: &str) -> Result<LicensingClass> {
        match stated {
            Some(code) => licensing_from_code(code),
            None => self.config.licensing.ok_or_else(|| {
                Error::denied(format!(
                    "{}: the reading {observation:?} states no licensing class and this feed \
                     configures no default, so there is nothing that says whether its value may \
                     be displayed. It is refused rather than admitted under the `Internal` \
                     default, which permits raw display: an unstated licence is not a permissive \
                     one, and alternative data is where that difference is most often expensive",
                    self.config.name
                ))
            }),
        }
    }

    /// One reading, with the three instants kept apart.
    fn decode(&mut self, wire: WireReading, until: Timestamp) -> Result<(SensedRecord, Timestamp)> {
        if wire.observation_id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} sent a reading with no observation id; the id is what a reconciliation \
                 against the vendor joins on",
                self.config.name
            )));
        }
        if !self.datasets.contains(&wire.dataset) {
            return Err(Error::not_found(format!(
                "{} returned the dataset {:?}, which this deployment does not cover ({}). A \
                 dataset nobody configured is one nobody modelled, and admitting it would put an \
                 unreviewed proxy in front of the signal layer",
                self.config.name,
                wire.dataset,
                self.datasets.iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let subject = self.subjects.get(&wire.subject).ok_or_else(|| {
            Error::not_found(format!(
                "{} returned the reading {:?} for the subject key {:?}, which is not in this \
                 deployment's subject map ({}). A reading whose subject cannot be identified is \
                 refused rather than published under an invented id, which would merge it with \
                 another subject's history",
                self.config.name,
                wire.observation_id,
                wire.subject,
                self.subject_keys().join(", ")
            ))
        })?;
        let subject_id = subject.subject_id.clone();
        if wire.metric.trim().is_empty() {
            return Err(Error::schema(format!(
                "{}: the reading {:?} names no metric; the metric is half of what the value \
                 means",
                self.config.name, wire.observation_id
            )));
        }

        // The three instants, in the order the world produces them. A vendor
        // that reports them out of order has either mislabelled a field or run
        // a pipeline that cannot have happened, and either way the publication
        // instant this record is gated on cannot be trusted to mean what it
        // says.
        if wire.processed_at < wire.captured_at {
            return Err(Error::schema(format!(
                "{}: the reading {:?} says it was processed at {} and captured at {}, which is a \
                 number derived from an observation that had not happened yet. The instants are \
                 refused rather than reordered: whichever field is mislabelled, the publication \
                 instant this record is gated on cannot be trusted either",
                self.config.name,
                wire.observation_id,
                wire.processed_at.to_rfc3339(),
                wire.captured_at.to_rfc3339()
            )));
        }
        if wire.published_at < wire.processed_at {
            return Err(Error::schema(format!(
                "{}: the reading {:?} says it was published at {} and processed at {}, so it was \
                 released before the number existed. Taking the earlier instant would make it \
                 knowable before anyone could have acted on it, which is leakage no downstream \
                 check can see",
                self.config.name,
                wire.observation_id,
                wire.published_at.to_rfc3339(),
                wire.processed_at.to_rfc3339()
            )));
        }

        let licensing = self.licensing_for(wire.licensing.as_deref(), &wire.observation_id)?;
        let quality = self.quality_for(&wire)?;
        if quality.is_imputed {
            self.stats.imputed += 1;
        }
        if quality.validation_failures > 0 {
            self.stats.with_declared_failures += 1;
        }

        // The publication instant, which is the one a consumer could have acted
        // at, and the caller's clock rather than the wall clock, so the same
        // fetch replayed in a backtest produces the same record it produced
        // live.
        let provenance = Provenance::new(self.config.name.clone(), wire.published_at, until)
            .with_licensing(licensing)
            .with_upstream_id(wire.observation_id);

        let point = AlternativeDataPoint {
            dataset: wire.dataset,
            subject_id,
            metric: wire.metric,
            // Passed through rather than clamped or checked for finiteness: a
            // non-finite value reaches `crate::service::IngestionService`'s
            // validation gate and becomes a `DataQualityFailure`, which is the
            // house rule `rest.rs` set for an incoherent bar. An adapter that
            // quietly corrected bad vendor data would make it invisible.
            value: wire.value,
            unit: wire.unit,
            // The capture instant: when the phenomenon was observed, which is
            // this record's valid time and is not what gated it.
            observed_at: wire.captured_at,
            lead_days: wire.lead_days,
            // Absent means zero, which `AlternativeDataPoint::is_actionable`
            // reads as not actionable. Defaulting is safe here for the one
            // reason it is unsafe for licensing: the default is the
            // restrictive value, not the permissive one.
            proxy_correlation: wire.proxy_correlation,
            proxies_for: wire.proxies_for,
            provenance,
            quality,
        };
        let knowable = wire
            .published_at
            .saturating_add(self.config.publication_delay);
        Ok((SensedRecord::AlternativeData(Box::new(point)), knowable))
    }

    /// The quality assessment a reading travels under.
    ///
    /// Built through [`DataQuality`]'s own constructors rather than by setting
    /// its fields, so the platform's model of what a failure and an imputation
    /// cost — each declared failure halves the remaining confidence, an
    /// imputation takes a fifth of it — applies to a vendor's readings exactly
    /// as it applies to everything else. A vendor that declares both a high
    /// confidence and the checks that failed has told us two things, and the
    /// lower of them is the one that should reach a decision.
    fn quality_for(&self, wire: &WireReading) -> Result<DataQuality> {
        let Some(stated) = &wire.quality else {
            return Err(Error::schema(format!(
                "{}: the reading {:?} states no quality. It is refused rather than given the \
                 default, which is completeness 1.0, confidence 1.0 and not imputed — a \
                 perfectly measured value. \"The vendor did not say\" and \"the vendor measured \
                 it exactly\" must not produce the same record, because that score is what \
                 decides whether the reading may drive a decision",
                self.config.name, wire.observation_id
            )));
        };
        for (label, value) in [
            ("completeness", stated.completeness),
            ("confidence", stated.confidence),
        ] {
            // Refused here rather than passed to the validation gate, which is
            // the opposite of what this crate does with an incoherent bar — and
            // deliberately so. `SensedRecord::validate` checks an
            // alternative-data point's value and nothing else, so an
            // out-of-range confidence would not be caught anywhere downstream;
            // it would simply be clamped by `DataQuality::score` and become a
            // number nobody chose.
            if !(0.0..=1.0).contains(&value) {
                return Err(Error::schema(format!(
                    "{}: the reading {:?} states a {label} of {value}, which is outside [0, 1]. \
                     Nothing downstream re-checks it — `DataQuality::score` would clamp it into \
                     range and the reading would carry a quality nobody stated",
                    self.config.name, wire.observation_id
                )));
            }
        }

        let mut quality = DataQuality {
            completeness: stated.completeness,
            confidence: stated.confidence,
            validation_failures: 0,
            issues: Vec::new(),
            is_imputed: false,
        };
        for issue in &stated.issues {
            quality = quality.with_issue(issue.clone());
        }
        if let WireBasis::Imputed { method } = &stated.basis {
            if method.trim().is_empty() {
                return Err(Error::schema(format!(
                    "{}: the reading {:?} is marked imputed but names no method. \"We filled this \
                     gap\" and \"we carried the last value forward for eleven days\" are \
                     different facts, and only the second one can be reasoned about",
                    self.config.name, wire.observation_id
                )));
            }
            quality = quality
                .with_issue(format!("value imputed: {method}"))
                .imputed();
        }
        Ok(quality)
    }
}

impl DataAdapter for AlternativeFeedAdapter {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: self.config.name.clone(),
            provider: self.config.provider.clone(),
            // With no feed-wide class configured, every reading states its own
            // and the source as a whole is described by the strictest class
            // that still admits a production decision. Reporting the `Internal`
            // default here would tell a caller that raw values may be
            // displayed, which is the one thing an unconfigured feed cannot
            // promise.
            licensing: self.config.licensing.unwrap_or(LicensingClass::Restricted),
            topics: vec![Topic::AlternativeDataReceived],
            expected_latency: self.config.publication_delay,
            // Never `None`, even fully configured: see [`Self::REQUIREMENTS`].
            production_requirement: Some(self.requirement()),
        }
    }

    /// Refuse at startup rather than at the first tick, so a deployment missing
    /// a credential fails while somebody is watching the rollout.
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
        // reading the vendor's arbitrary array order instead. Note this orders
        // on the capture instant: the records are sorted by when they were
        // true, while the gate above admitted them by when they were knowable,
        // and for alternative data those two orders genuinely differ.
        records.sort_by_key(|r| r.occurred_at().as_nanos());
        self.stats.emitted += records.len() as u64;
        Ok(records)
    }
}

impl AlternativeDataAdapter for AlternativeFeedAdapter {
    fn datasets(&self) -> Vec<String> {
        self.datasets.iter().cloned().collect()
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

/// Dataset names and subject keys go into the request line, so what may be in
/// one is decided here rather than discovered when a vendor's identifier splits
/// a request.
fn validate_query_token(kind: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::invalid(format!("an empty {kind}")));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
    {
        return Err(Error::invalid(format!(
            "the {kind} {value:?} contains a character this adapter will not put in a request \
             line: only ASCII letters, digits and . - _ : are accepted, since these are joined by \
             commas into a query this adapter builds by hand"
        )));
    }
    Ok(())
}

/// Licensing codes. An unknown class is refused rather than mapped to the
/// nearest one: the classes differ in whether raw values may be displayed, and
/// there is no safe direction to guess in.
fn licensing_from_code(code: &str) -> Result<LicensingClass> {
    match code {
        "public" => Ok(LicensingClass::Public),
        "internal" => Ok(LicensingClass::Internal),
        "licensed" => Ok(LicensingClass::Licensed),
        "restricted" => Ok(LicensingClass::Restricted),
        // Accepted so a deployment pointed at a mock vendor can label what it
        // receives honestly. `Synthetic` is the one class barred from a
        // production decision, so admitting it costs nothing and forbidding it
        // would push a staging feed to claim `Internal`.
        "synthetic" => Ok(LicensingClass::Synthetic),
        other => Err(Error::schema(format!(
            "unknown licensing class {other:?}: this decoder accepts public, internal, licensed, \
             restricted and synthetic. The classes differ in whether a raw value may be shown, so \
             an unrecognised one is refused rather than mapped to the nearest"
        ))),
    }
}

// --- the wire schema --------------------------------------------------------
//
// The shape this decoder accepts, and the whole of what it promises to read. No
// vendor is obliged to speak it: a deployment whose vendor does not either
// points this adapter at a translating endpoint or writes a second decoder
// beside this one.
//
// Unknown fields are ignored rather than refused, because a vendor adding a
// field is not a fault and must not stop the feed. Unknown *values* in a field
// this decoder reads are refused, because those change what the record means.
//
// Three instants are required on every reading and none of them has a
// `serde(default)` standing in for it, because a defaulted instant is a claim
// about when a number could have been acted on. `quality` is modelled as an
// `Option` so that a missing one produces this module's own refusal, naming
// what the default would have asserted, rather than serde's "missing field".

#[derive(Debug, Default, Deserialize)]
struct WireFeed {
    #[serde(default)]
    readings: Vec<WireReading>,
}

#[derive(Debug, Deserialize)]
struct WireReading {
    /// The vendor's identity for this observation. Becomes
    /// [`Provenance::upstream_id`], which a reconciliation joins on.
    observation_id: String,
    /// e.g. `satellite.parking_lot_counts`. Must be one this deployment covers.
    dataset: String,
    /// The vendor's key for the entity or region, resolved through the subject
    /// map. A key this deployment has not mapped is refused.
    subject: String,
    /// e.g. `vehicles`, `footfall`, `container_throughput`.
    metric: String,
    value: f64,
    unit: String,
    /// When the sensor saw the thing. The reading's valid time, and never the
    /// instant that decides whether it may be handed over.
    captured_at: Timestamp,
    /// When the vendor finished deriving the number. Not carried onto the
    /// record — see the module docs — but required, and required to sit between
    /// the other two.
    processed_at: Timestamp,
    /// When the vendor released the number. The knowable anchor.
    published_at: Timestamp,
    /// Typical lead over the fundamental this proxies for. Absent means zero,
    /// which claims no lead.
    #[serde(default)]
    lead_days: f64,
    /// Historical correlation with the fundamental this proxies for. Absent
    /// means zero, which `AlternativeDataPoint::is_actionable` reads as not
    /// actionable — the restrictive direction.
    #[serde(default)]
    proxy_correlation: f64,
    #[serde(default)]
    proxies_for: Option<String>,
    #[serde(default)]
    licensing: Option<String>,
    /// Required. See [`AlternativeFeedAdapter::quality_for`] for what an absent
    /// one would have asserted.
    #[serde(default)]
    quality: Option<WireQuality>,
}

#[derive(Debug, Deserialize)]
struct WireQuality {
    /// Fraction of the expected inputs that were present, in `[0, 1]`.
    completeness: f64,
    /// The vendor's own confidence in the value, in `[0, 1]`. The starting
    /// point, not the last word: declared failures and imputation reduce it.
    confidence: f64,
    /// Checks the vendor ran and failed. Each becomes a
    /// [`DataQuality::with_issue`], which records it and lowers confidence.
    #[serde(default)]
    issues: Vec<String>,
    /// Flattened, so `basis` and its `method` read as fields of the quality
    /// block rather than as a nested object. The tag has to sit beside
    /// `completeness` and `confidence` because it is the same kind of statement
    /// about the same value.
    #[serde(flatten)]
    basis: WireBasis,
}

/// Whether the value was measured or filled in.
///
/// Internally tagged so the two cases read as one field in the payload, and so
/// a vendor cannot express "imputed by no method" by omitting the method. There
/// is deliberately no third variant for "the vendor did not say": that case is
/// the absent `quality` block, and it is refused before it can become a value.
#[derive(Debug, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
enum WireBasis {
    /// The sensor measured it.
    Observed,
    /// The vendor filled a gap, and says how.
    Imputed { method: String },
}
