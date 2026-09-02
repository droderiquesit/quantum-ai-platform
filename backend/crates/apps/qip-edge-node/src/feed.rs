//! The venue feed the node's pass loop prices from, and the one value it may
//! be configured as.
//!
//! Until this module existed the node configured no feed and never called
//! `Cell::work`. The halt gauge, the policy sequence and the mesh series
//! reached a deployed process; every pass-time series — freshness, refusals,
//! signals, orders, netting, crosses, the feasibility and desk gates — was
//! recorded by code nothing in production ran. The blueprint's execution
//! node (§41.4) is a process that runs passes, and a node whose every
//! pass-time control is exercised only by tests is a node whose controls
//! read as present and are not.
//!
//! # `simulated` is the only value, and unset is not it
//!
//! [`FEED_VARIABLE`] accepts exactly one value, [`SIMULATED_FEED`]. Unset is
//! a node that runs no passes — what every deployment of this binary did
//! until now — announced at start-up in the production requirements rather
//! than silently defaulted to the simulator, because a feed nobody asked for
//! is a feed nobody will notice is not a market. Anything else is refused at
//! start naming ADR 0003: a live feed is not a configuration value, it is an
//! architecture decision, and a node that could be aimed at one by an
//! environment edit would put the paper boundary into a file.
//!
//! The feed is simulated by construction as well as by name.
//! [`SimulatedFeed::publish`] takes a [`SimulatedGateway`] and reads
//! [`qip_brokers::exchange::SimulatedDepth`] — the venue's own type, built only by the venue from
//! its own matching engine — so there is no call through which a quote from
//! anywhere else reaches the cell. A live gateway has no such accessor, and
//! the node refuses to pair this feed with one before it serves.
//!
//! # What the feed carries
//!
//! Per pass, the simulated venue's resting depth: the top
//! [`MAX_LEVELS_PER_SIDE`] levels per side for up to [`MAX_FEED_INSTRUMENTS`]
//! listed instruments, delivered to the cell through `Cell::on_bytes` — the
//! same decode, sequence, apply and feature path a venue's packets take — as
//! `LevelSet` messages on one stream. What is published is what changed
//! since the last pass; a level that left the venue's book is published at
//! size zero, which is what removes it from the cell's. The cell's book is
//! therefore the venue's book, which is the one property a paper feed must
//! hold for a fill to mean anything: an order priced off it meets exactly
//! the depth it was priced against.
//!
//! # Bounds, and what happens past each
//!
//! * **Instruments.** The first [`MAX_FEED_INSTRUMENTS`] in the venue's
//!   listing order are tracked and published; any beyond are counted in
//!   [`FeedTick::instruments_omitted`] and never tracked. A strategy naming
//!   one of them refuses under the cell's `book` gate, which is the honest
//!   outcome — a cell should say it cannot see an instrument, not guess.
//! * **Levels.** Only the top [`MAX_LEVELS_PER_SIDE`] per side reach the
//!   cell. Depth below is invisible, which understates what the feasibility
//!   gate sizes against: the conservative direction.
//! * **The frame.** At most instruments × sides × 2 × levels lines — the
//!   current levels plus the removals of the previous ones — so a pass's
//!   publication is bounded regardless of what rests at the venue.
//! * **The remembered snapshot** the diff is taken against: one per tracked
//!   instrument, at most [`MAX_LEVELS_PER_SIDE`] per side.
//!
//! Nothing here reads a clock, opens a socket or touches a file. The venue
//! is in this process and the frame is a `String`.

use crate::gateway::SimulatedGateway;
use qip_brokers::exchange::BookLevel;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_edge::cell::Cell;
use qip_orderbook::venue::VenueState;
use qip_protocols::decoder::{Decoder, Diagnostics, SkipReason, SkipRecord};
use qip_protocols::registry::FeedKey;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Names the venue feed the node's passes price from.
pub const FEED_VARIABLE: &str = "QIP_VENUE_FEED";

