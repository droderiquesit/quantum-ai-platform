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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

impl MarketStatePack {
    /// Build a pack over `events`, in tape order. Refuses an empty or inverted
    /// interval, a non-positive horizon, an empty or blank lineage, and an
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
