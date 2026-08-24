//! A document adapter that opens a socket.
//!
//! [`crate::rest`] made prices real. This makes *text* real: news items,
//! corporate filings and macroeconomic releases fetched over HTTP from a vendor
//! and decoded into [`SensedRecord::News`], [`SensedRecord::Fundamental`] and
//! [`SensedRecord::Macro`]. Until this module existed the catalyst layer in
//! `qip_opportunity_engine::catalyst` and the reasoning engine had only ever
//! seen documents this platform wrote itself, in
//! [`crate::synthetic::narrative`], which means they had never met a headline
//! nobody here chose the wording of.
//!
//! Transport, refusal discipline and the untrusted-peer stance are the ones
//! [`crate::rest`] established, and this module deliberately does not restate
//! them. What follows is only what is *different* about documents, which is
//! most of what is hard.
//!
//! # Two instants, and which one this adapter takes
//!
//! A price has one instant. A document has two, and they can be months apart:
//!
//! | record | the platform's timestamp field | what this adapter takes as the knowable anchor |
//! | --- | --- | --- |
//! | [`NewsItem`] | `published_at` — when the story was published | the publication instant |
//! | [`FundamentalUpdate`] | `period_end` — the end of the quarter reported | the **filing** instant |
//! | [`MacroObservation`] | `reference_date` — the month the statistic describes | the **release** instant |
//!
//! In every row the knowable anchor is *when the document entered the world*,
//! never what the document is about. A 10-Q filed in May reports a quarter that
//! ended in March; keying it on the period end would make it appear knowable
//! two months before anyone could read it, and that is leakage no downstream
//! check catches — [`crate::adapter::SensedRecord::occurred_at`] returns
//! `period_end` for a fundamental precisely because that is its valid time, and
//! nothing downstream is in a position to know when it was filed. So the anchor
//! is recorded, not just used: [`Provenance::event_time`] carries the filing or
//! release instant on every record this module produces, which is what lets an
//! audit afterwards subtract it from `period_end` and see the gap.
//!
//! What this does **not** promise: a news item's `NewsItem::published_at` is
//! the publication instant and nothing else. A vendor field describing when the
//! reported *event* happened is ignored rather than stored, because every field
//! on [`NewsItem`] already means something else and a consumer reading an event
//! time as a publication time would have no way to tell. Ignoring it loses
//! information; writing it into the publication field would lose the ability to
//! tell the two apart, which is worse.
//!
//! # Restatement
//!
//! Vendors revise documents and macro series in place. A revised figure
//! carrying its original timestamp is tomorrow's number wearing yesterday's
//! date, and `qip_lifecycle::evidence::LeakageAudit::restated_without_snapshots`
//! exists because no per-feature timing check can see it. So every document
//! must state whether it is an original or a revision, and a revision must name
//! what it revises. A document that says nothing is **refused** — not assumed
//! original, because "the vendor did not say" and "the vendor said no" produce
//! the same `false` in [`FundamentalUpdate::is_restatement`] and must not
//! produce the same record.
//!
//! What a revision becomes: the boolean the record type carries
//! ([`FundamentalUpdate::is_restatement`], [`MacroObservation::is_revision`]),
//! plus the revised document's id in [`Provenance::derived_from`], which is
//! where the lineage of a derived record already lives. A revised news item
//! also gains a `revision` topic tag, since [`NewsItem`] has no boolean.
//!
//! What this does **not** promise: it cannot detect a *silent* revision. A
//! vendor that edits a document in place, keeps its id and keeps its original
//! timestamp is indistinguishable from one that never edited it, from here.
//! Only an endpoint that keeps originals addressable can answer that, which is
//! why it is one of [`NarrativeAdapter::REQUIREMENTS`] rather than a claim made
//! in this file.
//!
//! # Licensing, which is not optional for text
//!
//! Prices are facts; a headline is somebody's copyrighted expression. Whether
//! the raw text may be shown, or only features derived from it, is a licence
//! term, so every record leaves here carrying a [`LicensingClass`] in its
//! provenance: the class the document states, or failing that the class the
//! feed is configured with. Where neither exists the record is **refused**.
//! That refusal is the whole point of the field: [`LicensingClass`]'s `Default`
//! is `Internal`, and `Internal` permits raw display, so an unset class does
//! not read as "unknown" downstream — it reads as permission. This follows
//! `qip_data_finder::legal`, where unknown is a third value and is never a
//! grant.
//!
//! What this does **not** promise: it labels, it does not redact. A
//! `Restricted` news item still arrives with its body, because the reasoning
//! engine's use of the text is a derived use and stripping it would blind the
//! platform to comply with a rule about *display*. The class travels with the
//! record so that the boundary which does display it can decide.
//!
//! # Bounds
//!
//! A document is orders of magnitude larger than a quote, so the response cap
//! is not the only cap that matters: [`NarrativeFeedConfig::max_document_bytes`]
//! bounds a single news document's text, and a document over it is refused
//! rather than truncated — a truncated document is not a smaller document, it
//! is a document that says something else, and the reasoning engine cannot tell
//! the difference. [`NarrativeFeedConfig::max_records`] counts every record the
//! response would expand into, including one per figure in a filing, so a small
//! response carrying a filing with a hundred thousand line items is refused
//! before any of it is allocated.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_events::Topic;
use qip_financial::intelligence::{
    EntityMention, FiscalPeriod, FundamentalUpdate, MacroObservation, NewsItem, NewsSource,
    Sentiment,
};
use qip_financial::quality::{DataQuality, LicensingClass, Provenance};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration as StdDuration;

