//! ADR 0100 §8: the canonical market-event tape that seeds the node's
//! `SimulatedGateway` — known-at ordering is refused, not sorted. Recording
//! the tape events a pass actually applied is `event_fabric::inputs`, not
//! here. SLICE-19.
//!
//! Red-team finding B1: [`SimulatedGateway::seed_touch`] and
//! [`SimulatedGateway::seed_aggressor`] were called from nothing but tests
//! (`gateway.rs`'s own doc calls the book "empty until a test fills it"), so
//! no deployed node's venue ever held a book and no strategy's order had
//! anything to fill against. [`TapeDriver`] is what a composition root
//! reaches for instead: it reads a committed, line-delimited tape of
//! [`MarketEvent`]s — one venue fact per line, already timestamped and
//! licensed — and seeds the gateway with exactly what each line says, in the
//! order the tape states it, never resorted into one.
//!
//! # What a line is
//!
//! Each line is either a **touch** — resting interest belonging to somebody
//! else, seeded with [`SimulatedGateway::seed_touch`] — or an **aggressor** —
//! flow that takes from the book, seeded with
//! [`SimulatedGateway::seed_aggressor`]. An aggressor's fill is capped by
//! whatever depth is actually resting at the venue, never by the line's own
//! request (TICK-006): [`TapeDriver::seed`] returns what the venue actually
//! matched, not what the line asked for.
//!
//! # Three refusals, checked at [`TapeDriver::parse`]
//!
//! * A `receive_time` earlier than the line before it is **refused, not
//!   sorted** — a tape whose account of arrival order cannot be trusted is
//!   not repaired by guessing at the right one, the same argument
//!   `qip_market_ingestion::tape::Tape` makes of its own `known_at`.
//! * A line naming no entitlement is refused. A missing licence is never
//!   defaulted to a grant, because a default that always grants is a control
//!   that can never fire.
//! * The whole tape is refused unless the feed mode the root passed in is
//!   [`FeedChoice::Simulated`] (ADR 0003): a paper fixture may not seed a
//!   node with no feed to publish it, or a feed this build does not have.
//!   This module reads no environment variable itself and names no feed
//!   literal of its own — [`FeedChoice`] and its constants are `crate::feed`'s
//!   only, read here to compare against.
//!
//! # The clock
//!
//! [`TapeDriver::advance`] moves an owned [`ManualClock`] to the next line's
//! own `receive_time` and returns it — never a wall clock, so the same tape
//! read twice moves the clock to the same instants on any machine, at any
//! hour. [`TapeDriver::seed`] stamps every call it makes to the gateway with
//! that line's own `event_time`, never with the clock or the instant the
//! process happens to be running at, so two independent runs of one tape
//! seed the venue identically.

use crate::feed::FeedChoice;
use crate::gateway::SimulatedGateway;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::{
    BookSide, Entitlement, MarketMessage, MessageBody, Origin, TradeCondition, Usage, VenueId,
};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_execution_engine::order::Side;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// One line's own idea of what it is: resting interest, or flow that takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TapeLineKind {
    Touch,
    Aggressor,
}

/// The licence a line carries in the committed file.
///
/// `Option<TapeGrant>` at the field it fills, never this type alone: a tape
/// line with no `entitlement` key is `None`, and [`TapeDriver::parse`]
/// refuses that rather than defaulting it. See the module doc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TapeGrant {
    dataset: String,
    expires_at: Timestamp,
}

/// One line of the committed tape, exactly as its JSON spells it.
///
/// Never trusted with [`MarketEvent`]'s own refusals directly: a line this
/// type deserialises still has to pass every check [`MarketEvent::new`]
/// holds, which [`TapeDriver::parse`] runs when it rebuilds one from these
/// fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TapeLine {
    object_id: String,
    venue: String,
    /// The feed or channel within the venue, e.g. `xlon-tape`.
    feed: String,
    sequence: u64,
    kind: TapeLineKind,
    /// For a touch, the side the level rests on. For an aggressor, the side
    /// of the book it hits — hitting the ask is a buy, hitting the bid is a
    /// sell, the same convention [`SimulatedGateway::place`] reads a cell's
    /// own order under.
    side: BookSide,
    price: Decimal,
    quantity: Decimal,
    event_time: Timestamp,
    receive_time: Timestamp,
    normalized_time: Timestamp,
    /// Never negative; [`MarketEvent::new`] is what actually refuses that,
    /// so this field carries only the sign the caller wrote.
    uncertainty_ms: i64,
    /// `None` is refused by [`TapeDriver::parse`]; see the module doc.
    entitlement: Option<TapeGrant>,
}

