//! An order-book depth adapter that opens a socket.
//!
//! [`crate::rest`] made prices real and [`crate::narrative`] made text real.
//! This makes *depth* real. Until it existed, `qip_orderbook` — the crate that
//! holds every level, every resting order and every rule about what a broken
//! book means — had only ever been fed books this platform wrote itself, and
//! the live market-data adapter decoded bars, quotes, trades and reference
//! data with no book depth in it at all.
//!
//! Transport, credential handling, the untrusted-peer stance and the refusal to
//! substitute generated data are the ones [`crate::rest`] established, and this
//! module does not restate them. What follows is only what is different about a
//! book, which is nearly everything that is hard.
//!
//! # A book is state, and state can be wrong in ways a record cannot
//!
//! Every other adapter in this crate is a decoder: a response arrives, records
//! come out, and a response that never arrived costs exactly the records it
//! carried. A book is not like that. It is built by applying increments to a
//! snapshot, so a single increment that is lost, applied twice or applied out
//! of order does not damage one record — it damages *every* book published
//! afterwards, silently, until something rebuilds it. There is no field on
//! [`OrderBook`] that says "one of the updates behind this is missing".
//!
//! So this module is a state machine with three properties, and each one exists
//! because of a specific way a book goes quietly wrong:
//!
//! * **Gaps are detected by `qip_sequencing`, not here.** [`SequenceTracker`]
//!   already reorders within a bounded window, drops duplicates and, when a
//!   hole will not fill, emits [`MessageBody::Reset`] *before* releasing
//!   anything that assumes the book survived. A second gap check written in
//!   this file would be a second opinion about whether a book is safe, and two
//!   opinions is one too many. An increment that does not follow the last
//!   applied sequence is never applied on the assumption that it probably
//!   follows.
//! * **A book that has diverged is not published.** [`VenueState`] already
//!   distinguishes "empty" from "known to be wrong" through
//!   [`VenueState::is_stale`], and every read that a strategy would size
//!   against is gated on it. This adapter withholds rather than emits, and
//!   re-snapshots to recover. A stale book is worse than no book precisely
//!   because it produces confident prices that are not the market's, and
//!   nothing downstream can tell them from prices that are.
//! * **A crossed book is never normalised.** Not by dropping the offending
//!   level, not by swapping the sides, not by clamping one to the other. See
//!   below.
//!
//! # What happens to a crossed or locked book
//!
//! `qip_orderbook` has already decided what these mean and this module follows
//! it rather than inventing a second answer.
//!
//! A **locked** book — best bid exactly equal to best ask — is published.
//! [`BookCondition::Locked`] is documented there as legal on several venues and
//! common at the open: unusual, but nothing about it is inconsistent, and
//! [`BookCondition::is_consistent`] is false only for a crossed book.
//!
//! A **crossed** book is handled by asking the venue what session it is in,
//! because [`qip_orderbook::auction`] models an auction as something that
//! happens *beside* the continuous book rather than inside it. Its own words:
//! during an auction the continuous book is not what trades, orders accumulate,
//! and the venue publishes an indicative price and imbalance which are kept
//! separately rather than folded into the levels. That gives two cases and they
//! get opposite treatment:
//!
//! * **The venue has declared an auction.** The continuous touch is a book
//!   nobody can hit, whether it is crossed or not, so it is withheld — counted
//!   in [`DepthStats::withheld_auction`] — and the state is left alone. Nothing
//!   is wrong with it. This is [`VenueState::continuous_trading`]'s distinction,
//!   used rather than re-derived.
//! * **The venue says it is trading continuously.** Then a bid above an ask is
//!   corruption: this adapter holds one instrument's book at one venue, so
//!   there is no second venue for the cross to be an arbitrage against. The
//!   book is reset through [`VenueState::reset`] and rebuilt from a fresh
//!   snapshot. If the fresh snapshot is crossed too, the book is withheld and
//!   counted; it is not reset again, because re-snapshotting a vendor that is
//!   sending a crossed book in a continuous session is a loop, not a recovery.
//!
//! What this module does **not** promise: it cannot tell a venue that is in an
//! auction and has not said so from one that is trading and sending a corrupt
//! book. That is why a snapshot must state the venue's status, and why one that
//! does not is refused rather than assumed open — see [`WireSnapshot::status`].
//!
//! # What this module does not promise
//!
//! It does not deduplicate. A poll in which no increment arrived re-publishes
//! the book it published last time, carrying the same venue sequence; the bus
//! recognises that through [`OrderBook::idempotency_key`]. Withholding it
//! instead would mean a book lost to a dropped poll is never offered again,
//! and losing a book is worse than publishing one twice — the rule
//! [`crate::rest`] set.
//!
//! It does not re-validate the levels the vendor sent. A book whose sides are
//! genuinely inconsistent in some way `qip_orderbook` does not model reaches
//! [`crate::service::IngestionService`]'s validation gate and becomes a
//! [`qip_financial::intelligence::DataQualityFailure`], which is where bad
//! vendor data is supposed to become visible.
//!
//! It does not recover a book across a process restart. A cell that starts with
//! no state takes a snapshot, and everything the venue published while it was
//! down is not a gap it can see. [`SequenceTracker::expecting`] exists for the
//! deployment that keeps a durable resume point; wiring one is
//! [`DepthFeedAdapter::REQUIREMENTS`]' problem, not this module's.
//!
//! It does not merge venues. One instrument at one venue is one book, which is
//! what makes a crossed book here corruption rather than an arbitrage.
//!
//! It does not stamp a licensing class on the book it publishes.
//! [`qip_market::book::OrderBook`] carries no [`qip_financial::quality::Provenance`]
//! and no place to put one, so the class this feed is configured with reaches
//! consumers through [`SourceDescriptor::licensing`] — a statement about the
//! source, not about the record. That is worth stating plainly rather than
//! working around: a deployment whose depth licence differs per instrument
//! cannot express that here, and inventing a field on the record to hold it
//! would put a class downstream consumers have no reason to look for.
//!
//! # Point in time
//!
//! A book's event time is the venue time of the last message applied to it,
//! never the local clock and never the instant the response arrived. Its
//! knowable time is that plus the vendor's dissemination delay, and a book not
//! yet knowable at the poll's `until` is withheld and counted rather than
//! handed over early. The `capture_time` stamped on every decoded message is
//! the caller's `until` rather than the wall clock, so the same responses
//! replayed in a backtest build the same book they built live.

