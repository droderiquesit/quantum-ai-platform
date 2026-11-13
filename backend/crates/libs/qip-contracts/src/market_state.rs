//! Market state pack contract (CONTRACT-019).
//!
//! A pack is a reconstructed book, flow, liquidity or regime state — or a
//! learned embedding of one — over a bounded interval. It exists so a
//! consumer can tell a state built over a clean tape from one built over a
//! tape with a hole in it: the integrity flags are computed here, from the
//! events the pack was built over, and a caller cannot hand in a pack that
//! claims to be clean. Gap detection reuses
//! [`MarketEvent::sequence_gap`], so the pack and the event never disagree
//! about what a gap is.

use crate::market_event::MarketEvent;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the pack reconstructs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    Book,
    Flow,
    Liquidity,
    Regime,
    Embedding,
}

/// A defect in the tape the pack was built over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityFlag {
    /// `missing` messages are absent from `stream` (venue/feed/partition).
    SequenceGap { stream: String, missing: u64 },
}

/// A time-bounded reconstructed market state.
///
/// Decoding goes back through the shape refusals of [`MarketStatePack::new`]
/// ([`MarketStatePackWire`]); a derived `Deserialize` admitted an inverted
/// interval, a zero horizon or a blank lineage from any line that parsed,
/// so a pack refused at construction could still arrive from another brain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "MarketStatePackWire", try_from = "MarketStatePackWire")]
pub struct MarketStatePack {
    kind: StateKind,
    from: Timestamp,
    to: Timestamp,
    /// How far ahead of `to` the state is meant to stay valid.
    horizon: Duration,
    /// The features (or upstream artifacts) the state was derived from.
    feature_lineage: Vec<String>,
    integrity: Vec<IntegrityFlag>,
}

/// [`MarketStatePack`]'s wire form, field for field.
///
/// Every field is required: none carries `#[serde(default)]`, so a line
/// missing one is refused by the decoder rather than filled with a value
/// nobody stated — an absent `integrity` read as `[]` would be a clean tape
/// claimed by omission.
///
/// What decode can and cannot re-check. The interval, the horizon and the
/// lineage are re-run through the same checks as [`MarketStatePack::new`]
/// ([`MarketStatePack::check_shape`]). The integrity flags cannot be
/// recomputed — the events they were derived from do not travel with the
/// pack — so decode refuses only a flag that could not have been computed:
/// a gap of zero messages, or one on a blank stream. A producer that drops a
/// flag before encoding is not caught here; the carrier's provenance is
/// what answers that, and this type does not pretend otherwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketStatePackWire {
    kind: StateKind,
    from: Timestamp,
    to: Timestamp,
    horizon: Duration,
    feature_lineage: Vec<String>,
    integrity: Vec<IntegrityFlag>,
}

impl From<MarketStatePack> for MarketStatePackWire {
    fn from(pack: MarketStatePack) -> Self {
        let MarketStatePack {
            kind,
            from,
            to,
            horizon,
            feature_lineage,
            integrity,
        } = pack;
        Self {
            kind,
            from,
            to,
            horizon,
            feature_lineage,
            integrity,
        }
    }
}

impl TryFrom<MarketStatePackWire> for MarketStatePack {
    type Error = Error;

    fn try_from(wire: MarketStatePackWire) -> Result<Self> {
        Self::check_shape(wire.from, wire.to, wire.horizon, &wire.feature_lineage)?;
        for flag in &wire.integrity {
            let IntegrityFlag::SequenceGap { stream, missing } = flag;
            if *missing == 0 || stream.trim().is_empty() {
                return Err(Error::invalid(
                    "a market state pack line carries a sequence-gap flag of zero messages or \
                     on a blank stream; no gap check computes either, so the line was not \
                     written by MarketStatePack::new",
                ));
            }
        }
        Ok(Self {
            kind: wire.kind,
            from: wire.from,
            to: wire.to,
            horizon: wire.horizon,
            feature_lineage: wire.feature_lineage,
            integrity: wire.integrity,
        })
    }
}

impl MarketStatePack {
    /// The refusals a pack's own fields must pass, shared by the
    /// constructor and the decoder so the two cannot drift apart.
    fn check_shape(
        from: Timestamp,
        to: Timestamp,
        horizon: Duration,
        feature_lineage: &[String],
    ) -> Result<()> {
        if to <= from {
            return Err(Error::invalid(
                "a market state pack needs `from` strictly before `to`; give the interval it \
                 was reconstructed over",
            ));
        }
        if horizon.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a market state pack needs a positive horizon; state how long it stays valid",
            ));
        }
        if feature_lineage.is_empty() || feature_lineage.iter().any(|l| l.trim().is_empty()) {
            return Err(Error::invalid(
                "a market state pack needs a non-blank feature lineage naming what it was \
                 derived from",
            ));
        }
        Ok(())
    }

    /// Build a pack over `events`, in tape order. Refuses an empty `events`
    /// list, an empty or inverted interval, a non-positive horizon, an empty or blank lineage, and an
    /// event outside the interval — a pack that quietly included events from
    /// beyond its own bounds would not be time-bounded.
    pub fn new(
        kind: StateKind,
        from: Timestamp,
        to: Timestamp,
        horizon: Duration,
        feature_lineage: Vec<String>,
        events: &[MarketEvent],
    ) -> Result<Self> {
        Self::check_shape(from, to, horizon, &feature_lineage)?;
        if events.is_empty() {
            return Err(Error::invalid(
                "a market state pack needs at least one event; an interval with no drained \
                 events records no tape, and a pack over none would report a clean tape nobody \
                 read (ADR 0151 decision 2)",
            ));
        }
        let mut last: BTreeMap<String, &MarketEvent> = BTreeMap::new();
        let mut integrity = Vec::new();
        for event in events {
            if event.event_time() < from || event.event_time() > to {
                return Err(Error::invalid(format!(
                    "event on stream {} falls outside the pack's interval; build the pack \
                     over the events of its own window",
                    event.origin().stream_key()
                )));
            }
            let stream = event.origin().stream_key();
            if let Some(previous) = last.get(&stream)
                && let Some(missing) = event.sequence_gap(previous)
            {
                integrity.push(IntegrityFlag::SequenceGap {
                    stream: stream.clone(),
                    missing,
                });
            }
            last.insert(stream, event);
        }
        Ok(Self {
            kind,
            from,
            to,
            horizon,
            feature_lineage,
            integrity,
        })
    }

    pub const fn kind(&self) -> StateKind {
        self.kind
    }

    pub const fn from(&self) -> Timestamp {
        self.from
    }

    pub const fn to(&self) -> Timestamp {
        self.to
    }

    pub const fn horizon(&self) -> Duration {
        self.horizon
    }

    pub fn feature_lineage(&self) -> &[String] {
        &self.feature_lineage
    }

    pub fn integrity(&self) -> &[IntegrityFlag] {
        &self.integrity
    }
}
