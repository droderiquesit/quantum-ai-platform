//! What a source actually covers, and how much of another's coverage it can
//! take over.
//!
//! Coverage is the unit a replacement search compares. It is deliberately
//! four independent axes rather than one similarity number: a feed that
//! covers the right instruments in the wrong region, or at the right
//! granularity a day late, is not a substitute, and a single blended score
//! would let it look like one.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_financial::asset_class::AssetClass;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A geography the platform runs cells in, or a source publishes for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRegion {
    UsEast,
    UsWest,
    Europe,
    Apac,
    SouthAmerica,
    MiddleEast,
    /// Not tied to a geography — a global macro series, a reference dataset.
    Global,
}

impl SourceRegion {
    pub const ALL: [Self; 7] = [
        Self::UsEast,
        Self::UsWest,
        Self::Europe,
        Self::Apac,
        Self::SouthAmerica,
        Self::MiddleEast,
        Self::Global,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::UsEast => "us_east",
            Self::UsWest => "us_west",
            Self::Europe => "europe",
            Self::Apac => "apac",
            Self::SouthAmerica => "south_america",
            Self::MiddleEast => "middle_east",
            Self::Global => "global",
        }
    }

    /// Whether a source in this region can serve a need in `required`.
    ///
    /// `Global` serves anything; nothing else serves a region other than
    /// itself. Cross-region substitution is refused rather than discounted,
    /// because the reason a cell wants a regional feed is latency, and a
    /// feed on the wrong continent does not become closer by scoring well.
    pub fn serves(&self, required: Self) -> bool {
        *self == required || *self == Self::Global
    }
}

/// How often a source publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateFrequency {
    /// Every tick, as it happens.
    Streaming,
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    /// Published when there is something to publish.
    Irregular,
}

impl UpdateFrequency {
    /// Typical gap between publications. `Irregular` has none, which is why
    /// it compares as the least frequent.
    pub const fn typical_interval(&self) -> Option<Duration> {
        match self {
            Self::Streaming => Some(Duration::from_millis(1)),
            Self::Secondly => Some(Duration::from_secs(1)),
            Self::Minutely => Some(Duration::from_mins(1)),
            Self::Hourly => Some(Duration::from_hours(1)),
            Self::Daily => Some(Duration::from_days(1)),
            Self::Weekly => Some(Duration::from_days(7)),
            Self::Monthly => Some(Duration::from_days(30)),
            Self::Irregular => None,
        }
    }

    /// Rank, from most frequent (0) to least.
    pub const fn rank(&self) -> u8 {
        match self {
            Self::Streaming => 0,
            Self::Secondly => 1,
            Self::Minutely => 2,
            Self::Hourly => 3,
            Self::Daily => 4,
            Self::Weekly => 5,
            Self::Monthly => 6,
            Self::Irregular => 7,
        }
    }

    /// Whether publishing at this frequency satisfies a need for `required`.
    pub const fn satisfies(&self, required: Self) -> bool {
        self.rank() <= required.rank()
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Secondly => "secondly",
            Self::Minutely => "minutely",
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Irregular => "irregular",
        }
    }
}

/// What a source covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCoverage {
    asset_classes: BTreeSet<AssetClass>,
    regions: BTreeSet<SourceRegion>,
    instruments: BTreeSet<String>,
    update_frequency: UpdateFrequency,
    history_starts: Option<Timestamp>,
}

impl SourceCoverage {
    /// Build a coverage claim.
    ///
    /// Empty asset classes or regions are refused. A source that covers
    /// nothing in particular cannot be compared with anything, and would
    /// silently become a universal replacement for every dead feed.
    pub fn new(
        asset_classes: impl IntoIterator<Item = AssetClass>,
        regions: impl IntoIterator<Item = SourceRegion>,
        instruments: impl IntoIterator<Item = String>,
        update_frequency: UpdateFrequency,
    ) -> Result<Self> {
        let asset_classes: BTreeSet<AssetClass> = asset_classes.into_iter().collect();
        let regions: BTreeSet<SourceRegion> = regions.into_iter().collect();
        if asset_classes.is_empty() {
            return Err(Error::invalid(
                "coverage must name at least one asset class; a source covering nothing \
                 in particular would substitute for everything",
            ));
        }
        if regions.is_empty() {
            return Err(Error::invalid("coverage must name at least one region"));
        }
        Ok(Self {
            asset_classes,
            regions,
            instruments: instruments.into_iter().collect(),
            update_frequency,
            history_starts: None,
        })
    }

    pub fn with_history_from(mut self, at: Timestamp) -> Self {
        self.history_starts = Some(at);
        self
    }

    pub fn asset_classes(&self) -> &BTreeSet<AssetClass> {
        &self.asset_classes
    }

    pub fn regions(&self) -> &BTreeSet<SourceRegion> {
        &self.regions
    }

    pub fn instruments(&self) -> &BTreeSet<String> {
        &self.instruments
    }

    pub fn update_frequency(&self) -> UpdateFrequency {
        self.update_frequency
    }

    pub fn history_starts(&self) -> Option<Timestamp> {
        self.history_starts
    }

