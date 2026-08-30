//! Liquidity topology: where liquidity actually lives, across venues, and how
//! that map is shifting.
//!
//! The rest of the platform answers narrower questions. A venue state in
//! `qip-orderbook` knows what one venue quotes; the arbitrage seam's
//! `LiquiditySource` answers what a given size would cost at one venue; the
//! capacity model in `qip-capital` answers how much alpha survives size. None
//! of them answer the global question this module owns: for this instrument,
//! which venues hold the depth, how concentrated is that depth, and how has the
//! distribution been moving? Capacity says how much can be traded; topology
//! says where the size can physically go. Nothing here prices impact, and
//! nothing in the capacity model knows about venues — the two are complements,
//! not duplicates.
//!
//! **Bitemporal discipline.** An observation carries the instant the depth was
//! observed at the venue; [`LiquidityTopology::absorb`] additionally takes when
//! this platform learned of it, exactly as `WorldModel::absorb_bar` does, and
//! clamps a `known_at` earlier than the observation forward for the same
//! reason: depth cannot have been knowable before it was observed, and the
//! combination always means a clock or a parser rather than a fast feed. Every
//! read takes both a `valid_at` and a `known_at`, so a backtest asking "what
//! did the map look like in March, as known in March?" gets an answer free of
//! look-ahead.
//!
//! **Honest staleness.** The edge cell enforces that a stale book trades
//! nothing: `qip-orderbook`'s `VenueState` serves no derived price while its
//! book is awaiting a snapshot, and `qip-edge`'s `CellLiquidity` answers `None`
//! rather than serving depth it no longer believes. This module applies the
//! same rule at map scale. An observation older than the stated bound stops
//! counting; a venue whose only observations have aged out is reported as
//! *aged out* so the caller can see the basis shrank; and an instrument whose
//! entire map has aged out answers `None` — unknown — never a stale map
//! presented as current and never a zero that looks like an observed absence
//! of liquidity. Zero observed depth is knowledge; no fresh observation is
//! ignorance; the API keeps the two apart.
//!
//! **Total versus usable.** Visible depth and tradeable depth are different
//! numbers. A venue that is halted, closed or unreachable still appears on the
//! map with the depth it last showed — where liquidity *lives* is exactly what
//! this module is for, and a venue going dark while holding half the depth is
//! the single-point-of-failure fact worth surfacing — but its depth is never
//! summed into [`LiquidityMap::usable_bid_depth`] or
//! [`LiquidityMap::usable_ask_depth`], so a caller cannot mistake unreachable
//! depth for a tradeable figure.
//!
//! **The failure designed against.** A topology that double-counts (the same
//! venue observed twice summed as two venues), presents a stale share as
//! current, or reports concentration over a half-aged-out map without saying
//! so. Storage is keyed by instrument and venue so a venue contributes exactly
//! its latest known observation; every map carries the two timestamps it was
//! computed at, the staleness bound applied, and the number of venues that
//! aged out of its basis; every concentration figure carries the number of
//! venues it was computed over.
//!
//! Depth and spreads are [`Decimal`] and exact; shares, indices and drifts are
//! `f64` statistics and named as such.

use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{OrderBook, Side};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How stale a depth observation may be before it stops counting, unless the
/// topology is built with an explicit bound.
///
/// Five minutes: long enough to bridge a snapshot cadence, short enough that a
/// map built from it still describes the current session rather than a
/// remembered one.
pub const DEFAULT_MAX_STALENESS: Duration = Duration::from_mins(5);

/// One venue's visible depth in one instrument at one instant.
///
/// Depth is size at or near the touch — the caller states how many levels it
/// summed, this type only records the result. `spread` is optional because a
/// one-sided book has no spread, and inventing one would be a number rather
/// than a fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DepthObservation {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The venue's status when the depth was observed. Decides whether this
    /// depth can count toward the usable figures.
    pub status: VenueStatus,
    /// Resting size on the bid side, at or near the touch.
    pub bid_depth: Decimal,
    /// Resting size on the ask side, at or near the touch.
    pub ask_depth: Decimal,
    /// Best ask minus best bid, where both sides existed.
    pub spread: Option<Decimal>,
    /// Venue time of the observation — when this depth was true.
    pub observed_at: Timestamp,
}