use crate::adapter::{
    DataAdapter, FundamentalsAdapter, MacroAdapter, NewsAdapter, SensedRecord, SourceDescriptor,
};

/// Default endpoint path, overridden per vendor.
const DEFAULT_PATH: &str = "/v1/narrative";
/// Default header the credential travels in. Never a query parameter, for the
/// reason [`crate::rest`] gives: a URL reaches every access log on the path.
const DEFAULT_KEY_HEADER: &str = "x-api-key";
/// Delay between a document being published and this deployment being entitled
/// to see it. Conservative by default: a deployment that has not stated its
/// vendor's terms withholds documents it could have published, which shows up
/// in [`DocumentStats::withheld`], rather than publishing them before it was
/// allowed to, which shows up in nothing until a backtest is wrong.
const DEFAULT_PUBLICATION_DELAY: Duration = Duration::from_mins(15);
/// Headers `qip_transport::HttpRequest` writes itself and drops a caller's copy
/// of. Stated here as well as in [`crate::rest`] rather than shared from it:
/// that module is a landed, tested unit this one does not restructure. The two
/// lists describe the same transport and have to change together.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];

/// One issuer this feed is allowed to report on.
///
/// The mapping is configuration for the reason [`crate::rest::RestInstrument`]
/// gives about symbols, and more sharply: `FundamentalUpdate::entity_id` is the
/// key every downstream join uses, and one vendor's issuer code is not another
/// vendor's. An id invented from a filer code would merge two companies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NarrativeSubject {
    /// The platform's canonical entity id, e.g. `ent-northwind`.
    pub entity_id: String,
    /// The issuer key as the vendor writes it: a filer code, a permanent id, a
    /// ticker. Whatever it is, it is what arrives in the response.
    pub vendor_key: String,
}

