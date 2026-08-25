//! Live streams, as server-sent events.
//!
//! The REST surface answers the question "what is true now"; a client that
//! wants to know when something changes has to ask again. That is fine for an
//! operator refreshing a page and wrong for anything watching a trading loop:
//! a dashboard polling `/api/v1/orders` once a second is both a second late
//! and, most of those seconds, asking about nothing.
//!
//! Server-sent events rather than WebSockets, for three reasons that are
//! really one reason. They are one-directional, which is all a read-only
//! surface needs; they are plain HTTP, so the authentication, the rate limit
//! and the role check in front of every other route apply unchanged; and they
//! are text frames over a normal response body, so the in-tree server serves
//! them without a protocol upgrade, a frame codec or an async runtime.
//!
//! Four things are worth reading before the code.
//!
//! * [`StreamKind`] is the whole live surface in one table, with the source
//!   each stream reads and what a reconnecting client can and cannot recover.
//!   Nothing here invents data: four of the five streams are views over the
//!   platform's own append-only event log, and the fifth reports the process's
//!   observed health. There is no generator of plausible-looking events.
//! * Every event carries a type, a **per-connection sequence** that increases
//!   by exactly one, a **cursor** that is the log position and is what the SSE
//!   `id:` field carries, both timestamps, and a correlation id. The two
//!   numbers answer two different questions and neither substitutes for the
//!   other: the sequence is contiguous so a client can see that it missed
//!   something, and the cursor is global and stable so a client can say where
//!   to resume from. A filtered stream's cursors are sparse by construction —
//!   the log interleaves every topic — so a client that tried to detect gaps
//!   from cursors alone would report a loss on every event.
//! * [`EventStream::next_frame`] is a bounded loop. It returns within
//!   [`StreamLimits::heartbeat_after`] whether or not anything has happened,
//!   because a producer that blocks waiting for an event is a connection whose
//!   death is never noticed and a thread held against the server's concurrency
//!   limit for the life of the process. The connection is also given a total
//!   lifetime, after which it closes with a final event naming the cursor to
//!   resume from — a stream that never ends is a memory leak with a schedule.
//! * A write failure is the disconnect. It is handled in [`crate::http::pump`],
//!   which stops the loop and reports it as an ordinary ending, because that is
//!   what a closed browser tab looks like from this side.

use crate::cells::CellRegistry;
use crate::http::ResponseStream;
use qip_core::Clock;
use qip_core::time::Timestamp;
use qip_events::log::LogRecord;
use qip_events::topic::Topic;
use qip_kernel::Platform;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

/// Where the live streams sit under the version prefix.
///
/// A constant rather than a literal, for the same reason [`crate::routes`]
/// keeps one: the route table, the dispatcher and the source lookup all have
/// to agree about where a stream lives, and two of them agreeing is not enough.
pub const STREAM_PREFIX: &str = "/stream";

/// The header a client sends to resume a stream it was already reading.
pub const LAST_EVENT_ID: &str = "last-event-id";

// --- the surface ------------------------------------------------------------

/// Every live stream this API serves.
///
/// Written out so the whole live surface, what feeds it, and what a reconnect
/// recovers can be read in one place — the same discipline
/// [`crate::routes::ROUTES`] applies to the REST surface, and for the same
/// reason: a stream that a client will hold open for hours is a contract, and a
/// contract nobody can read in one sitting is a contract nobody checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// Normalised market data as the platform recorded it.
    Market,
    /// Signals, anomalies, regime changes and ranked opportunities.
    Signals,
    /// The order lifecycle, transition by transition.
    Orders,
    /// Position, P&L and reconciliation changes.
    Positions,
    /// The health of this process, as it changes.
    Health,
}

impl StreamKind {
    /// Every stream, in the order they are declared.
    pub const ALL: [Self; 5] = [
        Self::Market,
        Self::Signals,
        Self::Orders,
        Self::Positions,
        Self::Health,
    ];

