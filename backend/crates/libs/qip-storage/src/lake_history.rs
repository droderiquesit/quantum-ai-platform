//! Reading recorded market history back out of the Tick Lake, for exact
//! replay and training (R12-TICK-2059, 2067, 2076; R12-DATA-1039).
//!
//! The lake had writers and no production reader of market history: the
//! edge node sealed every tick its feed applied, and nothing ever opened a
//! market segment again outside a test. This is the reader a replay job
//! calls, and it is built around three refusals.
//!
//! # A job reads a pinned input, not "whatever is there"
//!
//! [`Lake::pin_market_history`] lists the segments a [`HistoryQuery`]
//! selects, hashes each, and seals the list as a [`Manifest`] before a byte
//! of it is replayed. The replay then reads *the manifest* through
//! [`Lake::materialize`], which re-checks every checksum. So a replay can be
//! repeated later against exactly the bytes it first read, and a segment that
//! changed underneath it fails by name rather than producing a slightly
//! different market.
//!
//! # A job reads only what it is entitled to
//!
//! A segment under an entitlement the query was not granted is listed as
//! withheld and never read — restricted by path, before a byte is fetched,
//! which is the property the lake's partition layout exists to provide.
//!
//! # Every tick keeps both time dimensions
//!
//! A [`RecordedTick`] carries the venue's event time *and* the instant the
//! platform received it — when it became knowable here. A replay sequences
//! on the second, and a feature or label built from the history may read a
//! tick only at or after its `knowable_at`. Collapsing the two is how a
//! backtest trades on a print it had not yet received.
//!
//! # Bounded
//!
//! A query names how many segments and ticks it will hold, and a history
//! larger than either is refused rather than truncated: a replay over a cut
//! prefix of a session is a different session.

use crate::lake::{Entitlement, Lake, Manifest, Partition, RecordClass, SegmentRecord};
use crate::lake_codec::SegmentCodec;
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::MarketMessage;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The prefix every market segment lives under.
pub const MARKET_PREFIX: &str = "lake/class=market/";

/// Which recorded market history a job wants, and what it may hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryQuery {
    /// First partition date, `YYYY-MM-DD`, inclusive.
    pub from_date: String,
    /// Last partition date, `YYYY-MM-DD`, inclusive.
    pub to_date: String,
    /// Venues to read; empty reads every venue.
    pub venues: BTreeSet<String>,
    /// Instruments to read; empty reads every instrument.
    pub instruments: BTreeSet<String>,
    /// Entitlements the job is granted. Empty grants nothing, and every
    /// segment is withheld — the fail-closed reading of "no grant".
    pub granted: BTreeSet<String>,
    /// The most segments the job will pin.
    pub max_segments: usize,
    /// The most ticks the job will hold in memory.
    pub max_ticks: usize,
}

impl HistoryQuery {
    fn validate(&self) -> Result<()> {
        for (name, date) in [("from", &self.from_date), ("to", &self.to_date)] {
            let shaped = date.len() == 10
                && date.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                });
            if !shaped {
                return Err(Error::invalid(format!(
                    "history query {name} date {date:?} must be YYYY-MM-DD"
                )));
            }
        }
        if self.from_date > self.to_date {
            return Err(Error::invalid(format!(
                "history query runs from {} to {}, which is backwards",
                self.from_date, self.to_date
            )));
        }
        if self.max_segments == 0 || self.max_ticks == 0 {
            return Err(Error::invalid(
                "a history query needs a positive segment and tick bound; a zero bound reads \
                 nothing and an absent one reads without limit",
            ));
        }
        Ok(())
    }
}