use qip_contracts::{
    BookSide, MarketMessage, MessageBody, Origin, TradeCondition, VenueId, VenueStatus,
};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_events::Topic;
use qip_financial::quality::LicensingClass;
use qip_market::book::{BookLevel, OrderBook};
use qip_orderbook::snapshot::BookKind;
use qip_orderbook::view::{BookCondition, BookView};
use qip_orderbook::{Book, VenueState};
use qip_sequencing::{ReorderPolicy, SequenceEvent, SequenceTracker, delivery_units, synthetic_id};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use crate::adapter::{DataAdapter, MarketDataAdapter, SensedRecord, SourceDescriptor};

/// Default path of the depth-snapshot endpoint, overridden per vendor.
const DEFAULT_SNAPSHOT_PATH: &str = "/v1/depth/snapshot";
/// Default path of the incremental-update endpoint, overridden per vendor.
const DEFAULT_UPDATES_PATH: &str = "/v1/depth/updates";
/// Default header the credential travels in. Never a query parameter, for the
/// reason [`crate::rest`] gives: a URL reaches every access log on the path.
const DEFAULT_KEY_HEADER: &str = "x-api-key";
/// The delay a standard non-realtime depth entitlement carries. Conservative by
/// default, so a deployment that has not stated its vendor's terms withholds
/// books it could have published — visible in [`DepthStats::withheld_late`] —
/// rather than publishing depth before it was entitled to see it.
const DEFAULT_PUBLICATION_DELAY: Duration = Duration::from_mins(15);
/// Levels a side this adapter publishes when the deployment does not choose.
const DEFAULT_DEPTH: usize = 10;
/// Headers `qip_transport::HttpRequest` writes itself and drops a caller's copy
/// of. Stated here as well as in [`crate::rest`] rather than shared from it:
/// that module is a landed, tested unit this one does not restructure.
const CLIENT_OWNED_HEADERS: [&str; 4] =
    ["host", "content-length", "connection", "transfer-encoding"];

/// One instrument whose book this adapter maintains.
///
/// The mapping is configuration for the reason [`crate::rest::RestInstrument`]
/// gives, plus one this adapter adds: `feed` and `partition` name the stream a
/// vendor numbers this instrument's updates in, and sequence numbers are only
/// comparable within a stream. Two instruments sharing a stream key would have
/// their sequences interleaved into manufactured gaps; two streams collapsed
/// into one key would hide real ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepthInstrument {
    pub object_id: ObjectId,
    /// The symbol as the vendor writes it, which is what goes in the request.
    pub symbol: String,
    /// Venue this book belongs to. Stamped on every record and checked against
    /// every message applied, so a mis-wired subscription cannot merge two
    /// venues' books into a plausible book that is nobody's.
    pub venue: VenueId,
    /// The vendor's channel or feed name for this instrument's updates.
    pub feed: String,
    /// The partition the sequence numbers are scoped to. Zero for the many
    /// vendors that have no such concept.
    pub partition: u32,
}

impl DepthInstrument {
    /// An instrument whose updates are numbered in `feed`, partition zero.
    pub fn new(
        object_id: ObjectId,
        symbol: impl Into<String>,
        venue: impl Into<String>,
        feed: impl Into<String>,
    ) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            venue: VenueId::new(venue),
            feed: feed.into(),
            partition: 0,
        }
    }

    /// The same instrument, numbered in a named partition of its feed.
    pub fn with_partition(mut self, partition: u32) -> Self {
        self.partition = partition;
        self
    }

    /// The stream this instrument's sequence numbers belong to.
    pub fn stream_key(&self) -> String {
        self.origin(0).stream_key()
    }

    fn origin(&self, sequence: u64) -> Origin {
        Origin::new(
            self.venue.clone(),
            self.feed.clone(),
            self.partition,
            sequence,
        )
    }
}

/// Everything a deployment has to decide before this adapter can fetch.
///
/// Not `Serialize`, and its [`std::fmt::Debug`] redacts the credential: a
/// config that round-trips through JSON ends up in a log line or a support
/// ticket, and the API key would go with it.
#[derive(Clone)]
pub struct DepthFeedConfig {
    /// Stable adapter name. Recorded as the provenance source, so changing it
    /// renames the origin of every book already published under it.
    pub name: String,
    /// Human-readable vendor description, for the source listing.
    pub provider: String,
    /// `http://host[:port]` of the vendor. `None` means unconfigured, which is
    /// what makes the adapter report itself unavailable rather than guess.
    pub base_url: Option<String>,
    /// Path of the depth-snapshot endpoint under `base_url`. May not carry a
    /// query string: this adapter owns the query.
    pub snapshot_path: String,
    /// Path of the incremental-update endpoint under `base_url`.
    ///
    /// A separate path rather than one endpoint with a mode flag, because the
    /// two answers mean different things: a snapshot is a complete book at a
    /// sequence, an increment is an operation to apply to one. A vendor that
    /// serves both from one path points both fields at it.
    pub updates_path: String,
    /// The credential. `None` is unconfigured, not "this vendor is open".
    pub api_key: Option<String>,
    /// Header the credential travels in, since vendors disagree.
    pub api_key_header: String,
    /// Licensing class of the depth this vendor sends. Depth is commonly
    /// licensed more tightly than the top of book, and the class decides
    /// whether a level may be displayed raw.
    pub licensing: LicensingClass,
    /// How long after a book changes the vendor publishes the change to this
    /// deployment. Decides when a book became knowable.
    pub publication_delay: Duration,
    /// Levels a side to publish. Bounds the record, not the book: the book
    /// holds every level the venue published and this is how much of it a
    /// consumer is handed.
    pub depth: usize,
    /// Which resolution this vendor publishes.
    ///
    /// Not inferred from the first response. `qip_orderbook` refuses an
    /// order-by-order message applied to an aggregated book and the reverse,
    /// which is a refusal worth keeping: a feed that switched resolution
    /// mid-stream would otherwise build a book that double-counts every
    /// update it re-read as a level.
    pub book_kind: BookKind,
    /// Most book messages one response may expand into, counting one per level
    /// in a snapshot rather than one per snapshot.
    pub max_messages: usize,
    /// How long a hole may stay open and how much may be held while waiting,
    /// handed to [`SequenceTracker`] unchanged.
    ///
    /// The deadline is a latency budget rather than a hope: everything behind
    /// an open hole is withheld until it closes, so a generous one is a
    /// deliberate decision to publish no book for that long. Because polls are
    /// discrete, a hole that a poll's own response did not fill is normally
    /// abandoned on the next poll.
    pub reorder: ReorderPolicy,
    /// Transport limits. The peer chooses how much to send; these decide how
    /// much this process will hold and how long it will wait.
    pub http: ClientLimits,
}