    /// The last path segment, and the name that appears in every event.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Signals => "signals",
            Self::Orders => "orders",
            Self::Positions => "positions",
            Self::Health => "health",
        }
    }

    /// The route pattern, under [`crate::routes::VERSION_PREFIX`].
    pub const fn pattern(&self) -> &'static str {
        match self {
            Self::Market => "/stream/market",
            Self::Signals => "/stream/signals",
            Self::Orders => "/stream/orders",
            Self::Positions => "/stream/positions",
            Self::Health => "/stream/health",
        }
    }

    /// Resolve a path under the version prefix to the stream it names.
    pub fn from_pattern(pattern: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.pattern() == pattern)
    }

    /// What the stream carries, for the route table and the OpenAPI document.
    pub const fn summary(&self) -> &'static str {
        match self {
            Self::Market => {
                "live: normalised market events (ticks, quotes, trades, books, bars, corporate \
                 actions, reference and macro updates) as they are recorded"
            }
            Self::Signals => {
                "live: signals, anomalies, regime changes and opportunities as they are detected \
                 and ranked"
            }
            Self::Orders => {
                "live: the order lifecycle — proposed, approved, submitted, amended, cancelled, \
                 rejected, filled"
            }
            Self::Positions => "live: position, P&L and reconciliation changes",
            Self::Health => {
                "live: the health of this process — halt state, autonomy, reconciliation breaks, \
                 cycle and event counts, cell freshness — emitted whenever it changes"
            }
        }
    }

    /// The topics a log-backed stream carries, or an empty slice for a stream
    /// that is not log-backed.
    ///
    /// Listed rather than derived from [`qip_events::TopicGroup`] because the
    /// `act` group holds both halves of a trade — the order lifecycle and the
    /// position it moves — and a dashboard wants those on separate streams.
    /// Deriving the other three from their groups and writing this one out
    /// would be worse than writing all four out: a reader could not tell, at a
    /// glance, which streams were exhaustive.
    pub const fn topics(&self) -> &'static [Topic] {
        match self {
            Self::Market => &[
                Topic::MarketTick,
                Topic::MarketQuote,
                Topic::MarketTrade,
                Topic::MarketOrderBook,
                Topic::MarketBar,
                Topic::MarketCorporateAction,
                Topic::FundamentalUpdated,
                Topic::MacroUpdated,
                Topic::NewsReceived,
                Topic::AlternativeDataReceived,
                Topic::ReferenceDataUpdated,
                Topic::DataQualityFailed,
            ],
            Self::Signals => &[
                Topic::SignalGenerated,
                Topic::AnomalyDetected,
                Topic::RegimeChanged,
                Topic::OpportunityDetected,
                Topic::OpportunityRanked,
            ],
            Self::Orders => &[
                Topic::OrderProposed,
                Topic::OrderApproved,
                Topic::OrderSubmitted,
                Topic::OrderAmended,
                Topic::OrderCancelled,
                Topic::OrderRejected,
                Topic::OrderFilled,
            ],
            Self::Positions => &[
                Topic::PositionUpdated,
                Topic::PnlUpdated,
                Topic::ReconciliationCompleted,
            ],
            Self::Health => &[],
        }
    }

    /// Whether `Last-Event-ID` can genuinely resume this stream.
    pub const fn replays(&self) -> bool {
        !matches!(self, Self::Health)
    }

    /// Where the stream's events come from, stated for the client.
    ///
    /// Served in the stream's descriptor and in the OpenAPI summary, because a
    /// client plotting a line has to know whether a quiet stream means a quiet
    /// market or a subsystem this deployment does not run.
    pub const fn source(&self) -> &'static str {
        match self {
            Self::Health => {
                "this process's own observed state: the same reading GET /api/v1/health \
                 computes, plus the cycle and event counters and the freshness of every cell \
                 that has reported here. It is a state stream, not a history: an event is \
                 emitted when the reading changes and at the moment a client connects."
            }
            _ => {
                "the platform's append-only event log, filtered to this stream's topics. \
                 Nothing is synthesised: an event appears here only after the platform \
                 recorded it, so a silent stream means the platform recorded nothing on \
                 these topics — not that the feed is broken."
            }
        }
    }

    /// Exactly what a reconnecting client recovers, and what it does not.
    ///
    /// The honest answer differs per stream and the difference matters, so it
    /// is stated per stream rather than once in a README nobody reads at three
    /// in the morning.
    pub const fn replay_note(&self) -> &'static str {
        match self {
            Self::Health => {
                "Last-Event-ID does not replay this stream. It carries state rather than \
                 history and the process keeps no record of past readings, so a reconnecting \
                 client is sent the current reading as its first event and loses every \
                 intermediate transition that happened while it was away. What it does not \
                 lose is correctness about now: the first event after a reconnect is the \
                 present state, not a stale one. A client that must not miss a transition \
                 should read the log-backed streams, where the transitions are events."
            }
            Self::Market => {
                "Last-Event-ID resumes from the log, which is held in memory and begins again \
                 at each restart: a client reconnecting to a process that restarted is sent \
                 the retained backlog rather than everything it missed. This stream is also \
                 the one the log is allowed to forget — market ticks, quotes and books are \
                 the topics its capacity bound evicts first — so a client that was away long \
                 enough may find its cursor has been evicted. That case is not silent: the \
                 stream opens with a `stream.gap` event naming how many records could not be \
                 replayed. The durable copy is the archived hash chain, which this stream \
                 does not read."
            }
            _ => {
                "Last-Event-ID resumes from the log, which is held in memory and begins again \
                 at each restart: a client reconnecting to a process that restarted is sent \
                 the retained backlog rather than everything it missed, and one reconnecting \
                 within the life of the process is sent exactly what it missed. Events on \
                 these topics are never evicted by the log's capacity bound. If a cursor \
                 cannot be replayed the stream says so in a `stream.gap` event rather than \
                 silently skipping ahead. The durable copy is the archived hash chain, which \
                 this stream does not read."
            }
        }
    }

    /// The descriptor a caller gets when it asks for the stream without being
    /// able to read one.
    ///
    /// Reached by anything that calls the handler directly rather than over a
    /// socket — a test, an embedder, a client whose HTTP library buffers the
    /// whole body before returning. Answering with the contract is more useful
    /// than answering with a `406`, and it is the same text the doc comments
    /// above carry, so the two cannot drift.
    pub fn descriptor(&self) -> String {
        let mut topics = String::new();
        for (index, topic) in self.topics().iter().enumerate() {
            if index > 0 {
                topics.push(',');
            }
            topics.push_str(&crate::json::string(topic.name()));
        }
        format!(
            r#"{{"stream":{},"path":{},"content_type":"text/event-stream","summary":{},"source":{},"replays":{},"reconnect":{},"topics":[{}],"event_fields":["type","sequence","cursor","event_time","ingest_time","correlation_id"]}}"#,
            crate::json::string(self.name()),
            crate::json::string(&format!(
                "{}{}",
                crate::routes::VERSION_PREFIX,
                self.pattern()
            )),
            crate::json::string(self.summary()),
            crate::json::string(self.source()),
            self.replays(),
            crate::json::string(self.replay_note()),
            topics
        )
    }
}

