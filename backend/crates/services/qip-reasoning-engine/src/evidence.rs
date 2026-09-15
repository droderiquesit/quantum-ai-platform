//! Evidence.
//!
//! A hypothesis is only as good as what stands behind it, so evidence is a
//! first-class object with a source, a reliability, a direction and a time.
//!
//! Two properties do most of the work:
//!
//! * **Independence.** Five wire stories rewriting one press release are one
//!   piece of evidence, not five. [`EvidenceSet::independent_weight`] groups by
//!   the originating source before it counts anything, because the alternative
//!   — treating correlated reports as independent confirmations — is how a
//!   thesis reaches high confidence on a single fact.
//! * **Stance.** [`EvidenceSet::posture`] separates two states that a
//!   Bayesian update collapses into one number: evidence that never existed
//!   and evidence that cancels. Both leave a belief at its prior, so both
//!   size small, but only one of them is a question somebody could go and
//!   settle. A platform that cannot tell them apart cannot say why it sized
//!   small, which is the thing this one exists to be able to say.
//! * **Point-in-time.** Evidence carries when it became knowable, and
//!   [`EvidenceSet::as_of`] is the only way to read a set, so a hypothesis
//!   evaluated for last Tuesday cannot rest on Wednesday's news.

use qip_core::error::{Error, Result};
use qip_core::ids::EvidenceId;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// What kind of thing the evidence is.
///
/// Kind sets a ceiling on reliability: a rumour cannot be as reliable as a
/// filing however confidently it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A regulatory filing or an audited statement.
    Filing,
    /// An official statistical release.
    OfficialStatistic,
    /// An observed market price or quote.
    MarketObservation,
    /// A number the platform computed from data it holds.
    Computation,
    /// A backtested or simulated result.
    Simulation,
    /// Reported news from an identified outlet.
    News,
    /// A licensed alternative dataset.
    AlternativeData,
    /// A sell-side or third-party research note.
    ThirdPartyResearch,
    /// An unattributed report.
    Rumour,
}

impl EvidenceKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Filing => "filing",
            Self::OfficialStatistic => "official_statistic",
            Self::MarketObservation => "market_observation",
            Self::Computation => "computation",
            Self::Simulation => "simulation",
            Self::News => "news",
            Self::AlternativeData => "alternative_data",
            Self::ThirdPartyResearch => "third_party_research",
            Self::Rumour => "rumour",
        }
    }

    /// The most reliability this kind of evidence can be assigned.
    ///
    /// A ceiling rather than a value: a filing can still be misread, but a
    /// rumour cannot be promoted to a fact by an enthusiastic analyst.
    pub const fn reliability_ceiling(&self) -> f64 {
        match self {
            Self::Filing | Self::OfficialStatistic | Self::MarketObservation => 0.98,
            Self::Computation => 0.95,
            Self::Simulation => 0.80,
            Self::AlternativeData => 0.75,
            Self::News => 0.70,
            Self::ThirdPartyResearch => 0.60,
            Self::Rumour => 0.25,
        }
    }

    /// Whether the evidence is a direct observation rather than an inference.
    pub const fn is_primary(&self) -> bool {
        matches!(
            self,
            Self::Filing | Self::OfficialStatistic | Self::MarketObservation
        )
    }
}

impl fmt::Display for EvidenceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which way a piece of evidence cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Contradicts,
    /// Relevant background that does not decide the question.
    Contextual,
}

impl Stance {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Contextual => "contextual",
        }
    }

    pub const fn sign(&self) -> f64 {
        match self {
            Self::Supports => 1.0,
            Self::Contradicts => -1.0,
            Self::Contextual => 0.0,
        }
    }
}

impl fmt::Display for Stance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which epistemic state a body of evidence is in.
///
/// The distinction this enum exists for: **an absence of evidence is not a
/// conflict of evidence.** Both drive a belief back towards its prior, so
/// both arrive at the sizing stage as the same low number, and until this
/// existed the platform sized them identically and could not report which
/// had happened. `Absent` means nothing is known. `Conflicted` means
/// something is known and it disagrees with itself, which is a question a
/// desk can go and settle and a position it should not take in the meantime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePosture {
    /// Nothing carries deciding weight.
    ///
    /// Not the same as an empty set: an item with zero diagnosticity is a
    /// document rather than a reason, and a set of nothing but those is an
    /// absence however many pages it runs to.
    Absent,
    /// Deciding weight stands on one side only.
    Unopposed,
    /// Deciding weight stands on both sides.
    Conflicted,
}

