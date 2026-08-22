//! The decoder contract every wire format is held to.
//!
//! Three rules make the difference between a decoder that can be trusted with a
//! book and one that cannot:
//!
//! 1. **A partial message consumes nothing.** [`Decoder::consumed`] reports how
//!    many leading bytes formed whole messages; the caller keeps the remainder
//!    and re-presents it. Emitting half a message and consuming it is how a book
//!    ends up with a level nobody sent.
//! 2. **A message that fails its own integrity check is refused, by name.** A
//!    silently accepted corrupt message is worse than a dropped one: the drop is
//!    visible as a sequence gap, the corruption is not visible at all.
//! 3. **A message the decoder does not understand is skipped with a record.**
//!    Aborting the batch would throw away the messages either side of it, which
//!    are perfectly good, to punish one the venue added last week.
//!
//! Decoding is a pure function of the bytes, the decoder's configured identity
//! and the timestamps handed in. There is no clock and no randomness in this
//! crate, so replaying a capture reproduces the messages down to their
//! identifiers.

use qip_contracts::{MarketMessage, Origin, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Hasher256, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many skip records a decoder keeps.
///
/// Bounded for the same reason the reorder buffer downstream is bounded: the
/// condition that produces skip records — a venue rolling out a message type
/// this build does not know — produces them at feed rate, and an unbounded log
/// of them is a memory leak triggered by exactly the event you wanted to
/// survive. The counters stay exact; only the examples are capped.
pub const MAX_RETAINED_SKIPS: usize = 64;

/// Why a decoder declined to turn some bytes into a market message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The message type is well-framed but this build has no mapping for it.
    UnknownMessageType { code: String },
    /// A known message type that carries no market fact — a session heartbeat,
    /// an order acknowledgement, an exchange-wide administrative event.
    NoMarketFact { code: String },
    /// The message names an instrument this feed is not configured to carry.
    UnmappedInstrument { symbol: String },
    /// The message references a resting order the decoder never saw added,
    /// which happens whenever a session starts mid-day.
    UnknownOrderReference { order_ref: u64 },
    /// A field was present but could not be interpreted.
    Malformed { detail: String },
}

impl SkipReason {
    /// A stable label for metrics and log grouping.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::UnknownMessageType { .. } => "unknown_message_type",
            Self::NoMarketFact { .. } => "no_market_fact",
            Self::UnmappedInstrument { .. } => "unmapped_instrument",
            Self::UnknownOrderReference { .. } => "unknown_order_reference",
            Self::Malformed { .. } => "malformed",
        }
    }

    /// Whether the skip means data was lost rather than merely ignored.
    ///
    /// An unmapped instrument or an administrative message costs nothing. A
    /// malformed field or an unknown order reference means a book somewhere is
    /// now missing an update, and the operator has to know the difference.
    pub const fn loses_information(&self) -> bool {
        matches!(
            self,
            Self::Malformed { .. }
                | Self::UnknownOrderReference { .. }
                | Self::UnknownMessageType { .. }
        )
    }
}

/// One skipped message, kept so the gap in the output can be explained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkipRecord {
    pub protocol: String,
    pub reason: SkipReason,
    /// Offset of the skipped message within the buffer it arrived in.
    pub offset: usize,
    /// The capture time of the batch the message arrived in.
    pub at: Timestamp,
}

/// What a decoder has done since it was created.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub messages_decoded: u64,
    pub messages_skipped: u64,
    pub bytes_consumed: u64,
    /// Frames refused outright for failing an integrity check.
    pub frames_refused: u64,
    /// The most recent [`MAX_RETAINED_SKIPS`] skips.
    pub recent_skips: Vec<SkipRecord>,
}

impl Diagnostics {
    pub fn record_skip(&mut self, record: SkipRecord) {
        self.messages_skipped += 1;
        if self.recent_skips.len() == MAX_RETAINED_SKIPS {
            self.recent_skips.remove(0);
        }
        self.recent_skips.push(record);
    }

    /// Whether any skip since construction lost information.
    pub fn lost_information(&self) -> bool {
        self.recent_skips
            .iter()
            .any(|skip| skip.reason.loses_information())
    }
}

/// Turns a venue's wire format into [`MarketMessage`] values.
///
/// Implementations are stateless with respect to buffering: `decode` reads whole
/// messages from the front of `bytes` and leaves any trailing partial message
/// for the caller to re-present. [`crate::framing::StreamAssembler`] does that
/// re-presentation once, so no individual decoder has to get it right.
pub trait Decoder: std::fmt::Debug {
    /// Decode every whole message at the front of `bytes`.
    ///
    /// `captured_at` is when this cell's hardware saw the packet, and becomes
    /// the messages' `capture_time`. It is a parameter because a decoder that
    /// read a clock would replay differently than it ran.
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>>;

    /// A stable name for the wire format, e.g. `fix.4.4`.
    fn protocol(&self) -> &str;

    /// Bytes of `bytes` consumed by the most recent [`Decoder::decode`] call.
    ///
    /// Always a whole number of messages. The caller must retain
    /// `bytes[consumed..]` and present it again with the next read, or it will
    /// lose the message that straddled the boundary.
    fn consumed(&self) -> usize;

