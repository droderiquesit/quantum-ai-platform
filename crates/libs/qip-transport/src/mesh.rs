//! The mesh transport: envelopes between the cells and the central plane.
//!
//! This is the arrow in the architecture diagram. Nine regional cells publish
//! state deltas up; the central plane publishes signed capital envelopes back
//! down; both directions go through the types in this module, over the HTTP/1.1
//! client in [`crate::http`].
//!
//! # What travels
//!
//! A [`qip_events::AnyEvent`] — the platform's own type-erased event, with its
//! identity, topic, both timestamps, lineage, idempotency key and payload hash
//! already on it. Nothing here invents a second envelope format.
//! `qip_streaming::StreamEnvelope::to_frame` and `from_frame` convert in both
//! directions, so a stream envelope goes over this transport without losing
//! anything and arrives verifiable: `from_frame` recomputes the payload hash
//! and refuses a frame that was edited in transit.
//!
//! # The protocol
//!
//! Three endpoints, JSON over HTTP/1.1, one connection per request:
//!
//! | route | what it does |
//! |---|---|
//! | `POST /v1/mesh/publish` | takes a [`MeshBatch`], answers a [`PublishAck`] |
//! | `POST /v1/mesh/poll` | takes a [`PollRequest`], answers a [`PollResponse`] |
//! | `GET /v1/mesh/health` | answers the inbox's depth and capacity |
//!
//! Timestamps on the wire are raw nanoseconds rather than
//! [`qip_core::Timestamp`]'s RFC 3339 rendering, which is millisecond
//! precision. A poll boundary that silently rounded to the millisecond would
//! drop or double-deliver everything inside that millisecond.
//!
//! # Delivery: at-least-once, and **not** exactly-once
//!
//! **Every consumer of this transport must be idempotent.** That is not a
//! recommendation; it is the precondition for using it at all.
//!
//! The reason is the oldest one in distributed systems and it has no fix at
//! this layer. A publisher sends a message, the peer accepts it, and the
//! acknowledgement is lost on the way back. The publisher cannot tell that
//! apart from the message never arriving, so it retries, and the peer sees the
//! message twice. Choosing not to retry does not make it exactly-once; it makes
//! it at-most-once, which for a capital envelope is worse.
//!
//! What this transport does instead is make the duplicate **detectable**. Every
//! message carries [`MeshMessage::key`], which is `AnyEvent::dedup_key` — the
//! body's own idempotency key where it has one, otherwise the topic and the
//! payload hash. The receiving [`MeshInbox`] remembers the keys it has seen and
//! reports a repeat in [`PublishAck::duplicates`] rather than queueing it
//! twice. That memory is bounded (see [`MeshInbox::new`]): beyond the window, a
//! duplicate is delivered again and the consumer's own idempotency is what
//! saves it. Which is why the consumer has to have some.
//!
//! Pulls are at-least-once for the same reason, in the other direction.
//! [`PollRequest::acknowledged`] carries the position the subscriber has
//! already taken; the inbox discards up to it and returns what is after it,
//! *without* removing it. A poll whose response is lost is simply repeated, and
//! nothing is lost — at the cost of the frames being delivered twice. An inbox
//! that deleted on read would turn one dropped response into permanently
//! missing state deltas.
//!
//! # Ordering: per-publisher FIFO, and nothing more
//!
//! **What is guaranteed.** One [`MeshPublisher`] sends one message at a time,
//! in the order it was enqueued, and does not begin the next until the current
//! one has been delivered or dead-lettered. A retry therefore blocks the
//! messages behind it — head-of-line blocking, deliberately — so the peer sees
//! that publisher's messages in enqueue order, with a hole wherever one was
//! dead-lettered.
//!
//! **What is not.** There is no global order. Two publishers — two pods, two
//! threads, a cell and the central plane — have no ordering relative to each
//! other, and the inbox interleaves them by arrival. There is no partition key
//! and therefore no per-key ordering across publishers. A consumer that needs
//! order beyond one publisher's stream must get it from the data:
//! `qip_streaming::SequenceCoordinator` and the origin sequence numbers on the
//! envelopes exist for exactly that, and they work regardless of what the wire
//! did.
//!
//! # Backpressure runs end to end
//!
//! A full [`MeshInbox`] answers `503`. The publisher treats that as transient,
//! backs off and retries, which means its own outbound queue stops draining and
//! fills. A full [`crate::OutboundQueue`] refuses with
//! [`crate::TransportError::QueueFull`]. So a slow consumer becomes a refusal
//! at the producer, synchronously, at the call site — rather than memory growth
//! somewhere in between.
//!
//! # A market tick may not travel this way
//!
//! [`MeshPublisher::enqueue`] refuses a frame whose topic is both
//! latency-critical and lossy-tolerable — the market-tick class. The reasoning
//! is `qip_streaming::routing`'s: a network hop with a retry ladder and a
//! dead-letter path in front of a decision that is measured in microseconds is
//! the latency the edge exists to avoid, and the tick is replaceable within
//! milliseconds anyway. This refusal reads the topic alone, so it is slightly
//! stricter than `RoutingClass::derive`, which also asks whether the source is
//! venue-critical. Stricter is the right direction: a tick from a non-venue
//! source has no more business crossing a WAN than one from a venue.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use qip_contracts::time::Watermark;
use qip_core::rng::Xoshiro256;
use qip_core::{Clock, Error, Result, Timestamp};
use qip_events::AnyEvent;
use serde::{Deserialize, Serialize};

