//! Where an event came from, what it is about, and what it cost to obtain.
//!
//! These are the envelope fields that `qip_events::AnyEvent` does not already
//! carry. `AnyEvent` describes an event the platform produced about itself: it
//! knows its topic, its lineage and its payload hash, but it has no idea which
//! upstream feed the fact entered through, which region that feed serves, or
//! what the vendor charged for it. Those are properties of *ingestion*, and
//! ingestion is what this crate is about.
//!
//! Every type here is a newtype or a closed enum rather than a `String`. A
//! region compared against a venue, or a source id compared against an
//! instrument, is the kind of mistake that reads as "no data for this market"
//! rather than as an error — which is the safe direction only if the mistake
//! cannot happen at all.

use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Money, ObjectId};
use qip_financial::asset_class::AssetClass;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The upstream feed, vendor or connector an event entered through.
///
/// Distinct from `qip_core::Lineage::producer`, which names the *platform*
/// component that emitted the event. Both are needed: the producer says which
/// of our services to look at, the source says whose data it was.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The market or jurisdiction a source serves.
///
/// A newtype rather than a string, for the reason `qip_cost_router::Region`
/// gives: a mis-keyed region reads as "nothing here" rather than as an error.
/// Deliberately *not* imported from that crate — this is the transport spine
/// and the cost router rides on top of it, so the dependency would run the
/// wrong way and make the router's release schedule the spine's problem.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Region(String);

impl Region {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of thing produced an event.
///
/// The routing rules in [`crate::routing`] read this, so it is a closed set:
/// a new source type is a compile error at every match that decides which
/// transport an event takes, which is exactly where a new source type needs a
/// decision made about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    /// A direct or consolidated market-data feed from a trading venue.
    VenueFeed,
    /// A broker or clearer reporting on our own orders.
    Broker,
    /// Instrument, calendar and entity reference data.
    ReferenceData,
    /// Company financials.
    Fundamentals,
    /// Macroeconomic series.
    MacroSeries,
    /// News, filings and other text.
    News,
    /// Anything bought for signal rather than for record-keeping.
    AlternativeData,
    /// An on-chain log or mempool.
    Chain,
    /// Produced by the platform itself rather than received.
    Internal,
}

impl SourceType {
    pub const ALL: [Self; 9] = [
        Self::VenueFeed,
        Self::Broker,
        Self::ReferenceData,
        Self::Fundamentals,
        Self::MacroSeries,
        Self::News,
        Self::AlternativeData,
        Self::Chain,
        Self::Internal,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::VenueFeed => "venue_feed",
            Self::Broker => "broker",
            Self::ReferenceData => "reference_data",
            Self::Fundamentals => "fundamentals",
            Self::MacroSeries => "macro_series",
            Self::News => "news",
            Self::AlternativeData => "alternative_data",
            Self::Chain => "chain",
            Self::Internal => "internal",
        }
    }

    /// Whether this source sits on the path a venue-facing decision is made on.
    ///
    /// True only for a venue feed. A broker drop-copy is latency-sensitive too,
    /// but losing one corrupts the position record, so it is never allowed on a
    /// lossy path — [`crate::routing`] enforces that separately and the two
    /// rules must not be conflated into one flag.
    pub const fn is_venue_critical(&self) -> bool {
        matches!(self, Self::VenueFeed)
    }

    /// Whether records from this source are numbered by their publisher.
    ///
    /// Gap detection only means something where they are. Running a sequence
    /// tracker over a source that numbers nothing manufactures gaps out of
    /// arrival order.
    pub const fn is_sequenced(&self) -> bool {
        matches!(self, Self::VenueFeed | Self::Broker | Self::Chain)
    }
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who sent this, of what kind, serving where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub source_id: SourceId,
    pub source_type: SourceType,
    pub region: Region,
}

impl SourceIdentity {
    pub fn new(source_id: SourceId, source_type: SourceType, region: Region) -> Self {
        Self {
            source_id,
            source_type,
            region,
        }
    }
}