impl Default for DepthFeedConfig {
    fn default() -> Self {
        Self {
            name: "depth-book".into(),
            provider: "unconfigured depth vendor".into(),
            base_url: None,
            snapshot_path: DEFAULT_SNAPSHOT_PATH.into(),
            updates_path: DEFAULT_UPDATES_PATH.into(),
            api_key: None,
            api_key_header: DEFAULT_KEY_HEADER.into(),
            licensing: LicensingClass::Licensed,
            publication_delay: DEFAULT_PUBLICATION_DELAY,
            depth: DEFAULT_DEPTH,
            book_kind: BookKind::Aggregated,
            max_messages: 50_000,
            reorder: ReorderPolicy::default(),
            http: ClientLimits {
                // A depth snapshot is larger than a page of bars and smaller
                // than a page of filings.
                max_body: 2 * 1024 * 1024,
                max_headers: 32,
                connect_timeout: StdDuration::from_secs(2),
                read_timeout: StdDuration::from_secs(5),
                write_timeout: StdDuration::from_secs(5),
                ..ClientLimits::default()
            },
        }
    }
}

impl std::fmt::Debug for DepthFeedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepthFeedConfig")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("snapshot_path", &self.snapshot_path)
            .field("updates_path", &self.updates_path)
            // Present or absent is worth knowing; the value never is.
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("api_key_header", &self.api_key_header)
            .field("licensing", &self.licensing)
            .field("publication_delay", &self.publication_delay)
            .field("depth", &self.depth)
            .field("book_kind", &self.book_kind)
            .field("max_messages", &self.max_messages)
            .field("reorder", &self.reorder)
            .field("http", &self.http)
            .finish()
    }
}

/// What this adapter has done, for metrics and for tests that assert a book was
/// withheld for the reason they think it was.
///
/// The withheld counters are split rather than summed because the operator
/// action differs for each: a feed that is permanently gapped is a vendor
/// problem, a feed that is permanently in an auction is a status decoder
/// problem, and a feed whose books are all too recent to be knowable is a
/// `publication_delay` set wrong.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DepthStats {
    /// Depth snapshots fetched, including those taken to recover.
    pub snapshots: u64,
    /// Snapshots taken because a book had to be rebuilt rather than because it
    /// had never been built. The number that says how often this feed is
    /// losing data.
    pub resynchronisations: u64,
    /// Update responses successfully read and decoded.
    pub fetches: u64,
    /// Books handed to the caller.
    pub emitted: u64,
    /// Books not yet knowable at the poll's `until`.
    pub withheld_late: u64,
    /// Books withheld because a hole was still open in the stream behind them.
    pub withheld_gapped: u64,
    /// Books withheld because the venue was in an auction, where the continuous
    /// touch is not what trades.
    pub withheld_auction: u64,
    /// Books withheld because the venue was not trading continuously — halted,
    /// closed, or not yet reached.
    pub withheld_not_trading: u64,
    /// Books withheld because they were crossed and a fresh snapshot did not
    /// fix it.
    pub withheld_crossed: u64,
    /// Holes that opened in a stream.
    pub gaps_opened: u64,
    /// Holes that filled before their deadline.
    pub gaps_filled: u64,
    /// Holes given up on. Each forced a rebuild.
    pub gaps_abandoned: u64,
    /// Updates dropped because they had already been applied.
    pub duplicates: u64,
    /// Books found crossed while the venue said it was trading continuously.
    pub crossed_in_continuous: u64,
}

/// One instrument's book and the sequence discipline guarding it.
#[derive(Debug)]
struct DepthState {
    instrument: DepthInstrument,
    state: VenueState,
    /// `None` until the first snapshot. Replaced, not rewound, by every
    /// snapshot: a tracker carrying the old stream position would read the
    /// updates already folded into the new snapshot as a gap.
    tracker: Option<SequenceTracker>,
}

impl DepthState {
    /// Whether the book has to be rebuilt before it can be read.
    ///
    /// Two states, one answer: never built, and built then discarded.
    /// [`VenueState`] deliberately does not conflate them — an empty book we
    /// were never told is wrong is simply empty — so the "never built" half
    /// lives here as the absent tracker.
    fn needs_snapshot(&self) -> bool {
        self.tracker.is_none() || self.state.is_stale()
    }

    fn has_open_gap(&self) -> bool {
        self.tracker
            .as_ref()
            .is_some_and(SequenceTracker::has_open_gap)
    }
}

/// Maintains order books from a vendor's snapshot and increment endpoints.
#[derive(Debug)]
pub struct DepthFeedAdapter {
    config: DepthFeedConfig,
    /// Prebuilt endpoints, parsed once at construction so a malformed vendor
    /// address fails where it was configured rather than on the first fetch.
    snapshot_endpoint: Option<Url>,
    updates_endpoint: Option<Url>,
    /// Keyed by vendor symbol, which is what goes in the request and comes back
    /// in the response.
    books: BTreeMap<String, DepthState>,
    client: HttpClient,
    stats: DepthStats,
}

impl DepthFeedAdapter {
    /// What a deployment must supply on top of a working configuration.
    ///
    /// These stand even when every field is set, which is why the descriptor's
    /// requirement is never `None`.
    pub const REQUIREMENTS: [&'static str; 5] = [
        "a TLS-terminating egress proxy in front of this adapter, or a vendor reachable over the \
         cluster network: `qip_transport::http` has no TLS stack and refuses `https` by name \
         rather than downgrading it, so a credential sent straight to a public vendor would \
         cross the internet in clear text",
        "a depth licence with the vendor and a `licensing` class that matches it. Depth is \
         routinely licensed apart from, and more tightly than, the top of book, and the class is \
         what decides whether a level may be displayed raw",
        "the vendor's dissemination delay in `publication_delay`, because that is what decides \
         when a book became knowable; a delay set to zero on a delayed depth feed publishes a \
         book before the deployment was entitled to see it",
        "an increment endpoint that resumes from a sequence this deployment names rather than \
         from a time, and a snapshot endpoint that states the sequence its book is complete as \
         of. Without both, a rebuild cannot be aligned with the stream and every recovery either \
         loses updates or applies them twice",
        "an alert on the abandoned-gap and resynchronisation counts, and on the withheld ones. A \
         feed that has started dropping updates, or whose venue status this decoder cannot read, \
         looks from the outside like a quiet market rather than a broken feed",
    ];