use crate::deadletter::{DeadLetter, DeadLetterReason, DeadLetterSink};
use crate::error::TransportError;
use crate::http::{ClientLimits, HttpClient, HttpRequest, Method, Url};
use crate::queue::{OutboundQueue, QueueStats};
use crate::retry::{RetryPolicy, Sleeper};

/// Where a publisher posts.
pub const PUBLISH_PATH: &str = "/v1/mesh/publish";
/// Where a subscriber pulls.
pub const POLL_PATH: &str = "/v1/mesh/poll";
/// Where a probe looks.
pub const HEALTH_PATH: &str = "/v1/mesh/health";

// --- the wire -----------------------------------------------------------

/// One message on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshMessage {
    /// `AnyEvent::dedup_key`. Sent explicitly so the receiver can detect a
    /// redelivery without having to reconstruct it, and so the key a duplicate
    /// is reported under is unambiguously the one the sender used.
    pub key: String,
    /// Which attempt this is, 1-based. Carried so the receiver's logs can say
    /// "this arrived on the sender's third try", which is the difference
    /// between a duplicate caused by a retry and one caused by a bug upstream.
    pub attempt: u32,
    pub frame: AnyEvent,
}

/// A batch of messages from one publisher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshBatch {
    /// The publisher's name, for the receiver's logs. Not an identity claim:
    /// there is no authentication on this transport. See the crate docs.
    pub sender: String,
    pub messages: Vec<MeshMessage>,
}

/// What the receiver did with a batch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishAck {
    /// How many were queued.
    pub accepted: usize,
    /// The keys that had been seen before and were therefore not queued again.
    pub duplicates: Vec<String>,
    /// The inbox position of the last message accepted.
    pub position: u64,
    /// The inbox's depth after this batch, so a publisher can see the peer
    /// filling up before it starts being refused.
    pub depth: usize,
}

/// A pull, on the caller's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollRequest {
    /// Known-time upper bound, in nanoseconds since the epoch. Frames whose
    /// `recorded_at` is after this are not returned, however long they have
    /// been sitting in the inbox.
    pub until_nanos: i64,
    /// The highest position this subscriber has already taken. Everything at
    /// or below it is discarded by the inbox; everything above it is returned.
    pub acknowledged: u64,
}

/// One frame in the inbox, with the position the inbox assigned it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InboxFrame {
    pub position: u64,
    pub frame: AnyEvent,
}

/// What a pull returned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PollResponse {
    pub frames: Vec<InboxFrame>,
    /// How much is still held past what was returned — either because it is
    /// stamped after `until`, or because the response was cut at the batch
    /// limit. A hint for a scheduler, never a promise.
    pub remaining: usize,
    /// The highest position the inbox has ever assigned.
    pub highest: u64,
}

/// What a health probe sees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxHealth {
    pub depth: usize,
    pub capacity: usize,
    pub accepted: u64,
    pub duplicates: u64,
    pub refused: u64,
}

// --- the receiving side -------------------------------------------------

/// Counters for one inbox.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxStats {
    /// Messages queued.
    pub accepted: u64,
    /// Messages recognised as redeliveries and not queued again.
    pub duplicates: u64,
    /// Messages refused because the inbox was full. Non-zero means this
    /// consumer is slower than its publishers, and the publishers were told so.
    pub refused: u64,
    /// Frames handed to a subscriber, counting redeliveries.
    pub served: u64,
    /// Frames discarded because a subscriber acknowledged past them.
    pub acknowledged: u64,
    /// The most ever held at once.
    pub peak_depth: usize,
    /// Idempotency keys forgotten because the dedup window filled. Beyond this
    /// point a redelivery is no longer detectable here and the consumer's own
    /// idempotency is the only thing left.
    pub forgotten_keys: u64,
}

/// What happened to one accepted message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Queued, at this position.
    Accepted(u64),
    /// Seen before, within the dedup window. Not queued again.
    Duplicate,
    /// The inbox is full. The publisher is told, and backs off.
    Full,
}

#[derive(Debug)]
struct InboxState {
    capacity: usize,
    frames: VecDeque<InboxFrame>,
    seen_order: VecDeque<String>,
    seen: BTreeSet<String>,
    seen_capacity: usize,
    position: u64,
    stats: InboxStats,
}