/// The side a resting touch takes: a bid rests as a buy, an ask as a sell —
/// [`qip_brokers::matching::MatchingEngine::seed`]'s own convention, read
/// here for a tape's.
const fn resting_side(side: BookSide) -> Side {
    match side {
        BookSide::Bid => Side::Buy,
        BookSide::Ask => Side::Sell,
    }
}

/// The side an aggressor takes: hitting the ask is a buy, hitting the bid is
/// a sell — [`SimulatedGateway::place`]'s own convention for a cell's order,
/// read here for a tape's.
const fn taking_side(side: BookSide) -> Side {
    match side {
        BookSide::Ask => Side::Buy,
        BookSide::Bid => Side::Sell,
    }
}

/// A committed tape of [`MarketEvent`]s, in the order the file states them.
///
/// Built once by [`Self::parse`] or [`Self::open`], which is where every
/// refusal in the module doc is checked; from then on [`Self::advance`] and
/// [`Self::seed`] only ever look forward through a tape already known to
/// hold together.
#[derive(Debug)]
pub struct TapeDriver {
    events: Vec<MarketEvent>,
    /// How many of `events`, counting from the front, [`Self::seed`] has
    /// already applied.
    applied: usize,
    /// The clock this driver owns. [`Self::advance`] is the only thing that
    /// moves it, and it only ever moves to an instant a line of the tape
    /// actually names — never a wall clock.
    clock: Arc<ManualClock>,
}

