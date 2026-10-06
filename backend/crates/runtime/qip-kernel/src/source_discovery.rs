//! Where source discovery starts (DATA-031): from what this platform is
//! blind to and what it is wrong about, where nothing covers either.
//!
//! `qip_data_finder::discovery_targets::targets_from` has ranked gaps and
//! forecast-error spikes since it was written, and nothing fed it: discovery
//! began from an operator's candidate list, so the platform looked only
//! where a person had already thought to look, and its own scored theses —
//! the cheapest evidence a source is missing — went unread. The three inputs
//! live in three services (the world model, the learning window, the
//! reference ledger and finder), which is why they meet here and not in any
//! one of them.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::discovery_targets::{
    DiscoveryTarget, ForecastError, KnowledgeGap, targets_from,
};
use std::collections::{BTreeMap, BTreeSet};

/// The claim and outcome [`Platform::learn_from`] takes — the call that fills
/// the window [`Platform::forecast_errors`] reads. Re-exported because a
/// public function whose argument types its caller cannot name is callable
/// only from inside this crate, and the composition root that runs discovery
/// is not.
pub use qip_learning_engine::evaluation::{Outcome as ThesisOutcome, ThesisClaim};

/// How many times the platform's usual forecast error an entity's latest one
/// must reach to be read as a spike rather than as a bad day.
pub const DISCOVERY_SPIKE_MULTIPLE: f64 = 3.0;

/// The feature whose absence is a gap: an instrument with no close the world
/// model would serve now is one the platform is asked about and cannot see.
const GAP_FEATURE: &str = "close";

impl Platform {
    /// Universe entities the world model holds no current close for, in
    /// identifier order.
    pub fn knowledge_gaps(&self, now: Timestamp) -> Vec<KnowledgeGap> {
        let market = self.market_view();
        let world = self.world();
        let mut gaps: Vec<KnowledgeGap> = market
            .universe
            .ids()
            .filter(|id| {
                world
                    .features()
                    .current(GAP_FEATURE, id.as_str(), now)
                    .is_none()
            })
            .map(|id| KnowledgeGap {
                entity: id.as_str().to_string(),
            })
            .collect();
        gaps.sort_by(|left, right| left.entity.cmp(&right.entity));
        gaps
    }

    /// Each entity's latest forecast error against the error the platform
    /// usually makes, from the calibration window, in entity order.
    ///
    /// The error is how far the realised move landed from the expected one,
    /// in the claim's own direction, in basis points. The baseline is the
    /// mean of every *other* error in the window: an entity nothing covers
    /// rarely has a history of its own to be "usual" against, and the
    /// platform's own typical miss is the honest yardstick. An entity is
    /// left out — not given a spike — when there is nothing to measure it
    /// against, when its claim stated no magnitude, or when its record
    /// predates the subject field; `targets_from` refuses a baseline that
    /// would manufacture one.
    pub fn forecast_errors(&self) -> Vec<ForecastError> {
        // Statistics, not money: basis-point moves are graded as floats in
        // the learning engine and stay floats here.
        let scored: Vec<(&str, f64)> = self
            .evaluations()
            .iter()
            .filter(|evaluation| !evaluation.subject.is_empty())
            .filter_map(|evaluation| {
                let expected = evaluation.expected_move_bps.abs();
                let error = (evaluation.magnitude_ratio - 1.0).abs() * expected;
                (expected > 1e-9 && error.is_finite())
                    .then_some((evaluation.subject.as_str(), error))
            })
            .collect();
        let total: f64 = scored.iter().map(|(_, error)| error).sum();
        // Oldest first, so the last write per entity is its latest error.
        let latest: BTreeMap<&str, f64> = scored.iter().copied().collect();
        latest
            .into_iter()
            .filter_map(|(entity, error)| {
                let others = scored.len().checked_sub(1).filter(|count| *count > 0)?;
                let baseline = (total - error) / others as f64;
                (baseline.is_finite() && baseline > 0.0).then(|| ForecastError {
                    entity: entity.to_string(),
                    error,
                    baseline,
                })
            })
            .collect()
    }

    /// Whether some source already covers `entity`: a vendor has delivered
    /// data naming it, or a registered source that is not quarantined
    /// declares it. A source the platform generated is not cover — it cannot
    /// tell the platform anything it did not already believe.
    fn has_source_for(&self, entity: &str) -> bool {
        self.sources_backing(entity)
            .values()
            .any(|origin| origin.is_independent_vendor())
            || self.registered_sources().values().any(|registered| {
                !registered.is_quarantined()
                    && registered
                        .source()
                        .coverage()
                        .instruments()
                        .contains(entity)
            })
    }

    /// What discovery should look for now: uncovered entities the world
    /// model has a gap on or the platform's forecasts spiked on, largest
    /// spike first.
    pub fn discovery_targets(&self, now: Timestamp) -> Result<Vec<DiscoveryTarget>> {
        let gaps = self.knowledge_gaps(now);
        let errors = self.forecast_errors();
        let covered: BTreeSet<String> = gaps
            .iter()
            .map(|gap| gap.entity.as_str())
            .chain(errors.iter().map(|error| error.entity.as_str()))
            .filter(|entity| self.has_source_for(entity))
            .map(str::to_string)
            .collect();
        targets_from(&gaps, &errors, &covered, DISCOVERY_SPIKE_MULTIPLE)
    }
}
