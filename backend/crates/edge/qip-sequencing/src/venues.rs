//! Cross-venue alignment of recorded ticks (R12-TICK-1124).
//!
//! [`crate::align`] states an order between two events only beyond both
//! clocks' uncertainty, and [`crate::regions::CrossRegionAligner`] relates
//! ticks captured by different cells. Neither related ticks of one instrument
//! printed on two *venues* and read back from the Tick Lake: a lake reader
//! that wanted "the other venue's state when this tick printed" sorted both
//! venues on one timestamp column, which asserts an order the stamps cannot
//! carry. `qip_feature_dag`'s `CrossVenueSpread` has the same exposure — it
//! compares fresh mids with no clock uncertainty at all.
//!
//! [`CrossVenueAligner`] is that reader's seam. Every tick arrives with the
//! normalized instant and half-width the capture path recorded for it, and
//! is related to the latest tick of every *other* venue on the same
//! instrument. An order is stated only beyond the two recorded
//! uncertainties; anything closer is [`Order::Unordered`].
//!
//! **No clock bound is assumed.** Two venues captured by one cell share that
//! cell's clock, so the recorded uncertainty is the whole error. Venues
//! captured by different cells are [`crate::regions::CrossRegionAligner`]'s
//! business, which takes a stated bound as an argument; this type has no such
//! argument and must not be handed ticks from two cells (ADR 0678).
//!
//! Nothing here reads a clock. A replay of the same lake history relates the
//! same ticks in the same order and yields the same alignments.

use crate::align::{Order, Placed, align};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most `(venue, instrument)` keys the aligner holds a latest tick for.
/// Past it, the key whose latest tick is earliest leaves, ties by name.
pub const MAX_VENUE_PARTITIONS: usize = 1_024;

/// One recorded tick, as the aligner needs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueTick {
    pub venue: String,
    pub instrument: String,
    /// The feed's sequence number, carried through to the alignment.
    pub sequence: u64,
    /// The venue's instant in the capturing cell's clock.
    pub normalized_time: Timestamp,
    /// The half-width of the error on `normalized_time`.
    pub uncertainty: Duration,
    /// When the capturing cell learned of it, on that same clock.
    pub arrival: Timestamp,
}

/// One tick related to another venue's latest tick of the same instrument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueAlignment {
    pub venue: String,
    pub other_venue: String,
    pub instrument: String,
    pub sequence: u64,
    pub other_sequence: u64,
    /// The other venue's tick relative to this one; `Before` means it
    /// happened first by more than both ticks' uncertainty.
    pub order_of_other: Order,
    /// This tick's normalized instant minus the other's.
    pub lag_ns: i64,
    /// Both ticks' uncertainties, summed.
    pub lag_uncertainty_ns: i64,
    /// Whether the other tick had arrived by the time this one did. Exact:
    /// both arrivals are on one cell's clock.
    pub other_known_when_this_arrived: bool,
}

#[derive(Clone, Debug)]
struct Latest {
    placed: Placed,
    sequence: u64,
}

/// Relates ticks of one instrument across venues, with a bounded working set.
#[derive(Clone, Debug, Default)]
pub struct CrossVenueAligner {
    latest: BTreeMap<(String, String), Latest>,
}

impl CrossVenueAligner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keys currently held; never more than [`MAX_VENUE_PARTITIONS`].
    pub fn held(&self) -> usize {
        self.latest.len()
    }

    /// Relate `tick` to the latest tick of every other venue on its
    /// instrument, then record it as its own key's latest.
    ///
    /// Refuses a blank venue or instrument and a negative uncertainty, which
    /// is a sign error upstream and would, read as zero, order the tick
    /// against everything. Feed ticks in the order a reader wants "latest"
    /// to mean; the aligner does not sort.
    pub fn relate(&mut self, tick: VenueTick) -> Result<Vec<VenueAlignment>> {
        if tick.venue.trim().is_empty() || tick.instrument.trim().is_empty() {
            return Err(Error::invalid(
                "a tick to align names its venue and its instrument; without them it cannot be \
                 related to another venue's",
            ));
        }
        let placed = Placed::new(tick.normalized_time, tick.uncertainty, tick.arrival)?;
        let related: Vec<VenueAlignment> = self
            .latest
            .iter()
            .filter(|((venue, instrument), _)| {
                *venue != tick.venue && *instrument == tick.instrument
            })
            .map(|((venue, _), other)| {
                let alignment = align(&other.placed, &placed);
                VenueAlignment {
                    venue: tick.venue.clone(),
                    other_venue: venue.clone(),
                    instrument: tick.instrument.clone(),
                    sequence: tick.sequence,
                    other_sequence: other.sequence,
                    order_of_other: alignment.order,
                    lag_ns: alignment.lag.as_nanos(),
                    lag_uncertainty_ns: alignment.lag_uncertainty.as_nanos(),
                    other_known_when_this_arrived: alignment.first_known_when_second_arrived,
                }
            })
            .collect();
        let key = (tick.venue, tick.instrument);
        if !self.latest.contains_key(&key) && self.latest.len() >= MAX_VENUE_PARTITIONS {
            let oldest = self
                .latest
                .iter()
                .min_by(|a, b| {
                    a.1.placed
                        .event_time()
                        .cmp(&b.1.placed.event_time())
                        .then_with(|| a.0.cmp(b.0))
                })
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.latest.remove(&oldest);
            }
        }
        self.latest.insert(
            key,
            Latest {
                placed,
                sequence: tick.sequence,
            },
        );
        Ok(related)
    }
}