    /// Build an adapter. Succeeds even when nothing is configured: an adapter
    /// that cannot fetch still has to exist in order to say so.
    ///
    /// Fails only on configuration that is present and wrong.
    pub fn new(config: DepthFeedConfig, instruments: Vec<DepthInstrument>) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::invalid(
                "a depth feed needs a name: it is recorded as the provenance source of every \
                 book it produces",
            ));
        }
        if config.max_messages == 0 {
            return Err(Error::invalid(
                "max_messages is zero, which would refuse every response the vendor sends",
            ));
        }
        if config.depth == 0 {
            return Err(Error::invalid(
                "depth is zero, which would publish a book with no levels in it. A book with no \
                 levels is indistinguishable from an empty market",
            ));
        }
        if config.reorder.max_buffered_messages == 0 {
            return Err(Error::invalid(
                "the reorder buffer holds nothing, so any update arriving out of order would be \
                 an immediately unrecoverable gap and every rebuild would trigger another",
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
        for (label, path) in [
            ("snapshot", &config.snapshot_path),
            ("update", &config.updates_path),
        ] {
            if path.contains('?') || path.contains('#') {
                return Err(Error::invalid(format!(
                    "the {label} path {path:?} carries a query or a fragment; this adapter builds \
                     the query itself, and a second one would put the symbol and the resume \
                     sequence where the vendor does not read them"
                )));
            }
        }

        let (snapshot_endpoint, updates_endpoint) = match &config.base_url {
            Some(base) => {
                let url = Url::parse(base).map_err(Error::from)?;
                (
                    Some(url.with_path(&config.snapshot_path).map_err(Error::from)?),
                    Some(url.with_path(&config.updates_path).map_err(Error::from)?),
                )
            }
            None => (None, None),
        };

        let mut books: BTreeMap<String, DepthState> = BTreeMap::new();
        let mut streams: BTreeMap<String, String> = BTreeMap::new();
        for instrument in instruments {
            validate_symbol(&instrument.symbol)?;
            if instrument.venue.as_str().trim().is_empty() {
                return Err(Error::invalid(format!(
                    "instrument {} has no venue; the venue is part of every book's identity and \
                     is what stops one venue's messages reaching another's book",
                    instrument.symbol
                )));
            }
            if instrument.feed.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "instrument {} names no feed; the feed is part of the stream key that decides \
                     which sequence numbers are comparable with which",
                    instrument.symbol
                )));
            }
            let stream = instrument.stream_key();
            if let Some(existing) = streams.insert(stream.clone(), instrument.symbol.clone()) {
                return Err(Error::invalid(format!(
                    "{} and {} are both numbered in the stream {stream}: their sequences would be \
                     interleaved into gaps neither venue ever had",
                    existing, instrument.symbol
                )));
            }
            // Not `Closed` and not `Open`. Before the first snapshot this cell
            // has not asked the venue anything, and `Unreachable` is the status
            // that says exactly that — the venue may well be trading and this
            // process cannot see it, which is the more dangerous case and the
            // one worth naming.
            let state = VenueState::new(
                instrument.object_id.clone(),
                instrument.venue.clone(),
                Book::of_kind(config.book_kind),
                VenueStatus::Unreachable,
            );
            if let Some(existing) = books.insert(
                instrument.symbol.clone(),
                DepthState {
                    instrument,
                    state,
                    tracker: None,
                },
            ) {
                return Err(Error::invalid(format!(
                    "two instruments claim the vendor symbol {}: a response keyed by it could \
                     not be resolved to one of them",
                    existing.instrument.symbol
                )));
            }
        }

        let client = HttpClient::new(config.http);
        Ok(Self {
            config,
            snapshot_endpoint,
            updates_endpoint,
            books,
            client,
            stats: DepthStats::default(),
        })
    }

    pub fn stats(&self) -> DepthStats {
        self.stats
    }

    pub fn config(&self) -> &DepthFeedConfig {
        &self.config
    }

    /// Vendor symbols, in the order they are polled.
    pub fn symbols(&self) -> Vec<String> {
        self.books.keys().cloned().collect()
    }

    /// One instrument's venue state, for diagnostics and for a test that wants
    /// to assert what the book actually holds rather than what was published.
    ///
    /// Read-only on purpose: everything that changes a book goes through a
    /// decoded message and the sequence tracker, so there is no way in from
    /// outside that skips the gap check.
    pub fn venue_state(&self, symbol: &str) -> Option<&VenueState> {
        self.books.get(symbol).map(|held| &held.state)
    }

    /// The state of one instrument's touch, whatever its publishability.
    pub fn condition(&self, symbol: &str) -> Option<BookCondition> {
        self.books.get(symbol).map(|held| held.state.condition())
    }

    /// Whether one instrument's book is waiting to be rebuilt.
    pub fn awaiting_snapshot(&self, symbol: &str) -> Option<bool> {
        self.books.get(symbol).map(DepthState::needs_snapshot)
    }

    /// Whether this adapter can fetch at all.
    pub fn is_available(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// Configuration a deployment has not supplied, each named on its own, so
    /// an operator with two of the three learns which one is left.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.snapshot_endpoint.is_none() || self.updates_endpoint.is_none() {
            missing.push(format!(
                "no endpoint: set `base_url` to the vendor's depth base address, which this \
                 adapter requests `{}` and `{}` under",
                self.config.snapshot_path, self.config.updates_path
            ));
        }
        if self.config.api_key.is_none() {
            missing.push(format!(
                "no credential: set `api_key`, which is sent in the `{}` header. A depth feed \
                 that needs no credential is one whose entitlement nobody agreed to, and this \
                 adapter will not treat an anonymous endpoint as a licensed one",
                self.config.api_key_header
            ));
        }
        if self.books.is_empty() {
            missing.push(
                "no instruments: supply the vendor symbol, venue, feed and platform ObjectId of \
                 every instrument whose book this feed carries. The mapping is configuration \
                 because an id invented from a ticker merges two vendors' instruments, and a \
                 stream key invented from a symbol merges two sequence spaces"
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
            "{} cannot fetch and will not substitute a generated book: {}",
            self.config.name,
            self.requirement()
        ))
    }

    /// Advance every instrument's book and return the ones safe to publish,
    /// without the knowable-time gate.
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

    /// One pass over every instrument, decoded into books paired with the
    /// instant each became knowable.
    ///
    /// A refusal on one instrument fails the whole pass rather than being
    /// swallowed, and the books advanced before it keep what they applied —
    /// they are state, not a partial result to be discarded. What must never
    /// happen is a *half-applied* book surviving as publishable, which is why
    /// every apply failure marks its book for rebuild before the error is
    /// returned.
    fn fetch_dated(&mut self, until: Timestamp) -> Result<Vec<(SensedRecord, Timestamp)>> {
        if self.snapshot_endpoint.is_none()
            || self.updates_endpoint.is_none()
            || self.config.api_key.is_none()
            || self.books.is_empty()
        {
            return Err(self.unavailable());
        }

        let mut dated = Vec::with_capacity(self.books.len());
        for symbol in self.symbols() {
            if let Some(record) = self.refresh(&symbol, until)? {
                dated.push(record);
            }
        }
        Ok(dated)
    }

    /// Bring one instrument's book up to date and decide whether it may be
    /// published.
    ///
    /// The order of the steps is the whole of the correctness argument, so it
    /// is written out rather than left to be inferred: rebuild if the book is
    /// missing, apply what the vendor sent, decide whether what came out is
    /// corrupt, recover if it is, and only then ask whether the result may be
    /// read.
    fn refresh(
        &mut self,
        symbol: &str,
        until: Timestamp,
    ) -> Result<Option<(SensedRecord, Timestamp)>> {
        if self
            .books
            .get(symbol)
            .is_some_and(DepthState::needs_snapshot)
        {
            self.snapshot(symbol, until)?;
        }
        self.apply_increments(symbol, until)?;

        // Corruption first, and only where the venue says it is trading
        // continuously: `qip_orderbook::auction` models an auction as running
        // beside the continuous book, so a cross during one is expected rather
        // than evidence of anything.
        let crossed_in_continuous = match self.books.get(symbol) {
            Some(held) => {
                held.state.status() != VenueStatus::Auction
                    && !held.state.is_stale()
                    && held.state.condition() == BookCondition::Crossed
            }
            None => false,
        };
        if crossed_in_continuous {
            self.stats.crossed_in_continuous += 1;
            if let Some(held) = self.books.get_mut(symbol) {
                let crossed_by = held
                    .state
                    .book()
                    .crossed_by()
                    .map(|by| by.to_string())
                    .unwrap_or_else(|| "an unmeasurable amount".into());
                // Reset rather than repair. Dropping the offending level or
                // moving one side to meet the other would produce a book that
                // looks like the market and is not, and nothing downstream
                // could tell which levels this adapter invented.
                held.state.reset(format!(
                    "the book crossed by {crossed_by} while {} said it was trading continuously; \
                     it is rebuilt rather than normalised",
                    held.instrument.venue
                ));
            }
        }

        // One rebuild per pass. A vendor that answers a recovery snapshot with
        // another broken book is a vendor to alert on, not a loop to run.
        if self
            .books
            .get(symbol)
            .is_some_and(DepthState::needs_snapshot)
        {
            self.stats.resynchronisations += 1;
            self.snapshot(symbol, until)?;
        }

        let Some(held) = self.books.get(symbol) else {
            return Ok(None);
        };
        if held.state.status() == VenueStatus::Auction {
            self.stats.withheld_auction += 1;
            return Ok(None);
        }
        if !held.state.continuous_trading() {
            self.stats.withheld_not_trading += 1;
            return Ok(None);
        }
        if held.has_open_gap() {
            // The book is a correct prefix of the stream, not a wrong book. It
            // is still withheld: the updates behind the hole exist and this
            // cell cannot say how far they move the touch, and a book stamped
            // with an old venue time is read by anything taking the latest
            // book per instrument as the market now.
            self.stats.withheld_gapped += 1;
            return Ok(None);
        }
        if held.state.condition() == BookCondition::Crossed {
            self.stats.withheld_crossed += 1;
            return Ok(None);
        }
        let Some(at) = held.state.last_update() else {
            return Ok(None);
        };

        let book = self.book_record(held, at);
        let knowable = at.saturating_add(self.config.publication_delay);
        Ok(Some((SensedRecord::Book(Box::new(book)), knowable)))
    }

    /// The published record: the touch outward, to the configured depth.
    ///
    /// Built by walking [`qip_orderbook`]'s ladder rather than by keeping a
    /// parallel copy of the levels, so there is exactly one book in this
    /// process and no way for a second one to drift from it.
    fn book_record(&self, held: &DepthState, at: Timestamp) -> OrderBook {
        let levels = |side: BookSide| -> Vec<BookLevel> {
            held.state
                .book()
                .levels(side, self.config.depth)
                .into_iter()
                .map(|level| BookLevel {
                    price: level.price,
                    size: level.size,
                    order_count: level.order_count,
                })
                .collect()
        };
        OrderBook {
            object_id: held.instrument.object_id.clone(),
            venue: held.instrument.venue.as_str().to_string(),
            at,
            bids: levels(BookSide::Bid),
            asks: levels(BookSide::Ask),
            // The venue's own position in its stream, which is what lets the
            // bus recognise a redelivery and what a reconciliation joins on.
            sequence: held.state.last_sequence().unwrap_or_default(),
        }
    }

    /// Fetch a complete book and rebuild one instrument's state from it.
    ///
    /// The tracker is replaced, not reused: a snapshot is complete as of its
    /// own sequence, so an increment at or below it has already been folded in
    /// and applying it again would double-count. [`SequenceTracker::expecting`]
    /// is exactly that statement — start contiguous at the snapshot's sequence
    /// and treat anything earlier as a duplicate.
    fn snapshot(&mut self, symbol: &str, until: Timestamp) -> Result<()> {
        let Some(endpoint) = &self.snapshot_endpoint else {
            return Err(self.unavailable());
        };
        let target = format!(
            "{endpoint}?symbol={symbol}&depth={}&until={}",
            self.config.depth,
            until.to_rfc3339()
        );
        let wire: WireSnapshot = self.get(&target, "depth snapshot")?;
        if wire.symbol != symbol {
            return Err(Error::schema(format!(
                "{} answered a snapshot request for {symbol:?} with a book for {:?}. A book \
                 applied to the wrong instrument's state is a book that looks healthy and is \
                 another instrument's market",
                self.config.name, wire.symbol
            )));
        }
        if wire.sequence == 0 {
            return Err(Error::schema(format!(
                "{}: the snapshot for {symbol} is at sequence 0, which names no resume point for \
                 the increments that follow it. A book built from it also publishes with no \
                 idempotency key, so the bus cannot recognise a redelivery of it",
                self.config.name
            )));
        }
        let status = venue_status_from_code(&self.config.name, symbol, wire.status.as_deref())?;

        let levels = wire.bids.len() + wire.asks.len() + wire.orders.len();
        if levels > self.config.max_messages {
            return Err(Error::guard(format!(
                "{}: the snapshot for {symbol} carries {levels} levels and the cap is {}: a \
                 response small enough to read is not automatically a response worth expanding \
                 into a book",
                self.config.name, self.config.max_messages
            )));
        }

        let Some(held) = self.books.get_mut(symbol) else {
            return Err(Error::not_found(format!(
                "{} has no instrument for the symbol {symbol:?}",
                self.config.name
            )));
        };

        // Everything the old book held is now unknown-good. Discarded before
        // the new levels land, so no level from before the rebuild can survive
        // into a book that claims to be complete as of the new sequence.
        held.state.reset(format!(
            "rebuilding {symbol} from a snapshot complete as of sequence {}",
            wire.sequence
        ));

        let origin = held.instrument.origin(wire.sequence);
        let mut messages = Vec::with_capacity(levels + 1);
        messages.push(decoded_message(
            &origin,
            0,
            MessageBody::StatusChange { status },
            wire.at,
            until,
        ));
        for (side, wire_levels) in [(BookSide::Bid, &wire.bids), (BookSide::Ask, &wire.asks)] {
            for level in wire_levels {
                messages.push(decoded_message(
                    &origin,
                    messages.len(),
                    MessageBody::LevelSet {
                        side,
                        price: level.price,
                        quantity: level.size,
                        order_count: level.orders,
                    },
                    wire.at,
                    until,
                ));
            }
        }
        for order in &wire.orders {
            let side = side_from_code(&order.side)?;
            messages.push(decoded_message(
                &origin,
                messages.len(),
                MessageBody::OrderAdded {
                    order_ref: order.order_ref,
                    side,
                    price: order.price,
                    quantity: order.quantity,
                },
                wire.at,
                until,
            ));
        }

        for message in &messages {
            if let Err(error) = held.state.apply(message) {
                // The book is now half a snapshot. Leaving it stale is what
                // stops the next poll reading it as a complete one; the tracker
                // is dropped too, so nothing resumes from a position this
                // snapshot never reached.
                held.tracker = None;
                return Err(Error::invalid(format!(
                    "{}: the snapshot for {symbol} could not be applied to a {} book: {error}",
                    self.config.name,
                    self.config.book_kind.as_str()
                )));
            }
        }
        held.state.resynchronised(wire.at);
        held.tracker = Some(SequenceTracker::expecting(
            held.instrument.stream_key(),
            self.config.reorder,
            wire.sequence.saturating_add(1),
        ));
        self.stats.snapshots += 1;
        Ok(())
    }

    /// Fetch the increments since the last applied sequence and apply the ones
    /// the tracker releases.
    ///
    /// Nothing is applied straight from the response. Everything goes through
    /// [`SequenceTracker`], which is what turns "the next update is 1007 and we
    /// are at 1004" into a reset ahead of the messages that assume otherwise,
    /// instead of into a book with two updates missing from the middle of it.
    fn apply_increments(&mut self, symbol: &str, until: Timestamp) -> Result<()> {
        let Some(endpoint) = &self.updates_endpoint else {
            return Err(self.unavailable());
        };
        let after = match self.books.get(symbol) {
            Some(held) => held
                .tracker
                .as_ref()
                .and_then(SequenceTracker::position)
                .unwrap_or_default(),
            None => return Ok(()),
        };
        // Resumed from a sequence and bounded by the caller's clock. There is
        // no `since` instant: the resume point of a book is a position in a
        // stream, and no timestamp can express "everything through 1004 has
        // been applied". `until` is still sent, because a vendor that answers
        // with updates from after the caller's clock is handing a backtest the
        // future.
        let target = format!(
            "{endpoint}?symbol={symbol}&after_sequence={after}&until={}",
            until.to_rfc3339()
        );
        let wire: WireUpdates = self.get(&target, "depth updates")?;
        if wire.updates.len() > self.config.max_messages {
            return Err(Error::guard(format!(
                "{}: the update response for {symbol} carries {} messages and the cap is {}",
                self.config.name,
                wire.updates.len(),
                self.config.max_messages
            )));
        }
        self.stats.fetches += 1;

        let Some(held) = self.books.get_mut(symbol) else {
            return Ok(());
        };
        let origin_of = |sequence: u64| held.instrument.origin(sequence);
        let mut messages = Vec::with_capacity(wire.updates.len());
        for (ordinal, update) in wire.updates.iter().enumerate() {
            let body = decode_body(&self.config.name, symbol, &update.body)?;
            messages.push(decoded_message(
                &origin_of(update.sequence),
                ordinal,
                body,
                update.at,
                until,
            ));
        }

        let Some(tracker) = held.tracker.as_mut() else {
            return Err(Error::invalid(format!(
                "{}: {symbol} has no stream position, so an increment cannot be placed against \
                 one. A book is only ever resumed from a snapshot",
                self.config.name
            )));
        };

        let mut released = Vec::new();
        let mut events = Vec::new();
        for (_stream, sequence, unit) in delivery_units(messages) {
            let batch = tracker.accept_unit(sequence, unit, until);
            released.extend(batch.released);
            events.extend(batch.events);
        }
        // A poll is discrete: nothing more will arrive for this response, so
        // the deadline is offered a chance to pass here rather than only on the
        // next message. A stream that goes silent right after a gap is exactly
        // the case a message-driven deadline never reaches.
        let batch = tracker.poll(until);
        released.extend(batch.released);
        events.extend(batch.events);

        for event in &events {
            match event {
                SequenceEvent::Duplicate { .. } => self.stats.duplicates += 1,
                SequenceEvent::GapOpened { .. } => self.stats.gaps_opened += 1,
                SequenceEvent::GapFilled { .. } => self.stats.gaps_filled += 1,
                SequenceEvent::GapAbandoned { .. } => self.stats.gaps_abandoned += 1,
                SequenceEvent::StreamStarted { .. } => {}
            }
        }

        for message in &released {
            if let Err(error) = held.state.apply(message) {
                // A refused message leaves `VenueState` exactly as it was, but
                // the messages before it in this batch were applied. Marking
                // the book for rebuild is what stops that partial application
                // being published as a whole book.
                held.state.reset(format!(
                    "an update at sequence {} could not be applied: {error}",
                    message.origin.sequence
                ));
                return Err(Error::invalid(format!(
                    "{}: {symbol} sent an update this book cannot apply: {error}",
                    self.config.name
                )));
            }
        }
        Ok(())
    }

    /// One GET, checked and decoded.
    fn get<T: for<'de> Deserialize<'de>>(&self, target: &str, what: &str) -> Result<T> {
        let Some(key) = &self.config.api_key else {
            return Err(self.unavailable());
        };
        let request = HttpRequest::new(Method::Get, target)
            .map_err(Error::from)?
            .with_header("accept", "application/json")
            .with_header(&self.config.api_key_header, key);
        let response = self.client.send(&request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(self.status_refusal(response.status, &response.body_excerpt()));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "{} sent a {what} body this decoder cannot read: {error}. The first bytes of it \
                 were: {}",
                self.config.name,
                response.body_excerpt()
            ))
        })
    }

    /// What a non-2xx status means here.
    ///
    /// Separated by class because the operator action differs: a rejected
    /// credential is a deployment to fix, a 404 is a path to fix, a 429 or a
    /// 5xx is a vendor to wait for.
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
}