impl EvidencePosture {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Unopposed => "unopposed",
            Self::Conflicted => "conflicted",
        }
    }

    /// Whether the evidence contradicts itself.
    pub const fn is_conflicted(&self) -> bool {
        matches!(self, Self::Conflicted)
    }
}

impl fmt::Display for EvidencePosture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One piece of evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub evidence_id: EvidenceId,
    pub kind: EvidenceKind,
    pub stance: Stance,
    /// What the evidence says, in one sentence.
    pub statement: String,
    /// The record this came from, so it can be re-read.
    pub record_id: String,
    /// The originating source. Reports that trace to the same origin share
    /// this, which is how correlated evidence stops being counted twice.
    pub origin: String,
    /// When the fact was true.
    pub valid_at: Timestamp,
    /// When the platform could have known it. Reads filter on this.
    pub known_at: Timestamp,
    /// How much to trust it, in `[0, kind.reliability_ceiling()]`.
    pub reliability: f64,
    /// Diagnosticity: how much this evidence would move the question if true,
    /// in `[0, 1]`. Evidence can be entirely reliable and still tell you
    /// nothing about the hypothesis at hand.
    pub diagnosticity: f64,
}

impl Evidence {
    pub fn new(
        evidence_id: EvidenceId,
        kind: EvidenceKind,
        stance: Stance,
        statement: impl Into<String>,
        record_id: impl Into<String>,
        origin: impl Into<String>,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Self {
        Self {
            evidence_id,
            kind,
            stance,
            statement: statement.into(),
            record_id: record_id.into(),
            origin: origin.into(),
            valid_at,
            known_at,
            reliability: kind.reliability_ceiling(),
            diagnosticity: 0.5,
        }
    }

    /// Set reliability, clamped to the kind's ceiling.
    pub fn with_reliability(mut self, reliability: f64) -> Self {
        self.reliability = reliability.clamp(0.0, self.kind.reliability_ceiling());
        self
    }

    pub fn with_diagnosticity(mut self, diagnosticity: f64) -> Self {
        self.diagnosticity = diagnosticity.clamp(0.0, 1.0);
        self
    }

    /// How much this single piece should move a belief, in `[0, 1]`.
    pub fn weight(&self) -> f64 {
        self.reliability * self.diagnosticity
    }

    /// Signed weight: positive supports, negative contradicts.
    pub fn signed_weight(&self) -> f64 {
        self.weight() * self.stance.sign()
    }

    pub fn was_knowable_at(&self, as_of: Timestamp) -> bool {
        self.known_at <= as_of
    }

    pub fn validate(&self) -> Result<()> {
        if self.statement.trim().is_empty() {
            return Err(Error::invalid("evidence has no statement"));
        }
        if self.origin.trim().is_empty() {
            return Err(Error::invalid(format!(
                "evidence {} names no origin; correlated reports could not be detected",
                self.evidence_id.as_str()
            )));
        }
        if self.known_at < self.valid_at {
            return Err(Error::invalid(format!(
                "evidence {} was known at {} before it was true at {}",
                self.evidence_id.as_str(),
                self.known_at,
                self.valid_at
            )));
        }
        if self.reliability > self.kind.reliability_ceiling() + 1e-9 {
            return Err(Error::invalid(format!(
                "evidence {} claims reliability {:.2} above the {:.2} ceiling for {}",
                self.evidence_id.as_str(),
                self.reliability,
                self.kind.reliability_ceiling(),
                self.kind
            )));
        }
        Ok(())
    }
}

/// Independent weight at or below which a side carries no deciding weight.
///
/// Not a rounding tolerance. `Evidence::weight` is `reliability *
/// diagnosticity`, so a side whose every item is undiagnostic lands exactly
/// at zero, and that is the absence this module has to be able to name — a
/// stack of filings that bear on nothing is not evidence for the question.
const DECIDING_WEIGHT_FLOOR: f64 = 0.0;

/// A body of evidence about one question.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSet {
    items: Vec<Evidence>,
}