// --- events -----------------------------------------------------------------

/// One thing a source has to say, before a connection numbers it.
///
/// Separate from [`SseEvent`] because the sequence number is a property of the
/// *delivery*, not of the event: two clients reading the same stream from
/// different cursors are each owed a contiguous count of what they were sent,
/// and neither can be given the other's.
#[derive(Clone, Debug)]
pub struct Emission {
    /// Position in the source. The SSE `id:`, and what `Last-Event-ID` names.
    pub cursor: u64,
    /// The topic's wire name, or a `stream.*` name for an event about the
    /// stream itself.
    pub event_type: String,
    /// When the fact was true in the world.
    pub occurred_at: Timestamp,
    /// When this process recorded it. Kept separate from `occurred_at`
    /// because the gap between them is the ingestion latency, and a client
    /// that cannot see it cannot tell a slow feed from a quiet market.
    pub ingested_at: Timestamp,
    pub correlation_id: String,
    pub payload: serde_json::Value,
}

/// One event, as it goes on the wire.
#[derive(Clone, Debug)]
pub struct SseEvent {
    pub stream: &'static str,
    /// Delivery count on this connection, starting at 1 and increasing by
    /// exactly one. This is the number a client checks for gaps.
    pub sequence: u64,
    pub emission: Emission,
}

impl SseEvent {
    /// The `data:` object, as JSON.
    pub fn data(&self) -> String {
        let mut fields = serde_json::Map::new();
        fields.insert(
            "stream".to_string(),
            serde_json::Value::String(self.stream.to_string()),
        );
        fields.insert(
            "type".to_string(),
            serde_json::Value::String(self.emission.event_type.clone()),
        );
        fields.insert("sequence".to_string(), self.sequence.into());
        fields.insert("cursor".to_string(), self.emission.cursor.into());
        fields.insert(
            "event_time".to_string(),
            serde_json::Value::String(self.emission.occurred_at.to_rfc3339()),
        );
        fields.insert(
            "ingest_time".to_string(),
            serde_json::Value::String(self.emission.ingested_at.to_rfc3339()),
        );
        fields.insert(
            "correlation_id".to_string(),
            serde_json::Value::String(self.emission.correlation_id.clone()),
        );
        fields.insert("payload".to_string(), self.emission.payload.clone());
        let value = serde_json::Value::Object(fields);
        serde_json::to_string(&value).unwrap_or_else(|error| {
            // Reached only if a payload holds something JSON cannot represent.
            // Reported as an event rather than dropped: a client that silently
            // never receives an order transition has no way to notice.
            format!(
                r#"{{"stream":{},"type":"stream.encoding_failed","sequence":{},"cursor":{},"detail":{}}}"#,
                crate::json::string(self.stream),
                self.sequence,
                self.emission.cursor,
                crate::json::string(&error.to_string())
            )
        })
    }