impl DataAdapter for DepthFeedAdapter {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: self.config.name.clone(),
            provider: self.config.provider.clone(),
            licensing: self.config.licensing,
            topics: vec![Topic::MarketOrderBook],
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
                self.stats.withheld_late += 1;
                continue;
            }
            records.push(record);
        }
        // Event order, so a consumer that assumes a monotone stream is not
        // reading this adapter's symbol order instead.
        records.sort_by_key(|r| r.occurred_at().as_nanos());
        self.stats.emitted += records.len() as u64;
        Ok(records)
    }
}

impl MarketDataAdapter for DepthFeedAdapter {
    fn instruments(&self) -> Vec<ObjectId> {
        self.books
            .values()
            .map(|held| held.instrument.object_id.clone())
            .collect()
    }
}

/// A decoded message, identified deterministically.
///
/// The id is derived from the stream, the sequence and the message's position
/// within its delivery unit rather than generated, for the reason
/// `qip_sequencing::identity` gives: the platform replays logs and diffs the
/// results, and an id minted from a counter or a clock would make two identical
/// runs differ. The ordinal is part of the tag because one sequence carries
/// several messages — a snapshot's levels all share the position of the packet
/// that carried them — and ids that collided would look like redeliveries.
///
/// `capture_time` is the caller's `until`, not the wall clock: this cell did
/// see the packet at some real instant, but recording that would make a
/// backtest of the same response produce a different message every run.
fn decoded_message(
    origin: &Origin,
    ordinal: usize,
    body: MessageBody,
    venue_time: Timestamp,
    capture_time: Timestamp,
) -> MarketMessage {
    let object_id = synthetic_id(origin, &format!("depth-{ordinal}"), venue_time);
    MarketMessage::new(object_id, origin.clone(), body, venue_time, capture_time)
}

