//! Finding features worth adding to the vocabulary.
//!
//! Screening a thousand candidate features against one return series and
//! keeping the best correlations is the same mistake as generating a thousand
//! strategies and keeping the best backtest, and it has the same cure: say how
//! many were screened, and raise the bar by what that number alone would
//! produce.
//!
//! The bar here is `sqrt(2·ln(m) / n)`, the order of the largest absolute
//! correlation `m` independent noise series would show against `n`
//! observations. It plays the part [`qip_simulation_engine::validation::deflated_sharpe`]
//! plays for a Sharpe ratio, and for the same reason. Screening ten features
//! over five hundred observations sets a bar near 0.10; screening a thousand
//! sets it near 0.17, and a feature correlating at 0.13 is a discovery in the
//! first search and noise in the second. Nothing about the feature changed.
//!
//! A correlation over the whole sample is also not enough on its own, because
//! one regime can carry it. Every candidate is measured again on each fold of
//! a [`qip_simulation_engine::validation::PurgedSplit`] — purged and embargoed,
//! so a label spanning a fold boundary does not leak — and a candidate whose
//! sign does not survive the folds is reported as unstable rather than
//! promoted on its average.
//!
//! Confidence is a [`Conviction`] over the folds that agreed, so a feature
//! stable across three folds does not read like one stable across twenty.

use qip_contracts::{Conviction, FeatureKey};
use qip_core::error::{Error, Result};
use qip_numerics::stats;
use qip_simulation_engine::validation::PurgedSplit;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::ir::Type;

/// A feature offered for screening, with its history.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureCandidate {
    pub key: FeatureKey,
    /// The type the feature would be declared at.
    pub value_type: Type,
    /// The feature's value at each observation, in time order.
    pub values: Vec<f64>,
}

impl FeatureCandidate {
    pub fn new(key: FeatureKey, value_type: Type, values: Vec<f64>) -> Self {
        Self {
            key,
            value_type,
            values,
        }
    }
}

/// What the screen demands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiscoveryPolicy {
    pub folds: usize,
    /// Observations a label spans, which sets the purge width.
    pub label_horizon: usize,
    /// Observations embargoed after each test fold.
    pub embargo: usize,
    pub minimum_observations: usize,
    /// Fraction of folds whose correlation must share the whole-sample sign.
    pub minimum_fold_agreement: f64,
    /// A floor under the multiple-testing bar. A correlation below this is not
    /// worth a feature however few were screened.
    pub floor_correlation: f64,
}

impl Default for DiscoveryPolicy {
    fn default() -> Self {
        Self {
            folds: 5,
            label_horizon: 10,
            embargo: 5,
            minimum_observations: 250,
            // Three folds in five. Two out of five is a coin flip dressed as
            // evidence; five out of five almost never happens on a real
            // feature and would set a bar nothing clears.
            minimum_fold_agreement: 0.6,
            floor_correlation: 0.03,
        }
    }
}

/// One feature the screen has something to say about.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureProposal {
    pub key: FeatureKey,
    pub value_type: Type,
    /// Rank correlation against the forward return over the whole sample.
    ///
    /// Rank rather than linear: a feature whose relationship is monotone but
    /// not straight is still a feature, and one extreme observation cannot
    /// manufacture a rank correlation the way it can a linear one.
    pub rank_correlation: f64,
    /// Fraction of folds whose correlation shared the whole-sample sign.
    pub fold_agreement: f64,
    pub folds: usize,
    pub observations: usize,
    /// How many features were screened to find this one.
    pub screened: usize,
    /// The correlation this feature had to beat, given how many were screened.
    pub bar: f64,
}

impl FeatureProposal {
    /// Whether the correlation survives the search that found it.
    pub fn clears_the_search(&self) -> bool {
        self.rank_correlation.abs() >= self.bar
    }

    /// Whether the relationship held across the folds.
    pub fn is_stable(&self, policy: &DiscoveryPolicy) -> bool {
        self.fold_agreement >= policy.minimum_fold_agreement
    }

    /// How much to believe the stability claim, shrunk by the number of folds
    /// behind it.
    pub fn confidence(&self) -> Conviction {
        Conviction::new(
            self.fold_agreement,
            u32::try_from(self.folds).unwrap_or(u32::MAX),
        )
    }

    /// Whether this feature has earned a place in the vocabulary.
    pub fn is_discovery(&self, policy: &DiscoveryPolicy) -> bool {
        self.clears_the_search() && self.is_stable(policy)
    }

    /// Declare the feature into a catalogue, so a generator may name it.
    ///
    /// The one path from discovery to generation, and it is a deliberate step
    /// somebody takes: a feature that has not been declared is refused by the
    /// compiler with its own `not_found`, which is exactly the friction that
    /// stops a screening result becoming a live input by accident.
    pub fn declare_into(&self, catalogue: &mut FeatureCatalogue) -> Result<()> {
        catalogue.declare(self.key.clone(), self.value_type)
    }

