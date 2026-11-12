//! Cross-region alignment of captured ticks (R12-TICK-2048, ADR 0438).
//!
//! A cell aligns each tick against every other partition it captured, on its
//! own clock ([`crate::capture`], and the edge node's lake sink). That says
//! nothing about a tick another cell captured: the two normalized instants
//! sit on two cells' clocks, and the offset between those clocks was never
//! measured by either. Sorting the lake's lines from two regions on their
//! timestamp column asserts an order neither clock supports, and a lead/lag
//! model trained on it learns the inter-cell clock error as a reaction.
//!
//! [`CrossRegionAligner`] is where captured ticks from more than one cell
//! meet. Each tick is widened by the bound its cell's clock is stated to hold
//! against the common reference — twice over a pair, once per cell — and an
//! order is stated only beyond that. The bound is an argument, never a
//! default: no figure for it has been stated by anyone who measured one, and
//! a desk that has none computes no cross-region order at all rather than
//! ordering on an assumed one.
//!
//! Arrival order is deliberately not reported across cells. Within one cell
//! both arrivals are on one clock and their order is exact; across two cells
//! they are on two clocks and it is not.

use crate::align::{Order, Placed, align};
use qip_contracts::sync::{ClockSource, SyncMetadata, SyncOrder};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most `(cell, partition)` keys the aligner holds a latest tick for.
/// Past it, the key whose latest tick is earliest leaves, ties by name.
pub const MAX_REGION_PARTITIONS: usize = 1_024;

/// One tick related to another cell's latest tick on a partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionAlignment {
    pub region: String,
    pub cell: String,
    pub venue: String,
    pub instrument: String,
    pub other_region: String,
    pub other_cell: String,
    pub other_venue: String,
    pub other_instrument: String,
    pub other_sequence: u64,
    /// The other tick relative to this one; `before` means it happened first
    /// by more than both ticks' uncertainty and both cells' clock bound.
    pub order_of_other: SyncOrder,
    /// This tick's normalized instant minus the other's.
    pub lag_ns: i64,
    /// Both ticks' uncertainty plus each cell's clock bound.
    pub lag_uncertainty_ns: i64,
}

impl RegionAlignment {
    /// Whether the two ticks were captured in different regions, rather than
    /// by two cells of one region.
    pub fn crosses_regions(&self) -> bool {
        self.region != self.other_region
    }
}

#[derive(Clone, Debug)]
struct Latest {
    placed: Placed,
    region: String,
    venue: String,
    instrument: String,
    sequence: u64,
}

/// Relates ticks captured by different cells, with a bounded working set.
#[derive(Clone, Debug)]
pub struct CrossRegionAligner {
    cell_clock_bound: Duration,
    latest: BTreeMap<(String, String), Latest>,
}

impl CrossRegionAligner {
    /// An aligner that takes every cell's clock to lie within
    /// `cell_clock_bound` of the common reference. Refuses a negative bound:
    /// a sign error upstream, and read as zero it would order everything.
    pub fn new(cell_clock_bound: Duration) -> Result<Self> {
        if cell_clock_bound < Duration::ZERO {
            return Err(Error::invalid(format!(
                "a cell clock bound of {cell_clock_bound:?} is negative; state the half-width \
                 each cell's clock is known to hold against the reference, zero or more"
            )));
        }
        Ok(Self {
            cell_clock_bound,
            latest: BTreeMap::new(),
        })
    }

    /// Keys currently held; never more than [`MAX_REGION_PARTITIONS`].
    pub fn held(&self) -> usize {
        self.latest.len()
    }

    /// Relate one tick to the latest tick of every partition *another cell*
    /// captured, then record it as its own key's latest.
    ///
    /// `normalized_time` and `uncertainty` are the tick's as its cell's lake
    /// line recorded them. A tick whose metadata says it is unplaced is
    /// refused: it has no instant on any clock. Feed ticks in the order a
    /// reader wants "latest" to mean; the aligner does not sort.
    pub fn relate(
        &mut self,
        sync: &SyncMetadata,
        sequence: u64,
        normalized_time: Timestamp,
        uncertainty: Duration,
    ) -> Result<Vec<RegionAlignment>> {
        if sync.clock_source() == ClockSource::Unplaced {
            return Err(Error::invalid(format!(
                "a tick of {} captured by {} was refused by its cell's clock discipline; it has \
                 no instant to relate across cells",
                sync.asset().path(),
                sync.cell()
            )));
        }
        let placed = Placed::new(
            normalized_time,
            uncertainty + self.cell_clock_bound,
            normalized_time,
        )?;
        let asset = sync.asset();
        let related: Vec<RegionAlignment> = self
            .latest
            .iter()
            .filter(|((cell, _), _)| cell != sync.cell())
            .map(|((cell, _), other)| {
                let alignment = align(&other.placed, &placed);
                RegionAlignment {
                    region: sync.region().to_string(),
                    cell: sync.cell().to_string(),
                    venue: asset.venue.clone(),
                    instrument: asset.instrument.clone(),
                    other_region: other.region.clone(),
                    other_cell: cell.clone(),
                    other_venue: other.venue.clone(),
                    other_instrument: other.instrument.clone(),
                    other_sequence: other.sequence,
                    order_of_other: match alignment.order {
                        Order::Before => SyncOrder::Before,
                        Order::After => SyncOrder::After,
                        Order::Unordered => SyncOrder::Unordered,
                    },
                    lag_ns: alignment.lag.as_nanos(),
                    lag_uncertainty_ns: alignment.lag_uncertainty.as_nanos(),
                }
            })
            .collect();
        let key = (sync.cell().to_string(), asset.path());
        if !self.latest.contains_key(&key) && self.latest.len() >= MAX_REGION_PARTITIONS {
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
                region: sync.region().to_string(),
                venue: asset.venue,
                instrument: asset.instrument,
                sequence,
            },
        );
        Ok(related)
    }
}