    /// The event as an SSE frame.
    ///
    /// The framing rule that matters: a `data:` line ends at the first newline,
    /// so a newline inside the payload would split one event into two frames
    /// and hand the client half an object. Compact JSON never contains a raw
    /// newline — the encoder escapes them — and the filter below is the second
    /// line of defence, applied for the same reason the header encoder strips
    /// CR and LF: the cost is a character comparison and the failure it
    /// prevents is silent corruption of the stream.
    pub fn frame(&self) -> String {
        let data: String = self
            .data()
            .chars()
            .filter(|c| *c != '\r' && *c != '\n')
            .collect();
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.emission.cursor,
            self.emission
                .event_type
                .chars()
                .filter(|c| *c != '\r' && *c != '\n')
                .collect::<String>(),
            data
        )
    }
}

/// An SSE comment line.
///
/// Ignored by every client, which is exactly what makes it useful: it is a
/// byte on the wire that proves the connection is alive without being an event
/// a client has to understand or de-duplicate.
pub fn comment(text: &str) -> String {
    let single_line: String = text.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    format!(": {single_line}\n\n")
}

// --- sources ----------------------------------------------------------------

/// What one poll of a source produced.
#[derive(Clone, Debug)]
pub enum Poll {
    /// Events, possibly none.
    Events(Vec<Emission>),
    /// The source cannot be read and will not recover on this connection.
    ///
    /// Distinct from an empty batch on purpose. An empty batch means nothing
    /// happened; this means nobody can tell whether anything happened, and a
    /// stream that reports the two identically is a stream that shows a
    /// crashed platform as a calm market.
    Faulted(String),
}

/// Where a stream's events come from.
///
/// Constructed per connection rather than shared, so a source may hold the
/// cursor bookkeeping for one reader without a lock.
pub trait EventSource: Send {
    /// Where this connection should start, and what to tell the client about
    /// what it is and is not getting.
    ///
    /// `after` is the client's `Last-Event-ID` when it sent one.
    fn resume(&mut self, after: Option<u64>) -> Resume;

    /// Emissions with a cursor above `after`, at most `limit` of them.
    fn since(&mut self, after: u64, limit: usize) -> Poll;
}

/// Where a connection starts and what it lost getting there.
#[derive(Clone, Debug)]
pub struct Resume {
    /// The cursor to poll from. The first event delivered has a cursor above
    /// this one.
    pub cursor: u64,
    /// How many source records could not be replayed, when that is knowable.
    ///
    /// `Some(0)` means the resume was exact. `None` means the source cannot
    /// answer the question — which is itself worth telling the client, and is
    /// why this is not simply a count that defaults to zero.
    pub skipped: Option<u64>,
    /// What the client is getting, in words, for the opening comment.
    pub note: String,
}

/// A stream backed by the platform's append-only event log.
///
/// Reads under the platform lock and copies out, releasing it before anything
/// is written to a socket. A stream that held the lock while writing would let
/// one slow reader stall the trading loop, which is the trade this whole
/// module exists to avoid making.
pub struct LoggedEvents {
    platform: Arc<Mutex<Platform>>,
    topics: &'static [Topic],
    /// How many retained events a client that sent no `Last-Event-ID` is given
    /// before the stream goes live.
    backlog: usize,
}

impl std::fmt::Debug for LoggedEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoggedEvents")
            .field("topics", &self.topics.len())
            .field("backlog", &self.backlog)
            .finish_non_exhaustive()
    }
}

impl LoggedEvents {
    pub fn new(platform: Arc<Mutex<Platform>>, topics: &'static [Topic], backlog: usize) -> Self {
        Self {
            platform,
            topics,
            backlog,
        }
    }
}