impl DepthObservation {
    pub fn new(
        object_id: ObjectId,
        venue: VenueId,
        status: VenueStatus,
        bid_depth: Decimal,
        ask_depth: Decimal,
        observed_at: Timestamp,
    ) -> Self {
        Self {
            object_id,
            venue,
            status,
            bid_depth,
            ask_depth,
            spread: None,
            observed_at,
        }
    }

    pub fn with_spread(mut self, spread: Decimal) -> Self {
        self.spread = Some(spread);
        self
    }

    /// Summarise a depth snapshot into an observation, taking `levels` price
    /// levels per side.
    ///
    /// Uses the book's own depth arithmetic rather than reimplementing the
    /// walk, so this producer and every consumer of [`OrderBook`] agree on
    /// what "depth at the touch" means.
    pub fn from_book(book: &OrderBook, levels: usize, status: VenueStatus) -> Self {
        Self {
            object_id: book.object_id.clone(),
            venue: VenueId::new(book.venue.clone()),
            status,
            bid_depth: book.depth(Side::Buy, levels),
            ask_depth: book.depth(Side::Sell, levels),
            spread: book.spread(),
            observed_at: book.at,
        }
    }
}

/// One observation plus when this platform could first have acted on it.
#[derive(Clone, Debug, PartialEq)]
struct StoredObservation {
    observation: DepthObservation,
    available_at: Timestamp,
}

/// One venue's entry on a [`LiquidityMap`], with its share of the visible
/// depth.
///
/// The shares are `f64` statistics over the map's counted basis; the depths
/// are the exact figures they were computed from, and `observed_at` is the
/// as-of of this venue's contribution — reported per venue because the map's
/// venues were not all observed at the same instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueDepth {
    pub venue: VenueId,
    pub status: VenueStatus,
    /// When this venue's depth was observed.
    pub observed_at: Timestamp,
    pub bid_depth: Decimal,
    pub ask_depth: Decimal,
    pub spread: Option<Decimal>,
    /// Statistic: this venue's fraction of the map's total bid depth.
    pub bid_share: f64,
    /// Statistic: this venue's fraction of the map's total ask depth.
    pub ask_share: f64,
    /// Statistic: this venue's fraction of the map's total depth, both sides.
    pub depth_share: f64,
}

impl VenueDepth {
    /// Whether an order could be sent to this venue when it was observed.
    pub fn accepts_orders(&self) -> bool {
        self.status.accepts_orders()
    }
}

/// A Herfindahl-style concentration statistic over venue shares of depth.
///
/// The index is the sum of squared shares: `1/n` when depth is spread evenly
/// over `n` venues, `1.0` when a single venue holds everything. The
/// interpretation, written down once: concentrated liquidity is
/// single-point-of-failure liquidity — a map with a high index depends on one
/// venue staying open, reachable and willing, and an outage there is not a
/// detour but a closure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Concentration {
    /// Statistic: the Herfindahl index over venue depth shares, in `(0, 1]`.
    pub herfindahl: f64,
    /// How many venues actually held depth — the basis the index was computed
    /// over. An index reported without this number hides a shrunken basis.
    pub venue_count: usize,
}

impl Concentration {
    /// Statistic: the number of equal-size venues that would produce this
    /// index. An intuitive reading of the Herfindahl: 4.0 means "as diverse as
    /// four equal venues", 1.1 means "one venue, in effect".
    pub fn effective_venues(&self) -> f64 {
        1.0 / self.herfindahl
    }

    pub fn describe(&self) -> String {
        format!(
            "herfindahl {:.3} over {} venues ({:.1} effective)",
            self.herfindahl,
            self.venue_count,
            self.effective_venues()
        )
    }
}

