//! Where the research node actually learns, and where its models age out.
//!
//! The third gap of exactly the same shape as the foundry's and the succession
//! desk's, and the widest of the three. `qip-training` fits models,
//! `qip_kernel::central::models::register_fit` writes the fit's own holdout
//! verdict onto a model card, `ModelRegistry` governs which cards may inform a
//! decision, and `ModelCard::decision_eligibility` refuses a card whose drift
//! score has passed its threshold.
//!
//! Every one of those worked. No running process built a [`ModelRegistry`],
//! called `register_fit`, or fitted anything at all: the whole model-governance
//! subsystem was reachable only from its own tests. `drift_score` was therefore
//! `0.0` on every card that could exist, and the drift branch of
//! `decision_eligibility` could not fire — the shape this codebase keeps
//! producing, where a control is present, correct, and connected to a value
//! nothing writes.
//!
//! # What a round does
//!
//! 1. **Assemble.** Features from the subject's own observed bars, target the
//!    next bar's return. Every feature at bar *i* reads bars up to and
//!    including *i*, and the target spans *i* to *i+1*, so the label is always
//!    on the far side of everything used to predict it.
//! 2. **Fit and register.** `register_fit` takes no `passed` argument: the
//!    verdict is the fit's own out-of-sample result against the skill policy.
//!    A model with no signal is registered as having none rather than not
//!    registered — a failed fit is a fact worth keeping, and deleting it would
//!    let the node retry until something passed with the failures invisible.
//! 3. **Watch it age.** The feature sample the model was fitted on is kept as
//!    its reference. Each later round compares the current window against it
//!    with [`DriftReport::compare`] and records the largest population
//!    stability index across features.
//!
//! # Why the maximum across features and not the mean
//!
//! A model reading eight features of which one has moved to a distribution it
//! never saw is a model making predictions from a value outside its training
//! range. Averaging that against seven stable features reports a calm number
//! for a model that is extrapolating, and extrapolation is where a fitted
//! function's error stops being bounded by anything it was measured on.
//!
//! # What it does not do
//!
//! It does not promote. A registered card enters at development stage and
//! moving it to one that permits decisions stays a governed act elsewhere.
//! This module only ensures that when that decision is taken, the evidence on
//! the card is true and its drift score is a measurement rather than a zero
//! nobody ever wrote.

use qip_ai::evaluation::DriftReport;
use qip_ai::registry::ModelRegistry;
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_kernel::central::models::{ModelRegistration, register_fit};
use qip_market::bar::Bar;
use qip_quant::signal::Horizon;
use qip_training::dataset::TrainingDataset;
use qip_training::job::TrainingSpec;
use qip_training::local::{LocalTrainer, ModelFamily, SkillPolicy};
use std::collections::BTreeMap;

/// The features a bar-derived model reads, in the order the dataset carries
/// them.
///
/// Deliberately the vocabulary the strategy harness already computes rather
/// than a second one: two definitions of "momentum over five bars" that drift
/// apart is a defect nobody finds, because both look right in isolation.
const FEATURES: [&str; 5] = [
    "return_1",
    "momentum_5",
    "volatility_10",
    "range_frac",
    "volume_share",
];

/// Bars of history a feature row needs behind it.
///
/// The longest window any feature above reads. A row assembled with less is not
/// a row with a smaller window; it is a row whose features are computed from
/// data that is not there.
const LOOKBACK: usize = 10;

/// How the learning loop is tuned.
#[derive(Clone, Debug)]
pub struct LearningConfig {
    /// Fit a model every this many research cycles. Zero disables the loop.
    pub every_cycles: u64,
    /// Bars a subject needs before a fit is worth attempting.
    pub minimum_bars: usize,
    /// Fraction of the tail held out of fitting, by time.
    pub holdout_fraction: f64,
    /// Histogram bins for the population stability index.
    ///
    /// Ten is the convention the index was tabulated against, and the
    /// interpretation bands on [`DriftReport::population_stability_index`]
    /// assume it. Changing it changes what 0.25 means.
    pub drift_bins: usize,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            every_cycles: 8,
            minimum_bars: 256,
            holdout_fraction: 0.25,
            drift_bins: 10,
        }
    }
}

