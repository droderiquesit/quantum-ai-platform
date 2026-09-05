//! The self-model: what the platform knows about its own reliability.
//!
//! Blueprint §13.1 asks the platform to hold an estimate of where it is
//! reliable and where it is guessing, per component, and to use it. Until
//! this module existed the answer was nowhere: every detector, every analyst
//! and every rung was weighted as if its record were perfect, because nothing
//! kept a record. A component that had been wrong sixty times in a row
//! contributed to the sixty-first thesis at full weight.
//!
//! Three commitments shape it:
//!
//! * **It is arithmetic** (ADR 0005). A [`CapabilityEstimate`] is a bounded
//!   window of graded outcomes, and [`CapabilityEstimate::estimate`] is a
//!   stated formula over that window — never a number somebody chose.
//! * **It refuses before it guesses.** Below [`MINIMUM_SAMPLE`] outcomes the
//!   estimate is an error naming the count, not `0.5`. A coin-flip reported
//!   as an estimate reads as measured indifference, and a consumer that
//!   scaled by it would halve a component's weight on no evidence at all.
//! * **It is bounded twice.** Each component keeps at most
//!   [`CAPABILITY_WINDOW`] outcomes, and the model keeps at most
//!   [`MAX_COMPONENTS`] components, evicting the least recently updated. An
//!   unbounded record is the retention failure the product direction forbids.
//!
//! What it is *not*: a rate the platform reports about itself and nothing
//! reads. [`SelfModel::origin_factors`] is the one consumer-facing surface —
//! the per-origin factors the REASON stage scales evidence by — and it names
//! only components with a sufficient sample, so an origin nobody has measured
//! is left at full weight rather than at a fabricated one.

use crate::evaluation::Evaluation;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

/// Outcomes each component keeps. Older ones fall off the front.
///
/// One hundred and twenty-eight is a window over which a component's record
/// can change — a detector retuned last quarter should not be judged on the
/// year before — without being so short that one bad week reads as a broken
/// component.
pub const CAPABILITY_WINDOW: usize = 128;

/// Components the model keeps before evicting the stalest.
///
/// The roster, the detector set and the rung set are each a few dozen at
/// most; five hundred and twelve is room for every strategy family beside
/// them, and a cap rather than a forecast.
pub const MAX_COMPONENTS: usize = 512;

/// Outcomes a component needs before an estimate is reported at all.
///
/// The same ten the feedback engine withholds a per-agent rate below: not
/// statistical significance, but the point below which reporting a number
/// would mislead more than withholding it.
pub const MINIMUM_SAMPLE: usize = 10;

/// Pseudo-observations at one half that the hit rate is shrunk toward.
///
/// The sample-size penalty, stated: with `k` pseudo-counts, ten outcomes all
/// correct estimate `(10 + 2) / (10 + 4) = 0.857` rather than `1.0`, and the
/// penalty fades as the sample grows. Four is small enough that a hundred
/// outcomes are barely touched and large enough that the minimum sample is
/// not a certainty.
const PSEUDO_COUNTS: f64 = 4.0;

/// The kind of component an estimate is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    /// An anomaly detector, keyed by its kind — the hypothesis class it
    /// raises.
    Detector,
    /// An analyst on the roster, keyed by its manifest id.
    Analyst,
    /// A cost-router rung, keyed by the tier name.
    Rung,
    /// A strategy family, keyed by its family name.
    Strategy,
}

impl ComponentKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Detector => "detector",
            Self::Analyst => "analyst",
            Self::Rung => "rung",
            Self::Strategy => "strategy",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "detector" => Some(Self::Detector),
            "analyst" => Some(Self::Analyst),
            "rung" => Some(Self::Rung),
            "strategy" => Some(Self::Strategy),
            _ => None,
        }
    }
}

/// One component the platform can be reliable or unreliable at.
///
/// Ordered so a `BTreeMap` of them iterates deterministically — the order
/// reaches the API and the journal, and a replay that reorders is not a
/// replay. Serialised as `kind:id` because a JSON object key must be a
/// string, and the two halves are recoverable from that form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComponentKey {
    pub kind: ComponentKind,
    pub id: String,
}