/// One market tick as the lake recorded it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedTick {
    /// The decoded venue message, with its origin and sequence.
    pub message: MarketMessage,
    /// When the venue says it happened.
    pub event_time: Timestamp,
    /// When the platform received it: the instant it became knowable here.
    pub knowable_at: Timestamp,
    /// The entitlement it was obtained under.
    pub entitlement: String,
    /// The segment key it was read from, for lineage.
    pub segment: String,
    /// What the capture path's clock discipline made of the tick, where the
    /// line recorded it (R12-TICK-1201, R12-TICK-2051). `None` for a line
    /// whose discipline refused the tick, or one written before the lake
    /// carried a clock block: a reader is told the clock is unknown rather
    /// than handed the venue's instant as if it had been disciplined.
    ///
    /// Until 2026-10-24 the reader dropped the block, so a replay from the
    /// lake saw the venue's and the receive instants and never the
    /// normalized time or its uncertainty the lake had been written with.
    #[serde(default)]
    pub clock: Option<RecordedClock>,
}

/// A tick's disciplined clock, as the lake line holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedClock {
    /// The venue's instant in this cell's clock.
    pub normalized_time: Timestamp,
    /// The half-width of the error on `normalized_time`.
    pub uncertainty: Duration,
}

/// The edge node's clock block: present only when both figures are, so a
/// refused or half-written block reads as unknown.
fn edge_clock(value: &serde_json::Value) -> Option<RecordedClock> {
    let clock = value.get("clock")?;
    let normalized = clock.get("normalized_time_ns")?.as_i64()?;
    let uncertainty = clock.get("uncertainty_ns")?.as_i64()?;
    Some(RecordedClock {
        normalized_time: Timestamp::from_nanos(normalized),
        uncertainty: Duration::from_nanos(uncertainty),
    })
}

/// What [`Lake::list_market_history`] selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryListing {
    /// Selected segments, in key order, each with the SHA-256 of its
    /// canonical bytes as listed.
    pub segments: Vec<SegmentRecord>,
    /// Keys selected by the query and held under an entitlement it was not
    /// granted. Never read.
    pub withheld: Vec<String>,
}

/// What a replay job read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordedHistory {
    /// The manifest the history was read through.
    pub manifest: String,
    /// Every tick, in segment order then line order. A replay sequences
    /// them itself; this is storage order, not market order.
    pub ticks: Vec<RecordedTick>,
    /// Every entitlement actually read — the lineage a model trained on this
    /// history must carry (R12-TICK-2091).
    pub entitlements_read: BTreeSet<String>,
    /// Segments in the manifest the reader was not granted.
    pub withheld: Vec<String>,
    /// Lines in a recognised segment that decode as neither lake tick shape.
    /// Counted, never guessed at.
    pub unparsed: usize,
}

/// Split `lake/class=market/venue=V/date=D/instrument=I/entitlement=E/ID.canonical[.zst]`.
fn parse_market_key(key: &str) -> Option<(Partition, SegmentCodec)> {
    let codec = SegmentCodec::of_key(key)?;
    let rest = key.strip_prefix(MARKET_PREFIX)?;
    let mut parts = rest.split('/');
    let venue = parts.next()?.strip_prefix("venue=")?;
    let date = parts.next()?.strip_prefix("date=")?;
    let instrument = parts.next()?.strip_prefix("instrument=")?;
    let entitlement = parts.next()?.strip_prefix("entitlement=")?;
    let _file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let partition = Partition::new(
        RecordClass::Market,
        venue,
        date,
        instrument,
        Entitlement {
            id: entitlement.to_string(),
            raw_retention_permitted: false,
        },
    )
    .ok()?;
    Some((partition, codec))
}

/// Decode one lake line into a tick, accepting both shapes the tree writes:
/// the edge node's (`exchange_time_ns`, `capture_time_ns`, `event`) and
/// [`crate::tick_capture::CapturedMarketEvent`]'s, whose `canonical` is a
/// [`MarketMessage`] where the capture path stored one.
fn decode_line(line: &str, segment: &str, entitlement: &str) -> Option<RecordedTick> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if let Some(event) = value.get("event") {
        let event: MarketEvent = serde_json::from_value(event.clone()).ok()?;
        return Some(RecordedTick {
            message: event.payload().clone(),
            event_time: event.event_time(),
            knowable_at: event.receive_time(),
            entitlement: event.entitlement().dataset().to_string(),
            segment: segment.to_string(),
            clock: edge_clock(&value),
        });
    }
    let captured: crate::tick_capture::CapturedMarketEvent = serde_json::from_value(value).ok()?;
    let message: MarketMessage = serde_json::from_value(captured.canonical.clone()).ok()?;
    Some(RecordedTick {
        message,
        event_time: captured.event_time,
        knowable_at: captured.receive_time,
        entitlement: if captured.entitlement.is_empty() {
            entitlement.to_string()
        } else {
            captured.entitlement
        },
        segment: segment.to_string(),
        clock: Some(RecordedClock {
            normalized_time: captured.normalized_time,
            uncertainty: captured.uncertainty,
        }),
    })
}