    /// Counters and the recent skip log.
    fn diagnostics(&self) -> &Diagnostics;
}

/// Which partition an instrument's messages belong to.
///
/// [`Origin`] scopes a sequence number to a partition, and nothing downstream
/// looks at symbols — a book is built per partition. So the wire's symbol has to
/// be resolved to a partition here, at the only place that has ever seen the
/// symbol at all.
///
/// The default is to refuse unknown symbols. A feed carries thousands of
/// instruments and a cell subscribes to a handful; inventing a partition for an
/// instrument nobody asked for would spend memory and book state on it forever.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentPartitions {
    map: BTreeMap<String, u32>,
    /// Assign a partition to any symbol seen for the first time.
    ///
    /// For capture tooling and tests, where the point is to decode everything.
    /// Deterministic: partitions are handed out in the order symbols appear, so
    /// a replay of the same capture assigns the same numbers.
    auto_assign: bool,
    next_auto: u32,
}

impl InstrumentPartitions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand a partition to every symbol on first sight.
    pub fn auto() -> Self {
        Self {
            auto_assign: true,
            ..Self::default()
        }
    }

    /// Bind one symbol to one partition.
    pub fn with(mut self, symbol: impl Into<String>, partition: u32) -> Self {
        self.map.insert(symbol.into(), partition);
        self
    }

    pub fn insert(&mut self, symbol: impl Into<String>, partition: u32) {
        self.map.insert(symbol.into(), partition);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The partition for `symbol`, or `None` if this feed does not carry it.
    pub fn resolve(&mut self, symbol: &str) -> Option<u32> {
        if let Some(partition) = self.map.get(symbol) {
            return Some(*partition);
        }
        if !self.auto_assign {
            return None;
        }
        let assigned = self.next_auto;
        self.next_auto += 1;
        self.map.insert(symbol.to_string(), assigned);
        Some(assigned)
    }
}

/// The identity a decoder stamps on everything it produces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedIdentity {
    pub venue: VenueId,
    /// The channel within the venue, e.g. `itch-a`.
    pub feed: String,
}

impl FeedIdentity {
    pub fn new(venue: VenueId, feed: impl Into<String>) -> Self {
        Self {
            venue,
            feed: feed.into(),
        }
    }

    pub fn origin(&self, partition: u32, sequence: u64) -> Origin {
        Origin::new(self.venue.clone(), self.feed.clone(), partition, sequence)
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The identifier for the `ordinal`-th fact decoded from one wire message.
///
/// Derived from the message's identity rather than drawn from a generator, for
/// two reasons. Replaying a capture must produce byte-identical output, and a
/// generator's state depends on how many messages preceded it — so one extra
/// skipped message upstream would renumber everything after it. And a redundant
/// B-line copy of the same message decodes to the same identifier, which is what
/// lets a downstream arbiter recognise the duplicate as a duplicate.
///
/// The layout matches [`qip_core::ids`]: 48 bits of milliseconds so identifiers
/// sort by time, then 80 bits of digest.
pub fn message_id(origin: &Origin, ordinal: u32, at: Timestamp) -> ObjectId {
    let mut hasher = Hasher256::new();
    hasher.update(origin.venue.as_str().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(origin.feed.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(&origin.partition.to_be_bytes());
    hasher.update(&origin.sequence.to_be_bytes());
    hasher.update(&ordinal.to_be_bytes());
    let digest = hasher.finish();

    let mut entropy: u128 = 0;
    for &byte in digest.iter().take(10) {
        entropy = (entropy << 8) | u128::from(byte);
    }
    let millis = u128::try_from(at.as_millis().max(0)).unwrap_or(0) & 0xFFFF_FFFF_FFFF;
    let raw = (millis << 80) | entropy;

    let mut buf = [0u8; 26];
    let mut value = raw;
    for slot in buf.iter_mut().rev() {
        *slot = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    ObjectId::from_string(String::from_utf8_lossy(&buf).into_owned())
}

/// Build a message with a derived identifier.
pub fn build_message(
    origin: Origin,
    ordinal: u32,
    body: qip_contracts::MessageBody,
    venue_time: Timestamp,
    capture_time: Timestamp,
) -> MarketMessage {
    let id = message_id(&origin, ordinal, venue_time);
    MarketMessage::new(id, origin, body, venue_time, capture_time)
}

/// Reject a frame whose declared length is implausible.
///
/// A corrupt length field is indistinguishable from a message that has not
/// finished arriving, and waiting for the rest of a message that will never come
/// stalls the feed silently. Above this bound the decoder calls it corruption.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

pub(crate) fn check_frame_bound(protocol: &str, declared: usize) -> Result<()> {
    if declared > MAX_FRAME_BYTES {
        return Err(Error::schema(format!(
            "{protocol}: declared frame length {declared} exceeds the {MAX_FRAME_BYTES}-byte bound, so it is corruption rather than a partial message"
        )));
    }
    Ok(())
}