impl ComponentKey {
    /// Refuses an empty id: a component with no name cannot be charged
    /// anything, and a blank key would silently pool every unnamed source.
    pub fn new(kind: ComponentKind, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a {} component needs an id; an unnamed component cannot hold an estimate",
                kind.as_str()
            )));
        }
        if id.contains(':') {
            return Err(Error::invalid(format!(
                "a component id may not contain ':' ({id:?}); it is the separator the \
                 serialised key uses"
            )));
        }
        Ok(Self { kind, id })
    }

    pub fn detector(id: impl Into<String>) -> Result<Self> {
        Self::new(ComponentKind::Detector, id)
    }

    pub fn analyst(id: impl Into<String>) -> Result<Self> {
        Self::new(ComponentKind::Analyst, id)
    }

    pub fn rung(id: impl Into<String>) -> Result<Self> {
        Self::new(ComponentKind::Rung, id)
    }

    pub fn strategy(id: impl Into<String>) -> Result<Self> {
        Self::new(ComponentKind::Strategy, id)
    }

    fn parse(text: &str) -> Result<Self> {
        let Some((kind, id)) = text.split_once(':') else {
            return Err(Error::invalid(format!(
                "component key {text:?} is not of the form kind:id"
            )));
        };
        let Some(kind) = ComponentKind::parse(kind) else {
            return Err(Error::invalid(format!(
                "component key {text:?} names an unknown kind {kind:?}"
            )));
        };
        Self::new(kind, id)
    }
}

impl fmt::Display for ComponentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.id)
    }
}

impl Serialize for ComponentKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ComponentKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(|error| D::Error::custom(error.message()))
    }
}

/// One graded outcome, as the self-model keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoredOutcome {
    pub hypothesis_id: String,
    /// The confidence the thesis was stated at, in `[0, 1]`.
    pub confidence: f64,
    /// Whether the verdict counted as correct — never a lucky one, because
    /// [`crate::evaluation::Verdict::counts_as_correct`] already refuses those.
    pub correct: bool,
    pub graded_at: Timestamp,
}

impl ScoredOutcome {
    /// Refuses a confidence outside `[0, 1]`: the Brier contribution of one
    /// would be a number about nothing.
    pub fn new(
        hypothesis_id: impl Into<String>,
        confidence: f64,
        correct: bool,
        graded_at: Timestamp,
    ) -> Result<Self> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(Error::invalid(format!(
                "confidence {confidence} is not a probability; a scored outcome cannot carry it"
            )));
        }
        Ok(Self {
            hypothesis_id: hypothesis_id.into(),
            confidence,
            correct,
            graded_at,
        })
    }

    /// Squared error of the stated confidence against the outcome.
    pub fn brier(&self) -> f64 {
        (self.confidence - f64::from(u8::from(self.correct))).powi(2)
    }
}

/// What a component's record says about it, once there is enough of one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// The estimated accuracy, in `(0, 1)`: the hit rate shrunk toward one
    /// half by [`PSEUDO_COUNTS`] pseudo-observations. See
    /// [`CapabilityEstimate::estimate`] for the formula.
    pub accuracy: f64,
    /// The raw hit rate over the window.
    pub hit_rate: f64,
    /// The mean Brier contribution over the window. Lower is better
    /// calibrated; a component right half the time at confidence 0.5 scores
    /// 0.25, and one right half the time at 0.9 scores worse.
    pub mean_brier: f64,
    pub sample_count: usize,
    pub last_updated: Timestamp,
}

/// A bounded record of one component's graded outcomes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapabilityEstimate {
    /// Oldest first, at most [`CAPABILITY_WINDOW`].
    window: VecDeque<ScoredOutcome>,
}

impl CapabilityEstimate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one outcome, dropping the oldest when the window is full.
    pub fn record(&mut self, outcome: ScoredOutcome) {
        self.window.push_back(outcome);
        while self.window.len() > CAPABILITY_WINDOW {
            self.window.pop_front();
        }
    }

    pub fn sample_count(&self) -> usize {
        self.window.len()
    }

    pub fn hits(&self) -> usize {
        self.window.iter().filter(|o| o.correct).count()
    }

    pub fn brier_sum(&self) -> f64 {
        self.window.iter().map(ScoredOutcome::brier).sum()
    }

    /// When the newest outcome was graded, or `None` for an empty record.
    pub fn last_updated(&self) -> Option<Timestamp> {
        self.window.back().map(|o| o.graded_at)
    }

    /// The outcomes in the window, oldest first.
    pub fn outcomes(&self) -> impl Iterator<Item = &ScoredOutcome> {
        self.window.iter()
    }

    /// Whether [`Self::estimate`] would report rather than refuse.
    pub fn is_estimable(&self) -> bool {
        self.window.len() >= MINIMUM_SAMPLE
    }

    /// The component's estimated accuracy, or a refusal naming the sample.
    ///
    /// The formula, with `h` hits over `n` outcomes and `k` =
    /// [`PSEUDO_COUNTS`]:
    ///
    /// ```text
    /// accuracy = (h + k / 2) / (n + k)
    /// ```
    ///
    /// That is the hit rate shrunk toward one half by `k` pseudo-observations,
    /// the same shape `BaseRate::shrunk` uses for a thin base rate, and it is
    /// the whole of the sample-size penalty: a component is never estimated
    /// at exactly 0 or 1, and the shrinkage is largest where the sample is
    /// smallest. Below [`MINIMUM_SAMPLE`] it is not computed at all — a
    /// consumer must be able to tell "measured at one half" from "not
    /// measured", and returning `0.5` for both would erase that difference.
    pub fn estimate(&self) -> Result<Capability> {
        let n = self.window.len();
        if n < MINIMUM_SAMPLE {
            return Err(Error::invalid(format!(
                "{n} graded outcome(s) is below the {MINIMUM_SAMPLE} a capability estimate \
                 needs; reporting one would be a guess dressed as a measurement"
            )));
        }
        let Some(last_updated) = self.last_updated() else {
            // Unreachable once `n >= MINIMUM_SAMPLE >= 1`; refused rather than
            // unwrapped so the invariant is checked, not assumed.
            return Err(Error::invalid(
                "a non-empty window has no newest outcome; the record is inconsistent",
            ));
        };
        // Statistics, so `f64`: a hit rate is not money.
        let hits = self.hits() as f64;
        let samples = n as f64;
        Ok(Capability {
            accuracy: (hits + PSEUDO_COUNTS / 2.0) / (samples + PSEUDO_COUNTS),
            hit_rate: hits / samples,
            mean_brier: self.brier_sum() / samples,
            sample_count: n,
            last_updated,
        })
    }
}