    /// How far back the source's history reaches at `now`.
    pub fn history_depth(&self, now: Timestamp) -> Duration {
        match self.history_starts {
            Some(start) if start <= now => now.since(start),
            _ => Duration::ZERO,
        }
    }

    /// How much of `required` this coverage supplies.
    pub fn against(&self, required: &Self) -> CoverageMatch {
        let missing_asset_classes: BTreeSet<AssetClass> = required
            .asset_classes
            .iter()
            .filter(|class| !self.asset_classes.contains(class))
            .copied()
            .collect();
        let missing_regions: BTreeSet<SourceRegion> = required
            .regions
            .iter()
            .filter(|region| !self.regions.iter().any(|mine| mine.serves(**region)))
            .copied()
            .collect();
        let missing_instruments: BTreeSet<String> = required
            .instruments
            .iter()
            .filter(|instrument| !self.instruments.contains(*instrument))
            .cloned()
            .collect();
        CoverageMatch {
            asset_class_fraction: fraction(
                required.asset_classes.len(),
                missing_asset_classes.len(),
            ),
            region_fraction: fraction(required.regions.len(), missing_regions.len()),
            instrument_fraction: fraction(required.instruments.len(), missing_instruments.len()),
            frequency_sufficient: self.update_frequency.satisfies(required.update_frequency),
            gap: CoverageGap {
                asset_classes: missing_asset_classes,
                regions: missing_regions,
                instruments: missing_instruments,
                frequency_shortfall: (!self.update_frequency.satisfies(required.update_frequency))
                    .then_some((self.update_frequency, required.update_frequency)),
            },
        }
    }

    /// Fraction of this coverage's instruments also covered by `other`, in
    /// `[0, 1]`. Used for uniqueness: a source duplicating what is already
    /// registered adds cost and no information.
    pub fn overlap_with(&self, other: &Self) -> f64 {
        if self.instruments.is_empty() && other.instruments.is_empty() {
            let shared = self
                .asset_classes
                .intersection(&other.asset_classes)
                .count();
            return fraction(self.asset_classes.len(), self.asset_classes.len() - shared);
        }
        if self.instruments.is_empty() {
            return 0.0;
        }
        let shared = self
            .instruments
            .iter()
            .filter(|instrument| other.instruments.contains(*instrument))
            .count();
        shared as f64 / self.instruments.len() as f64
    }
}

/// `1.0` when nothing of `total` is missing; `1.0` when `total` is zero,
/// because nothing was required.
fn fraction(total: usize, missing: usize) -> f64 {
    if total == 0 {
        return 1.0;
    }
    (total - missing.min(total)) as f64 / total as f64
}

/// Exactly what a candidate does not cover.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageGap {
    pub asset_classes: BTreeSet<AssetClass>,
    pub regions: BTreeSet<SourceRegion>,
    pub instruments: BTreeSet<String>,
    /// `(offered, required)` when the candidate publishes too slowly.
    pub frequency_shortfall: Option<(UpdateFrequency, UpdateFrequency)>,
}

impl CoverageGap {
    pub fn is_empty(&self) -> bool {
        self.asset_classes.is_empty()
            && self.regions.is_empty()
            && self.instruments.is_empty()
            && self.frequency_shortfall.is_none()
    }

    /// The gap in words, for a decision record an operator has to read.
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "nothing is uncovered".to_string();
        }
        let mut parts = Vec::new();
        if !self.asset_classes.is_empty() {
            parts.push(format!(
                "asset classes {}",
                self.asset_classes
                    .iter()
                    .map(AssetClass::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.regions.is_empty() {
            parts.push(format!(
                "regions {}",
                self.regions
                    .iter()
                    .map(SourceRegion::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.instruments.is_empty() {
            parts.push(format!(
                "instruments {}",
                self.instruments.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some((offered, required)) = self.frequency_shortfall {
            parts.push(format!(
                "update frequency ({} offered, {} required)",
                offered.as_str(),
                required.as_str()
            ));
        }
        parts.join("; ")
    }
}

/// How completely one coverage answers another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageMatch {
    /// Fraction of required asset classes covered, in `[0, 1]`.
    pub asset_class_fraction: f64,
    /// Fraction of required regions covered, in `[0, 1]`.
    pub region_fraction: f64,
    /// Fraction of required instruments covered, in `[0, 1]`.
    pub instrument_fraction: f64,
    /// Whether the candidate publishes at least as often as required.
    pub frequency_sufficient: bool,
    pub gap: CoverageGap,
}

impl CoverageMatch {
    /// Whether every axis is fully covered. Only a complete match is a
    /// replacement; everything else is a partial that must be reported as
    /// one.
    pub fn is_complete(&self) -> bool {
        self.gap.is_empty()
    }

    /// A single number in `[0, 1]` for ranking *complete* matches against one
    /// another. Meaningless for incomplete ones, which is why
    /// [`Self::is_complete`] is checked first everywhere it is used.
    pub fn completeness(&self) -> f64 {
        let frequency = if self.frequency_sufficient { 1.0 } else { 0.0 };
        (self.asset_class_fraction + self.region_fraction + self.instrument_fraction + frequency)
            / 4.0
    }
}