/// What one learning round produced.
#[derive(Clone, Debug)]
pub struct LearningRound {
    pub subject: String,
    /// The model registered this round, where one was.
    pub registration: Option<ModelRegistration>,
    /// Drift measured against each standing model's own reference sample.
    pub drift: Vec<DriftObservation>,
    /// Models the registry will no longer let inform a decision, with why.
    pub ineligible: Vec<String>,
}

impl LearningRound {
    pub fn describe(&self) -> String {
        let registered = match &self.registration {
            Some(registration) => registration.summarise(),
            None => "no fit this round".to_string(),
        };
        format!(
            "learning: {registered}; {} model(s) measured for drift, {} ineligible",
            self.drift.len(),
            self.ineligible.len()
        )
    }
}

/// One model's drift, and the feature that produced it.
#[derive(Clone, Debug)]
pub struct DriftObservation {
    pub reference: String,
    /// The largest population stability index across the model's features.
    pub population_stability_index: f64,
    /// The feature that produced it. The number alone tells an operator
    /// something moved; this tells them what to look at.
    pub worst_feature: String,
    /// Whether the index has passed *this card's* drift threshold.
    ///
    /// Deliberately not "is this model still decision-eligible".
    /// `decision_eligibility` refuses a development-stage card before it ever
    /// reads the drift score, so reporting eligibility here would say
    /// "ineligible" for every freshly fitted model and read as though drift had
    /// disqualified it. Conflating a stage with a measurement is how a number
    /// that means nothing ends up on a dashboard meaning something.
    pub above_threshold: bool,
}

/// Running totals for the shutdown report.
#[derive(Clone, Copy, Debug, Default)]
pub struct LearningStats {
    pub rounds: u64,
    pub registered: u64,
    /// Fits that did not clear the skill bar. Kept separately because a node
    /// registering only failures is a different problem from one registering
    /// nothing.
    pub without_skill: u64,
    pub drift_measurements: u64,
}

/// Fits models from observed bars and watches the ones it has fitted age.
pub struct LearningDesk {
    config: LearningConfig,
    policy: SkillPolicy,
    registry: ModelRegistry,
    /// Per registered model, the feature columns it was fitted on.
    ///
    /// Kept because drift is a comparison against *what this model saw*, and
    /// nothing else in the platform remembers that. A drift score computed
    /// against the current window's own earlier half would be measuring the
    /// data's recent stability rather than this model's distance from its
    /// training set.
    reference: BTreeMap<String, BTreeMap<String, Vec<f64>>>,
    /// Fits attempted, which is what a model's version is drawn from.
    fits: u64,
    seed: u64,
    stats: LearningStats,
}