/// The receiving buffer, shared between whatever serves HTTP and whatever
/// consumes.
///
/// Cheap to clone: a clone is another handle on the same inbox, which is how
/// the endpoint and the consumer see the same messages.
#[derive(Clone, Debug)]
pub struct MeshInbox {
    name: String,
    state: Arc<Mutex<InboxState>>,
}

impl MeshInbox {
    /// An inbox holding at most `capacity` frames and remembering at most
    /// `dedup_window` idempotency keys.
    ///
    /// Both bounds are refusals of an unbounded allocation, and the dedup
    /// window is the more interesting one. It is what makes duplicate detection
    /// best-effort rather than absolute: a redelivery arriving after
    /// `dedup_window` newer keys have been seen is not recognised. Sizing it
    /// is a judgement about how far behind a retrying publisher can fall, and
    /// getting it wrong degrades to "the consumer sees the message twice" —
    /// which is what the consumer must already tolerate.
    pub fn new(name: impl Into<String>, capacity: usize, dedup_window: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an inbox with zero capacity refuses every message, which presents as a peer that \
                 never delivers rather than as a misconfiguration",
            ));
        }
        Ok(Self {
            name: name.into(),
            state: Arc::new(Mutex::new(InboxState {
                capacity,
                frames: VecDeque::new(),
                seen_order: VecDeque::new(),
                seen: BTreeSet::new(),
                seen_capacity: dedup_window.max(1),
                position: 0,
                stats: InboxStats::default(),
            })),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, InboxState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Offer one message.
    pub fn accept(&self, message: &MeshMessage) -> Admission {
        let mut state = self.locked();
        if state.seen.contains(&message.key) {
            state.stats.duplicates += 1;
            return Admission::Duplicate;
        }
        if state.frames.len() >= state.capacity {
            state.stats.refused += 1;
            return Admission::Full;
        }

        state.position += 1;
        let position = state.position;
        state.frames.push_back(InboxFrame {
            position,
            frame: message.frame.clone(),
        });
        state.seen.insert(message.key.clone());
        state.seen_order.push_back(message.key.clone());
        while state.seen_order.len() > state.seen_capacity {
            if let Some(oldest) = state.seen_order.pop_front() {
                state.seen.remove(&oldest);
                state.stats.forgotten_keys += 1;
            }
        }
        state.stats.accepted += 1;
        state.stats.peak_depth = state.stats.peak_depth.max(state.frames.len());
        Admission::Accepted(position)
    }

    /// Discard everything at or below `acknowledged`, then return what is
    /// above it and stamped at or before `until`.
    ///
    /// Stops at the first frame past `until` rather than skipping it, so the
    /// positions a subscriber sees stay contiguous and a frame that arrived
    /// late but is stamped early is picked up on a later poll instead of being
    /// jumped over — the same rule `qip_streaming::durable` follows, for the
    /// same reason.
    pub fn read(&self, acknowledged: u64, until: Timestamp, limit: usize) -> PollResponse {
        let mut state = self.locked();
        while state
            .frames
            .front()
            .is_some_and(|entry| entry.position <= acknowledged)
        {
            state.frames.pop_front();
            state.stats.acknowledged += 1;
        }

        let mut frames = Vec::new();
        for entry in &state.frames {
            if frames.len() >= limit {
                break;
            }
            if entry.frame.recorded_at > until {
                break;
            }
            frames.push(entry.clone());
        }
        state.stats.served += frames.len() as u64;
        let remaining = state.frames.len() - frames.len();
        let highest = state.position;
        PollResponse {
            frames,
            remaining,
            highest,
        }
    }

    /// Whether anything is waiting at or past `acknowledged` and stamped at or
    /// before `until`, without taking it.
    pub fn has_pending(&self, acknowledged: u64, until: Timestamp) -> bool {
        self.locked()
            .frames
            .iter()
            .any(|entry| entry.position > acknowledged && entry.frame.recorded_at <= until)
    }

    pub fn depth(&self) -> usize {
        self.locked().frames.len()
    }

    pub fn capacity(&self) -> usize {
        self.locked().capacity
    }

    pub fn stats(&self) -> InboxStats {
        self.locked().stats
    }

    pub fn health(&self) -> InboxHealth {
        let state = self.locked();
        InboxHealth {
            depth: state.frames.len(),
            capacity: state.capacity,
            accepted: state.stats.accepted,
            duplicates: state.stats.duplicates,
            refused: state.stats.refused,
        }
    }
}

/// One HTTP response from the endpoint.
///
/// Deliberately not tied to a server type. `qip_api::http::Server` already
/// exists and this crate is below it, so the endpoint answers in terms of a
/// status and a body and whatever is serving HTTP writes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl EndpointResponse {
    /// Every response from this endpoint is JSON.
    pub const CONTENT_TYPE: &'static str = "application/json; charset=utf-8";

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            body: body.into_bytes(),
        }
    }

    fn error(status: u16, detail: &str) -> Self {
        let escaped = detail.replace('\\', "\\\\").replace('"', "\\\"");
        Self::json(status, format!(r#"{{"error":"{escaped}"}}"#))
    }

    pub fn body_as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

/// The receiving half: turns mesh requests into inbox operations.
///
/// The maximum batch a poll returns is bounded here rather than by the caller,
/// because the caller is the peer and a peer that asked for a million frames at
/// once would be choosing this process's memory for it.
#[derive(Clone, Debug)]
pub struct MeshEndpoint {
    inbox: MeshInbox,
    max_poll_batch: usize,
}

impl MeshEndpoint {
    pub fn new(inbox: MeshInbox) -> Self {
        Self {
            inbox,
            max_poll_batch: 512,
        }
    }

    pub fn with_max_poll_batch(mut self, limit: usize) -> Self {
        self.max_poll_batch = limit.max(1);
        self
    }

    pub fn inbox(&self) -> &MeshInbox {
        &self.inbox
    }

    /// Handle one request. `target` is the request target, path and query.
    pub fn handle(&self, method: Method, target: &str, body: &[u8]) -> EndpointResponse {
        let path = target.split(['?', '#']).next().unwrap_or(target);
        match path {
            PUBLISH_PATH => {
                if method != Method::Post {
                    return EndpointResponse::error(405, "publish accepts POST");
                }
                self.publish(body)
            }
            POLL_PATH => {
                if method != Method::Post {
                    return EndpointResponse::error(
                        405,
                        "poll accepts POST: the request carries the caller's clock and its \
                         acknowledged position, which do not belong in a URL",
                    );
                }
                self.poll(body)
            }
            HEALTH_PATH => {
                if method != Method::Get {
                    return EndpointResponse::error(405, "health accepts GET");
                }
                match serde_json::to_string(&self.inbox.health()) {
                    Ok(rendered) => EndpointResponse::json(200, rendered),
                    Err(error) => EndpointResponse::error(500, &error.to_string()),
                }
            }
            other => EndpointResponse::error(404, &format!("{other} is not a mesh endpoint")),
        }
    }

    fn publish(&self, body: &[u8]) -> EndpointResponse {
        let batch: MeshBatch = match serde_json::from_slice(body) {
            Ok(batch) => batch,
            Err(error) => {
                return EndpointResponse::error(400, &format!("unreadable batch: {error}"));
            }
        };

        let mut ack = PublishAck::default();
        for message in &batch.messages {
            // The hash is checked here rather than trusted, because a frame
            // whose payload no longer matches its hash is either corrupt in
            // transit or edited on purpose, and this endpoint has no
            // authentication that would let it tell those apart.
            if qip_core::hash::sha256_hex(
                qip_events::envelope::canonical_json(&message.frame.payload).as_bytes(),
            ) != message.frame.payload_hash
            {
                return EndpointResponse::error(
                    422,
                    &format!(
                        "the payload hash on {} does not match its payload",
                        message.frame.event_id
                    ),
                );
            }
            match self.inbox.accept(message) {
                Admission::Accepted(position) => {
                    ack.accepted += 1;
                    ack.position = position;
                }
                Admission::Duplicate => ack.duplicates.push(message.key.clone()),
                Admission::Full => {
                    // 503 rather than 429: the inbox is not rate limiting, it
                    // is out of room, and the publisher's correct response is
                    // to back off and try the same message again.
                    return EndpointResponse::error(
                        503,
                        "the inbox is full: this consumer is behind its publishers",
                    );
                }
            }
        }
        ack.depth = self.inbox.depth();
        match serde_json::to_string(&ack) {
            Ok(rendered) => EndpointResponse::json(200, rendered),
            Err(error) => EndpointResponse::error(500, &error.to_string()),
        }
    }

    fn poll(&self, body: &[u8]) -> EndpointResponse {
        let request: PollRequest = match serde_json::from_slice(body) {
            Ok(request) => request,
            Err(error) => {
                // No default `until`. The caller owns the clock, and an
                // endpoint that supplied one would be handing back events
                // against a time nobody chose — the exact ambient-clock
                // dependency `qip_streaming::ports` exists to prevent.
                return EndpointResponse::error(
                    400,
                    &format!("a poll must state its own until and acknowledged position: {error}"),
                );
            }
        };
        let response = self.inbox.read(
            request.acknowledged,
            Timestamp::from_nanos(request.until_nanos),
            self.max_poll_batch,
        );
        match serde_json::to_string(&response) {
            Ok(rendered) => EndpointResponse::json(200, rendered),
            Err(error) => EndpointResponse::error(500, &error.to_string()),
        }
    }
}

// --- the sending side ---------------------------------------------------

/// What a publisher is and what a deployment must still supply for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshDescriptor {
    pub name: String,
    pub peer: String,
    /// Whether a message survives this process. It does not: the outbound
    /// queue and the dead-letter sink are in memory unless a deployment wires
    /// a durable sink.
    pub durable: bool,
    /// Whether this transport works in this build. It does.
    pub available: bool,
    /// What production must add. Never `None` here — see [`MeshPublisher::REQUIREMENTS`].
    pub production_requirement: Option<String>,
}

/// Counters for one publisher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherStats {
    pub enqueued: u64,
    pub delivered: u64,
    /// Deliveries the peer reported as redeliveries. Non-zero is normal and is
    /// the at-least-once guarantee being visible rather than a fault.
    pub duplicates: u64,
    /// Sends attempted, including retries.
    pub attempts: u64,
    /// Sends that failed and were retried.
    pub retries: u64,
    pub dead_lettered: u64,
}