/// Where liquidity in one instrument lived, as of a stated pair of instants.
///
/// Only fresh observations are counted; the basis is fully reported — how many
/// venues were counted, how many aged out, the staleness bound applied and the
/// two timestamps the map was computed at — so no statistic read off this map
/// can quietly rest on a smaller basis than the caller assumes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityMap {
    pub object_id: ObjectId,
    /// The instant the map describes.
    pub valid_at: Timestamp,
    /// What was known when it was computed.
    pub known_at: Timestamp,
    /// The staleness bound the basis was filtered by.
    pub max_staleness: Duration,
    /// The counted venues, largest depth share first.
    pub venues: Vec<VenueDepth>,
    /// Venues known for this instrument whose latest observation was older
    /// than the bound. They are not in the totals; they are counted here so
    /// the shrunken basis is visible.
    pub venues_aged_out: usize,
    /// Visible bid depth over every counted venue, whatever its status.
    pub total_bid_depth: Decimal,
    /// Visible ask depth over every counted venue, whatever its status.
    pub total_ask_depth: Decimal,
    /// Bid depth at counted venues currently accepting orders. The only
    /// figure a router may treat as tradeable.
    pub usable_bid_depth: Decimal,
    /// Ask depth at counted venues currently accepting orders.
    pub usable_ask_depth: Decimal,
}

impl LiquidityMap {
    /// Venues counted into the map.
    pub fn venue_count(&self) -> usize {
        self.venues.len()
    }

    /// Counted venues that were accepting orders.
    pub fn usable_venue_count(&self) -> usize {
        self.venues.iter().filter(|v| v.accepts_orders()).count()
    }

    /// Venues known for the instrument at this instant, counted or aged out.
    pub fn known_venue_count(&self) -> usize {
        self.venues.len() + self.venues_aged_out
    }

    /// Statistic: one venue's share of the visible depth, both sides.
    pub fn share_of(&self, venue: &VenueId) -> Option<f64> {
        self.venues
            .iter()
            .find(|v| &v.venue == venue)
            .map(|v| v.depth_share)
    }

    /// Concentration of the visible depth, over every counted venue.
    ///
    /// `None` when the counted venues held no depth at all: an index over an
    /// empty market would be a statistic about nothing.
    pub fn concentration(&self) -> Option<Concentration> {
        Self::herfindahl(self.venues.iter())
    }

    /// Concentration of the depth at venues accepting orders.
    ///
    /// Reported separately because the two can diverge sharply: a map that
    /// looks diversified can be one venue deep the moment the others are
    /// halted or unreachable, and this is the figure that says so.
    pub fn usable_concentration(&self) -> Option<Concentration> {
        Self::herfindahl(self.venues.iter().filter(|v| v.accepts_orders()))
    }

    fn herfindahl<'a>(venues: impl Iterator<Item = &'a VenueDepth>) -> Option<Concentration> {
        let mut depths = Vec::new();
        let mut total = 0.0f64;
        for venue in venues {
            let depth = (venue.bid_depth + venue.ask_depth).to_f64();
            if depth > 0.0 {
                depths.push(depth);
                total += depth;
            }
        }
        if total <= 0.0 {
            return None;
        }
        let herfindahl = depths
            .iter()
            .map(|depth| {
                let share = depth / total;
                share * share
            })
            .sum();
        Some(Concentration {
            herfindahl,
            venue_count: depths.len(),
        })
    }

    /// A sentence an operator can read next to the map.
    pub fn describe(&self) -> String {
        let concentration = self
            .concentration()
            .map_or_else(|| "no visible depth".to_string(), |c| c.describe());
        format!(
            "{} liquidity as of {} (known at {}): {} venues counted, {} accepting orders, \
             {} aged out; {}",
            self.object_id,
            self.valid_at.to_rfc3339(),
            self.known_at.to_rfc3339(),
            self.venue_count(),
            self.usable_venue_count(),
            self.venues_aged_out,
            concentration
        )
    }
}

