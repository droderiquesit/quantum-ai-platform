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
//! # What every line says about itself
//!
//! Each line leads with two facts the venue's publisher states and the
//! decoder reads back rather than supplying: the publisher's own sequence
//! number, and the venue's clock at publication. Until the wire carried
//! them the decoder numbered each line as it decoded it and stamped the
//! venue's time with the node's own receipt instant. A decoder that counts
//! what it was handed can never see that it was handed too little, so the
//! cell's sequencer — wired, and tested in isolation — could not fire in the
//! one configuration that runs: a control that reads as present and is not.
//! And two timestamps written from one value never differ, so the transit
//! they exist to measure was zero by construction rather than by
//! measurement. Read from the wire, a dropped, repeated or reordered line is
//! the sequencer's to find, and the venue's instant is the venue's.
//!
//! The receipt stamp is the node's, and each decoder refuses a frame whose
//! receipt instant is earlier than the last it stamped: a receipt clock that
//! ran backwards would order this node's own observations wrongly, and
//! stamping the later instant instead would be a correction nobody could
//! see. One frame is one receipt — the venue is in this process and hands
//! the whole frame over at once — so every line of a frame carries the same
//! receipt instant, and that is what was observed, not a shortcut.
//!
//! # After a gap
//!
//! A gap the sequencer abandons resets every book at the venue, and a reset
//! book answers nothing until it has been rebuilt. [`SimulatedFeed::publish`]
//! rebuilds it by answering the cell's standing snapshot request with the
//! venue's whole depth ([`Cell::apply_snapshot`]) before the difference is
//! cut, because a difference against a snapshot the cell no longer holds
//! would leave every level that did not happen to change missing for good.
//! While a stream of the venue still has a gap open the request is left
//! standing rather than answered: the cell would refuse it, and a refusal
//! returned from here would end the pass before the pass reached the
//! sequencer's deadline — the one thing that closes the gap.
//!
//! Nothing here reads a clock, opens a socket or touches a file. The venue
//! is in this process and the frame is a `String`; the venue's instant is
//! the one the pass last advanced its gateway to.

use crate::gateway::SimulatedGateway;
use qip_brokers::exchange::BookLevel;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::Timestamp;
use qip_edge::cell::Cell;
use qip_edge::settlement::SettlementTerms;
use qip_orderbook::venue::VenueState;
use qip_protocols::decoder::{Decoder, Diagnostics, SkipReason, SkipRecord};
use qip_protocols::registry::FeedKey;
use std::collections::BTreeMap;

/// Names the venue feed the node's passes price from.
pub const FEED_VARIABLE: &str = "QIP_VENUE_FEED";

/// The one value [`FEED_VARIABLE`] accepts.
pub const SIMULATED_FEED: &str = "simulated";

/// The feed's channel name, as the cell's protocol registry and sequencer
/// key it beside the venue.
pub const FEED_NAME: &str = "simulated-depth";

/// The wire format's name, for the decoder's diagnostics.
const PROTOCOL: &str = "qip.simulated-depth.2";

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
    /// Books the cell had discarded — behind an abandoned sequence gap, or
    /// crossed at the end of a frame — that this pass rebuilt from the
    /// venue's own depth through [`Cell::apply_snapshot`]. Zero on every
    /// ordinary pass.
    pub resynchronised: usize,
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
    /// The publisher's own count of the lines it has put on the wire: the
    /// venue sequence number of the last one. Written on each line and read
    /// back by the decoder, never supplied by it.
    sequence: u64,
}