impl EventSource for LoggedEvents {
    fn resume(&mut self, after: Option<u64>) -> Resume {
        let Ok(platform) = self.platform.lock() else {
            return Resume {
                cursor: after.unwrap_or(0),
                skipped: None,
                note: "the platform is in an inconsistent state after an internal failure; \
                       this stream cannot say what it has missed"
                    .to_string(),
            };
        };
        let records = platform.event_log().records();
        let earliest = records.first().map_or(0, |record| record.sequence);
        let latest = records.last().map_or(0, |record| record.sequence);

        let Some(after) = after else {
            // No Last-Event-ID: a fresh client. Give it a bounded backlog so a
            // dashboard has something to render immediately, rather than the
            // whole log — which on a long-running process is megabytes a
            // browser did not ask for — or nothing at all, which leaves the
            // panel blank until the platform happens to do something.
            let matching: Vec<u64> = self.selected(records).collect();
            let cursor = matching
                .len()
                .checked_sub(self.backlog)
                .and_then(|first| matching.get(first))
                .map_or(earliest.saturating_sub(1), |sequence| sequence - 1);
            return Resume {
                cursor,
                skipped: Some(0),
                note: format!(
                    "opening at the live edge with up to {} retained event(s) of context; \
                     send Last-Event-ID to resume exactly",
                    self.backlog
                ),
            };
        };

        // A resume the log can no longer honour. The count is what the client
        // needs to decide whether to reconcile over REST, and saying nothing
        // would let it carry on believing its sequence was contiguous.
        if after < earliest.saturating_sub(1) && !records.is_empty() {
            let skipped = earliest.saturating_sub(1) - after;
            return Resume {
                cursor: earliest.saturating_sub(1),
                skipped: Some(skipped),
                note: format!(
                    "resuming after cursor {after}, but the oldest record this process still \
                     holds is {earliest}: {skipped} log record(s) cannot be replayed. Reconcile \
                     over the REST surface before trusting the sequence."
                ),
            };
        }
        Resume {
            cursor: after,
            skipped: Some(0),
            note: format!("resuming exactly after cursor {after} (log head is {latest})"),
        }
    }

    fn since(&mut self, after: u64, limit: usize) -> Poll {
        let Ok(platform) = self.platform.lock() else {
            return Poll::Faulted(
                "the platform is in an inconsistent state after an internal failure and is not \
                 serving"
                    .to_string(),
            );
        };
        Poll::Events(select(
            platform.event_log().records(),
            self.topics,
            after,
            limit,
        ))
    }
}

impl LoggedEvents {
    /// The sequences of the records this stream carries, oldest first.
    fn selected<'a>(&'a self, records: &'a [LogRecord]) -> impl Iterator<Item = u64> + 'a {
        records
            .iter()
            .filter(|record| self.topics.contains(&record.event.topic))
            .map(|record| record.sequence)
    }
}

/// The records a stream carries, above `after` and capped at `limit`.
///
/// A linear scan of the in-memory log on every poll. Deliberate: the log is a
/// `Vec` a few tens of thousands of records long, a poll happens a handful of
/// times a second per connection, and the alternative — a per-stream index
/// maintained alongside the log — is a second copy of the ordering that can
/// disagree with the first. The scan cannot disagree with anything.
///
/// Free-standing rather than a method so it can be tested against records
/// built by hand, without a platform: the selection rule is the part that
/// decides whether an order transition reaches a dashboard, and testing it
/// only through a running platform would mean testing it only for the topics
/// that platform happens to emit.
pub fn select(records: &[LogRecord], topics: &[Topic], after: u64, limit: usize) -> Vec<Emission> {
    records
        .iter()
        .filter(|record| record.sequence > after && topics.contains(&record.event.topic))
        .take(limit)
        .map(|record| Emission {
            cursor: record.sequence,
            event_type: record.event.topic.name().to_string(),
            occurred_at: record.event.occurred_at,
            ingested_at: record.event.recorded_at,
            correlation_id: record.event.lineage.correlation_id.as_str().to_string(),
            payload: record.event.payload.clone(),
        })
        .collect()
}

/// The health reading every subscriber sees, and its sequence.
///
/// Shared across connections so that two clients watching the same process
/// agree about which transition they are looking at. Without it each
/// connection would number the transitions it happened to observe, and an
/// operator comparing two dashboards would find them disagreeing about how
/// many times the platform had halted.
#[derive(Debug, Default)]
pub struct HealthPulse {
    latest: Mutex<Option<HealthReading>>,
}

#[derive(Clone, Debug)]
struct HealthReading {
    sequence: u64,
    digest: String,
    at: Timestamp,
    body: serde_json::Value,
}

impl HealthPulse {
    /// Read the current health, advancing the sequence when it has changed.
    ///
    /// `None` when the shared reading cannot be taken, which a caller reports
    /// as a fault rather than as "nothing changed" — the two look identical on
    /// a stream and mean opposite things.
    fn observe(
        &self,
        platform: &Platform,
        cells: &CellRegistry,
        now: Timestamp,
    ) -> Option<HealthReading> {
        let body = health_body(platform, cells, now);
        let digest = serde_json::to_string(&body).unwrap_or_default();
        let mut latest = self.latest.lock().ok()?;
        let unchanged = latest
            .as_ref()
            .is_some_and(|reading| reading.digest == digest);
        if unchanged {
            return latest.clone();
        }
        let sequence = latest.as_ref().map_or(0, |reading| reading.sequence) + 1;
        let reading = HealthReading {
            sequence,
            digest,
            at: now,
            body,
        };
        *latest = Some(reading.clone());
        Some(reading)
    }
}