/// Turn one wire update into the message `qip_orderbook` applies.
///
/// Nothing here decides whether the message *may* be applied — an order message
/// against an aggregated book, a negative size, an unknown order reference are
/// all refusals `qip_orderbook` already owns, and re-deciding them here would
/// be a second book model that could disagree with the first.
fn decode_body(feed: &str, symbol: &str, wire: &WireBody) -> Result<MessageBody> {
    Ok(match wire {
        WireBody::LevelSet {
            side,
            price,
            size,
            orders,
        } => MessageBody::LevelSet {
            side: side_from_code(side)?,
            price: *price,
            quantity: *size,
            order_count: *orders,
        },
        WireBody::Quote { bid, ask } => MessageBody::Quote {
            bid: bid.as_ref().map(|touch| (touch.price, touch.size)),
            ask: ask.as_ref().map(|touch| (touch.price, touch.size)),
        },
        WireBody::OrderAdded {
            order_ref,
            side,
            price,
            quantity,
        } => MessageBody::OrderAdded {
            order_ref: *order_ref,
            side: side_from_code(side)?,
            price: *price,
            quantity: *quantity,
        },
        WireBody::OrderReduced {
            order_ref,
            remaining,
        } => MessageBody::OrderReduced {
            order_ref: *order_ref,
            remaining: *remaining,
        },
        WireBody::OrderRemoved { order_ref } => MessageBody::OrderRemoved {
            order_ref: *order_ref,
        },
        WireBody::OrderReplaced {
            order_ref,
            price,
            quantity,
        } => MessageBody::OrderReplaced {
            order_ref: *order_ref,
            price: *price,
            quantity: *quantity,
        },
        WireBody::Trade {
            price,
            size,
            condition,
            aggressor,
        } => MessageBody::Trade {
            price: *price,
            quantity: *size,
            // An absent condition is refused for the same reason an unreadable
            // one is, and for the same reason `rest.rs` refuses it on a
            // top-of-book trade: the only value that could stand in for it is
            // `Regular`, and `TradeCondition::updates_last` and
            // `::counts_toward_volume` are both true for `Regular`. A late
            // report or an off-exchange cross whose condition the vendor left
            // out would otherwise move the book's last-sale price and session
            // volume as an ordinary continuous print, and nothing downstream
            // could tell the difference. This decoder cannot tell "the vendor
            // says this printed normally" from "the vendor said nothing", so
            // it declines to guess.
            condition: match condition.as_deref() {
                Some(code) => trade_condition_from_code(code)?,
                None => {
                    return Err(Error::schema(format!(
                        "{feed}: {symbol} sent a trade at {price} for {size} with no condition, \
                         and this decoder will not read that as {:?}: an unstated condition is \
                         not a regular print, and regular is the one condition that updates the \
                         last sale and counts toward volume. Have the vendor send a condition, or \
                         map its feed to one.",
                        TradeCondition::Regular
                    )));
                }
            },
            aggressor: aggressor.as_deref().map(side_from_code).transpose()?,
        },
        WireBody::Status { status } => MessageBody::StatusChange {
            status: venue_status_from_code(feed, symbol, Some(status))?,
        },
        WireBody::Auction {
            indicative_price,
            paired,
            imbalance,
            imbalance_side,
        } => MessageBody::AuctionUpdate {
            indicative_price: *indicative_price,
            paired: *paired,
            imbalance: *imbalance,
            imbalance_side: imbalance_side.as_deref().map(side_from_code).transpose()?,
        },
        WireBody::Reset { reason } => MessageBody::Reset {
            reason: match reason {
                Some(reason) => format!("{feed} asked {symbol} to resynchronise: {reason}"),
                None => format!("{feed} asked {symbol} to resynchronise"),
            },
        },
    })
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
             line: only ASCII letters, digits and . - _ : are accepted"
        )));
    }
    Ok(())
}