impl NarrativeSubject {
    pub fn new(entity_id: impl Into<String>, vendor_key: impl Into<String>) -> Self {
        Self {
            entity_id: entity_id.into(),
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
pub struct NarrativeFeedConfig {
    /// Stable adapter name. Recorded as the provenance source, so changing it
    /// renames the origin of every record already published under it.
    pub name: String,
    /// Human-readable vendor description, for the source listing.
    pub provider: String,
    /// `http://host[:port]` of the vendor. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    pub base_url: Option<String>,
    /// Path of the document endpoint under `base_url`. May not carry a query
    /// string: this adapter owns the query.
    pub path: String,
    /// The credential. `None` is unconfigured, not "this vendor is open".
    pub api_key: Option<String>,
    /// Header the credential travels in, since vendors disagree.
    pub api_key_header: String,
    /// Licensing class for documents that do not state their own.
    ///
    /// `None` is a legitimate configuration, not a missing one: a vendor whose
    /// feed carries per-document redistribution terms states them per document,
    /// and inventing a feed-wide default would override them. What `None` means
    /// is that a document arriving without terms is refused rather than given
    /// the `Internal` default, which permits raw display.
    pub licensing: Option<LicensingClass>,
    /// How long after publication the vendor releases a document to this
    /// deployment. Decides when a document became knowable.
    pub publication_delay: Duration,
    /// How far back each poll asks, over *publication* time. Larger than the
    /// polling interval on purpose: the overlap covers a poll whose response
    /// was lost.
    pub window: Duration,
    /// Most records one response may expand into, counting one per figure in a
    /// filing rather than one per filing.
    pub max_records: usize,
    /// Largest single news document, in bytes of headline plus body. A
    /// document over this is refused, never truncated.
    pub max_document_bytes: usize,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for NarrativeFeedConfig {
    fn default() -> Self {
        Self {
            name: "narrative-documents".into(),
            provider: "unconfigured document vendor".into(),
            base_url: None,
            path: DEFAULT_PATH.into(),
            api_key: None,
            api_key_header: DEFAULT_KEY_HEADER.into(),
            // Deliberately not `Some(LicensingClass::Internal)`: see the field.
            licensing: None,
            publication_delay: DEFAULT_PUBLICATION_DELAY,
            // Documents arrive in bursts around market open, earnings season
            // and statistical release calendars, so the window is wider than a
            // price feed's: a poll lost at 08:30 on a release morning must be
            // covered by the next one.
            window: Duration::from_mins(30),
            max_records: 5_000,
            max_document_bytes: 128 * 1024,
            http: ClientLimits {
                // Four megabytes where the price feed allows one: a page of
                // filings carries full text, and a limit that refuses the
                // normal case is an outage this adapter caused.
                max_body: 4 * 1024 * 1024,
                max_headers: 32,
                connect_timeout: StdDuration::from_secs(2),
                // Longer than the price feed's: a vendor assembling documents
                // is slower than one serving a quote cache, and a timeout that
                // fires on the normal case teaches operators to raise it
                // everywhere.
                read_timeout: StdDuration::from_secs(15),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

impl std::fmt::Debug for NarrativeFeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NarrativeFeedConfig")
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
            .field("max_document_bytes", &self.max_document_bytes)
            .field("http", &self.http)
            .finish()
    }
}

/// What this adapter has done, for metrics and for tests that assert a fetch
/// happened rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    /// Responses successfully read and decoded.
    pub fetches: u64,
    /// Records handed to the caller.
    pub emitted: u64,
    /// Records the vendor sent that were not yet knowable at the poll's
    /// `until`. Not an error and not a loss: the next poll's window covers
    /// them again.
    pub withheld: u64,
    /// Records that revise a previously published document.
    ///
    /// Counted separately because a vendor whose revision rate suddenly changes
    /// has changed something about its history, and that is the event a
    /// point-in-time deployment most needs to notice.
    pub revisions: u64,
    /// Entity mentions naming an issuer this deployment has not mapped.
    ///
    /// Not an error — an unresolved mention is a modelled state and identity
    /// resolution runs downstream — but it is counted, because the failure mode
    /// of a mistyped issuer key is silence: unresolved mentions produce no
    /// catalysts, and nothing else says so.
    pub unresolved_mentions: u64,
}

/// Whether a document is an original or revises an earlier one.
///
/// Three states collapse to two here only because the third — "the vendor did
/// not say" — is refused before it can become a value. There is deliberately no
/// `Unknown` variant to be pattern-matched into a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revision {
    /// The first publication of this document.
    Original,
    /// A revision, naming the document id it supersedes.
    Revises(String),
}

impl Revision {
    pub fn is_revision(&self) -> bool {
        matches!(self, Self::Revises(_))
    }

    /// The superseded document, for [`Provenance::derived_from`].
    pub fn supersedes(&self) -> Option<&str> {
        match self {
            Self::Original => None,
            Self::Revises(id) => Some(id.as_str()),
        }
    }
}

/// Polls a vendor's document endpoint and produces narrative records.
#[derive(Debug)]
pub struct NarrativeAdapter {
    config: NarrativeFeedConfig,
    /// Prebuilt endpoint, parsed once at construction so a malformed vendor
    /// address fails where it was configured rather than on the first fetch.
    endpoint: Option<Url>,
    /// Keyed by vendor issuer key, which is what arrives in the response.
    subjects: BTreeMap<String, NarrativeSubject>,
    /// Macro series this deployment ingests. A release outside the set is
    /// refused, for the reason the issuer map exists: coverage is configured,
    /// not discovered from whatever the vendor happens to send.
    series: BTreeSet<String>,
    client: HttpClient,
    stats: DocumentStats,
}

impl NarrativeAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why the descriptor's
    /// requirement is never `None`.
    pub const REQUIREMENTS: [&'static str; 5] = [
        "a TLS-terminating egress proxy in front of this adapter, or a vendor reachable over the \
         cluster network: `qip_transport::http` has no TLS stack and refuses `https` by name \
         rather than downgrading it, so a credential sent straight to a public vendor would \
         cross the internet in clear text",
        "a redistribution licence covering text, which is a different agreement from a market-data \
         licence and is what the per-document licensing class encodes: the class decides whether \
         a headline may be shown to a user or only features derived from it, and this adapter \
         refuses a document that carries no class rather than defaulting to a permissive one",
        "the vendor's dissemination delay in `publication_delay`, because that is what decides \
         when a document became knowable; a delay set to zero on an embargoed or delayed feed \
         publishes documents before the deployment was entitled to read them",
        "a vendor endpoint that serves documents as published rather than as currently amended, \
         and states revision status per document. This adapter records the revision a vendor \
         declares and cannot see one it does not: a document edited in place, keeping its id and \
         its original timestamp, is indistinguishable from here from one never edited, and that \
         is exactly the restatement a point-in-time backtest reads as history",
        "an alert on the withheld, revision and unresolved-mention counts: a vendor that has \
         started sending documents this decoder cannot place, or issuer keys this deployment has \
         not mapped, looks from the outside like a quiet feed rather than a broken one",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot fetch still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong.
    pub fn new(
        config: NarrativeFeedConfig,
        subjects: Vec<NarrativeSubject>,
        series: Vec<String>,
    ) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::invalid(
                "a document feed needs a name: it is recorded as the provenance source of every \
                 record it produces",
            ));
        }
        if config.max_records == 0 {
            return Err(Error::invalid(
                "max_records is zero, which would refuse every response the vendor sends",
            ));
        }
        if config.max_document_bytes == 0 {
            return Err(Error::invalid(
                "max_document_bytes is zero, which would refuse every document that has any text \
                 in it at all",
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
                 query itself, and a second one would put the issuers where the vendor does not \
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

        let mut mapped: BTreeMap<String, NarrativeSubject> = BTreeMap::new();
        for subject in subjects {
            validate_query_token("issuer key", &subject.vendor_key)?;
            if subject.entity_id.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "the issuer key {} maps to an empty entity id; that id is the join key every \
                     downstream consumer uses",
                    subject.vendor_key
                )));
            }
            if let Some(existing) = mapped.insert(subject.vendor_key.clone(), subject) {
                return Err(Error::invalid(format!(
                    "two subjects claim the vendor issuer key {}: a filing keyed by it could not \
                     be resolved to one of them",
                    existing.vendor_key
                )));
            }
        }

        let mut covered: BTreeSet<String> = BTreeSet::new();
        for id in series {
            validate_query_token("macro series id", &id)?;
            covered.insert(id);
        }

        let client = HttpClient::new(config.http);
        Ok(Self {
            config,
            endpoint,
            subjects: mapped,
            series: covered,
            client,
            stats: DocumentStats::default(),
        })
    }