impl std::fmt::Debug for LearningDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearningDesk")
            .field("config", &self.config)
            .field("models", &self.reference.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl LearningDesk {
    pub fn new(config: LearningConfig, seed: u64) -> Self {
        Self {
            config,
            policy: SkillPolicy::default(),
            registry: ModelRegistry::new(),
            reference: BTreeMap::new(),
            fits: 0,
            seed,
            stats: LearningStats::default(),
        }
    }

    pub const fn stats(&self) -> LearningStats {
        self.stats
    }

    pub const fn enabled(&self) -> bool {
        self.config.every_cycles > 0
    }

    pub const fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// The registry, mutably, for the governed acts this module does not
    /// perform itself -- promotion above all. Kept separate from `registry`
    /// so that "this desk fitted a model" and "somebody staged it for
    /// decisions" stay two different sentences in the call graph.
    pub const fn registry_mut(&mut self) -> &mut ModelRegistry {
        &mut self.registry
    }

    /// Models this desk has fitted and still holds a reference sample for.
    pub fn tracked(&self) -> usize {
        self.reference.len()
    }

    /// Fit a model if the cadence says so, then measure every standing model
    /// against the window that has arrived since it was fitted.
    ///
    /// `None` means "not this cycle", which is the normal case.
    pub fn maybe_learn(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        cycle: u64,
        now: Timestamp,
    ) -> Result<Option<LearningRound>> {
        if self.config.every_cycles == 0 || cycle % self.config.every_cycles != 0 {
            return Ok(None);
        }
        if bars.len() < self.config.minimum_bars {
            return Ok(None);
        }
        Ok(Some(self.learn(subject, bars, now)?))
    }

    fn learn(&mut self, subject: &ObjectId, bars: &[Bar], now: Timestamp) -> Result<LearningRound> {
        self.stats.rounds += 1;
        let columns = feature_columns(bars);

        // Drift first, against the window as it stands *before* this round's
        // fit joins the registry. Measuring a model against a window that
        // includes the data a newer model was fitted on would be comparing two
        // different questions.
        let mut drift = Vec::new();
        for (reference, fitted_on) in &self.reference {
            let Some((worst_feature, index)) =
                worst_drift(fitted_on, &columns, self.config.drift_bins)
            else {
                continue;
            };
            drift.push(DriftObservation {
                reference: reference.clone(),
                population_stability_index: index,
                worst_feature,
                above_threshold: false,
            });
        }
        for observation in &mut drift {
            self.registry.record_drift(
                &observation.reference,
                observation.population_stability_index,
            )?;
            observation.above_threshold = self
                .registry
                .get(&observation.reference)
                .is_some_and(|card| observation.population_stability_index > card.drift_threshold);
            self.stats.drift_measurements += 1;
        }

        let registration = match self.fit(subject, bars, &columns, now) {
            Ok(registration) => {
                if registration.passed {
                    self.stats.registered += 1;
                } else {
                    self.stats.without_skill += 1;
                }
                self.reference
                    .insert(registration.reference.clone(), columns);
                Some(registration)
            }
            // A round that could not fit is not a round that found nothing:
            // too little history, a degenerate target, a dataset the trainer
            // refused. Returned on the round rather than propagated, so one
            // unfittable subject does not stop the node.
            Err(error) => {
                return Ok(LearningRound {
                    subject: subject.as_str().to_string(),
                    registration: None,
                    drift,
                    ineligible: vec![error.message().to_string()],
                });
            }
        };

        let ineligible = self
            .registry
            .ineligible(now)
            .into_iter()
            .map(|(card, reason)| format!("{}: {reason}", card.reference()))
            .collect();

        Ok(LearningRound {
            subject: subject.as_str().to_string(),
            registration,
            drift,
            ineligible,
        })
    }

    fn fit(
        &mut self,
        subject: &ObjectId,
        bars: &[Bar],
        columns: &BTreeMap<String, Vec<f64>>,
        now: Timestamp,
    ) -> Result<ModelRegistration> {
        let targets = next_bar_returns(bars);
        let times: Vec<Timestamp> = bars
            .iter()
            .skip(LOOKBACK)
            .take(targets.len())
            .map(Bar::close_time)
            .collect();
        let names: Vec<String> = FEATURES.iter().map(|name| (*name).to_string()).collect();
        let rows: Vec<Vec<f64>> = (0..targets.len())
            .map(|row| {
                names
                    .iter()
                    .map(|name| {
                        columns
                            .get(name)
                            .and_then(|c| c.get(row))
                            .copied()
                            .unwrap_or(0.0)
                    })
                    .collect()
            })
            .collect();
        if rows.len() != times.len() {
            return Err(Error::invalid(format!(
                "{} feature row(s) against {} timestamp(s); a row whose instant is unknown \
                 cannot be split by time, and a split that is not by time leaks the future",
                rows.len(),
                times.len()
            )));
        }

        let dataset = TrainingDataset::new(
            format!("bars-{}", subject.as_str()),
            names,
            rows,
            targets,
            times,
        )?;
        // Versioned by the desk's own fit count, not by the observation
        // count. `ModelRegistry::register` replaces a card of the same
        // reference outright, and two rounds over the same amount of history
        // produced the same version -- so the second fit silently overwrote the
        // first, taking its drift score back to zero with it. A model whose
        // measured drift is erased by the arrival of its successor is a control
        // that resets itself exactly when it matters.
        self.fits += 1;
        let spec = TrainingSpec::new(
            format!("bar-teacher-{}", subject.as_str()),
            format!("0.{}.0", self.fits),
            "central-research",
            dataset.name(),
            ModelFamily::Linear { ridge: 1e-3 },
        )
        .with_purpose(format!(
            "predicts the next bar's return on {} from its own observed bars",
            subject.as_str()
        ))
        .with_horizon(Horizon::Intraday)
        .with_holdout(self.config.holdout_fraction)
        .with_seed(self.seed);

        let teacher = LocalTrainer::new().fit(&spec, &dataset, now)?;
        register_fit(
            &mut self.registry,
            &teacher,
            &self.policy,
            "central-research",
            now,
        )
    }
}

/// The largest population stability index across the features two samples
/// share, and the feature that produced it.
///
/// The maximum and not the mean: a model reading eight features of which one
/// has moved outside its training range is extrapolating, and averaging that
/// against seven stable features reports a calm number for exactly the case
/// where a fitted function's error stops being bounded by anything measured.
fn worst_drift(
    reference: &BTreeMap<String, Vec<f64>>,
    current: &BTreeMap<String, Vec<f64>>,
    bins: usize,
) -> Option<(String, f64)> {
    reference
        .iter()
        .filter_map(|(name, sample)| {
            let live = current.get(name)?;
            let report = DriftReport::compare(sample, live, bins);
            Some((name.clone(), report.population_stability_index))
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
}

/// Feature columns over `bars`, one row per bar that has both a full lookback
/// behind it and a next bar ahead of it.
///
/// Row *i* reads bars up to and including `bars[LOOKBACK + i]`, and the target
/// for that row spans that bar to the next. The label is therefore always on
/// the far side of every value used to predict it, which is the property a
/// backtest cannot recover if the dataset does not have it.
fn feature_columns(bars: &[Bar]) -> BTreeMap<String, Vec<f64>> {
    let mut columns: BTreeMap<String, Vec<f64>> = FEATURES
        .iter()
        .map(|name| ((*name).to_string(), Vec::new()))
        .collect();
    if bars.len() <= LOOKBACK + 1 {
        return columns;
    }
    let closes: Vec<f64> = bars.iter().map(|bar| bar.close.to_f64()).collect();
    let volumes: Vec<f64> = bars.iter().map(|bar| bar.volume.to_f64()).collect();

    let mut push = |name: &str, value: f64| {
        if let Some(column) = columns.get_mut(name) {
            column.push(if value.is_finite() { value } else { 0.0 });
        }
    };

    for at in LOOKBACK..bars.len() - 1 {
        let close = closes[at];
        push("return_1", ratio(close, closes[at - 1]));
        push("momentum_5", ratio(close, closes[at - 5]));

        let window: Vec<f64> = (at - 9..=at)
            .map(|i| ratio(closes[i], closes[i - 1]))
            .collect();
        push("volatility_10", qip_numerics::stats::stddev(&window));

        let bar = &bars[at];
        let high = bar.high.to_f64();
        let low = bar.low.to_f64();
        push(
            "range_frac",
            if close.abs() > f64::EPSILON {
                (high - low) / close
            } else {
                0.0
            },
        );

        // Volume relative to its own trailing mean, not raw volume. A raw
        // level is an instrument-specific magnitude, and a model fitted on one
        // instrument's volume learns that instrument's size rather than
        // anything about markets.
        let trailing: f64 = volumes[at - LOOKBACK..at].iter().sum::<f64>() / LOOKBACK as f64;
        push(
            "volume_share",
            if trailing > f64::EPSILON {
                volumes[at] / trailing
            } else {
                0.0
            },
        );
    }
    columns
}

/// The return from each feature row's bar to the next.
fn next_bar_returns(bars: &[Bar]) -> Vec<f64> {
    if bars.len() <= LOOKBACK + 1 {
        return Vec::new();
    }
    (LOOKBACK..bars.len() - 1)
        .map(|at| ratio(bars[at + 1].close.to_f64(), bars[at].close.to_f64()))
        .collect()
}

/// A simple return, guarded against a zero denominator.
///
/// The crossing point from money to statistics: the closes are `Decimal`
/// because they are prices, and everything from here is `f64` because a return
/// is a ratio and a ratio is not money.
fn ratio(current: f64, previous: f64) -> f64 {
    if previous.abs() < 1e-12 {
        return 0.0;
    }
    current / previous - 1.0
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn subject() -> ObjectId {
        ObjectId::from_string("OBJ0000000000000000000AAA")
    }

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn learning_desk() -> LearningDesk {
        LearningDesk::new(
            LearningConfig {
                every_cycles: 1,
                minimum_bars: 64,
                ..LearningConfig::default()
            },
            7,
        )
    }

    #[test]
    fn a_round_registers_a_model_carrying_its_own_out_of_sample_verdict() -> Result<()> {
        // Nothing in any running process had ever built a registry, fitted a
        // model, or called `register_fit`. The whole model-governance
        // subsystem -- skill verdicts, evaluations, drift, eligibility -- was
        // reachable only from its own tests.
        let mut desk = learning_desk();
        // The premise: nothing is registered before the round.
        assert_eq!(desk.registry().len(), 0);
        assert_eq!(desk.tracked(), 0);

        let bars = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &bars, 1, at())?
            .ok_or_else(|| Error::not_found("a round on a cadence of every cycle"))?;

        let registration = round.registration.ok_or_else(|| {
            Error::not_found(format!("a registration; got {:?}", round.ineligible))
        })?;
        assert_eq!(
            desk.registry().len(),
            1,
            "the registry did not receive the fit"
        );
        assert_eq!(
            desk.tracked(),
            1,
            "no reference sample was kept to measure drift against"
        );

        // The card's evaluation is the fit's own holdout result, not a claim
        // about it: `register_fit` takes no `passed` argument.
        let card = desk
            .registry()
            .get(&registration.reference)
            .ok_or_else(|| Error::not_found("the card just registered"))?;
        let evaluation = card
            .latest_evaluation()
            .ok_or_else(|| Error::not_found("the evaluation on the card"))?;
        assert_eq!(
            evaluation.passed, registration.passed,
            "the card's evaluation disagrees with the verdict the fit produced"
        );
        let holdout_r2 = evaluation
            .metrics
            .get("holdout_r2")
            .copied()
            .ok_or_else(|| Error::not_found("the out-of-sample figure on the evaluation"))?;

        // The verdict is the fit's own out-of-sample result and not a claim
        // about it: `register_fit` takes no `passed` argument. Asserted against
        // the policy's own bar in both directions, because a card that simply
        // agreed with itself would agree just as happily if the verdict were
        // inverted.
        let policy = SkillPolicy::default();
        assert_eq!(
            evaluation.passed,
            holdout_r2 >= policy.minimum_holdout_r2,
            "the card says passed={} on a holdout R-squared of {holdout_r2:.4} against a \
             {:.4} bar",
            evaluation.passed,
            policy.minimum_holdout_r2
        );

        // And the other direction, on a series with nothing to learn. Without
        // this, every assertion above holds for a registry that marks
        // everything as having skill.
        let mut fresh = learning_desk();
        let noise = super::tests_support::unlearnable(400);
        let refused = fresh
            .maybe_learn(&subject(), &noise, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration for the noise fit"))?;
        assert!(
            !refused.passed,
            "a model fitted on a series with no predictable structure was registered as \
             having cleared the skill bar"
        );
        let noise_card = fresh
            .registry()
            .get(&refused.reference)
            .ok_or_else(|| Error::not_found("the noise card"))?;
        assert!(
            noise_card
                .latest_evaluation()
                .is_some_and(|evaluation| !evaluation.passed),
            "the card disagrees with the registration about whether the fit had skill"
        );
        Ok(())
    }

    #[test]
    fn a_model_whose_features_have_moved_records_drift_past_its_own_threshold() -> Result<()> {
        // The control that could not fire. `decision_eligibility` refuses a
        // card whose drift score has passed its threshold, and `drift_score`
        // was 0.0 on every card that could exist because nothing outside a
        // test ever called `record_drift`.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let round = desk
            .maybe_learn(&subject(), &calm, 1, at())?
            .ok_or_else(|| Error::not_found("the first round"))?;
        let reference = round
            .registration
            .ok_or_else(|| Error::not_found("a registration"))?
            .reference;

        // The premise, in two parts: the model starts at zero drift, and the
        // first round measured nothing because there was nothing standing to
        // measure.
        assert!(
            round.drift.is_empty(),
            "the first round measured drift against itself"
        );
        let threshold = desk
            .registry()
            .get(&reference)
            .map(|card| card.drift_threshold)
            .ok_or_else(|| Error::not_found("the card"))?;
        assert_eq!(
            desk.registry().get(&reference).map(|card| card.drift_score),
            Some(0.0),
            "the card began with a drift score somebody had already written"
        );

        // A regime the model has never seen: the same instrument, now moving
        // in violent alternating jumps instead of a steady drift.
        let shocked = super::tests_support::shocked(400);
        let second = desk
            .maybe_learn(&subject(), &shocked, 2, at())?
            .ok_or_else(|| Error::not_found("the second round"))?;

        let observation = second
            .drift
            .iter()
            .find(|observation| observation.reference == reference)
            .ok_or_else(|| Error::not_found("drift measured against the standing model"))?;
        assert!(
            observation.population_stability_index > threshold,
            "a regime change produced a stability index of {:.3} against a {:.3} threshold; \
             the comparison is not sensitive enough to be a control",
            observation.population_stability_index,
            threshold
        );
        assert!(observation.above_threshold);
        assert!(
            FEATURES.contains(&observation.worst_feature.as_str()),
            "the drift names a feature the model does not read: {}",
            observation.worst_feature
        );

        // The reported index is the *largest* across features, not an average
        // and not the smallest. A model reading five features of which one has
        // moved outside its training range is extrapolating, and a statistic
        // that lets four calm features speak for the fifth reports a calm
        // number for exactly that case.
        let before = super::feature_columns(&calm);
        let after = super::feature_columns(&shocked);
        let mut indices: Vec<(String, f64)> = before
            .iter()
            .filter_map(|(name, sample)| {
                let live = after.get(name)?;
                Some((
                    name.clone(),
                    DriftReport::compare(sample, live, LearningConfig::default().drift_bins)
                        .population_stability_index,
                ))
            })
            .collect();
        indices.sort_by(|left, right| left.1.total_cmp(&right.1));
        // The premise: the features disagree. If every one moved by the same
        // amount, maximum and minimum would be the same number and this would
        // prove nothing about which was chosen.
        let (lowest, highest) = match indices.as_slice() {
            [first, .., last] => (first, last),
            other => {
                return Err(Error::not_found(format!(
                    "at least two features; got {}",
                    other.len()
                )));
            }
        };
        assert!(
            highest.1 - lowest.1 > 1e-6,
            "every feature drifted by the same amount ({:.6}), so this cannot tell a maximum \
             from a minimum",
            highest.1
        );
        assert!(
            (observation.population_stability_index - highest.1).abs() < 1e-9,
            "the reported index {:.6} is not the largest across features ({:.6} on {})",
            observation.population_stability_index,
            highest.1,
            highest.0
        );

        // And the registry now holds that number, so the eligibility check has
        // something to refuse on.
        let card = desk
            .registry()
            .get(&reference)
            .ok_or_else(|| Error::not_found("the card"))?;
        assert!(
            card.drift_score > card.drift_threshold,
            "the measurement did not reach the card: {:.3} against {:.3}",
            card.drift_score,
            card.drift_threshold
        );
        Ok(())
    }

    #[test]
    fn a_drifted_model_in_production_may_no_longer_inform_a_decision() -> Result<()> {
        // The half that matters to an operator. A development-stage card is
        // refused for its stage before drift is ever read, so only a promoted
        // model can demonstrate that drift is what disqualifies it.
        let mut desk = learning_desk();
        let calm = super::tests_support::learnable(400);
        let reference = desk
            .maybe_learn(&subject(), &calm, 1, at())?
            .and_then(|round| round.registration)
            .ok_or_else(|| Error::not_found("a registration"))?
            .reference;

        // Promote it. Refused unless the fit actually cleared the skill bar,
        // which is the point: this cannot be staged for a model with no signal.
        if desk.registry_mut().promote(&reference, at()).is_err() {
            return Err(Error::invalid(
                "the fit did not clear the skill bar, so this test cannot reach the drift \
                 branch; the fixture must produce a learnable series",
            ));
        }
        // The premise: with no drift, this model is decision-eligible. Without
        // asserting it, the refusal below could be about anything.
        desk.registry()
            .require_for_decision(&reference, at())
            .map_err(|error| {
                Error::invalid(format!(
                    "the premise failed: an undrifted, promoted model was already ineligible: {}",
                    error.message()
                ))
            })?;

        let shocked = super::tests_support::shocked(400);
        desk.maybe_learn(&subject(), &shocked, 2, at())?;

        let refusal = desk
            .registry()
            .require_for_decision(&reference, at())
            .err()
            .ok_or_else(|| Error::invalid("a drifted model was still allowed to decide"))?;
        assert!(
            refusal.message().contains("drift"),
            "the model was refused for some other reason: {}",
            refusal.message()
        );
        Ok(())
    }

    #[test]
    fn a_disabled_desk_never_learns() {
        // `every_cycles = 0` is a deployment saying it does not want this loop,
        // and it must be distinguishable from a loop that ran and found
        // nothing.
        let desk = LearningDesk::new(
            LearningConfig {
                every_cycles: 0,
                ..LearningConfig::default()
            },
            7,
        );
        assert!(!desk.enabled());
        assert_eq!(desk.tracked(), 0);
        assert_eq!(desk.stats().rounds, 0);
    }

    #[test]
    fn a_feature_row_never_reads_the_bar_its_target_spans() {
        // Point-in-time. Row `i` is computed from bars up to `LOOKBACK + i`,
        // and its target is the return from that bar to the next -- so the
        // label always sits on the far side of everything used to predict it.
        // A dataset without this property produces a backtest nobody should
        // trust and there is no way to recover it afterwards.
        let bars = super::tests_support::rising(40);
        let columns = feature_columns(&bars);
        let targets = next_bar_returns(&bars);
        // The premise: rows were produced at all.
        let returns = columns.get("return_1").expect("the return column exists");
        assert!(!returns.is_empty(), "no feature row was produced");
        assert_eq!(
            returns.len(),
            targets.len(),
            "the feature rows and targets describe different numbers of instants"
        );

        // Row 0 reads bars[LOOKBACK] and its target spans LOOKBACK -> +1.
        let expected = ratio(
            bars[LOOKBACK].close.to_f64(),
            bars[LOOKBACK - 1].close.to_f64(),
        );
        assert!(
            (returns[0] - expected).abs() < 1e-12,
            "the first row's return is not the one ending at its own bar"
        );
        let target = ratio(
            bars[LOOKBACK + 1].close.to_f64(),
            bars[LOOKBACK].close.to_f64(),
        );
        assert!(
            (targets[0] - target).abs() < 1e-12,
            "the first target is not the return into the next bar"
        );
    }
}

#[cfg(test)]
mod tests_support {
    use qip_core::rng::{Rng, Xoshiro256};
    use qip_core::{Decimal, ObjectId, Timestamp};
    use qip_financial::quality::DataQuality;
    use qip_market::bar::{Bar, Interval};

    /// A series whose next return is genuinely predictable from its last.
    ///
    /// Returns follow `r(t+1) = 0.6 * r(t) + noise`, so a linear fit on
    /// `return_1` explains a real share of out-of-sample variance and the skill
    /// bar can be cleared honestly. A constant ramp cannot: its returns have no
    /// variance, so there is nothing for a holdout R-squared to be a share of,
    /// and a model that "passed" on it would have passed on nothing.
    pub(super) fn learnable(count: usize) -> Vec<Bar> {
        let mut rng = Xoshiro256::seeded(0x1EA2);
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        let mut previous = 0.0_f64;
        for bar in bars.iter_mut() {
            let shock = rng.uniform(-0.004, 0.004);
            let ret = 0.6 * previous + shock;
            previous = ret;
            level *= 1.0 + ret;
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * (1.0 - ret / 2.0)).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.001).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.999).unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A series whose next return is independent of everything before it.
    ///
    /// The other half of the skill bar: a fit here has nothing to find, so its
    /// out-of-sample R-squared sits at or below zero and the verdict must say
    /// so. Written as fresh draws rather than a shuffle, because a shuffle of a
    /// predictable series preserves its marginal distribution and can leave
    /// enough structure to score above a low bar by luck.
    pub(super) fn unlearnable(count: usize) -> Vec<Bar> {
        let mut rng = Xoshiro256::seeded(0x0B5C_0DE5);
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        for bar in bars.iter_mut() {
            let ret = rng.uniform(-0.01, 0.01);
            level *= 1.0 + ret;
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * (1.0 - ret / 2.0)).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.002).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.998).unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A series in a regime the calm one never visits: violent alternating
    /// jumps instead of a steady drift, so every return-based feature lands in
    /// a distribution the reference sample has no mass in.
    pub(super) fn shocked(count: usize) -> Vec<Bar> {
        let mut bars = rising(count);
        let mut level = 100.0_f64;
        for (i, bar) in bars.iter_mut().enumerate() {
            level *= if i % 2 == 0 { 1.06 } else { 0.945 };
            bar.close = Decimal::from_f64(level).unwrap_or(Decimal::ONE);
            bar.open = Decimal::from_f64(level * 0.99).unwrap_or(Decimal::ONE);
            bar.high = Decimal::from_f64(level * 1.05).unwrap_or(Decimal::ONE);
            bar.low = Decimal::from_f64(level * 0.95).unwrap_or(Decimal::ONE);
            bar.volume = Decimal::from_f64(if i % 2 == 0 { 40_000.0 } else { 120.0 })
                .unwrap_or(Decimal::ONE);
        }
        bars
    }

    /// A steadily rising series with real volume, enough to build features on.
    pub(super) fn rising(count: usize) -> Vec<Bar> {
        (0..count)
            .map(|i| {
                let close = 100.0 + i as f64 * 0.5;
                Bar {
                    object_id: ObjectId::from_string("OBJ0000000000000000000AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Minute,
                    open_time: Timestamp::from_secs(1_760_000_000 + i as i64 * 60),
                    open: Decimal::from_f64(close * 0.999).unwrap_or(Decimal::ONE),
                    high: Decimal::from_f64(close * 1.002).unwrap_or(Decimal::ONE),
                    low: Decimal::from_f64(close * 0.998).unwrap_or(Decimal::ONE),
                    close: Decimal::from_f64(close).unwrap_or(Decimal::ONE),
                    volume: Decimal::from_f64(1_000.0 + i as f64).unwrap_or(Decimal::ONE),
                    trade_count: 100,
                    vwap: Decimal::from_f64(close),
                    quality: DataQuality::default(),
                }
            })
            .collect()
    }
}