/// The health of this process, as a stream of changes.
///
/// Not log-backed: the platform records a system event for some of these
/// transitions and none for others — a reconciliation break appears in the
/// order manager without an event of its own — so a stream assembled from the
/// log alone would be a health stream that misses exactly the conditions a
/// health stream exists for.
pub struct PlatformHealth {
    platform: Arc<Mutex<Platform>>,
    cells: Arc<CellRegistry>,
    pulse: Arc<HealthPulse>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PlatformHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformHealth").finish_non_exhaustive()
    }
}

impl PlatformHealth {
    pub fn new(
        platform: Arc<Mutex<Platform>>,
        cells: Arc<CellRegistry>,
        pulse: Arc<HealthPulse>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            platform,
            cells,
            pulse,
            clock,
        }
    }
}

impl EventSource for PlatformHealth {
    fn resume(&mut self, _after: Option<u64>) -> Resume {
        // Always from zero, whatever the client sent. The shared sequence only
        // ever increases, so starting at zero guarantees the connection's first
        // poll emits the current reading — which is the one thing this stream
        // can promise a reconnecting client.
        Resume {
            cursor: 0,
            skipped: None,
            note: StreamKind::Health.replay_note().to_string(),
        }
    }

    fn since(&mut self, after: u64, _limit: usize) -> Poll {
        let now = self.clock.now();
        let Ok(platform) = self.platform.lock() else {
            return Poll::Faulted(
                "the platform is in an inconsistent state after an internal failure and is not \
                 serving"
                    .to_string(),
            );
        };
        let Some(reading) = self.pulse.observe(&platform, self.cells.as_ref(), now) else {
            return Poll::Faulted(
                "the shared health reading is in an inconsistent state after an internal failure"
                    .to_string(),
            );
        };
        drop(platform);
        if reading.sequence <= after {
            return Poll::Events(Vec::new());
        }
        Poll::Events(vec![Emission {
            cursor: reading.sequence,
            event_type: "health.changed".to_string(),
            // Both timestamps are the moment the change was observed. Stated
            // rather than left out: this process learns of its own health by
            // looking, so there is no earlier moment at which the fact was
            // true and unobserved, and a fabricated `occurred_at` would make
            // the ingestion latency look like a measurement.
            occurred_at: reading.at,
            ingested_at: reading.at,
            correlation_id: format!("health-{}", reading.sequence),
            payload: reading.body,
        }])
    }
}

/// The reading the health stream publishes.
///
/// The same fields `GET /api/v1/health` computes, plus the counters and the
/// cell freshness an operator watching a live dashboard needs. Every one of
/// them is read from this process's own state; none is estimated.
fn health_body(platform: &Platform, cells: &CellRegistry, now: Timestamp) -> serde_json::Value {
    let switch = platform.autonomy().kill_switch();
    let halted = switch.is_globally_tripped();
    let breaks = platform.orders().reconciliation_breaks().len();
    let bound = cells.freshness_bound();
    let observations = cells.observations();
    let stale = observations
        .iter()
        .filter(|observation| observation.is_stale(now, bound))
        .count();

    let mut fields = serde_json::Map::new();
    fields.insert(
        "status".to_string(),
        serde_json::Value::String(
            if halted {
                "halted"
            } else if breaks > 0 {
                "reconciliation-break"
            } else {
                "ok"
            }
            .to_string(),
        ),
    );
    fields.insert("halted".to_string(), halted.into());
    fields.insert(
        "halted_scopes".to_string(),
        serde_json::Value::Array(
            switch
                .halted_scopes()
                .iter()
                .map(|scope| serde_json::Value::String(scope.to_string()))
                .collect(),
        ),
    );
    fields.insert(
        "autonomy".to_string(),
        serde_json::Value::String(platform.autonomy().level().as_str().to_string()),
    );
    fields.insert(
        "ceiling".to_string(),
        serde_json::Value::String(platform.autonomy().ceiling().as_str().to_string()),
    );
    fields.insert(
        "live_capable".to_string(),
        platform.is_live_capable().into(),
    );
    fields.insert("reconciliation_breaks".to_string(), breaks.into());
    fields.insert("cycles".to_string(), platform.cycle_count().into());
    fields.insert(
        "events_logged".to_string(),
        platform.event_log().len().into(),
    );
    fields.insert(
        "chain_intact".to_string(),
        platform.event_log().verify_chain().is_ok().into(),
    );
    fields.insert("cells_reporting".to_string(), observations.len().into());
    // Counted rather than implied by the reporting count: a cell that has gone
    // quiet still contributes its last book to the aggregate, and a dashboard
    // that cannot see the staleness renders an hour-old position as current.
    fields.insert("cells_stale".to_string(), stale.into());
    serde_json::Value::Object(fields)
}