/// One venue's change in depth share over a drift window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueShift {
    pub venue: VenueId,
    /// Statistic: depth share at the start of the window. Zero when the venue
    /// was not on the earlier map.
    pub previous_share: f64,
    /// Statistic: depth share at the end of the window. Zero when the venue
    /// has left the map.
    pub current_share: f64,
    /// Statistic: `current_share - previous_share`.
    pub change: f64,
}

/// How an instrument's liquidity map shifted over a stated window.
///
/// This is the "understanding" the topology exists to produce: a venue gaining
/// or losing share is the market rearranging itself, and a sudden rise in
/// concentration is an alert-worthy fact the opportunity engine can consume.
/// Both ends of the window are read with the same `known_at`, so a backtest
/// passing the knowledge instant of its decision sees the drift as it was
/// knowable then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityDrift {
    pub object_id: ObjectId,
    pub window: Duration,
    /// Start of the window.
    pub from: Timestamp,
    /// End of the window — the instant the drift describes.
    pub to: Timestamp,
    /// The knowledge instant both ends were read at.
    pub known_at: Timestamp,
    /// Per-venue share changes, largest absolute change first.
    pub venues: Vec<VenueShift>,
    /// Statistic: Herfindahl at `to` minus Herfindahl at `from`. `None` when
    /// either end held no depth to concentrate.
    pub concentration_change: Option<f64>,
    /// Venues counted at the start of the window.
    pub from_venue_count: usize,
    /// Venues counted at the end of the window.
    pub to_venue_count: usize,
}

impl LiquidityDrift {
    /// Shifts whose absolute change meets `threshold`, largest first.
    pub fn material_shifts(&self, threshold: f64) -> Vec<&VenueShift> {
        self.venues
            .iter()
            .filter(|s| s.change.abs() >= threshold)
            .collect()
    }

    pub fn describe(&self) -> String {
        let concentration = self.concentration_change.map_or_else(
            || "concentration change unknown".to_string(),
            |change| format!("concentration {change:+.3}"),
        );
        let leader = self.venues.first().map_or_else(
            || "no venues".to_string(),
            |s| format!("{} {:+.1}pp", s.venue, s.change * 100.0),
        );
        format!(
            "{} over {:?} to {}: {} venues ({} before), largest shift {leader}, {concentration}",
            self.object_id,
            self.window,
            self.to.to_rfc3339(),
            self.to_venue_count,
            self.from_venue_count,
        )
    }
}

/// The global map of where liquidity lives, per instrument, across venues.
///
/// Absorbs per-venue depth observations and answers point-in-time questions
/// about the distribution. Keyed by instrument and venue, so a venue observed
/// twice counts once, at its latest observation known by the queried instant.
#[derive(Debug)]
pub struct LiquidityTopology {
    max_staleness: Duration,
    /// `(object, venue)` to observations, kept in observed-time order.
    observations: BTreeMap<(String, String), Vec<StoredObservation>>,
}

impl Default for LiquidityTopology {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_STALENESS)
    }
}

impl LiquidityTopology {
    /// A topology whose maps count observations no older than `max_staleness`.
    pub fn new(max_staleness: Duration) -> Self {
        Self {
            max_staleness,
            observations: BTreeMap::new(),
        }
    }

    /// The staleness bound applied to every map.
    pub fn max_staleness(&self) -> Duration {
        self.max_staleness
    }

    /// Observations held, across all instruments and venues.
    pub fn observation_count(&self) -> usize {
        self.observations.values().map(Vec::len).sum()
    }

    /// Instruments ever observed, in identifier order.
    pub fn instruments(&self) -> Vec<ObjectId> {
        let mut out: Vec<ObjectId> = Vec::new();
        for (object, _) in self.observations.keys() {
            if out.last().map(AsRef::as_ref) != Some(object.as_str()) {
                out.push(ObjectId::from_string(object.clone()));
            }
        }
        out
    }