    pub fn summarise(&self) -> String {
        format!(
            "{}: rank correlation {:+.3} against a {:.3} bar from {} screened, \
             holding in {:.0}% of {} fold(s)",
            self.key.canonical(),
            self.rank_correlation,
            self.bar,
            self.screened,
            self.fold_agreement * 100.0,
            self.folds
        )
    }
}

/// Screens candidate features against a forward return series.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeatureScreen {
    pub policy: DiscoveryPolicy,
}

impl FeatureScreen {
    pub fn new(policy: DiscoveryPolicy) -> Self {
        Self { policy }
    }

    /// Screen every candidate, refusing a search whose size was not declared.
    ///
    /// `screened` is the number of features the search actually looked at,
    /// which may exceed the number offered here — a caller that tried two
    /// thousand and forwards the best fifty must say two thousand. `None` is
    /// refused for the same reason an undeclared trial count is refused by
    /// [`crate::challenger::ChallengerTest`]: the correction is not optional,
    /// and the only number that could stand in for a missing one is the
    /// flattering one.
    ///
    /// Results come back in the order the candidates were given, so a run is
    /// reproducible and diffable.
    pub fn screen(
        &self,
        candidates: &[FeatureCandidate],
        forward_returns: &[f64],
        screened: Option<usize>,
    ) -> Result<Vec<FeatureProposal>> {
        let Some(screened) = screened else {
            return Err(Error::invalid(
                "feature discovery was run without declaring how many features were \
                 screened; the best of an undeclared search is not a discovery",
            ));
        };
        if screened < candidates.len() {
            return Err(Error::invalid(format!(
                "{screened} features declared as screened but {} were offered",
                candidates.len()
            )));
        }
        if forward_returns.len() < self.policy.minimum_observations {
            return Err(Error::invalid(format!(
                "{} observations is below the {} a screen needs",
                forward_returns.len(),
                self.policy.minimum_observations
            )));
        }

        let splits = PurgedSplit::new(
            self.policy.folds,
            self.policy.label_horizon,
            self.policy.embargo,
        )?
        .split(forward_returns.len())?;
        let bar = self.selection_bar(screened, forward_returns.len());

        let mut proposals = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if candidate.values.len() != forward_returns.len() {
                return Err(Error::invalid(format!(
                    "{} has {} values against {} returns; a feature measured over a \
                     different window is not being screened against these returns",
                    candidate.key.canonical(),
                    candidate.values.len(),
                    forward_returns.len()
                )));
            }
            let overall = stats::rank_correlation(&candidate.values, forward_returns);
            let mut agreeing = 0usize;
            for split in &splits {
                let feature: Vec<f64> = split
                    .test
                    .iter()
                    .filter_map(|index| candidate.values.get(*index).copied())
                    .collect();
                let returns: Vec<f64> = split
                    .test
                    .iter()
                    .filter_map(|index| forward_returns.get(*index).copied())
                    .collect();
                if feature.len() < 3 {
                    continue;
                }
                let fold = stats::rank_correlation(&feature, &returns);
                if fold * overall > 0.0 {
                    agreeing += 1;
                }
            }
            let folds = splits.len();
            proposals.push(FeatureProposal {
                key: candidate.key.clone(),
                value_type: candidate.value_type,
                rank_correlation: overall,
                fold_agreement: if folds == 0 {
                    0.0
                } else {
                    agreeing as f64 / folds as f64
                },
                folds,
                observations: forward_returns.len(),
                screened,
                bar,
            });
        }
        Ok(proposals)
    }

    /// The correlation a feature must beat, given how many were screened.
    ///
    /// `sqrt(2·ln(m)/n)`, floored. This is the order of the largest absolute
    /// correlation `m` independent noise series show against `n` observations,
    /// and it is the whole multiple-testing correction: the bar rises with the
    /// search and falls with the evidence.
    pub fn selection_bar(&self, screened: usize, observations: usize) -> f64 {
        if observations == 0 {
            return f64::INFINITY;
        }
        let m = (screened.max(1) as f64).max(std::f64::consts::E);
        let expected_max = (2.0 * m.ln() / observations as f64).sqrt();
        expected_max.max(self.policy.floor_correlation)
    }

    /// The candidates that cleared both the search and the folds.
    pub fn discoveries(&self, proposals: &[FeatureProposal]) -> Vec<FeatureProposal> {
        proposals
            .iter()
            .filter(|proposal| proposal.is_discovery(&self.policy))
            .cloned()
            .collect()
    }
}