impl SimulatedFeed {
    /// A feed for one venue, publishing nothing until [`Self::publish`].
    pub fn new(venue: VenueId) -> Self {
        Self {
            key: FeedKey::new(venue.clone(), FEED_NAME),
            venue,
            published: BTreeMap::new(),
            omitted_total: 0,
            sequence: 0,
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

    /// The venue sequence number of the last line this feed published; zero
    /// before the first.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Bind this feed's decoder to the cell, and state the venue's
    /// settlement terms in the same breath.
    ///
    /// Once: the registry refuses a second binding of the same feed, and that
    /// refusal is the right answer here too — two decoders on one stream
    /// would keep two sequence positions.
    ///
    /// The terms are the simulator's own fact, not a setting: the in-process
    /// venue books a fill the instant it reports it and holds nothing in
    /// settlement, so a cycle's later leg spends what its earlier leg
    /// delivered the moment it is delivered. Stated here rather than read
    /// from a variable because nothing else can drive this feed, and a
    /// variable that could only ever be `instant` would be a control that
    /// reads as a choice. Left unstated, every venue this node drives would
    /// stand on `qip_edge_settlement_unprojected_venues` for ever — a
    /// healthy process running a fallback, which is the shape the manifest
    /// suite exists to refuse. A venue adapter with a settlement cycle
    /// (§34.1) states its own terms at its own seam; none exists yet.
    pub fn attach(&self, cell: &mut Cell) -> Result<()> {
        cell.protocols_mut().register(
            self.venue.clone(),
            FEED_NAME,
            Box::new(DepthDecoder::new(self.venue.clone())),
        )?;
        cell.install_settlement(&self.venue, SettlementTerms::instant())
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
        let quotes = gateway.quotes();
        // The venue was read, so its feed is alive whether or not anything
        // moved. This feed publishes differences: a quiet market publishes
        // nothing, and without this line the cell could not tell that from
        // a feed that had died and would refuse every order under its
        // silent-feed gate after five quiet seconds.
        cell.feed_heartbeat(&self.venue, now);
        // The cell's standing request for a snapshot, answered before the
        // difference is cut: a book the cell discarded is rebuilt whole from
        // the venue's depth as it stands, and remembered as published so the
        // difference below is taken against what the cell now holds.
        // Not while a gap is open at this venue: `apply_snapshot` refuses
        // then, and that refusal propagated from here would abort every pass
        // before `on_bytes` or `work` could hand the sequencer the clock that
        // abandons the gap — a cell wedged by its own recovery. The request
        // stands and the book stays discarded, so `stale_book` keeps refusing
        // orders against it in the meantime.
        let gapped = cell.sequence_gap_open_at(&self.venue);
        for request in cell.snapshot_requests() {
            if gapped || request.venue != self.venue {
                continue;
            }
            let id = request.object_id.as_str();
            let (Some(published), Some(depth)) = (
                self.published.get_mut(id),
                quotes
                    .iter()
                    .find(|depth| depth.object_id == request.object_id),
            ) else {
                // Not an instrument this feed publishes; whoever handed the
                // cell that book answers for it.
                continue;
            };
            let mut edits = Vec::new();
            *published = Published::default();
            for (side, levels, remembered) in [
                (BookSide::Bid, &depth.bids, &mut published.bids),
                (BookSide::Ask, &depth.asks, &mut published.asks),
            ] {
                for level in levels.iter().take(MAX_LEVELS_PER_SIDE) {
                    if level.size <= Decimal::ZERO {
                        continue;
                    }
                    remembered.insert(level.price, level.size);
                    edits.push(MessageBody::LevelSet {
                        side,
                        price: level.price,
                        quantity: level.size,
                        order_count: None,
                    });
                }
            }
            cell.apply_snapshot(&self.venue, &request.object_id, &edits, now)?;
            tick.resynchronised += 1;
        }
        let mut wire = Wire {
            frame: String::new(),
            sequence: self.sequence,
            // The venue's clock, not this call's `now`: the instant the pass
            // last advanced the gateway to. The pass hands both the same
            // value, so the two agree in a running node — which is what a
            // venue with no wire between it and the cell measures — and they
            // are still two facts from two sources.
            venue_time: gateway.now(),
        };
        for depth in quotes {
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
            tick.messages += diff_side(
                &mut published.bids,
                &depth.bids,
                id,
                BookSide::Bid,
                &mut wire,
            );
            tick.messages += diff_side(
                &mut published.asks,
                &depth.asks,
                id,
                BookSide::Ask,
                &mut wire,
            );
        }
        self.omitted_total = self
            .omitted_total
            .saturating_add(tick.instruments_omitted as u64);
        // Advanced whether or not the cell takes the frame: a frame the cell
        // refused is a frame it did not see, and the hole its numbers leave
        // is how the sequencer finds that out on the next one.
        self.sequence = wire.sequence;
        let frame = wire.frame;
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

/// One level, as the venue's publisher writes it and the feed's decoder
/// reads it:
/// `sequence ⇥ venue_time_nanos ⇥ object_id ⇥ B|A ⇥ price ⇥ size ⇤`.
///
/// Public because it is the wire: a replay that injects a dropped, repeated
/// or reordered line has to write lines the decoder will read, and a second
/// encoder kept in a test would be a second place the format lives.
pub fn level_line(
    sequence: u64,
    venue_time: Timestamp,
    object_id: &str,
    side: BookSide,
    price: Decimal,
    size: Decimal,
) -> String {
    let side = match side {
        BookSide::Bid => 'B',
        BookSide::Ask => 'A',
    };
    format!(
        "{sequence}\t{}\t{object_id}\t{side}\t{price}\t{size}\n",
        venue_time.as_nanos()
    )
}

/// The frame one publication is building, and the two facts every line of
/// it carries from the venue.
struct Wire {
    frame: String,
    /// The sequence number of the last line written.
    sequence: u64,
    venue_time: Timestamp,
}

impl Wire {
    fn level(&mut self, id: &str, side: BookSide, price: Decimal, size: Decimal) {
        self.sequence = self.sequence.saturating_add(1);
        self.frame.push_str(&level_line(
            self.sequence,
            self.venue_time,
            id,
            side,
            price,
            size,
        ));
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
    side: BookSide,
    wire: &mut Wire,
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
            wire.level(id, side, *price, *size);
            written += 1;
        }
    }
    for price in previous.keys() {
        if !next.contains_key(price) {
            wire.level(id, side, *price, Decimal::ZERO);
            written += 1;
        }
    }
    *previous = next;
    written
}

/// The feed's decoder: one level per line, tab-separated, as
/// [`level_line`] writes it.
///
/// Registered in the cell's protocol registry like any venue's decoder, so
/// the frame the feed builds takes the path a packet would — sequenced,
/// applied to the book, and fed to the feature graph — rather than being
/// written into the book directly by the node, which would be a second way
/// for a book to change that no replay could see.
///
/// It supplies exactly one fact of its own, the receipt stamp. The sequence
/// number and the venue's instant are read from the line: a decoder that
/// numbered what it decoded would report every stream as contiguous, however
/// much of it never arrived.
#[derive(Debug)]
struct DepthDecoder {
    venue: VenueId,
    /// The receipt instant of the last frame this decoder stamped, so the
    /// next cannot be stamped earlier. One stream, one partition: the
    /// sequence is the publisher's and is scoped to the feed, not to an
    /// instrument.
    last_receipt: Option<Timestamp>,
    consumed: usize,
    diagnostics: Diagnostics,
}

impl DepthDecoder {
    fn new(venue: VenueId) -> Self {
        Self {
            venue,
            last_receipt: None,
            consumed: 0,
            diagnostics: Diagnostics::default(),
        }
    }
}

/// One line as the decoder reads it.
struct Level {
    sequence: u64,
    venue_time: Timestamp,
    object_id: ObjectId,
    side: BookSide,
    price: Decimal,
    size: Decimal,
}

/// One line's fields, or why it could not be read.
fn parse_line(line: &str) -> std::result::Result<Level, String> {
    let mut fields = line.split('\t');
    let sequence = fields.next();
    let venue_time = fields.next();
    let id = fields.next().filter(|id| !id.is_empty());
    let side = fields.next();
    let price = fields.next();
    let size = fields.next();
    let (Some(sequence), Some(venue_time), Some(id), Some(side), Some(price), Some(size)) =
        (sequence, venue_time, id, side, price, size)
    else {
        return Err("a level needs six tab-separated fields".to_string());
    };
    if fields.next().is_some() {
        return Err("a level has more than six fields".to_string());
    }
    let sequence = sequence
        .parse::<u64>()
        .ok()
        .filter(|sequence| *sequence > 0)
        .ok_or_else(|| format!("sequence {sequence:?} is not a positive integer"))?;
    let venue_time = venue_time
        .parse::<i64>()
        .map(Timestamp::from_nanos)
        .map_err(|_| format!("venue time {venue_time:?} is not a count of nanoseconds"))?;
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
    Ok(Level {
        sequence,
        venue_time,
        object_id: ObjectId::from_string(id),
        side,
        price,
        size,
    })
}

impl Decoder for DepthDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        let text = std::str::from_utf8(bytes).map_err(|error| {
            Error::invalid(format!("{PROTOCOL}: the frame is not text: {error}"))
        })?;
        // The receipt stamp never runs backwards within this adapter. Refused
        // rather than stamped at the later instant: the caller's clock is
        // wrong, and a frame quietly restamped would hide that from the one
        // reader — a latency measurement — the stamp exists for.
        if let Some(last) = self.last_receipt
            && captured_at < last
        {
            self.consumed = 0;
            self.diagnostics.frames_refused = self.diagnostics.frames_refused.saturating_add(1);
            return Err(Error::invalid(format!(
                "{PROTOCOL}: a frame received at {} follows one this adapter stamped at {}; a \
                 receipt stamp that runs backwards would order the node's own observations \
                 wrongly, so nothing of the frame is consumed. Present it again once the \
                 node's clock reads no earlier than the last receipt",
                captured_at.to_rfc3339(),
                last.to_rfc3339()
            )));
        }
        self.last_receipt = Some(captured_at);
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
                Ok(level) => {
                    let origin = Origin::new(self.venue.clone(), FEED_NAME, 0, level.sequence);
                    let lineage = Lineage::root(
                        CorrelationId::from_string(&origin.stream_key()),
                        "qip-edge-node",
                    );
                    messages.push(MarketMessage::new(
                        level.object_id,
                        origin,
                        MessageBody::LevelSet {
                            side: level.side,
                            price: level.price,
                            quantity: level.size,
                            order_count: None,
                        },
                        // Two facts from two sources: the venue's instant as
                        // the line states it, and this node's receipt.
                        level.venue_time,
                        captured_at,
                        lineage,
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

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    //! The decoder's own two stamps (REFLEX-019), on the decoder itself: it
    //! is private, and the receipt stamp it writes reaches nothing a test
    //! outside this module can read.

    use super::*;
    use qip_core::{Duration, dec};

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    fn line(sequence: u64, venue_time: Timestamp) -> String {
        level_line(
            sequence,
            venue_time,
            "obj-STAMP",
            BookSide::Bid,
            dec!("99"),
            dec!("5"),
        )
    }

    #[test]
    fn a_decoded_level_carries_the_venues_instant_and_the_nodes_receipt_in_separate_fields()
    -> Result<()> {
        let mut decoder = DepthDecoder::new(VenueId::new("XLON"));
        let venue_said = t(10);
        let received = t(10).saturating_add(Duration::from_millis(3));
        assert_ne!(
            venue_said, received,
            "the premise is two different instants"
        );

        let messages = decoder.decode(line(7, venue_said).as_bytes(), received)?;
        assert_eq!(messages.len(), 1, "the premise is one decoded level");
        let message = &messages[0];
        assert_eq!(
            message.venue_time, venue_said,
            "the venue's instant was not read from the line"
        );
        assert_eq!(
            message.capture_time, received,
            "the receipt stamp is not the instant the node received the frame"
        );
        assert_eq!(message.transit(), Duration::from_millis(3));
        assert_eq!(
            message.origin.sequence, 7,
            "the sequence is the decoder's own count, not the venue's number"
        );
        Ok(())
    }

    #[test]
    fn a_frame_received_earlier_than_the_last_is_refused_and_consumes_nothing() -> Result<()> {
        let mut decoder = DepthDecoder::new(VenueId::new("XLON"));
        let first = decoder.decode(line(1, t(10)).as_bytes(), t(10))?;
        assert_eq!(first.len(), 1, "the premise is a frame already stamped");
        assert_eq!(decoder.diagnostics().frames_refused, 0);

        // The node's clock reads a second earlier than its last receipt.
        let frame = line(2, t(10));
        let error = match decoder.decode(frame.as_bytes(), t(9)) {
            Ok(messages) => panic!(
                "a frame received before the last one was stamped anyway: {:?}",
                messages
                    .iter()
                    .map(|message| message.capture_time)
                    .collect::<Vec<_>>()
            ),
            Err(error) => error,
        };
        assert_eq!(error.code(), "invalid");
        assert!(
            error.message().contains("runs backwards"),
            "the refusal does not say why: {}",
            error.message()
        );
        assert_eq!(decoder.consumed(), 0, "a refused frame was consumed");
        assert_eq!(decoder.diagnostics().frames_refused, 1);

        // Presented again at the last receipt instant — equal is not
        // backwards — it is read whole, and the stamps never decreased.
        let again = decoder.decode(frame.as_bytes(), t(10))?;
        assert_eq!(again.len(), 1);
        assert!(again[0].capture_time >= first[0].capture_time);
        assert_eq!(decoder.consumed(), frame.len());
        Ok(())
    }
}