    /// Venues ever observed quoting an instrument, in identifier order.
    pub fn venues_observed(&self, object: &ObjectId) -> Vec<VenueId> {
        self.for_object(object)
            .map(|(venue, _)| VenueId::new(venue))
            .collect()
    }

    /// Absorb one observation, stating when this platform learned of it.
    ///
    /// `known_at` is the arrival time, not the observation time — the same
    /// discipline as `WorldModel::absorb_bar`, for the same reason: stamping
    /// availability at the venue clock makes the map readable before the data
    /// that built it had arrived. A `known_at` earlier than `observed_at` is
    /// clamped forward, because depth cannot have been knowable before it was
    /// observed.
    ///
    /// A re-observation of the same venue at the same instant replaces the
    /// earlier record (a restatement); at a different instant it extends the
    /// venue's history, of which any map counts exactly one entry. Negative
    /// depth and a negative spread are refused: the first is a parser bug that
    /// would corrupt every share downstream, and the second is a crossed book,
    /// which `OrderBook::validate` already rules must not reach a decision.
    pub fn absorb(&mut self, observation: DepthObservation, known_at: Timestamp) -> Result<()> {
        if observation.bid_depth.is_negative() || observation.ask_depth.is_negative() {
            return Err(Error::invalid(format!(
                "depth observation for {} at {} carries a negative depth",
                observation.object_id, observation.venue
            )));
        }
        if observation.spread.is_some_and(|s| s.is_negative()) {
            return Err(Error::invalid(format!(
                "depth observation for {} at {} carries a negative spread: a crossed book \
                 must not reach the map",
                observation.object_id, observation.venue
            )));
        }

        let available_at = if known_at < observation.observed_at {
            observation.observed_at
        } else {
            known_at
        };
        let key = (
            observation.object_id.as_str().to_string(),
            observation.venue.as_str().to_string(),
        );
        let stored = StoredObservation {
            observation,
            available_at,
        };
        let series = self.observations.entry(key).or_default();
        match series.binary_search_by_key(&stored.observation.observed_at.as_nanos(), |o| {
            o.observation.observed_at.as_nanos()
        }) {
            Ok(position) => series[position] = stored,
            Err(position) => series.insert(position, stored),
        }
        Ok(())
    }

    /// The map for an instrument as of a point in both time dimensions.
    ///
    /// Each venue contributes its latest observation that was observed at or
    /// before `valid_at` and had arrived by `known_at` — once, however many
    /// times it was observed. Venues whose latest such observation is older
    /// than the staleness bound are excluded from every figure and counted in
    /// [`LiquidityMap::venues_aged_out`].
    ///
    /// `None` when no venue has a fresh observation: an instrument whose
    /// entire map has aged out — or was never observed — is unknown, and
    /// unknown is not an empty map.
    pub fn map(
        &self,
        object: &ObjectId,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<LiquidityMap> {
        let mut counted: Vec<&DepthObservation> = Vec::new();
        let mut aged_out = 0usize;
        for (_, series) in self.for_object(object) {
            let Some(stored) = series
                .iter()
                .rfind(|o| o.observation.observed_at <= valid_at && o.available_at <= known_at)
            else {
                continue;
            };
            if valid_at.since(stored.observation.observed_at) > self.max_staleness {
                aged_out += 1;
            } else {
                counted.push(&stored.observation);
            }
        }
        if counted.is_empty() {
            return None;
        }

        let total_bid: Decimal = counted.iter().map(|o| o.bid_depth).sum();
        let total_ask: Decimal = counted.iter().map(|o| o.ask_depth).sum();
        let usable = |side: fn(&DepthObservation) -> Decimal| -> Decimal {
            counted
                .iter()
                .filter(|o| o.status.accepts_orders())
                .map(|o| side(o))
                .sum()
        };
        let usable_bid = usable(|o| o.bid_depth);
        let usable_ask = usable(|o| o.ask_depth);

        let bid_total = total_bid.to_f64();
        let ask_total = total_ask.to_f64();
        let combined_total = (total_bid + total_ask).to_f64();
        let mut venues: Vec<VenueDepth> = counted
            .into_iter()
            .map(|o| {
                let bid = o.bid_depth.to_f64();
                let ask = o.ask_depth.to_f64();
                VenueDepth {
                    venue: o.venue.clone(),
                    status: o.status,
                    observed_at: o.observed_at,
                    bid_depth: o.bid_depth,
                    ask_depth: o.ask_depth,
                    spread: o.spread,
                    bid_share: share(bid, bid_total),
                    ask_share: share(ask, ask_total),
                    depth_share: share(bid + ask, combined_total),
                }
            })
            .collect();
        venues.sort_by(|a, b| {
            b.depth_share
                .partial_cmp(&a.depth_share)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.venue.cmp(&b.venue))
        });

        Some(LiquidityMap {
            object_id: object.clone(),
            valid_at,
            known_at,
            max_staleness: self.max_staleness,
            venues,
            venues_aged_out: aged_out,
            total_bid_depth: total_bid,
            total_ask_depth: total_ask,
            usable_bid_depth: usable_bid,
            usable_ask_depth: usable_ask,
        })
    }