fn side_from_code(code: &str) -> Result<BookSide> {
    match code {
        "bid" | "buy" => Ok(BookSide::Bid),
        "ask" | "sell" | "offer" => Ok(BookSide::Ask),
        other => Err(Error::schema(format!(
            "unknown book side {other:?}: this decoder accepts bid, buy, ask, sell and offer. A \
             guessed side puts liquidity on the wrong half of the book, which reads downstream \
             as a market that has moved"
        ))),
    }
}

/// Venue trading state, which every snapshot must name.
///
/// There is no default, and the absent case is refused rather than read as
/// open. The status decides whether the continuous touch may be priced off at
/// all and, when the book is crossed, whether that is an auction accumulating
/// orders or a feed sending corruption. `Open` is the one value that reads
/// downstream as permission, so it is the one value this decoder will not
/// invent.
fn venue_status_from_code(feed: &str, symbol: &str, code: Option<&str>) -> Result<VenueStatus> {
    match code {
        None => Err(Error::schema(format!(
            "{feed}: the snapshot for {symbol} states no venue status. It is refused rather than \
             assumed open: the status is what decides whether the continuous book may be priced \
             off, and whether a crossed book is an auction accumulating orders or a feed sending \
             a book nobody can hit"
        ))),
        Some("open") => Ok(VenueStatus::Open),
        Some("auction") => Ok(VenueStatus::Auction),
        Some("halted") => Ok(VenueStatus::Halted),
        Some("closed") => Ok(VenueStatus::Closed),
        Some("unreachable") => Ok(VenueStatus::Unreachable),
        Some(other) => Err(Error::schema(format!(
            "unknown venue status {other:?}: this decoder accepts open, auction, halted, closed \
             and unreachable"
        ))),
    }
}

