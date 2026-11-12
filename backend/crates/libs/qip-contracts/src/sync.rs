//! Cross-asset and cross-region synchronisation metadata on a captured tick
//! (R12-TICK-2048, ADR 0438).
//!
//! Until 2026-11-12 the edge node's lake line carried `region`, `cell`,
//! `clock_source` and `cross_partition` as fields of an ad hoc JSON object
//! built in the drain, and the lake reader dropped all four. Nothing typed
//! them, so nothing refused a line whose stamp claimed an order its own lag
//! and uncertainty could not support, and a reader learning a lead/lag
//! across instruments, venues or regions had to re-parse an untyped blob it
//! could not trust.
//!
//! [`SyncMetadata`] is that record: who captured the tick (region and cell),
//! which clock its normalized instant came from ([`ClockSource`]), the asset
//! key it is aligned under (venue, feed, partition, instrument), and its
//! alignment against the latest tick of every other partition the capturing
//! cell holds ([`CrossPartition`]). It is written once, by the drain, and
//! decoded back through [`SyncMetadata::validate`], so a hand-edited or
//! corrupted order is an unparsed line rather than a learned reaction.
//!
//! The wire form is the one the lake already held — the same keys at the top
//! level of the line — plus `venue` and `instrument`, so lines written before
//! the asset key existed fail to decode as this record and are read as
//! carrying no sync metadata, never as carrying a blank one.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The most partitions one tick's [`CrossPartition::Aligned`] relates it to.
/// The drain bounds its working set at the same figure; a line carrying more
/// was not written by it.
pub const MAX_SYNC_ALIGNMENTS: usize = 256;

/// Which clock a tick's normalized instant and uncertainty come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    /// The venue's stamp moved onto the cell's reference clock by the
    /// discipline's measured offset, with that estimate's uncertainty.
    OffsetEstimate,
    /// The venue's own stamp, before the estimate is trusted, with the
    /// uncertainty the tick stated for itself.
    VenueStated,
    /// The discipline refused the tick; it has no normalized instant.
    Unplaced,
}

/// What two stamps support about which tick happened first. There is no
/// "simultaneous": two independent clocks can never support it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOrder {
    Before,
    After,
    Unordered,
}

impl SyncOrder {
    /// The order a signed `lag` (this tick minus the other) supports against
    /// a combined half-width `lag_uncertainty`: the other came first only
    /// when this one is later by strictly more than the bound.
    pub fn of_lag(lag_ns: i64, lag_uncertainty_ns: i64) -> Self {
        if lag_ns.unsigned_abs() <= lag_uncertainty_ns.unsigned_abs() {
            Self::Unordered
        } else if lag_ns > 0 {
            Self::Before
        } else {
            Self::After
        }
    }
}

/// One other partition's latest tick, related to this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionAlignment {
    /// `venue/feed/partition/instrument` of the other tick.
    pub partition: String,
    /// The other tick's venue and instrument, named on their own so a
    /// cross-asset reader does not parse them out of `partition`.
    pub venue: String,
    pub instrument: String,
    pub sequence: u64,
    /// The other tick relative to this one: `before` means it happened first
    /// by more than both clocks' error.
    pub order_of_other: SyncOrder,
    /// This tick's instant minus the other's. Signed.
    pub lag_ns: i64,
    /// Both uncertainties summed; the true lag is `lag_ns ± this`.
    pub lag_uncertainty_ns: i64,
    /// Whether the cell had received the other by the time it received this
    /// one — exact, since both arrivals are on one cell's clock.
    pub other_known_at_arrival: bool,
}

/// A tick's alignment against every other partition the capturing cell held,
/// or why it has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CrossPartition {
    Aligned(Vec<PartitionAlignment>),
    Refused { refused: String },
}

/// The synchronisation metadata a captured tick carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "SyncMetadataWire", try_from = "SyncMetadataWire")]
pub struct SyncMetadata {
    region: String,
    cell: String,
    clock_source: ClockSource,
    venue: String,
    feed: String,
    partition: u32,
    instrument: String,
    cross_partition: CrossPartition,
}

/// [`SyncMetadata`]'s wire form. Decoding goes back through
/// [`SyncMetadata::new`]'s refusals; a derived decoder would admit any order
/// a line claimed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SyncMetadataWire {
    region: String,
    cell: String,
    clock_source: ClockSource,
    venue: String,
    feed: String,
    partition: u32,
    instrument: String,
    cross_partition: CrossPartition,
}

impl From<SyncMetadata> for SyncMetadataWire {
    fn from(sync: SyncMetadata) -> Self {
        Self {
            region: sync.region,
            cell: sync.cell,
            clock_source: sync.clock_source,
            venue: sync.venue,
            feed: sync.feed,
            partition: sync.partition,
            instrument: sync.instrument,
            cross_partition: sync.cross_partition,
        }
    }
}

impl TryFrom<SyncMetadataWire> for SyncMetadata {
    type Error = Error;

    fn try_from(wire: SyncMetadataWire) -> Result<Self> {
        let sync = Self {
            region: wire.region,
            cell: wire.cell,
            clock_source: wire.clock_source,
            venue: wire.venue,
            feed: wire.feed,
            partition: wire.partition,
            instrument: wire.instrument,
            cross_partition: wire.cross_partition,
        };
        sync.validate()?;
        Ok(sync)
    }
}