/// What the event is about.
///
/// Every field is optional because the honest answer for most sources is that
/// they do not know. A macroeconomic series has no venue, no instrument and no
/// asset class; filling those in with a placeholder would make a query for
/// "everything on XNYS" return it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_class: Option<AssetClass>,
    /// The instrument the event is about, in the platform's object space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<ObjectId>,
    /// The venue, feed, partition and publisher sequence this arrived on.
    ///
    /// `qip_contracts::Origin` already carries all four and is the key
    /// `qip_sequencing` tracks streams by, so the envelope stores that rather
    /// than a loose `venue` and `sequence_number` pair that could disagree
    /// with each other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

impl Subject {
    /// A subject that names nothing — the correct value for a platform-internal
    /// event about no instrument in particular.
    pub fn unattributed() -> Self {
        Self::default()
    }

    pub fn with_asset_class(mut self, asset_class: AssetClass) -> Self {
        self.asset_class = Some(asset_class);
        self
    }

    pub fn with_instrument(mut self, instrument: ObjectId) -> Self {
        self.instrument = Some(instrument);
        self
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = Some(origin);
        self
    }

    /// The venue this arrived from, when it arrived from one.
    pub fn venue(&self) -> Option<&VenueId> {
        self.origin.as_ref().map(|origin| &origin.venue)
    }

    /// The publisher's own sequence number, when the publisher numbers its
    /// output.
    pub fn sequence_number(&self) -> Option<u64> {
        self.origin.as_ref().map(|origin| origin.sequence)
    }

    /// The ordered stream this event belongs to, for gap detection.
    pub fn stream_key(&self) -> Option<String> {
        self.origin.as_ref().map(Origin::stream_key)
    }
}

/// How much the source believes its own record, in `[0, 1]`.
///
/// `f64` because it is a statistic and never money. The constructor refuses
/// anything outside the range, including `NaN`: a confidence that silently
/// clamped would let a broken estimator publish `2.0` and be read as certainty.
///
/// Serialised as a bare number, and parsed back through [`Confidence::new`]
/// rather than through a transparent newtype: a wire form is exactly where an
/// out-of-range value arrives, so that is where the range has to be checked.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct Confidence(f64);

impl Confidence {
    /// Certainty. Used by sources that report facts rather than estimates.
    pub const CERTAIN: Self = Self(1.0);

    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::invalid(format!(
                "confidence must be a finite value in [0, 1], got {value}"
            )));
        }
        Ok(Self(value))
    }

    pub fn value(&self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Confidence {
    type Error = Error;

    fn try_from(value: f64) -> Result<Self> {
        Self::new(value)
    }
}

impl From<Confidence> for f64 {
    fn from(confidence: Confidence) -> Self {
        confidence.0
    }
}

/// What this event cost to acquire and to move.
///
/// Money is exact: the per-event charges are fractions of a cent and a platform
/// that accumulates them in `f64` reports a data bill that does not reconcile
/// with the invoice. [`Money`] refuses to add two currencies, so a vendor
/// billing in EUR cannot be quietly folded into a USD total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostMetadata {
    /// What the upstream vendor charges for this record.
    pub acquisition: Money,
    /// What carrying it through the platform costs.
    pub transport: Money,
    /// Serialised payload size, for capacity planning and egress attribution.
    pub payload_bytes: u64,
}

impl CostMetadata {
    pub fn new(acquisition: Money, transport: Money, payload_bytes: u64) -> Self {
        Self {
            acquisition,
            transport,
            payload_bytes,
        }
    }

    /// A cost of nothing, in a stated currency.
    ///
    /// Takes the currency rather than defaulting to USD, because a zero with
    /// the wrong currency tag poisons every sum it later joins.
    pub fn free(currency: qip_core::Currency, payload_bytes: u64) -> Self {
        Self {
            acquisition: Money::zero(currency),
            transport: Money::zero(currency),
            payload_bytes,
        }
    }

    /// Acquisition plus transport, refusing a currency mismatch.
    pub fn total(&self) -> Result<Money> {
        self.acquisition.checked_add(self.transport)
    }
}