impl EvidenceSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_items(items: Vec<Evidence>) -> Self {
        Self { items }
    }

    pub fn push(&mut self, evidence: Evidence) {
        self.items.push(evidence);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Evidence> {
        self.items.iter()
    }

    /// The subset knowable at `as_of`.
    ///
    /// The only read path that a hypothesis evaluation should use.
    pub fn as_of(&self, as_of: Timestamp) -> EvidenceSet {
        EvidenceSet {
            items: self
                .items
                .iter()
                .filter(|e| e.was_knowable_at(as_of))
                .cloned()
                .collect(),
        }
    }

    pub fn with_stance(&self, stance: Stance) -> Vec<&Evidence> {
        self.items.iter().filter(|e| e.stance == stance).collect()
    }

    /// Distinct originating sources.
    pub fn origins(&self) -> Vec<&str> {
        let mut origins: Vec<&str> = self.items.iter().map(|e| e.origin.as_str()).collect();
        origins.sort_unstable();
        origins.dedup();
        origins
    }

    /// Weight per stance after collapsing correlated evidence.
    ///
    /// Within one origin the strongest item counts in full and the rest are
    /// heavily discounted: three articles from one newsroom on one press
    /// release are more than one article, but nothing like three.
    pub fn independent_weight(&self, stance: Stance) -> f64 {
        const CORRELATED_DISCOUNT: f64 = 0.15;
        let mut by_origin: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        for item in self.items.iter().filter(|e| e.stance == stance) {
            by_origin
                .entry(item.origin.as_str())
                .or_default()
                .push(item.weight());
        }
        by_origin
            .into_values()
            .map(|mut weights| {
                weights.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                weights
                    .into_iter()
                    .enumerate()
                    .map(|(i, w)| if i == 0 { w } else { w * CORRELATED_DISCOUNT })
                    .sum::<f64>()
            })
            .sum()
    }

    /// Whether any primary source stands behind the claim.
    ///
    /// A thesis resting entirely on news and third-party research is a thesis
    /// about what other people think.
    pub fn has_primary_source(&self) -> bool {
        self.items
            .iter()
            .any(|e| e.kind.is_primary() && e.stance == Stance::Supports)
    }

    /// Fraction of supporting weight coming from the single largest origin.
    ///
    /// Near 1.0 means the thesis has one source wearing several hats.
    pub fn concentration(&self) -> f64 {
        let mut by_origin: BTreeMap<&str, f64> = BTreeMap::new();
        let mut total = 0.0;
        for item in self.items.iter().filter(|e| e.stance == Stance::Supports) {
            *by_origin.entry(item.origin.as_str()).or_insert(0.0) += item.weight();
            total += item.weight();
        }
        if total <= 1e-12 {
            return 0.0;
        }
        by_origin.values().fold(0.0_f64, |a, b| a.max(*b)) / total
    }

    /// How far the deciding weight disagrees with itself, in `[0, 1]`.
    ///
    /// Zero when the deciding weight all points one way — *including* when
    /// there is none to point, which is why this is never read without
    /// [`EvidenceSet::posture`] beside it. One when both sides carry equal
    /// independent weight.
    ///
    /// Computed on [`EvidenceSet::independent_weight`] rather than on raw
    /// sums for the reason that function exists: one dissenting newsroom
    /// restated five times is one dissent, and a measure built on the raw sum
    /// would let a single contrary origin manufacture a conflict by
    /// republishing itself.
    ///
    /// The ratio is `min / max` rather than a signed net, because a net of
    /// zero is indistinguishable from no evidence at all and telling those
    /// two apart is the whole purpose. A statistic, and so `f64`; it crosses
    /// into `Decimal` only downstream, where the portfolio constructor turns
    /// a conviction into money.
    pub fn net_stance_disagreement(&self) -> f64 {
        let supports = self.independent_weight(Stance::Supports);
        let contradicts = self.independent_weight(Stance::Contradicts);
        let stronger = supports.max(contradicts);
        if stronger <= DECIDING_WEIGHT_FLOOR {
            return 0.0;
        }
        // Both are non-negative sums of non-negative weights, so the quotient
        // of the smaller by the larger is in `[0, 1]` without a clamp. A
        // clamp here would hide an arithmetic fault rather than refuse it.
        supports.min(contradicts) / stronger
    }

    /// Which epistemic state the set is in.
    ///
    /// Read this before reading [`EvidenceSet::net_stance_disagreement`]: the
    /// measure returns zero for `Absent` and `Unopposed` alike, and it is
    /// this function that says which of the two a zero meant.
    pub fn posture(&self) -> EvidencePosture {
        let supports = self.independent_weight(Stance::Supports) > DECIDING_WEIGHT_FLOOR;
        let contradicts = self.independent_weight(Stance::Contradicts) > DECIDING_WEIGHT_FLOOR;
        match (supports, contradicts) {
            (false, false) => EvidencePosture::Absent,
            (true, true) => EvidencePosture::Conflicted,
            _ => EvidencePosture::Unopposed,
        }
    }

    /// The most recent point at which any item became knowable.
    pub fn latest_known_at(&self) -> Option<Timestamp> {
        self.items.iter().map(|e| e.known_at).max()
    }

    pub fn validate(&self) -> Result<()> {
        for item in &self.items {
            item.validate()?;
        }
        Ok(())
    }
}