/// One delivered message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub transport: String,
    pub peer: String,
    pub key: String,
    /// This publisher's own send sequence, assigned at enqueue.
    pub position: u64,
    /// The position the peer's inbox assigned, where it accepted the message.
    /// `None` when the peer reported it as a duplicate and therefore did not
    /// queue it again.
    pub peer_position: Option<u64>,
    /// The caller's clock, as handed to the publish call.
    pub accepted_at: Timestamp,
    /// How many sends it took. `1` means it went first time.
    pub attempts: u32,
    /// Whether the peer had already seen this key.
    pub duplicate: bool,
}

/// One message that was not delivered, as the flush saw it.
///
/// The same facts as the [`DeadLetter`] recorded in the sink, minus the frame:
/// a caller deciding what to do next needs to know which message and why, and
/// handing it back the payload it just supplied would only invite a second
/// copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLettered {
    /// This publisher's send sequence, so a caller can match it to what it
    /// enqueued even when two messages share an idempotency key.
    pub position: u64,
    pub key: String,
    pub attempts: u32,
    pub reason: DeadLetterReason,
    pub last_error: String,
}

/// What one flush did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub delivered: Vec<Delivery>,
    /// Everything that ended in the dead-letter sink. Each one is there with
    /// its frame and its reason.
    pub dead_lettered: Vec<DeadLettered>,
}