// --- the connection ---------------------------------------------------------

/// The bounds one streamed connection runs under.
///
/// Every field is a way a live connection would otherwise consume something
/// without limit: a thread that never returns, a socket that a proxy closes
/// while both ends believe it is open, a poll that copies the whole log into
/// one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamLimits {
    /// How long to wait between polls of the source when it is quiet.
    pub poll_interval: StdDuration,
    /// How long the connection may be silent before a heartbeat comment goes
    /// out.
    ///
    /// Well under the idle timeout of any reverse proxy or load balancer worth
    /// deploying — sixty seconds is the common default and thirty is not rare —
    /// because the failure it prevents is a proxy quietly closing a stream that
    /// both ends still believe is open, which a client experiences as a feed
    /// that stopped without an error.
    pub heartbeat_after: StdDuration,
    /// The total lifetime of one connection.
    ///
    /// A stream with no end is a thread, a socket and a slot against
    /// [`crate::http::ServerLimits::max_concurrent`] held for as long as the
    /// process runs. Ending deliberately, with the cursor to resume from, costs
    /// a client one reconnect and bounds the server's exposure to a client that
    /// connects and is never heard from again.
    pub max_duration: StdDuration,
    /// The most events one poll may turn into frames.
    pub max_events_per_poll: usize,
    /// How many retained events a client with no `Last-Event-ID` is given for
    /// context before the stream goes live.
    pub backlog: usize,
    /// The reconnect delay suggested to the client, in milliseconds.
    pub retry_after_millis: u64,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            poll_interval: StdDuration::from_millis(250),
            heartbeat_after: StdDuration::from_secs(10),
            max_duration: StdDuration::from_secs(300),
            max_events_per_poll: 256,
            backlog: 64,
            retry_after_millis: 2_000,
        }
    }
}

/// One live connection.
///
/// Holds the cursor into the source and the delivery count for this client,
/// and nothing else: the socket belongs to [`crate::http::pump`] and the
/// events belong to the source.
pub struct EventStream {
    kind: StreamKind,
    source: Box<dyn EventSource>,
    limits: StreamLimits,
    clock: Arc<dyn Clock>,
    cursor: u64,
    delivered: u64,
    opened_at: Instant,
    last_frame_at: Instant,
    pending: VecDeque<String>,
    /// The opening comment, taken once.
    preamble: Option<String>,
    /// Set once the closing event has been handed over, so the next poll ends
    /// the stream rather than writing a second one.
    closed: bool,
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream")
            .field("stream", &self.kind.name())
            .field("cursor", &self.cursor)
            .field("delivered", &self.delivered)
            .finish_non_exhaustive()
    }
}

impl EventStream {
    /// Open a stream, resolving where it starts from `last_event_id`.
    pub fn open(
        kind: StreamKind,
        mut source: Box<dyn EventSource>,
        clock: Arc<dyn Clock>,
        limits: StreamLimits,
        last_event_id: Option<&str>,
    ) -> Self {
        // A malformed Last-Event-ID is treated as absent rather than refused.
        // The header is written by a browser replaying what the server sent
        // it, so a value that will not parse means something rewrote it in
        // transit; starting at the live edge loses history, and refusing the
        // connection loses the stream.
        let after = last_event_id.and_then(|raw| raw.trim().parse::<u64>().ok());
        let resume = source.resume(after);
        let now = Instant::now();
        let preamble = format!(
            "{}retry: {}\n\n",
            comment(&format!(
                "qip {} stream open at cursor {} — {}",
                kind.name(),
                resume.cursor,
                resume.note
            ))
            .trim_end_matches('\n'),
            limits.retry_after_millis
        );
        let mut stream = Self {
            kind,
            source,
            limits,
            clock,
            cursor: resume.cursor,
            delivered: 0,
            opened_at: now,
            last_frame_at: now,
            pending: VecDeque::new(),
            preamble: Some(preamble),
            closed: false,
        };
        // A resume that could not be honoured is an event, not a log line. A
        // client that is told in a comment has to parse comments to find out
        // it lost data; a client that is told in an event cannot miss it.
        if let Some(skipped) = resume.skipped.filter(|skipped| *skipped > 0) {
            stream.queue(stream.notice(
                "stream.gap",
                resume.cursor,
                &resume.note,
                Some(("skipped", skipped)),
            ));
        }
        stream
    }