/// A region or cell name the lake can place: letters, digits, `.`, `_`, `-`.
fn nameable(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The asset key a tick is aligned under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetKey {
    pub venue: String,
    pub feed: String,
    pub partition: u32,
    pub instrument: String,
}

impl AssetKey {
    /// `venue/feed/partition/instrument`, the key the drain aligns by.
    pub fn path(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.venue, self.feed, self.partition, self.instrument
        )
    }
}

impl SyncMetadata {
    /// The metadata for one tick, refused unless [`Self::validate`] holds.
    pub fn new(
        region: impl Into<String>,
        cell: impl Into<String>,
        clock_source: ClockSource,
        asset: AssetKey,
        cross_partition: CrossPartition,
    ) -> Result<Self> {
        let sync = Self {
            region: region.into(),
            cell: cell.into(),
            clock_source,
            venue: asset.venue,
            feed: asset.feed,
            partition: asset.partition,
            instrument: asset.instrument,
            cross_partition,
        };
        sync.validate()?;
        Ok(sync)
    }

    /// Refuse metadata a cross-region or cross-asset reader could not trust:
    ///
    /// - a region or cell the lake cannot name, since a line that does not
    ///   say which region stamped it does not say which clock error it
    ///   carries;
    /// - a blank venue, feed or instrument, since the asset key is what a
    ///   cross-asset reaction is learned under;
    /// - an unplaced tick that claims alignments, or a placed one that claims
    ///   a refusal: the two say the same fact and must agree;
    /// - more alignments than [`MAX_SYNC_ALIGNMENTS`], an alignment against
    ///   the tick's own partition, or a blank other venue or instrument;
    /// - a negative lag uncertainty, or an order the lag and its uncertainty
    ///   do not support — the forged `before` this record exists to stop
    ///   reaching a lead/lag model.
    pub fn validate(&self) -> Result<()> {
        if !nameable(&self.region) {
            return Err(Error::invalid(format!(
                "sync metadata names region {:?}; name the capturing cell's region in letters, \
                 digits, '.', '_' or '-', or the tick cannot be placed against another region",
                self.region
            )));
        }
        if !nameable(&self.cell) {
            return Err(Error::invalid(format!(
                "sync metadata names cell {:?}; name the capturing cell in letters, digits, '.', \
                 '_' or '-'",
                self.cell
            )));
        }
        for (name, value) in [
            ("venue", &self.venue),
            ("feed", &self.feed),
            ("instrument", &self.instrument),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "sync metadata has no {name}; the asset key a cross-asset reaction is \
                     learned under needs all of venue, feed and instrument"
                )));
            }
        }
        let own = self.asset().path();
        match (&self.cross_partition, self.clock_source) {
            (CrossPartition::Refused { refused }, ClockSource::Unplaced) => {
                if refused.trim().is_empty() {
                    return Err(Error::invalid(
                        "sync metadata refuses a tick's alignment without saying why; state \
                         the discipline's refusal",
                    ));
                }
            }
            (CrossPartition::Refused { .. }, _) => {
                return Err(Error::invalid(
                    "sync metadata says the tick was placed on a clock and also that it could \
                     not be aligned; a placed tick is aligned against every other partition",
                ));
            }
            (CrossPartition::Aligned(_), ClockSource::Unplaced) => {
                return Err(Error::invalid(
                    "sync metadata aligns a tick the discipline refused; an unplaced tick has \
                     no instant to order from",
                ));
            }
            (CrossPartition::Aligned(alignments), _) => {
                if alignments.len() > MAX_SYNC_ALIGNMENTS {
                    return Err(Error::invalid(format!(
                        "sync metadata relates a tick to {} partitions and the capture path \
                         holds at most {MAX_SYNC_ALIGNMENTS}",
                        alignments.len()
                    )));
                }
                for alignment in alignments {
                    alignment.validate(&own)?;
                }
            }
        }
        Ok(())
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub const fn clock_source(&self) -> ClockSource {
        self.clock_source
    }

    pub fn asset(&self) -> AssetKey {
        AssetKey {
            venue: self.venue.clone(),
            feed: self.feed.clone(),
            partition: self.partition,
            instrument: self.instrument.clone(),
        }
    }

    pub fn instrument(&self) -> &str {
        &self.instrument
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub const fn cross_partition(&self) -> &CrossPartition {
        &self.cross_partition
    }
}

impl PartitionAlignment {
    fn validate(&self, own: &str) -> Result<()> {
        if self.partition == own {
            return Err(Error::invalid(format!(
                "sync metadata aligns a tick of {own} against its own partition; a partition's \
                 own order is its stream's arrival order, not a cross-partition relation"
            )));
        }
        if self.venue.trim().is_empty() || self.instrument.trim().is_empty() {
            return Err(Error::invalid(format!(
                "an alignment against {} names no venue or instrument; a cross-asset reader \
                 cannot tell what it relates the tick to",
                self.partition
            )));
        }
        if self.lag_uncertainty_ns < 0 {
            return Err(Error::invalid(format!(
                "an alignment against {} carries a negative lag uncertainty; it is a sum of \
                 two half-widths, zero or more",
                self.partition
            )));
        }
        let supported = SyncOrder::of_lag(self.lag_ns, self.lag_uncertainty_ns);
        if supported != self.order_of_other {
            return Err(Error::invalid(format!(
                "an alignment against {} claims {:?} for a lag of {} ns ± {} ns, which supports \
                 {supported:?}; an order the clocks cannot support is clock error, not a \
                 reaction",
                self.partition, self.order_of_other, self.lag_ns, self.lag_uncertainty_ns
            )));
        }
        Ok(())
    }
}