/// The one value [`FEED_VARIABLE`] accepts.
pub const SIMULATED_FEED: &str = "simulated";

/// The feed's channel name, as the cell's protocol registry and sequencer
/// key it beside the venue.
pub const FEED_NAME: &str = "simulated-depth";

/// The wire format's name, for the decoder's diagnostics.
const PROTOCOL: &str = "qip.simulated-depth.1";

/// How many listed instruments the feed will track.
pub const MAX_FEED_INSTRUMENTS: usize = 64;

/// How many price levels per side reach the cell.
pub const MAX_LEVELS_PER_SIDE: usize = 10;

/// Which feed the node prices from, when it has one.
///
/// One variant, deliberately. The type exists so the choice is a value the
/// pass loop is constructed with rather than a string it compares, and so a
/// second variant cannot be added without every match in this crate
/// refusing to compile until it says what that feed is allowed to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedChoice {
    /// The in-process venue's own resting depth.
    Simulated,
}

impl FeedChoice {
    /// Interpret the variable's value.
    ///
    /// `None` and the empty string are a node with no feed, which is allowed
    /// and announced. `simulated` is the simulator. Everything else is
    /// refused with the decision it would need, because the alternative —
    /// treating an unknown value as "no feed" — would let a typo in a live
    /// deployment quietly turn a trading node into one that never trades,
    /// and treating it as "the simulator" would do the reverse.
    pub fn read(value: Option<&str>) -> Result<Option<Self>> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if value == SIMULATED_FEED {
            return Ok(Some(Self::Simulated));
        }
        Err(Error::invalid(format!(
            "configuration: {FEED_VARIABLE}={value} names a feed this node does not have. The \
             only value is `{SIMULATED_FEED}`, which prices passes off the in-process venue's own \
             depth. A feed from a market is not a configuration value: ADR 0003 makes this \
             platform paper-only, and wiring a live feed is an architecture decision recorded \
             there, not an environment edit"
        )))
    }

    /// Read the variable from the process environment.
    pub fn from_env() -> Result<Option<Self>> {
        Self::read(std::env::var(FEED_VARIABLE).ok().as_deref())
    }

    /// The value as it would be configured.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Simulated => SIMULATED_FEED,
        }
    }
}

/// What one publication did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedTick {
    /// Instruments the venue lists and this feed tracked this pass.
    pub instruments: usize,
    /// Listed instruments past [`MAX_FEED_INSTRUMENTS`], left untracked.
    pub instruments_omitted: usize,
    /// Level messages the cell was handed, including removals.
    pub messages: usize,
}

/// The remembered top of one instrument's book, so the next pass publishes
/// a difference rather than a copy.
#[derive(Debug, Default)]
struct Published {
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
}

/// The simulated venue's quote feed.
#[derive(Debug)]
pub struct SimulatedFeed {
    venue: VenueId,
    key: FeedKey,
    /// Keyed by instrument id, in the order the venue lists them. Bounded by
    /// [`MAX_FEED_INSTRUMENTS`]: an instrument is inserted only through
    /// [`Self::publish`], which refuses past the bound.
    published: BTreeMap<String, Published>,
    /// Instruments the venue listed that this feed would not track, in
    /// total, so the health surface can say a bound was hit.
    omitted_total: u64,
}