impl Lake<'_> {
    /// List the market segments `query` selects, hashing each. Reads only
    /// segments under a granted entitlement; refuses a listing larger than
    /// `query.max_segments` rather than cutting it.
    pub fn list_market_history(&self, query: &HistoryQuery) -> Result<HistoryListing> {
        query.validate()?;
        let mut listing = HistoryListing::default();
        for key in self.store.list(MARKET_PREFIX)? {
            let Some((partition, codec)) = parse_market_key(&key) else {
                continue;
            };
            if partition.date < query.from_date || partition.date > query.to_date {
                continue;
            }
            if !query.venues.is_empty() && !query.venues.contains(&partition.venue) {
                continue;
            }
            if !query.instruments.is_empty() && !query.instruments.contains(&partition.instrument) {
                continue;
            }
            if !query.granted.contains(&partition.entitlement.id) {
                listing.withheld.push(key);
                continue;
            }
            if listing.segments.len() >= query.max_segments {
                return Err(Error::invalid(format!(
                    "the history between {} and {} holds more than {} segment(s); narrow the \
                     query or raise its bound — a replay over a cut prefix is a different market",
                    query.from_date, query.to_date, query.max_segments
                )));
            }
            let Some(stored) = self.store.get(&key)? else {
                continue;
            };
            let canonical = codec.decode(&stored)?;
            listing.segments.push(SegmentRecord {
                partition,
                key,
                canonical_sha256: sha256_hex(&canonical),
                raw_sha256: None,
                raw_key: None,
                codec,
            });
        }
        Ok(listing)
    }

    /// List the history `query` selects and seal it as manifest `name`, so
    /// the replay that follows — and any later re-run of it — reads exactly
    /// these bytes. Withheld segments are not in the manifest.
    pub fn pin_market_history(&self, name: &str, query: &HistoryQuery) -> Result<HistoryListing> {
        let listing = self.list_market_history(query)?;
        self.write_manifest(&Manifest {
            name: name.to_string(),
            segments: listing.segments.clone(),
        })?;
        Ok(listing)
    }

    /// Read a pinned history back as ticks, verifying every checksum through
    /// [`Lake::materialize`]. Refuses a history holding more than `max_ticks`.
    pub fn read_market_history(
        &self,
        manifest: &str,
        granted: &BTreeSet<String>,
        max_ticks: usize,
    ) -> Result<RecordedHistory> {
        let granted: Vec<&str> = granted.iter().map(String::as_str).collect();
        let materialized = self.materialize(manifest, &granted)?;
        let mut history = RecordedHistory {
            manifest: manifest.to_string(),
            entitlements_read: materialized.entitlements_read,
            withheld: materialized.withheld,
            ..RecordedHistory::default()
        };
        for (record, bytes) in materialized.segments {
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                Error::invalid(format!("segment {} is not UTF-8: {error}", record.key))
            })?;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                match decode_line(line, &record.key, &record.partition.entitlement.id) {
                    Some(tick) => {
                        if history.ticks.len() >= max_ticks {
                            return Err(Error::invalid(format!(
                                "manifest {manifest} holds more than {max_ticks} tick(s); a \
                                 replay over a cut prefix of a session is a different session"
                            )));
                        }
                        history.ticks.push(tick);
                    }
                    None => history.unparsed += 1,
                }
            }
        }
        Ok(history)
    }
}