impl FlushReport {
    pub fn is_clean(&self) -> bool {
        self.dead_lettered.is_empty()
    }
}

/// How to build a publisher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshConfig {
    /// Stable transport name, recorded on every delivery and dead letter.
    pub name: String,
    /// The peer's base URL, `http://host:port`.
    pub peer: String,
    /// How many messages may wait to be sent before publishes are refused.
    pub queue_capacity: usize,
    pub limits: ClientLimits,
    pub retry: RetryPolicy,
    /// Seed for the backoff jitter. Two publishers with different seeds spread;
    /// the same seed reproduces a run exactly.
    pub seed: u64,
}

impl MeshConfig {
    pub fn new(name: impl Into<String>, peer: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            peer: peer.into(),
            queue_capacity: 1024,
            limits: ClientLimits::default(),
            retry: RetryPolicy::default(),
            seed: 0,
        }
    }

    pub fn with_queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = capacity;
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_limits(mut self, limits: ClientLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
}

#[derive(Clone, Debug)]
struct Pending {
    key: String,
    position: u64,
    frame: AnyEvent,
}

/// The sending half.
///
/// Synchronous and single-threaded by construction: `&mut self` on every
/// mutating method means one publisher is one ordered stream, which is the
/// mechanism behind the FIFO guarantee in the module docs. Concurrency is
/// obtained by having more than one publisher, and more than one publisher is
/// exactly where the ordering guarantee stops.
#[derive(Debug)]
pub struct MeshPublisher {
    name: String,
    peer: Url,
    publish_url: String,
    client: HttpClient,
    retry: RetryPolicy,
    queue: OutboundQueue<Pending>,
    dead_letters: Box<dyn DeadLetterSink>,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    rng: Xoshiro256,
    position: u64,
    stats: PublisherStats,
}

impl MeshPublisher {
    /// What this transport does not provide and a production deployment must.
    pub const REQUIREMENTS: [&'static str; 4] = [
        "mutual TLS between peers, from a service mesh sidecar or an ingress that terminates it: \
         this transport speaks plaintext HTTP and authenticates nobody",
        "a NetworkPolicy that denies egress by default and permits only the mesh peers by name, \
         since without authentication the network is the whole access control",
        "wiring `spool::DurableSpool` and `spool::DurableDeadLetters` over a persistent \
         KeyValueStore, because this publisher's own queue and default sink are in memory and \
         a pod restart loses both: the durable versions exist, and using them is a deployment \
         decision this type cannot make for itself",
        "an alert on the dead-letter count and on the outbound queue's refusal count, which are \
         the two numbers that say a delta never arrived",
    ];