/// Trade conditions decide whether a print updates the last sale and whether it
/// counts toward session volume, so an unreadable one is refused rather than
/// defaulted: reading a correction as a regular print double-counts the volume
/// it was sent to withdraw.
fn trade_condition_from_code(code: &str) -> Result<TradeCondition> {
    match code {
        "regular" => Ok(TradeCondition::Regular),
        "auction" => Ok(TradeCondition::Auction),
        "reported" => Ok(TradeCondition::Reported),
        "odd_lot" => Ok(TradeCondition::OddLot),
        "correction" => Ok(TradeCondition::Correction),
        "negotiated" => Ok(TradeCondition::Negotiated),
        other => Err(Error::schema(format!(
            "unknown trade condition {other:?}: this decoder accepts regular, auction, reported, \
             odd_lot, correction and negotiated"
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
// this decoder reads are refused, because those change what the message means —
// and a book is built by applying messages, so a misread one is not one bad
// record but every book published after it.
//
// Two fields have no `serde(default)` standing in for them and are modelled as
// `Option` so a missing one produces this module's own refusal rather than
// serde's "missing field": a snapshot's `sequence`, without which nothing can
// resume, and its `status`, without which a crossed book cannot be told from an
// auction.

/// A complete book as of one sequence.
#[derive(Debug, Deserialize)]
struct WireSnapshot {
    /// Echoed back so a vendor answering with the wrong instrument's book is
    /// caught here rather than by whoever prices off it.
    symbol: String,
    /// The venue position this book is complete as of. Increments at or below
    /// it are already folded in.
    sequence: u64,
    /// Venue time the book was taken.
    at: Timestamp,
    /// The venue's trading state. See [`venue_status_from_code`]: absent is
    /// refused, not read as open.
    #[serde(default)]
    status: Option<String>,
    /// Aggregated levels, dearest bid and cheapest ask first or in any order —
    /// `qip_orderbook` holds them in a ladder keyed by price, so the order they
    /// arrive in cannot matter.
    #[serde(default)]
    bids: Vec<WireLevel>,
    #[serde(default)]
    asks: Vec<WireLevel>,
    /// Resting orders, for an order-by-order feed. A snapshot carrying both
    /// these and levels is refused by the book itself, which will not take
    /// aggregated and order-by-order messages at once.
    #[serde(default)]
    orders: Vec<WireOrder>,
}

#[derive(Debug, Deserialize)]
struct WireLevel {
    price: Decimal,
    size: Decimal,
    /// Resting orders at the level, where the venue publishes a count.
    #[serde(default)]
    orders: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct WireOrder {
    order_ref: u64,
    side: String,
    price: Decimal,
    quantity: Decimal,
}

#[derive(Debug, Default, Deserialize)]
struct WireUpdates {
    #[serde(default)]
    updates: Vec<WireUpdate>,
}

/// One increment: where it sits in the stream, when the venue says it happened,
/// and what it does.
#[derive(Debug, Deserialize)]
struct WireUpdate {
    sequence: u64,
    at: Timestamp,
    #[serde(flatten)]
    body: WireBody,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireBody {
    /// Set an aggregated level's total size. Zero removes it.
    LevelSet {
        side: String,
        price: Decimal,
        size: Decimal,
        #[serde(default)]
        orders: Option<u32>,
    },
    /// Top of book only, for a level-1 feed. A side sent as `null` is a
    /// statement that the side is empty, which `qip_orderbook` acts on.
    Quote {
        #[serde(default)]
        bid: Option<WireTouch>,
        #[serde(default)]
        ask: Option<WireTouch>,
    },
    OrderAdded {
        order_ref: u64,
        side: String,
        price: Decimal,
        quantity: Decimal,
    },
    OrderReduced {
        order_ref: u64,
        remaining: Decimal,
    },
    OrderRemoved {
        order_ref: u64,
    },
    OrderReplaced {
        order_ref: u64,
        price: Decimal,
        quantity: Decimal,
    },
    Trade {
        price: Decimal,
        size: Decimal,
        #[serde(default)]
        condition: Option<String>,
        #[serde(default)]
        aggressor: Option<String>,
    },
    /// The venue changed trading state. What makes an auction visible.
    Status {
        status: String,
    },
    /// The auction's own numbers, kept beside the book rather than folded into
    /// it — `qip_orderbook::auction`'s model, not this adapter's.
    Auction {
        #[serde(default)]
        indicative_price: Option<Decimal>,
        paired: Decimal,
        imbalance: Decimal,
        #[serde(default)]
        imbalance_side: Option<String>,
    },
    /// The vendor asking for a resubscribe. Honoured: it becomes the same
    /// reset a detected gap produces, and the book is rebuilt.
    Reset {
        #[serde(default)]
        reason: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct WireTouch {
    price: Decimal,
    size: Decimal,
}