impl TapeDriver {
    /// Read a tape from disk. Every refusal names the file.
    pub fn open(path: impl AsRef<Path>, feed_mode: Option<FeedChoice>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("cannot read the tape at {}: {e}", path.display())))?;
        Self::parse(&text, feed_mode)
            .map_err(|e| Error::invalid(format!("the tape at {}: {}", path.display(), e.message())))
    }

    /// Validate a tape's text, or refuse it.
    ///
    /// The refusals, in the order they are checked: the feed mode is not
    /// simulated (cheapest, and the one that makes every other check moot);
    /// a line does not parse; a line's `receive_time` is earlier than the
    /// line before it; a line names no entitlement; the resulting event
    /// fails one of [`MarketEvent::new`]'s own refusals. None of them is
    /// repaired.
    pub fn parse(text: &str, feed_mode: Option<FeedChoice>) -> Result<Self> {
        if feed_mode != Some(FeedChoice::Simulated) {
            return Err(Error::denied(format!(
                "a tape may seed the venue only when {} is configured as {}; that is not a \
                 setting this driver can override, and a node with no feed or a different one is \
                 refused rather than seeded (ADR 0003)",
                crate::feed::FEED_VARIABLE,
                crate::feed::SIMULATED_FEED
            )));
        }
        let mut events = Vec::new();
        let mut previous_receive: Option<Timestamp> = None;
        for (index, raw_line) in text.lines().enumerate() {
            let raw_line = raw_line.trim();
            if raw_line.is_empty() {
                continue;
            }
            let line: TapeLine = serde_json::from_str(raw_line)
                .map_err(|e| Error::invalid(format!("tape line {index} does not parse: {e}")))?;
            if let Some(previous) = previous_receive
                && line.receive_time < previous
            {
                return Err(Error::invalid(format!(
                    "tape line {index} is receivable at {}, before the line before it at {}: the \
                     tape is out of order and is refused rather than sorted",
                    line.receive_time.to_rfc3339(),
                    previous.to_rfc3339()
                )));
            }
            previous_receive = Some(line.receive_time);
            let grant = line.entitlement.ok_or_else(|| {
                Error::invalid(format!(
                    "tape line {index} ({}) carries no entitlement; a market event may not be \
                     built from data with no stated licence, and one is never defaulted",
                    line.object_id
                ))
            })?;
            let entitlement = Entitlement::Granted {
                dataset: grant.dataset,
                usage: Usage::Trade,
                expires_at: grant.expires_at,
            };
            let body = match line.kind {
                TapeLineKind::Touch => MessageBody::LevelSet {
                    side: line.side,
                    price: line.price,
                    quantity: line.quantity,
                    order_count: None,
                },
                TapeLineKind::Aggressor => MessageBody::Trade {
                    price: line.price,
                    quantity: line.quantity,
                    condition: TradeCondition::Regular,
                    aggressor: Some(line.side),
                },
            };
            let origin = Origin::new(VenueId::new(line.venue), line.feed, 0, line.sequence);
            let payload = MarketMessage::new(
                ObjectId::from_string(&line.object_id),
                origin,
                body,
                line.event_time,
                line.receive_time,
            );
            let hash = MarketEvent::hash_payload(&payload)?;
            let event = MarketEvent::new(
                payload,
                line.event_time,
                line.receive_time,
                line.normalized_time,
                Duration::from_millis(line.uncertainty_ms),
                hash,
                entitlement,
            )?;
            events.push(event);
        }
        if events.is_empty() {
            return Err(Error::invalid(
                "the tape holds no event; a tape with nothing on it seeds nothing and is a wrong \
                 path, not an empty session",
            ));
        }
        let clock = Arc::new(ManualClock::new(events[0].receive_time()));
        Ok(Self {
            events,
            applied: 0,
            clock,
        })
    }

    /// The clock this driver owns. Build a pass loop's own `now` from this,
    /// never from a wall clock, or two runs of the same tape stop agreeing
    /// with each other — the property [`Self::advance`] exists to hold.
    pub fn clock(&self) -> Arc<ManualClock> {
        Arc::clone(&self.clock)
    }

    /// Lines the tape holds, applied or not.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Lines [`Self::seed`] has not yet applied.
    pub fn remaining(&self) -> usize {
        self.events.len() - self.applied
    }

    /// Move the clock to the next unapplied line's own `receive_time` and
    /// return it, or `None` once every line has been applied.
    ///
    /// Never reads a wall clock: the instant always comes from the tape
    /// itself, which [`Self::parse`] has already proven is never earlier
    /// than the one before it, so the clock this returns never moves
    /// backwards either.
    pub fn advance(&mut self) -> Option<Timestamp> {
        let next = self.events.get(self.applied)?.receive_time();
        self.clock.set(next);
        Some(next)
    }

    /// Apply every unapplied line receivable by `until`, in tape order, and
    /// return exactly what was applied.
    ///
    /// A touch is seeded verbatim through [`SimulatedGateway::seed_touch`].
    /// An aggressor is offered through [`SimulatedGateway::seed_aggressor`]
    /// for its stated quantity, but the event this returns carries what the
    /// venue actually matched — never more than the depth resting at the
    /// time, whatever the line asked for (TICK-006) — with the source hash
    /// recomputed so a corrected event's own hash still names its own
    /// payload rather than the request it started from.
    pub fn seed(
        &mut self,
        gateway: &mut SimulatedGateway,
        until: Timestamp,
    ) -> Result<Vec<MarketEvent>> {
        let mut applied = Vec::new();
        while let Some(event) = self.events.get(self.applied) {
            if event.receive_time() > until {
                break;
            }
            let object_id = event.payload().object_id.clone();
            let at = event.event_time();
            let seeded = match &event.payload().body {
                MessageBody::LevelSet {
                    side,
                    price,
                    quantity,
                    ..
                } => {
                    gateway.seed_touch(&object_id, resting_side(*side), *price, *quantity, at)?;
                    event.clone()
                }
                MessageBody::Trade {
                    price,
                    quantity,
                    aggressor,
                    ..
                } => {
                    let hitting = (*aggressor).ok_or_else(|| {
                        Error::invalid(
                            "a tape event names a trade with no aggressor side; a tape line this \
                             driver applies must say which side of the book it takes",
                        )
                    })?;
                    let traded = gateway.seed_aggressor(
                        &object_id,
                        taking_side(hitting),
                        *price,
                        *quantity,
                        at,
                    )?;
                    if traded == *quantity {
                        event.clone()
                    } else {
                        Self::corrected(event, traded)?
                    }
                }
                other => {
                    return Err(Error::invalid(format!(
                        "a tape event names a {} body; this driver seeds only a touch \
                         (level_set) or an aggressor (trade)",
                        other.kind()
                    )));
                }
            };
            applied.push(seeded);
            self.applied += 1;
        }
        Ok(applied)
    }

    /// Rebuild `event` with its trade quantity replaced by what actually
    /// traded, recomputing the source hash so the corrected event's own
    /// `source_hash` still names its own payload.
    fn corrected(event: &MarketEvent, traded: Decimal) -> Result<MarketEvent> {
        let mut payload = event.payload().clone();
        let MessageBody::Trade { quantity, .. } = &mut payload.body else {
            return Err(Error::invalid(
                "a corrected tape event was asked for on a payload that is not a trade",
            ));
        };
        *quantity = traded;
        let hash = MarketEvent::hash_payload(&payload)?;
        MarketEvent::new(
            payload,
            event.event_time(),
            event.receive_time(),
            event.normalized_time(),
            event.uncertainty(),
            hash,
            event.entitlement().clone(),
        )
    }
}