    /// How many events this connection has delivered.
    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    /// The cursor this connection has reached.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// An event about the stream itself, rather than about the platform.
    fn notice(
        &self,
        event_type: &str,
        cursor: u64,
        detail: &str,
        extra: Option<(&str, u64)>,
    ) -> Emission {
        let now = self.clock.now();
        let mut payload = serde_json::Map::new();
        payload.insert(
            "detail".to_string(),
            serde_json::Value::String(detail.to_string()),
        );
        payload.insert(
            "stream".to_string(),
            serde_json::Value::String(self.kind.name().to_string()),
        );
        payload.insert("resume_from".to_string(), cursor.into());
        if let Some((name, value)) = extra {
            payload.insert(name.to_string(), value.into());
        }
        Emission {
            cursor,
            event_type: event_type.to_string(),
            occurred_at: now,
            ingested_at: now,
            correlation_id: format!("{}-stream-{}", self.kind.name(), cursor),
            payload: serde_json::Value::Object(payload),
        }
    }

    /// Number an emission for this connection and queue its frame.
    fn queue(&mut self, emission: Emission) {
        self.delivered += 1;
        let event = SseEvent {
            stream: self.kind.name(),
            sequence: self.delivered,
            emission,
        };
        self.pending.push_back(event.frame());
    }

    fn heartbeat(&self) -> String {
        comment(&format!(
            "heartbeat {} stream={} cursor={} delivered={}",
            self.clock.now().to_rfc3339(),
            self.kind.name(),
            self.cursor,
            self.delivered
        ))
    }
}

impl ResponseStream for EventStream {
    fn headers(&self) -> Vec<(String, String)> {
        vec![
            (
                "content-type".to_string(),
                "text/event-stream; charset=utf-8".to_string(),
            ),
            // `no-cache` rather than the `no-store` every other response
            // carries: an event stream that a proxy serves from a cache is a
            // client watching a recording of a market.
            ("cache-control".to_string(), "no-cache".to_string()),
            // Read by proxies as "this connection is not idle, do not close
            // it". The body is delimited by the close of the connection rather
            // than reused for a second request — this server handles one
            // request per connection — but the header is what keeps an
            // intermediary from tearing the stream down between events, and it
            // is what every SSE client and proxy is written to expect.
            ("connection".to_string(), "keep-alive".to_string()),
            // nginx buffers a proxied response by default, which for a stream
            // means events arriving in batches whenever the buffer fills.
            // This is the documented opt-out and is inert everywhere else.
            ("x-accel-buffering".to_string(), "no".to_string()),
            ("x-qip-stream".to_string(), self.kind.name().to_string()),
            (
                "x-qip-stream-replays".to_string(),
                self.kind.replays().to_string(),
            ),
        ]
    }

    fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.closed {
            return None;
        }
        if let Some(preamble) = self.preamble.take() {
            self.last_frame_at = Instant::now();
            return Some(preamble.into_bytes());
        }
        loop {
            if let Some(frame) = self.pending.pop_front() {
                self.last_frame_at = Instant::now();
                return Some(frame.into_bytes());
            }
            if self.opened_at.elapsed() >= self.limits.max_duration {
                self.closed = true;
                let cursor = self.cursor;
                let notice = self.notice(
                    "stream.closing",
                    cursor,
                    "this connection has reached its lifetime bound. Reconnect with \
                     Last-Event-ID set to this cursor to continue where it stopped.",
                    None,
                );
                self.queue(notice);
                return self.pending.pop_front().map(String::into_bytes);
            }
            match self
                .source
                .since(self.cursor, self.limits.max_events_per_poll)
            {
                Poll::Events(emissions) if !emissions.is_empty() => {
                    for emission in emissions {
                        self.cursor = emission.cursor;
                        self.queue(emission);
                    }
                }
                Poll::Events(_) => {
                    if self.last_frame_at.elapsed() >= self.limits.heartbeat_after {
                        self.last_frame_at = Instant::now();
                        return Some(self.heartbeat().into_bytes());
                    }
                    // Sleeping is what makes this a blocking stream rather than
                    // a spin. Bounded by the heartbeat above, so the loop
                    // always returns to the writer — which is the only place a
                    // departed client is detected.
                    std::thread::sleep(self.limits.poll_interval);
                }
                Poll::Faulted(reason) => {
                    self.closed = true;
                    let cursor = self.cursor;
                    let notice = self.notice("stream.fault", cursor, &reason, None);
                    self.queue(notice);
                    return self.pending.pop_front().map(String::into_bytes);
                }
            }
        }
    }
}