    pub fn stats(&self) -> DocumentStats {
        self.stats
    }

    pub fn config(&self) -> &NarrativeFeedConfig {
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
                "no endpoint: set `base_url` to the vendor's document base address, which this \
                 adapter requests `{}` under",
                self.config.path
            ));
        }
        if self.config.api_key.is_none() {
            missing.push(format!(
                "no credential: set `api_key`, which is sent in the `{}` header. A text feed that \
                 needs no credential is one whose redistribution terms nobody agreed to, and this \
                 adapter will not treat an anonymous endpoint as a licensed one",
                self.config.api_key_header
            ));
        }
        if self.subjects.is_empty() && self.series.is_empty() {
            missing.push(
                "no coverage: supply the issuers this feed reports on, as vendor key to platform \
                 entity id, or the macro series it carries, or both. Coverage is configuration \
                 because an entity id invented from a filer code merges two companies, and a \
                 series this deployment never asked for is one nobody chose to model"
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
                "no feed-wide licensing class is configured, so every document must state its \
                 own; one that does not is refused rather than published under the `Internal` \
                 default, which would permit its raw text to be displayed"
                    .into(),
            );
        }
        parts.extend(Self::REQUIREMENTS.iter().map(|r| (*r).to_string()));
        parts.join("; ")
    }

    /// The refusal every entry point returns when the adapter cannot fetch.
    fn unavailable(&self) -> Error {
        Error::unavailable(format!(
            "{} cannot fetch and will not substitute generated documents: {}",
            self.config.name,
            self.requirement()
        ))
    }

    /// Vendor issuer keys, in the order they go into the request.
    pub fn issuers(&self) -> Vec<String> {
        self.subjects.keys().cloned().collect()
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
        if self.subjects.is_empty() && self.series.is_empty() {
            return Err(self.unavailable());
        }

        let since = until.saturating_sub(self.config.window);
        // `published_since`/`published_until` rather than `since`/`until`: the
        // window is over publication time, and the parameter name is the only
        // place this adapter can say so to a vendor. A vendor that filtered by
        // the period a filing covers would return the quarter's filings and
        // miss the one filed late, which is the one that moves a price.
        let mut query = vec![
            format!("published_since={}", since.to_rfc3339()),
            format!("published_until={}", until.to_rfc3339()),
        ];
        if !self.subjects.is_empty() {
            query.push(format!("issuers={}", self.issuers().join(",")));
        }
        if !self.series.is_empty() {
            query.push(format!(
                "series={}",
                self.series.iter().cloned().collect::<Vec<_>>().join(",")
            ));
        }
        let target = format!("{endpoint}?{}", query.join("&"));
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

        // One record per figure, not one per filing: the cap has to bound what
        // this response expands into, and a single filing can carry a line item
        // per subsidiary.
        let figures: usize = feed.filings.iter().map(|f| f.figures.len()).sum();
        let count = feed.news.len() + figures + feed.macro_releases.len();
        if count > self.config.max_records {
            return Err(Error::guard(format!(
                "{} sent {count} records and the cap is {}: a response small enough to read is \
                 not automatically a response worth expanding",
                self.config.name, self.config.max_records
            )));
        }

        let mut dated = Vec::with_capacity(count);
        for item in feed.news {
            dated.push(self.decode_news(item, until)?);
        }
        for filing in feed.filings {
            self.decode_filing(filing, until, &mut dated)?;
        }
        for release in feed.macro_releases {
            dated.push(self.decode_macro(release, until)?);
        }
        self.stats.fetches += 1;
        Ok(dated)
    }

    /// What a non-2xx status means here.
    ///
    /// Separated by class because the operator action differs: a rejected
    /// credential is a deployment to fix, a 404 is a path to fix, a 429 or a
    /// 5xx is a vendor to wait for. A 451 gets its own arm, because for a text
    /// feed it is the one status that means the licence, not the request.
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

    /// The licensing class a document travels under.
    ///
    /// Stated by the document where the vendor states it, otherwise the feed's
    /// configured class, otherwise refused. The third branch is the reason this
    /// function exists rather than an `unwrap_or_default`.
    fn licensing_for(
        &self,
        stated: Option<&str>,
        kind: &str,
        document: &str,
    ) -> Result<LicensingClass> {
        match stated {
            Some(code) => licensing_from_code(code),
            None => self.config.licensing.ok_or_else(|| {
                Error::denied(format!(
                    "{}: the {kind} {document:?} states no licensing class and this feed \
                     configures no default, so there is nothing that says whether its text may be \
                     displayed. It is refused rather than admitted under the `Internal` default, \
                     which permits raw display: an unstated licence is not a permissive one",
                    self.config.name
                ))
            }),
        }
    }

    /// The revision status a document travels under.
    ///
    /// Refuses a document that states nothing, because the boolean this becomes
    /// cannot represent the difference between "not a revision" and "the vendor
    /// did not say".
    fn revision_for(
        &self,
        stated: Option<WireRevision>,
        kind: &str,
        document: &str,
    ) -> Result<Revision> {
        match stated {
            None => Err(Error::schema(format!(
                "{}: the {kind} {document:?} states no revision status. It is refused rather than \
                 assumed original: a revision recorded as an original is a figure that did not \
                 exist at decision time wearing a timestamp that says it did, and no per-feature \
                 timing check downstream can see it",
                self.config.name
            ))),
            Some(WireRevision::Original) => Ok(Revision::Original),
            Some(WireRevision::Revision { revises }) => {
                if revises.trim().is_empty() {
                    return Err(Error::schema(format!(
                        "{}: the {kind} {document:?} is marked a revision but names nothing it \
                         revises; a revision with no antecedent cannot be reconciled against what \
                         it replaced",
                        self.config.name
                    )));
                }
                if revises == document {
                    // Both news and fundamental idempotency keys are built from
                    // the document's own identity, so a revision reusing it
                    // reaches the bus looking like a redelivery of the original
                    // and is dropped. Refusing here is the difference between a
                    // loud failure and a silently missing correction.
                    return Err(Error::schema(format!(
                        "{}: the {kind} {document:?} says it revises itself. A revision that \
                         reuses the original's id is deduplicated away by the bus as a \
                         redelivery, so the correction would never arrive",
                        self.config.name
                    )));
                }
                Ok(Revision::Revises(revises))
            }
        }
    }

    /// Provenance shared by every record this module produces.
    ///
    /// `entered_world` is the instant the *document* appeared — publication,
    /// filing, release — never the instant it describes. `until` is the
    /// caller's clock rather than the wall clock, so the same fetch replayed in
    /// a backtest produces the same record it produced live.
    fn provenance_for(
        &self,
        entered_world: Timestamp,
        until: Timestamp,
        licensing: LicensingClass,
        document_id: &str,
        revision: &Revision,
    ) -> Provenance {
        let mut provenance = Provenance::new(self.config.name.clone(), entered_world, until)
            .with_licensing(licensing)
            .with_upstream_id(document_id.to_string());
        if let Some(superseded) = revision.supersedes() {
            provenance = provenance.derived(vec![superseded.to_string()]);
        }
        provenance
    }

    /// A news item is knowable when it was published, plus the vendor's delay.
    ///
    /// `published_at` is taken from the vendor's publication field and from
    /// nowhere else. See the module docs for what happens to an event time the
    /// story describes: nothing.
    fn decode_news(
        &mut self,
        wire: WireNews,
        until: Timestamp,
    ) -> Result<(SensedRecord, Timestamp)> {
        if wire.document_id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} sent a news item with no document id; the id is what the bus deduplicates on \
                 and what a correction names",
                self.config.name
            )));
        }
        let size = wire.headline.len() + wire.body.len();
        if size > self.config.max_document_bytes {
            return Err(Error::guard(format!(
                "{}: the news item {:?} carries {size} bytes of text and the per-document cap is \
                 {}. It is refused rather than truncated: a truncated document is not a shorter \
                 document, it is one that says something else, and nothing downstream can tell",
                self.config.name, wire.document_id, self.config.max_document_bytes
            )));
        }
        let licensing =
            self.licensing_for(wire.licensing.as_deref(), "news item", &wire.document_id)?;
        let revision = self.revision_for(wire.revision, "news item", &wire.document_id)?;
        let source = news_source_from_code(&wire.source)?;

        let mut entities = Vec::with_capacity(wire.entities.len());
        for mention in wire.entities {
            let entity_id = match &mention.issuer {
                Some(key) => {
                    let resolved = self.subjects.get(key).map(|s| s.entity_id.clone());
                    if resolved.is_none() {
                        // Deliberately not an error: an unresolved mention is a
                        // state the model has a field for, and refusing every
                        // story that names an unmapped company would empty the
                        // feed. Counted, because the symptom is silence.
                        self.stats.unresolved_mentions += 1;
                    }
                    resolved
                }
                None => None,
            };
            entities.push(EntityMention {
                text: mention.text,
                entity_id,
                confidence: mention.confidence,
                is_primary: mention.is_primary,
                sentiment: None,
            });
        }

        let mut topics = wire.topics;
        if revision.is_revision() {
            // `NewsItem` has no restatement boolean, and the topic list is the
            // only field on it that carries this kind of fact. Appended rather
            // than prepended: `CatalystEvent::from_news` classifies on the
            // first topic, and a correction to an earnings story is still an
            // earnings story.
            topics.push("revision".into());
        }

        let provenance = self.provenance_for(
            wire.published_at,
            until,
            licensing,
            &wire.document_id,
            &revision,
        );
        if revision.is_revision() {
            self.stats.revisions += 1;
        }

        let item = NewsItem {
            item_id: wire.document_id,
            headline: wire.headline,
            body: wire.body,
            source,
            // The publication instant, and the only instant this record keys on.
            published_at: wire.published_at,
            entities,
            sentiment: wire.sentiment.map_or_else(unscored_sentiment, Into::into),
            topics,
            provenance,
            quality: DataQuality::clean(),
        };
        let knowable = item
            .published_at
            .saturating_add(self.config.publication_delay);
        Ok((SensedRecord::News(Box::new(item)), knowable))
    }

    /// A filing is knowable when it was filed, never when its period ended.
    ///
    /// The gap between the two is the point: a quarter that ended in March is
    /// filed in May, and a record keyed on the period end claims two months of
    /// foresight. `period_end` still goes on the record — it is the figure's
    /// valid time and every downstream comparison needs it — but the instant
    /// that decides whether this adapter may hand the record over, and the one
    /// written to [`Provenance::event_time`], is `filed_at`.
    ///
    /// One filing becomes one record per reported figure, because
    /// [`FundamentalUpdate`] is a metric, not a document. They share the
    /// filing's provenance, so all of them carry the same filing instant.
    fn decode_filing(
        &mut self,
        wire: WireFiling,
        until: Timestamp,
        out: &mut Vec<(SensedRecord, Timestamp)>,
    ) -> Result<()> {
        if wire.document_id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} sent a filing with no document id",
                self.config.name
            )));
        }
        let subject = self.subjects.get(&wire.issuer).ok_or_else(|| {
            Error::not_found(format!(
                "{} returned the filing {:?} for the issuer key {:?}, which is not in this \
                 deployment's subject map ({}). A figure whose entity cannot be identified is \
                 refused rather than published under an invented id, which would merge it with \
                 another company's history",
                self.config.name,
                wire.document_id,
                wire.issuer,
                self.issuers().join(", ")
            ))
        })?;
        let entity_id = subject.entity_id.clone();
        let licensing =
            self.licensing_for(wire.licensing.as_deref(), "filing", &wire.document_id)?;
        let revision = self.revision_for(wire.revision, "filing", &wire.document_id)?;
        let period = fiscal_period_from_code(&wire.period)?;
        if wire.figures.is_empty() {
            return Err(Error::schema(format!(
                "{}: the filing {:?} reports no figures. An empty filing is not an event with no \
                 content, it is a decode that lost the content",
                self.config.name, wire.document_id
            )));
        }

        let provenance = self.provenance_for(
            wire.filed_at,
            until,
            licensing,
            &wire.document_id,
            &revision,
        );
        let knowable = wire.filed_at.saturating_add(self.config.publication_delay);
        let is_restatement = revision.is_revision();

        for figure in wire.figures {
            if figure.metric.trim().is_empty() {
                return Err(Error::schema(format!(
                    "{}: the filing {:?} reports a figure with no metric name",
                    self.config.name, wire.document_id
                )));
            }
            if is_restatement {
                self.stats.revisions += 1;
            }
            let update = FundamentalUpdate {
                entity_id: entity_id.clone(),
                metric: figure.metric,
                value: figure.value,
                unit: figure.unit,
                // The period the figure covers. Not the filing instant, and not
                // what the knowable gate above used.
                period_end: wire.period_end,
                period,
                consensus: figure.consensus,
                prior_value: figure.prior_value,
                is_restatement,
                provenance: provenance.clone(),
                quality: DataQuality::clean(),
            };
            out.push((SensedRecord::Fundamental(Box::new(update)), knowable));
        }
        Ok(())
    }

    /// A macro release is knowable when it was released, never on the date it
    /// describes.
    ///
    /// A CPI print for March is published in April. `reference_date` is the
    /// month; `released_at` is when anyone could read the number, and it is
    /// what gates the record and what goes in [`Provenance::event_time`].
    fn decode_macro(
        &mut self,
        wire: WireMacro,
        until: Timestamp,
    ) -> Result<(SensedRecord, Timestamp)> {
        if wire.document_id.trim().is_empty() {
            return Err(Error::schema(format!(
                "{} sent a macro release with no document id",
                self.config.name
            )));
        }
        if !self.series.contains(&wire.series_id) {
            return Err(Error::not_found(format!(
                "{} returned the series {:?}, which this deployment does not cover ({}). A series \
                 nobody configured is one nobody modelled, and admitting it would put an \
                 unreviewed input in front of the macro layer",
                self.config.name,
                wire.series_id,
                self.series.iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let licensing = self.licensing_for(
            wire.licensing.as_deref(),
            "macro release",
            &wire.document_id,
        )?;
        let revision = self.revision_for(wire.revision, "macro release", &wire.document_id)?;
        let provenance = self.provenance_for(
            wire.released_at,
            until,
            licensing,
            &wire.document_id,
            &revision,
        );
        let is_revision = revision.is_revision();
        if is_revision {
            self.stats.revisions += 1;
        }
        let observation = MacroObservation {
            series_id: wire.series_id,
            region: wire.region,
            value: wire.value,
            unit: wire.unit,
            // The period the statistic describes, which is not when it was
            // published and must never be used as if it were.
            reference_date: wire.reference_date,
            consensus: wire.consensus,
            previous: wire.previous,
            is_revision,
            provenance,
            quality: DataQuality::clean(),
        };
        let knowable = wire
            .released_at
            .saturating_add(self.config.publication_delay);
        Ok((SensedRecord::Macro(Box::new(observation)), knowable))
    }
}

impl DataAdapter for NarrativeAdapter {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: self.config.name.clone(),
            provider: self.config.provider.clone(),
            // With no feed-wide class configured, every document states its
            // own and the source as a whole is described by the strictest class
            // that still admits a production decision. Reporting the `Internal`
            // default here would tell a caller that raw text may be displayed,
            // which is the one thing an unconfigured feed cannot promise.
            licensing: self.config.licensing.unwrap_or(LicensingClass::Restricted),
            topics: vec![
                Topic::NewsReceived,
                Topic::FundamentalUpdated,
                Topic::MacroUpdated,
            ],
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
        // reading the vendor's arbitrary array order instead. Note that for a
        // filing this orders on `period_end`: the records are sorted by when
        // they were true, while the gate above admitted them by when they were
        // knowable, and the two orders are genuinely different for documents.
        records.sort_by_key(|r| r.occurred_at().as_nanos());
        self.stats.emitted += records.len() as u64;
        Ok(records)
    }
}

impl NewsAdapter for NarrativeAdapter {}

impl FundamentalsAdapter for NarrativeAdapter {}

impl MacroAdapter for NarrativeAdapter {
    fn series(&self) -> Vec<String> {
        self.series.iter().cloned().collect()
    }
}

/// A sentiment reading nobody made.
///
/// Zero confidence rather than [`Sentiment::neutral`], whose 0.5 asserts a
/// middling certainty this adapter does not have. `effective()` is zero either
/// way; the difference is that a downstream model weighting by confidence can
/// tell "scored as neutral" from "not scored".
fn unscored_sentiment() -> Sentiment {
    Sentiment {
        polarity: 0.0,
        confidence: 0.0,
        novelty: 0.0,
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

/// Issuer keys and series ids go into the request line, so what may be in one
/// is decided here rather than discovered when a vendor's identifier splits a
/// request.
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

/// Vendor source codes. Guessing at an unknown one would set the document's
/// prior reliability, which is a multiplier on every piece of evidence drawn
/// from it: a forum post read as a filing is a rumour with a regulator's weight.
fn news_source_from_code(code: &str) -> Result<NewsSource> {
    match code {
        "regulatory_filing" => Ok(NewsSource::RegulatoryFiling),
        "company_announcement" => Ok(NewsSource::CompanyAnnouncement),
        "official_statistics" => Ok(NewsSource::OfficialStatistics),
        "newswire" => Ok(NewsSource::Newswire),
        "research" => Ok(NewsSource::Research),
        "transcript" => Ok(NewsSource::Transcript),
        "social" => Ok(NewsSource::Social),
        other => Err(Error::schema(format!(
            "unknown news source {other:?}: this decoder accepts regulatory_filing, \
             company_announcement, official_statistics, newswire, research, transcript and social"
        ))),
    }
}

/// Fiscal period codes. An unknown one is refused rather than defaulted to a
/// quarter, because the period decides what a figure is comparable with, and a
/// full year read as a quarter is a 300% growth surprise that never happened.
fn fiscal_period_from_code(code: &str) -> Result<FiscalPeriod> {
    match code {
        "quarter" => Ok(FiscalPeriod::Quarter),
        "half_year" => Ok(FiscalPeriod::HalfYear),
        "year" => Ok(FiscalPeriod::Year),
        "trailing_twelve_months" => Ok(FiscalPeriod::TrailingTwelveMonths),
        other => Err(Error::schema(format!(
            "unknown fiscal period {other:?}: this decoder accepts quarter, half_year, year and \
             trailing_twelve_months"
        ))),
    }
}

/// Licensing codes. An unknown class is refused rather than mapped to the
/// nearest one: the classes differ in whether raw text may be displayed, and
/// there is no safe direction to guess in.
fn licensing_from_code(code: &str) -> Result<LicensingClass> {
    match code {
        "public" => Ok(LicensingClass::Public),
        "internal" => Ok(LicensingClass::Internal),
        "licensed" => Ok(LicensingClass::Licensed),
        "restricted" => Ok(LicensingClass::Restricted),
        // Accepted so that a deployment pointed at a mock vendor can label what
        // it receives honestly. `Synthetic` is the one class barred from a
        // production decision, so admitting it costs nothing and forbidding it
        // would push a staging feed to claim `Internal`.
        "synthetic" => Ok(LicensingClass::Synthetic),
        other => Err(Error::schema(format!(
            "unknown licensing class {other:?}: this decoder accepts public, internal, licensed, \
             restricted and synthetic. The classes differ in whether raw text may be shown, so an \
             unrecognised one is refused rather than mapped to the nearest"
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
// Two fields are required on every document and have no `serde(default)`
// standing in for them, and both are absent from the price schema in
// `rest.rs` because prices do not have the problem: `licensing`, which decides
// whether text may be shown, and `revision`, which decides whether a figure is
// history or a rewrite of it. They are modelled as `Option` so that a missing
// one produces this module's own refusal, naming what it means, rather than
// serde's "missing field".

#[derive(Debug, Default, Deserialize)]
struct WireFeed {
    #[serde(default)]
    news: Vec<WireNews>,
    #[serde(default)]
    filings: Vec<WireFiling>,
    #[serde(default, rename = "macro")]
    macro_releases: Vec<WireMacro>,
}

/// Original or revision, stated per document.
///
/// Internally tagged so the two cases read as one field in the payload and so
/// that a vendor cannot express "a revision of nothing" by omitting the id.
#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WireRevision {
    Original,
    Revision { revises: String },
}

#[derive(Debug, Deserialize)]
struct WireNews {
    /// The vendor's identity for the document. Becomes `NewsItem::item_id`,
    /// which the bus deduplicates on and which a later correction names.
    document_id: String,
    headline: String,
    #[serde(default)]
    body: String,
    /// See [`news_source_from_code`].
    source: String,
    /// When the story was published. The only instant this decoder reads from a
    /// news document: a field describing when the reported event happened is
    /// ignored, since `NewsItem` has one timestamp and it means publication.
    published_at: Timestamp,
    #[serde(default)]
    entities: Vec<WireMention>,
    #[serde(default)]
    sentiment: Option<WireSentiment>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    licensing: Option<String>,
    #[serde(default)]
    revision: Option<WireRevision>,
}

#[derive(Debug, Deserialize)]
struct WireMention {
    /// Surface form as it appeared in the text.
    text: String,
    /// The vendor's issuer key, resolved through the subject map. A key this
    /// deployment has not mapped leaves the mention unresolved rather than
    /// inventing an entity id.
    #[serde(default)]
    issuer: Option<String>,
    confidence: f64,
    #[serde(default)]
    is_primary: bool,
}

/// The vendor's own sentiment reading, passed through rather than clamped.
///
/// A polarity outside `[-1, 1]` reaches `crate::service::IngestionService`'s
/// validation gate and becomes a `DataQualityFailure`, which is the house rule
/// `rest.rs` set for an incoherent bar: an adapter that quietly corrected bad
/// vendor data would make it invisible.
#[derive(Debug, Deserialize)]
struct WireSentiment {
    polarity: f64,
    confidence: f64,
    #[serde(default)]
    novelty: f64,
}

impl From<WireSentiment> for Sentiment {
    fn from(wire: WireSentiment) -> Self {
        Self {
            polarity: wire.polarity,
            confidence: wire.confidence,
            novelty: wire.novelty,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireFiling {
    document_id: String,
    /// The vendor's issuer key. Must be in the subject map.
    issuer: String,
    /// When the filing was made public. The knowable anchor.
    filed_at: Timestamp,
    /// End of the period the figures cover, months earlier than `filed_at` for
    /// an annual report. Never used as the knowable anchor.
    period_end: Timestamp,
    /// See [`fiscal_period_from_code`].
    period: String,
    figures: Vec<WireFigure>,
    #[serde(default)]
    licensing: Option<String>,
    #[serde(default)]
    revision: Option<WireRevision>,
}

#[derive(Debug, Deserialize)]
struct WireFigure {
    /// e.g. `revenue`, `eps_diluted`, `free_cash_flow`.
    metric: String,
    value: qip_core::Decimal,
    /// Unit or currency code, e.g. `USD_millions`.
    unit: String,
    #[serde(default)]
    consensus: Option<qip_core::Decimal>,
    #[serde(default)]
    prior_value: Option<qip_core::Decimal>,
}

#[derive(Debug, Deserialize)]
struct WireMacro {
    document_id: String,
    /// e.g. `US.CPI.YOY`. Must be one this deployment covers.
    series_id: String,
    /// ISO country or region code.
    region: String,
    value: f64,
    unit: String,
    /// The month or quarter the statistic describes.
    reference_date: Timestamp,
    /// When the statistical agency published it. The knowable anchor.
    released_at: Timestamp,
    #[serde(default)]
    consensus: Option<f64>,
    #[serde(default)]
    previous: Option<f64>,
    #[serde(default)]
    licensing: Option<String>,
    #[serde(default)]
    revision: Option<WireRevision>,
}