    /// The map given everything known now.
    pub fn current_map(&self, object: &ObjectId, now: Timestamp) -> Option<LiquidityMap> {
        self.map(object, now, now)
    }

    /// How the map shifted over the `window` ending at `valid_at`, as knowable
    /// at `known_at`.
    ///
    /// `None` when either end of the window is unknown — a drift from a map
    /// that was never seen is a first observation, not a drift — and for a
    /// non-positive window, which has no interval to drift over. A venue on
    /// only one end appears with a zero share on the other: entering the map
    /// is a gain from nothing and leaving it is a loss of everything, and both
    /// ends' venue counts are reported so the changed basis is visible.
    pub fn drift(
        &self,
        object: &ObjectId,
        window: Duration,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<LiquidityDrift> {
        if window.as_nanos() <= 0 {
            return None;
        }
        let to_map = self.map(object, valid_at, known_at)?;
        let from_at = valid_at.saturating_sub(window);
        let from_map = self.map(object, from_at, known_at)?;

        let mut shares: BTreeMap<&VenueId, (f64, f64)> = BTreeMap::new();
        for venue in &from_map.venues {
            shares.entry(&venue.venue).or_insert((0.0, 0.0)).0 = venue.depth_share;
        }
        for venue in &to_map.venues {
            shares.entry(&venue.venue).or_insert((0.0, 0.0)).1 = venue.depth_share;
        }
        let mut venues: Vec<VenueShift> = shares
            .into_iter()
            .map(|(venue, (previous, current))| VenueShift {
                venue: venue.clone(),
                previous_share: previous,
                current_share: current,
                change: current - previous,
            })
            .collect();
        venues.sort_by(|a, b| {
            b.change
                .abs()
                .partial_cmp(&a.change.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.venue.cmp(&b.venue))
        });

        let concentration_change = match (from_map.concentration(), to_map.concentration()) {
            (Some(from), Some(to)) => Some(to.herfindahl - from.herfindahl),
            _ => None,
        };

        Some(LiquidityDrift {
            object_id: object.clone(),
            window,
            from: from_at,
            to: valid_at,
            known_at,
            venues,
            concentration_change,
            from_venue_count: from_map.venue_count(),
            to_venue_count: to_map.venue_count(),
        })
    }

    /// The per-venue series for one instrument.
    fn for_object(
        &self,
        object: &ObjectId,
    ) -> impl Iterator<Item = (&str, &Vec<StoredObservation>)> {
        let start = (object.as_str().to_string(), String::new());
        self.observations
            .range(start..)
            .take_while(move |((observed_object, _), _)| observed_object == object.as_str())
            .map(|((_, venue), series)| (venue.as_str(), series))
    }
}

/// Statistic: `part` as a fraction of `whole`, zero when there is no whole.
fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { part / whole } else { 0.0 }
}