    /// Build a publisher.
    ///
    /// The clock, the sleeper and the dead-letter sink are injected rather than
    /// constructed here: they are the three places this transport would
    /// otherwise reach for something ambient, and each of them is what a test
    /// has to replace to assert on the retry ladder at all.
    pub fn new(
        config: MeshConfig,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
        dead_letters: Box<dyn DeadLetterSink>,
    ) -> Result<Self> {
        config.retry.validate()?;
        let peer = Url::parse(&config.peer).map_err(qip_core::Error::from)?;
        let publish_url = peer
            .with_path(PUBLISH_PATH)
            .map_err(qip_core::Error::from)?
            .to_string();
        Ok(Self {
            queue: OutboundQueue::new(config.name.clone(), config.queue_capacity)?,
            name: config.name,
            peer,
            publish_url,
            client: HttpClient::new(config.limits),
            retry: config.retry,
            dead_letters,
            clock,
            sleeper,
            rng: Xoshiro256::seeded(config.seed),
            position: 0,
            stats: PublisherStats::default(),
        })
    }

    pub fn descriptor(&self) -> MeshDescriptor {
        MeshDescriptor {
            name: self.name.clone(),
            peer: self.peer.to_string(),
            durable: false,
            available: true,
            production_requirement: Some(Self::REQUIREMENTS.join("; ")),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer(&self) -> &Url {
        &self.peer
    }

    pub fn stats(&self) -> PublisherStats {
        self.stats
    }

    pub fn queue_stats(&self) -> QueueStats {
        self.queue.stats()
    }

    pub fn queue_depth(&self) -> usize {
        self.queue.depth()
    }

    pub fn dead_letters(&self) -> &dyn DeadLetterSink {
        self.dead_letters.as_ref()
    }

    /// Why this frame may not travel the mesh, if it may not.
    ///
    /// Public so a router can ask before it builds a batch, rather than
    /// discovering it one refused enqueue at a time.
    pub fn refusal(frame: &AnyEvent) -> Option<String> {
        if frame.topic.group().is_latency_critical() && frame.topic.is_lossy_tolerable() {
            return Some(format!(
                "{} is replaceable within milliseconds and sits on the venue-critical path: a \
                 network hop with a retry ladder in front of it spends the latency budget the \
                 edge exists to protect",
                frame.topic.name()
            ));
        }
        None
    }

    /// Admit one frame to the outbound queue, returning the position it was
    /// given. Sends nothing.
    pub fn enqueue(&mut self, frame: AnyEvent) -> std::result::Result<u64, TransportError> {
        if let Some(detail) = Self::refusal(&frame) {
            return Err(TransportError::Inadmissible { detail });
        }
        let key = frame.dedup_key();
        let position = self.position + 1;
        self.queue.push(Pending {
            key,
            position,
            frame,
        })?;
        self.position = position;
        self.stats.enqueued += 1;
        Ok(position)
    }

    /// Send everything queued, in order, until the queue is empty.
    ///
    /// Every message ends in one of two places: delivered, or in the
    /// dead-letter sink. Nothing is dropped and nothing is left behind, so the
    /// queue is empty when this returns.
    pub fn flush(&mut self, at: Timestamp) -> FlushReport {
        let mut report = FlushReport::default();
        while let Some(pending) = self.queue.pop() {
            match self.deliver(&pending, at) {
                Ok(delivery) => report.delivered.push(delivery),
                Err(letter) => report.dead_lettered.push(letter),
            }
        }
        report
    }

    /// Enqueue one frame and send everything queued.
    ///
    /// `at` is the caller's clock, exactly as `qip_streaming::Publisher` hands
    /// one: it stamps the receipt. The publisher's own injected clock stamps
    /// the moments the caller did not name, which is the dead letters.
    pub fn publish_frame(
        &mut self,
        frame: AnyEvent,
        at: Timestamp,
    ) -> std::result::Result<Delivery, TransportError> {
        let position = self.enqueue(frame)?;
        let report = self.flush(at);
        match report
            .delivered
            .into_iter()
            .find(|delivery| delivery.position == position)
        {
            Some(delivery) => Ok(delivery),
            None => Err(Self::dead_letter_error(&report.dead_lettered, position)),
        }
    }

    /// Enqueue a whole batch or none of it, then send.
    ///
    /// All-or-nothing admission on purpose. `qip_streaming::Publisher`'s
    /// default batch implementation stops at the first failure and leaves the
    /// caller unable to say which half was taken; refusing the batch before
    /// anything is queued removes the question.
    pub fn publish_frames(
        &mut self,
        frames: Vec<AnyEvent>,
        at: Timestamp,
    ) -> std::result::Result<Vec<Delivery>, TransportError> {
        self.queue.check_capacity(frames.len())?;
        let mut positions = Vec::with_capacity(frames.len());
        for frame in frames {
            positions.push(self.enqueue(frame)?);
        }
        let report = self.flush(at);
        let mut delivered = Vec::with_capacity(positions.len());
        for position in positions {
            let found = report
                .delivered
                .iter()
                .find(|delivery| delivery.position == position);
            match found {
                Some(delivery) => delivered.push(delivery.clone()),
                None => {
                    return Err(Self::dead_letter_error(&report.dead_lettered, position));
                }
            }
        }
        Ok(delivered)
    }

    /// The error for a message that ended in the dead-letter sink.
    ///
    /// Built from the record rather than from the policy, so the error names
    /// the message's own idempotency key, the attempts actually made and the
    /// last thing the peer said — which is what somebody asking "why did that
    /// delta never arrive" is holding when they ask.
    fn dead_letter_error(letters: &[DeadLettered], position: u64) -> TransportError {
        match letters.iter().find(|letter| letter.position == position) {
            Some(letter) => TransportError::DeadLettered {
                key: letter.key.clone(),
                attempts: letter.attempts,
                reason: letter.reason,
                last_error: letter.last_error.clone(),
            },
            // Unreachable by construction — a flush leaves every message
            // either delivered or dead-lettered — but a panic here would turn
            // an accounting bug into a crash on the path that moves money.
            None => TransportError::Protocol {
                detail: format!(
                    "message {position} was neither delivered nor dead-lettered, which is an \
                     accounting bug in the flush loop"
                ),
            },
        }
    }

    /// Send one message, retrying inside the policy. `Err` describes the
    /// dead letter that was recorded for it.
    fn deliver(
        &mut self,
        pending: &Pending,
        at: Timestamp,
    ) -> std::result::Result<Delivery, DeadLettered> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let outcome = self.send_once(pending, attempt);
            self.stats.attempts += 1;

            let (reason, detail) = match outcome {
                SendOutcome::Delivered {
                    peer_position,
                    duplicate,
                } => {
                    self.stats.delivered += 1;
                    if duplicate {
                        self.stats.duplicates += 1;
                    }
                    return Ok(Delivery {
                        transport: self.name.clone(),
                        peer: self.peer.to_string(),
                        key: pending.key.clone(),
                        position: pending.position,
                        peer_position,
                        accepted_at: at,
                        attempts: attempt,
                        duplicate,
                    });
                }
                SendOutcome::Permanent(detail) => (DeadLetterReason::PermanentFailure, detail),
                SendOutcome::Rejected(detail) => (DeadLetterReason::Rejected, detail),
                SendOutcome::Transient(detail) => {
                    if self.retry.may_retry(attempt) {
                        self.stats.retries += 1;
                        let backoff = self.retry.backoff(attempt, &mut self.rng);
                        self.sleeper.sleep(backoff);
                        continue;
                    }
                    (DeadLetterReason::RetriesExhausted, detail)
                }
            };

            self.stats.dead_lettered += 1;
            self.dead_letters.record(DeadLetter {
                key: pending.key.clone(),
                transport: self.name.clone(),
                peer: self.peer.to_string(),
                reason,
                attempts: attempt,
                last_error: detail.clone(),
                recorded_at: self.clock.now(),
                frame: pending.frame.clone(),
            });
            return Err(DeadLettered {
                position: pending.position,
                key: pending.key.clone(),
                attempts: attempt,
                reason,
                last_error: detail,
            });
        }
    }