impl SimulatedFeed {
    /// A feed for one venue, publishing nothing until [`Self::publish`].
    pub fn new(venue: VenueId) -> Self {
        Self {
            key: FeedKey::new(venue.clone(), FEED_NAME),
            venue,
            published: BTreeMap::new(),
            omitted_total: 0,
        }
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    /// The venue's feed key, as the cell's registry knows it.
    pub fn key(&self) -> &FeedKey {
        &self.key
    }

    /// Instruments this feed has tracked so far.
    pub fn tracked(&self) -> usize {
        self.published.len()
    }

    /// Instruments the venue listed that the feed refused to track, in total.
    pub fn omitted_total(&self) -> u64 {
        self.omitted_total
    }

    /// Bind this feed's decoder to the cell.
    ///
    /// Once: the registry refuses a second binding of the same feed, and that
    /// refusal is the right answer here too — two decoders on one stream
    /// would keep two sequence positions.
    pub fn attach(&self, cell: &mut Cell) -> Result<()> {
        cell.protocols_mut().register(
            self.venue.clone(),
            FEED_NAME,
            Box::new(DepthDecoder::new(self.venue.clone())),
        )
    }

    /// Publish what changed at the venue since the last pass.
    ///
    /// Takes the simulated gateway and nothing wider: this is the only place
    /// a quote enters the cell from this node, and the type it enters from
    /// is the simulator's.
    pub fn publish(
        &mut self,
        gateway: &SimulatedGateway,
        cell: &mut Cell,
        now: Timestamp,
    ) -> Result<FeedTick> {
        let mut tick = FeedTick::default();
        let mut frame = String::new();
        for depth in gateway.quotes() {
            let id = depth.object_id.as_str();
            // An id the wire cannot carry is an instrument the feed cannot
            // publish. Counted as omitted rather than escaped: an escaping
            // rule would be a second place the id's spelling lives.
            if id.contains('\t') || id.contains('\n') || id.contains('\r') {
                tick.instruments_omitted += 1;
                continue;
            }
            if !self.published.contains_key(id) {
                if self.published.len() >= MAX_FEED_INSTRUMENTS {
                    tick.instruments_omitted += 1;
                    continue;
                }
                // The cell keeps a book only for what it has been told to
                // track; a message for an untracked instrument reaches the
                // feature engine and no book. Tracked here, at the bound,
                // so the cell's book count is this feed's instrument count.
                cell.track(VenueState::aggregated(
                    depth.object_id.clone(),
                    self.venue.clone(),
                    VenueStatus::Open,
                ));
                self.published.insert(id.to_string(), Published::default());
            }
            tick.instruments += 1;
            let Some(published) = self.published.get_mut(id) else {
                continue;
            };
            tick.messages += diff_side(&mut published.bids, &depth.bids, id, 'B', &mut frame);
            tick.messages += diff_side(&mut published.asks, &depth.asks, id, 'A', &mut frame);
        }
        self.omitted_total = self
            .omitted_total
            .saturating_add(tick.instruments_omitted as u64);
        if tick.messages > 0 {
            let decoded = cell.on_bytes(&self.key, frame.as_bytes(), now)?;
            if decoded != tick.messages {
                // The decoder is this module's own; a frame it built that its
                // decoder did not read back whole is a defect here, and it
                // is refused rather than left as a book missing a level.
                return Err(Error::invalid(format!(
                    "the simulated feed published {} level(s) and the cell decoded {decoded}; \
                     the feed's wire and its decoder disagree",
                    tick.messages
                )));
            }
        }
        Ok(tick)
    }
}

/// Publish one side's difference, top [`MAX_LEVELS_PER_SIDE`] only.
///
/// Returns how many lines were written. `previous` is left holding what was
/// published, so the next call diffs against it.
fn diff_side(
    previous: &mut BTreeMap<Decimal, Decimal>,
    current: &[BookLevel],
    id: &str,
    side: char,
    frame: &mut String,
) -> usize {
    let mut written = 0;
    let mut next: BTreeMap<Decimal, Decimal> = BTreeMap::new();
    for level in current.iter().take(MAX_LEVELS_PER_SIDE) {
        if level.size <= Decimal::ZERO {
            continue;
        }
        next.insert(level.price, level.size);
    }
    for (price, size) in &next {
        if previous.get(price) != Some(size) {
            // `writeln!` into a `String` cannot fail; the `let _` names that
            // rather than pretending the result is being inspected.
            let _ = writeln!(frame, "{id}\t{side}\t{price}\t{size}");
            written += 1;
        }
    }
    for price in previous.keys() {
        if !next.contains_key(price) {
            let _ = writeln!(frame, "{id}\t{side}\t{price}\t0");
            written += 1;
        }
    }
    *previous = next;
    written
}

/// The feed's decoder: one level per line, tab-separated.
///
/// `object_id ⇥ B|A ⇥ price ⇥ size ⇤`. Registered in the cell's protocol
/// registry like any venue's decoder, so the frame the feed builds takes the
/// path a packet would — sequenced, applied to the book, and fed to the
/// feature graph — rather than being written into the book directly by the
/// node, which would be a second way for a book to change that no replay
/// could see.
#[derive(Debug)]
struct DepthDecoder {
    venue: VenueId,
    /// One stream, one partition: the feed is in-process and cannot lose or
    /// reorder a frame, so per-instrument partitions would be structure for
    /// a failure that cannot occur here.
    sequence: u64,
    consumed: usize,
    diagnostics: Diagnostics,
}

impl DepthDecoder {
    fn new(venue: VenueId) -> Self {
        Self {
            venue,
            sequence: 0,
            consumed: 0,
            diagnostics: Diagnostics::default(),
        }
    }
}

/// One line's fields, or why it could not be read.
fn parse_line(line: &str) -> std::result::Result<(ObjectId, BookSide, Decimal, Decimal), String> {
    let mut fields = line.split('\t');
    let id = fields.next().filter(|id| !id.is_empty());
    let side = fields.next();
    let price = fields.next();
    let size = fields.next();
    let (Some(id), Some(side), Some(price), Some(size)) = (id, side, price, size) else {
        return Err("a level needs four tab-separated fields".to_string());
    };
    if fields.next().is_some() {
        return Err("a level has more than four fields".to_string());
    }
    let side = match side {
        "B" => BookSide::Bid,
        "A" => BookSide::Ask,
        other => return Err(format!("side {other:?} is neither B nor A")),
    };
    let price = Decimal::parse(price).ok_or_else(|| format!("price {price:?} is not a decimal"))?;
    let size = Decimal::parse(size).ok_or_else(|| format!("size {size:?} is not a decimal"))?;
    if price <= Decimal::ZERO {
        return Err(format!("price {price} is not positive"));
    }
    if size.is_negative() {
        return Err(format!("size {size} is negative"));
    }
    Ok((ObjectId::from_string(id), side, price, size))
}

impl Decoder for DepthDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        let text = std::str::from_utf8(bytes).map_err(|error| {
            Error::invalid(format!("{PROTOCOL}: the frame is not text: {error}"))
        })?;
        let mut messages = Vec::new();
        let mut consumed = 0usize;
        for line in text.split_inclusive('\n') {
            if !line.ends_with('\n') {
                // A partial trailing line is re-presented by the caller.
                break;
            }
            let offset = consumed;
            consumed += line.len();
            match parse_line(line.trim_end_matches(['\n', '\r'])) {
                Ok((object_id, side, price, quantity)) => {
                    self.sequence = self.sequence.saturating_add(1);
                    messages.push(MarketMessage::new(
                        object_id,
                        Origin::new(self.venue.clone(), FEED_NAME, 0, self.sequence),
                        MessageBody::LevelSet {
                            side,
                            price,
                            quantity,
                            order_count: None,
                        },
                        captured_at,
                        captured_at,
                    ));
                }
                Err(detail) => self.diagnostics.record_skip(SkipRecord {
                    protocol: PROTOCOL.to_string(),
                    reason: SkipReason::Malformed { detail },
                    offset,
                    at: captured_at,
                }),
            }
        }
        self.consumed = consumed;
        self.diagnostics.messages_decoded = self
            .diagnostics
            .messages_decoded
            .saturating_add(messages.len() as u64);
        self.diagnostics.bytes_consumed = self
            .diagnostics
            .bytes_consumed
            .saturating_add(consumed as u64);
        Ok(messages)
    }

    fn protocol(&self) -> &str {
        PROTOCOL
    }

    fn consumed(&self) -> usize {
        self.consumed
    }

    fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}