/// The platform's estimate of its own components, bounded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfModel {
    components: BTreeMap<ComponentKey, CapabilityEstimate>,
}

impl SelfModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one outcome against one component.
    ///
    /// Over [`MAX_COMPONENTS`] the least recently updated component is
    /// evicted — ties broken by key order, so two replays evict the same one.
    pub fn record(&mut self, key: ComponentKey, outcome: ScoredOutcome) {
        self.components.entry(key).or_default().record(outcome);
        while self.components.len() > MAX_COMPONENTS {
            let stalest = self
                .components
                .iter()
                .min_by_key(|(key, estimate)| (estimate.last_updated(), (*key).clone()))
                .map(|(key, _)| key.clone());
            match stalest {
                Some(key) => {
                    self.components.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Charge one graded evaluation to every component that produced it.
    ///
    /// An inconclusive verdict charges nothing: it is evidence about the
    /// tape, not about the component. Returns how many components were
    /// charged, so a caller can say so in the stage's account.
    pub fn absorb(
        &mut self,
        evaluation: &Evaluation,
        components: &[ComponentKey],
    ) -> Result<usize> {
        if !evaluation.verdict.is_informative() {
            return Ok(0);
        }
        let outcome = ScoredOutcome::new(
            evaluation.hypothesis_id.clone(),
            evaluation.confidence,
            evaluation.verdict.counts_as_correct(),
            evaluation.evaluated_at,
        )?;
        for key in components {
            self.record(key.clone(), outcome.clone());
        }
        Ok(components.len())
    }

    pub fn get(&self, key: &ComponentKey) -> Option<&CapabilityEstimate> {
        self.components.get(key)
    }

    /// The estimate for one component, refused where the component is
    /// unknown or its sample is thin.
    pub fn estimate(&self, key: &ComponentKey) -> Result<Capability> {
        match self.components.get(key) {
            Some(estimate) => estimate.estimate(),
            None => Err(Error::not_found(format!(
                "{key} has no graded outcomes; nothing it produced has resolved"
            ))),
        }
    }

    /// The factor the REASON stage scales a component's evidence by:
    /// its estimated accuracy, and `None` where there is not enough of a
    /// record to say. `None` means "leave the weight alone", which is the
    /// only honest reading of an unmeasured component.
    pub fn factor(&self, key: &ComponentKey) -> Option<f64> {
        self.estimate(key)
            .ok()
            .map(|capability| capability.accuracy)
    }

    /// Factors keyed by evidence origin, for every detector and analyst with
    /// a sufficient sample.
    ///
    /// Only those two kinds appear as an evidence origin — the anomaly's
    /// detector on the direct observation, the agent id on each finding — so
    /// only those two are offered. Where a detector and an analyst share an
    /// id the lower factor is kept: the conservative reading, and one a
    /// reader can predict.
    pub fn origin_factors(&self) -> BTreeMap<String, f64> {
        let mut factors = BTreeMap::new();
        for (key, estimate) in &self.components {
            if !matches!(key.kind, ComponentKind::Detector | ComponentKind::Analyst) {
                continue;
            }
            let Ok(capability) = estimate.estimate() else {
                continue;
            };
            factors
                .entry(key.id.clone())
                .and_modify(|held: &mut f64| *held = held.min(capability.accuracy))
                .or_insert(capability.accuracy);
        }
        factors
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Every component and its record, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&ComponentKey, &CapabilityEstimate)> {
        self.components.iter()
    }
}