    /// One HTTP round trip, classified.
    fn send_once(&self, pending: &Pending, attempt: u32) -> SendOutcome {
        let batch = MeshBatch {
            sender: self.name.clone(),
            messages: vec![MeshMessage {
                key: pending.key.clone(),
                attempt,
                frame: pending.frame.clone(),
            }],
        };
        let body = match serde_json::to_vec(&batch) {
            Ok(body) => body,
            // A frame that will not serialise will not serialise on the
            // fourth attempt either.
            Err(error) => return SendOutcome::Permanent(format!("unserialisable frame: {error}")),
        };
        let request = match HttpRequest::json(Method::Post, &self.publish_url, body) {
            Ok(request) => request,
            Err(error) => return SendOutcome::Permanent(error.to_string()),
        };

        let response = match self.client.send(&request) {
            Ok(response) => response,
            Err(error) => {
                let detail = error.to_string();
                return if error.is_transient() {
                    SendOutcome::Transient(detail)
                } else {
                    SendOutcome::Permanent(detail)
                };
            }
        };

        if response.is_success() {
            return match serde_json::from_slice::<PublishAck>(&response.body) {
                Ok(ack) => {
                    let duplicate = ack.duplicates.iter().any(|key| key == &pending.key);
                    SendOutcome::Delivered {
                        peer_position: (!duplicate).then_some(ack.position),
                        duplicate,
                    }
                }
                // A 2xx whose body is not an ack means something that is not
                // the peer answered — a proxy, a wrong port. Retrying will
                // reach the same thing.
                Err(error) => SendOutcome::Permanent(format!(
                    "the peer answered {} with something that is not a publish ack: {error}",
                    response.status
                )),
            };
        }

        let detail = format!(
            "{} answered {}: {}",
            self.peer,
            response.status,
            response.body_excerpt()
        );
        match response.status {
            // The peer is there and busy, restarting, or behind a proxy that
            // has not found it yet.
            408 | 425 | 429 | 500 | 502 | 503 | 504 => SendOutcome::Transient(detail),
            // The peer read the message and refused it on its merits.
            409 | 413 | 422 => SendOutcome::Rejected(detail),
            _ => SendOutcome::Permanent(detail),
        }
    }
}

enum SendOutcome {
    Delivered {
        peer_position: Option<u64>,
        duplicate: bool,
    },
    Transient(String),
    Permanent(String),
    Rejected(String),
}

// --- the pulling side ---------------------------------------------------

/// Counters for one remote subscription.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriberStats {
    pub polls: u64,
    pub frames: u64,
    pub attempts: u64,
    pub retries: u64,
    pub failures: u64,
}

/// A subscription that pulls from a peer's inbox over HTTP.
///
/// The caller keeps the clock — `poll` takes an `until` and returns what was
/// known by then — which is `qip_streaming::ports`' rule, unchanged by the fact
/// that the buffer is now on another host.
#[derive(Debug)]
pub struct RemoteSubscriber {
    name: String,
    peer: Url,
    poll_url: String,
    client: HttpClient,
    retry: RetryPolicy,
    sleeper: Arc<dyn Sleeper>,
    rng: Xoshiro256,
    cursor: u64,
    cursor_at: Timestamp,
    drained: bool,
    remaining_hint: usize,
    stats: SubscriberStats,
}

impl RemoteSubscriber {
    pub fn new(config: MeshConfig, sleeper: Arc<dyn Sleeper>) -> Result<Self> {
        config.retry.validate()?;
        let peer = Url::parse(&config.peer).map_err(qip_core::Error::from)?;
        let poll_url = peer
            .with_path(POLL_PATH)
            .map_err(qip_core::Error::from)?
            .to_string();
        Ok(Self {
            name: config.name,
            peer,
            poll_url,
            client: HttpClient::new(config.limits),
            retry: config.retry,
            sleeper,
            rng: Xoshiro256::seeded(config.seed),
            cursor: 0,
            cursor_at: Timestamp::EPOCH,
            drained: false,
            remaining_hint: 0,
            stats: SubscriberStats::default(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn stats(&self) -> SubscriberStats {
        self.stats
    }

    /// How far this subscriber has been drained.
    ///
    /// `None` before the first successful poll: a zero watermark would be a
    /// claim that the subscription has been read up to position zero, and a
    /// consumer resuming against it would believe it had missed nothing.
    pub fn watermark(&self) -> Option<Watermark> {
        self.drained
            .then(|| Watermark::new(self.name.clone(), self.cursor, self.cursor_at))
    }

    /// What the peer said was still waiting at the last poll.
    ///
    /// A hint for a scheduler, and explicitly not a guarantee: it is one
    /// network round trip out of date the moment it is returned, and asking
    /// the peer afresh would be a poll. A caller that intends to consume
    /// should just poll.
    pub fn pending_hint(&self) -> usize {
        self.remaining_hint
    }

    /// Take everything the peer knew at or before `until`.
    ///
    /// Acknowledges the previous poll's frames as a side effect. A frame is
    /// therefore delivered again if this subscriber is rebuilt, or if a
    /// response is lost — at-least-once, as the module documentation says.
    pub fn poll(&mut self, until: Timestamp) -> std::result::Result<Vec<AnyEvent>, TransportError> {
        let request = PollRequest {
            until_nanos: until.as_nanos(),
            acknowledged: self.cursor,
        };
        let body = serde_json::to_vec(&request)?;
        self.stats.polls += 1;

        let mut attempt = 0u32;
        let response = loop {
            attempt += 1;
            self.stats.attempts += 1;
            let outcome = self.poll_once(&body);
            match outcome {
                Ok(response) => break response,
                Err(error) => {
                    let retryable = match &error {
                        TransportError::Http(http) => http.is_transient(),
                        TransportError::PeerRejected { status, .. } => {
                            matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
                        }
                        _ => false,
                    };
                    if retryable && self.retry.may_retry(attempt) {
                        self.stats.retries += 1;
                        let backoff = self.retry.backoff(attempt, &mut self.rng);
                        self.sleeper.sleep(backoff);
                        continue;
                    }
                    self.stats.failures += 1;
                    return Err(error);
                }
            }
        };

        self.remaining_hint = response.remaining;
        if let Some(last) = response.frames.last() {
            self.cursor = last.position;
            self.cursor_at = last.frame.recorded_at;
            self.drained = true;
        }
        self.stats.frames += response.frames.len() as u64;
        Ok(response
            .frames
            .into_iter()
            .map(|entry| entry.frame)
            .collect())
    }

    fn poll_once(&self, body: &[u8]) -> std::result::Result<PollResponse, TransportError> {
        let request = HttpRequest::json(Method::Post, &self.poll_url, body.to_vec())?;
        let response = self.client.send(&request)?;
        if !response.is_success() {
            return Err(TransportError::PeerRejected {
                status: response.status,
                peer: self.peer.to_string(),
                excerpt: response.body_excerpt(),
            });
        }
        Ok(serde_json::from_slice(&response.body)?)
    }
}
