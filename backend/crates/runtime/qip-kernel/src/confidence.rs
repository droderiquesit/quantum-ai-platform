//! The confidence governor: four measured reasons to size smaller, and no
//! reason ever to size larger (R12-RISK-4111, 4112, 4114, 4115).
//!
//! Before this module the only thing that narrowed a construction budget on
//! the strength of the platform's own reliability was the self-model's
//! *staleness*. A platform whose claims had started resolving wrong, whose
//! panel was split on the thesis it was about to size, whose inputs had moved
//! outside anything its estimators had seen, or which was trading a regime it
//! had never been graded in, sized exactly as boldly as one that was right,
//! unanimous, in-distribution and at home. Each of those is now a multiplier:
//!
//! | Trigger | Measured from | Multiplier |
//! |---|---|---|
//! | Calibration decay | LEARN's Brier score over resolved claims | [`calibration_multiplier`] |
//! | Model disagreement | REASON's dissenting share of the agents that ran | [`disagreement_multiplier`] |
//! | Out-of-distribution | the population stability index over its own error floor, from `feature_statistics` | [`ood_multiplier`] |
//! | Regime mismatch | the instrument's regime now against the regimes the platform's claims were graded in | [`regime_multiplier`] |
//!
//! # Contraction only, structurally
//!
//! Every schedule below is a table of constants, and every constant is at most
//! one; `every_schedule_is_contraction_only` enumerates them. The product of
//! factors in `(0, 1]` is in `(0, 1]`, so nothing measured here can raise a
//! budget, and a signal that improves can only *remove* a contraction it
//! caused — never add headroom beyond the budget the survival kernel released.
//! The governor sizes; it does not decide orders. Enforcement stays with the
//! deterministic Risk Gate, which reads the survival ceiling this feeds
//! (`LimitKind::MaxSurvivalGross`).
//!
//! # Evidence, not absence
//!
//! Each trigger acts on a measurement that exists. A platform that has
//! resolved no claim has no Brier score and no graded regime, and this module
//! does not read that silence as decay: the self-model's staleness multiplier
//! is the control for "we do not know yet", and counting the same absence
//! twice would halve every new deployment for one fact. What this module
//! refuses is a measurement that is not a number — a Brier score outside
//! `[0, 1]`, a dissenting share above one — because a malformed figure read
//! as "fine" is the failure every limit in this workspace has been fixed for.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_cost_router::context::MarketRegime;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How many per-thesis and per-instrument signals the governor holds.
///
/// Bounded like every working set here; the oldest key leaves first. A thesis
/// that has waited this long for sizing has been drained from the pending set
/// already, and the event log keeps the record.
pub const SIGNAL_CAPACITY: usize = 256;

/// The Brier score of a forecaster who always says one half is 0.25. A
/// calibrated forecaster with any skill sits below it; one above it is doing
/// worse than declining to forecast.
///
/// `(upper bound inclusive, multiplier)`, read top to bottom; above the last
/// bound the floor applies.
pub const CALIBRATION_SCHEDULE: [(f64, Decimal); 3] = [
    (0.20, Decimal::ONE),
    (0.25, Decimal::from_raw(750_000_000)),
    (0.30, Decimal::from_raw(500_000_000)),
];
/// The multiplier for a Brier score worse than every bound above.
pub const CALIBRATION_FLOOR: Decimal = Decimal::from_raw(250_000_000);

/// Dissenting share of the agents that ran. A third is ordinary debate; past
/// half, the thesis was approved over its own panel's majority.
pub const DISAGREEMENT_SCHEDULE: [(f64, Decimal); 2] = [
    (1.0 / 3.0, Decimal::ONE),
    (0.5, Decimal::from_raw(500_000_000)),
];
/// The multiplier for a thesis most of its panel argued against.
pub const DISAGREEMENT_FLOOR: Decimal = Decimal::from_raw(250_000_000);

/// The population stability index as a multiple of the floor its own
/// estimators' error could manufacture. At or below one the shift is not
/// evidence of anything, which is `StreamingDrift::is_material`'s own rule.
pub const OOD_SCHEDULE: [(f64, Decimal); 3] = [
    (1.0, Decimal::ONE),
    (2.0, Decimal::from_raw(750_000_000)),
    (4.0, Decimal::from_raw(500_000_000)),
];
/// The multiplier past every bound above, and for a quantised feed — a series
/// that has stopped being a distribution is as far out of one as a series
/// can be.
pub const OOD_FLOOR: Decimal = Decimal::from_raw(250_000_000);

/// The multiplier for an instrument whose present regime is one the platform
/// has never been graded in.
pub const REGIME_MISMATCH: Decimal = Decimal::from_raw(500_000_000);

/// Which of the four measurements a contraction came from. Fixed set, so a
/// label or a journal field built from it is bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Calibration,
    Disagreement,
    OutOfDistribution,
    RegimeMismatch,
}

impl Trigger {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Calibration => "calibration",
            Self::Disagreement => "disagreement",
            Self::OutOfDistribution => "out_of_distribution",
            Self::RegimeMismatch => "regime_mismatch",
        }
    }
}

fn scheduled(value: f64, schedule: &[(f64, Decimal)], floor: Decimal) -> Decimal {
    schedule
        .iter()
        .find(|(bound, _)| value <= *bound)
        .map_or(floor, |(_, multiplier)| *multiplier)
}

/// The multiplier a Brier score earns. Refuses a score that is not in `[0, 1]`.
pub fn calibration_multiplier(brier: f64) -> Result<Decimal> {
    if !brier.is_finite() || !(0.0..=1.0).contains(&brier) {
        return Err(Error::invalid(format!(
            "a Brier score of {brier} is not a squared probability error; refuse the \
             calibration report that produced it rather than size against it"
        )));
    }
    Ok(scheduled(brier, &CALIBRATION_SCHEDULE, CALIBRATION_FLOOR))
}

/// The multiplier a dissenting share earns. Refuses a share outside `[0, 1]`.
pub fn disagreement_multiplier(share: f64) -> Result<Decimal> {
    if !share.is_finite() || !(0.0..=1.0).contains(&share) {
        return Err(Error::invalid(format!(
            "a dissenting share of {share} is not a share of a panel; more agents cannot \
             dissent than ran"
        )));
    }
    Ok(scheduled(share, &DISAGREEMENT_SCHEDULE, DISAGREEMENT_FLOOR))
}

/// The multiplier an out-of-distribution score earns. Refuses a negative or
/// `NaN` score.
pub fn ood_multiplier(score: f64) -> Result<Decimal> {
    // Positive infinity is admitted on purpose: it is how a quantised feed
    // and a zero error floor are carried, and it earns the floor.
    if score.is_nan() || score < 0.0 {
        return Err(Error::invalid(format!(
            "an out-of-distribution score of {score} is not a multiple of an error floor"
        )));
    }
    Ok(scheduled(score, &OOD_SCHEDULE, OOD_FLOOR))
}

/// The multiplier a regime earns against the regimes the platform's claims
/// were graded in. No graded regime at all is no evidence of mismatch — see
/// the module doc — and earns one.
pub fn regime_multiplier(current: MarketRegime, graded: &BTreeSet<MarketRegime>) -> Decimal {
    if graded.is_empty() || graded.contains(&current) {
        Decimal::ONE
    } else {
        REGIME_MISMATCH
    }
}

/// One contraction that bit, kept so the cycle report says which measurement
/// shrank the size and by how much.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contraction {
    pub trigger: Trigger,
    /// The instrument or hypothesis it was measured on, or `None` for a
    /// platform-wide figure.
    pub subject: Option<String>,
    pub multiplier: Decimal,
    pub reason: String,
}

/// What the governor decided for one construction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceDecision {
    /// In `(0, 1]`. The product of every contraction below.
    pub multiplier: Decimal,
    /// Every contraction that bit, in trigger then subject order. Empty when
    /// the multiplier is one.
    pub contractions: Vec<Contraction>,
}

/// One thesis as the governor needs to see it: what REASON approved and the
/// instrument it will be sized on.
#[derive(Clone, Debug, PartialEq)]
pub struct ThesisView {
    pub hypothesis_id: String,
    pub object_id: String,
    pub regime: MarketRegime,
}

/// The measurements the governor holds between the stage that takes each and
/// DECIDE, which reads them all.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceSignals {
    /// The latest informative calibration: Brier score and how many resolved
    /// claims it rests on.
    calibration: Option<(f64, usize)>,
    /// Dissenting share per approved hypothesis.
    disagreement: BTreeMap<String, f64>,
    /// Out-of-distribution score per instrument, from the latest feature
    /// statistics. Replaced whole each time: an instrument that has settled
    /// back into its distribution must stop being contracted.
    ood: BTreeMap<String, f64>,
    /// Regimes in which at least one claim has been graded informatively.
    graded_regimes: BTreeSet<MarketRegime>,
}

fn bounded_insert<V>(map: &mut BTreeMap<String, V>, key: String, value: V) {
    if !map.contains_key(&key) && map.len() >= SIGNAL_CAPACITY {
        let oldest = map.keys().next().cloned();
        if let Some(oldest) = oldest {
            map.remove(&oldest);
        }
    }
    map.insert(key, value);
}

impl ConfidenceSignals {
    /// LEARN produced an informative calibration, graded in `regimes`.
    pub fn observe_calibration(
        &mut self,
        brier: f64,
        evaluated: usize,
        regimes: impl IntoIterator<Item = MarketRegime>,
    ) {
        self.calibration = Some((brier, evaluated));
        self.graded_regimes.extend(regimes);
    }

    /// REASON approved `hypothesis_id` with `dissenters` of `ran` agents
    /// arguing against it. A panel of nobody is recorded as full dissent: a
    /// thesis no agent examined has no consensus to size on.
    pub fn observe_dissent(&mut self, hypothesis_id: &str, dissenters: usize, ran: usize) {
        let share = if ran == 0 {
            1.0
        } else {
            // Statistics: a share of a panel, `f64` by nature.
            dissenters as f64 / ran as f64
        };
        bounded_insert(&mut self.disagreement, hypothesis_id.to_string(), share);
    }

    /// The latest feature statistics: each shifted instrument's index over its
    /// floor, and every quantised feed at infinity.
    pub fn observe_distribution(
        &mut self,
        review: &crate::feature_statistics::FeatureStatisticsReview,
    ) {
        let mut ood = BTreeMap::new();
        for shift in &review.shifted {
            let drift = &shift.drift;
            let score = if drift.floor > 0.0 {
                drift.population_stability_index / drift.floor
            } else {
                // A floor of zero means any index at all is beyond the
                // estimators' error; `is_material` put it on the list.
                f64::INFINITY
            };
            bounded_insert(&mut ood, shift.object_id.clone(), score);
        }
        for object in &review.quantised {
            bounded_insert(&mut ood, object.clone(), f64::INFINITY);
        }
        self.ood = ood;
    }

    /// Forget the dissent recorded for theses that have now been sized, so the
    /// map holds only what is still pending.
    pub fn forget_theses<'a>(&mut self, hypothesis_ids: impl IntoIterator<Item = &'a str>) {
        for id in hypothesis_ids {
            self.disagreement.remove(id);
        }
    }

    pub fn graded_regimes(&self) -> &BTreeSet<MarketRegime> {
        &self.graded_regimes
    }

    /// Decide the contraction for a construction over `theses`.
    ///
    /// Calibration is platform-wide. The three per-thesis triggers take the
    /// **weakest** thesis in the construction, as mark confidence already does
    /// beside it: one budget is shared by every name in it, and a budget sized
    /// for the best-supported thesis would let the worst one ride on it.
    pub fn decide(&self, theses: &[ThesisView]) -> Result<ConfidenceDecision> {
        let mut contractions = Vec::new();
        if let Some((brier, evaluated)) = self.calibration {
            let multiplier = calibration_multiplier(brier)?;
            if multiplier < Decimal::ONE {
                contractions.push(Contraction {
                    trigger: Trigger::Calibration,
                    subject: None,
                    multiplier,
                    reason: format!(
                        "calibration has decayed to a Brier score of {brier:.3} over {evaluated} \
                         resolved claim(s); sized at {multiplier}"
                    ),
                });
            }
        }

        let mut weakest: BTreeMap<Trigger, Contraction> = BTreeMap::new();
        let mut keep_weakest = |candidate: Contraction| {
            let replace = weakest
                .get(&candidate.trigger)
                .is_none_or(|held| candidate.multiplier < held.multiplier);
            if replace {
                weakest.insert(candidate.trigger, candidate);
            }
        };
        for thesis in theses {
            if let Some(share) = self.disagreement.get(&thesis.hypothesis_id) {
                let multiplier = disagreement_multiplier(*share)?;
                if multiplier < Decimal::ONE {
                    keep_weakest(Contraction {
                        trigger: Trigger::Disagreement,
                        subject: Some(thesis.hypothesis_id.clone()),
                        multiplier,
                        reason: format!(
                            "{} was approved with {:.0}% of its panel dissenting; sized at \
                             {multiplier}",
                            thesis.hypothesis_id,
                            share * 100.0
                        ),
                    });
                }
            }
            if let Some(score) = self.ood.get(&thesis.object_id) {
                let multiplier = ood_multiplier(*score)?;
                if multiplier < Decimal::ONE {
                    keep_weakest(Contraction {
                        trigger: Trigger::OutOfDistribution,
                        subject: Some(thesis.object_id.clone()),
                        multiplier,
                        reason: format!(
                            "{}'s recent returns sit {score:.2}x past the shift its own \
                             estimators' error explains; sized at {multiplier}",
                            thesis.object_id
                        ),
                    });
                }
            }
            let multiplier = regime_multiplier(thesis.regime, &self.graded_regimes);
            if multiplier < Decimal::ONE {
                keep_weakest(Contraction {
                    trigger: Trigger::RegimeMismatch,
                    subject: Some(thesis.object_id.clone()),
                    multiplier,
                    reason: format!(
                        "{} is in a {:?} regime and no claim has been graded in one; sized at \
                         {multiplier}",
                        thesis.object_id, thesis.regime
                    ),
                });
            }
        }
        contractions.extend(weakest.into_values());

        let mut multiplier = Decimal::ONE;
        for contraction in &contractions {
            multiplier = multiplier
                .checked_mul(contraction.multiplier)
                .ok_or_else(|| {
                    Error::numeric("compounding the confidence contractions overflows".to_string())
                })?;
        }
        // Structural by the schedules, checked anyway: this is the line that
        // makes "never grows a size" a refusal rather than a comment.
        if multiplier > Decimal::ONE || !multiplier.is_positive() {
            return Err(Error::invalid(format!(
                "the confidence governor produced a multiplier of {multiplier}, outside (0, 1]; \
                 a governor that could raise a size is not a governor"
            )));
        }
        Ok(ConfidenceDecision {
            multiplier,
            contractions,
        })
    }
}

/// The `horizon` label on the per-horizon calibration series (R12-OBS-3118,
/// W0.8).
///
/// The label was `format!("{}s", horizon.as_secs_f64() as i64)` — a runtime
/// duration, so every distinct horizon a thesis was ever formed with minted a
/// new series, and the cardinality of `qip_belief_brier_score_by_horizon` was
/// bounded by nothing but the reasoning engine's imagination. Every label in
/// this platform is bounded by an enum or a source-file literal; this is the
/// enum for that one. Five buckets, each inclusive of its upper bound, so a
/// one-day horizon reads as `le_1d` and not as the next bucket up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HorizonBucket {
    Le1h,
    Le1d,
    Le7d,
    Le30d,
    Gt30d,
}

impl HorizonBucket {
    /// Every bucket, in order — the whole label set.
    pub const ALL: [Self; 5] = [Self::Le1h, Self::Le1d, Self::Le7d, Self::Le30d, Self::Gt30d];

    /// The bucket a horizon falls in. A zero or negative horizon is the
    /// shortest bucket, not a refusal: the label describes the claim, and the
    /// claim's own validation is not this function's to repeat.
    pub fn of(horizon: qip_core::time::Duration) -> Self {
        const HOUR: f64 = 3_600.0;
        let seconds = horizon.as_secs_f64();
        if seconds <= HOUR {
            Self::Le1h
        } else if seconds <= 24.0 * HOUR {
            Self::Le1d
        } else if seconds <= 7.0 * 24.0 * HOUR {
            Self::Le7d
        } else if seconds <= 30.0 * 24.0 * HOUR {
            Self::Le30d
        } else {
            Self::Gt30d
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Le1h => "le_1h",
            Self::Le1d => "le_1d",
            Self::Le7d => "le_7d",
            Self::Le30d => "le_30d",
            Self::Gt30d => "gt_30d",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_horizon_lands_in_one_of_five_fixed_buckets() {
        use qip_core::time::Duration;
        let cases = [
            (Duration::from_mins(1), HorizonBucket::Le1h),
            (Duration::from_mins(60), HorizonBucket::Le1h),
            (Duration::from_mins(61), HorizonBucket::Le1d),
            (Duration::from_days(1), HorizonBucket::Le1d),
            (Duration::from_days(3), HorizonBucket::Le7d),
            (Duration::from_days(30), HorizonBucket::Le30d),
            (Duration::from_days(400), HorizonBucket::Gt30d),
        ];
        for (horizon, bucket) in cases {
            assert_eq!(HorizonBucket::of(horizon), bucket, "{horizon:?}");
            assert!(HorizonBucket::ALL.contains(&bucket));
        }
        // A thousand distinct horizons produce at most five labels: the
        // property the runtime-duration label did not have.
        let labels: BTreeSet<&str> = (1..=1_000)
            .map(|hours| HorizonBucket::of(Duration::from_mins(hours * 61)).as_str())
            .collect();
        assert!(labels.len() <= HorizonBucket::ALL.len(), "{labels:?}");
        assert!(labels.len() > 1, "premise: the sweep crossed buckets");
    }

    fn thesis(id: &str, object: &str, regime: MarketRegime) -> ThesisView {
        ThesisView {
            hypothesis_id: id.to_string(),
            object_id: object.to_string(),
            regime,
        }
    }

    #[test]
    fn every_schedule_is_contraction_only() {
        let all = CALIBRATION_SCHEDULE
            .iter()
            .chain(DISAGREEMENT_SCHEDULE.iter())
            .chain(OOD_SCHEDULE.iter())
            .map(|(_, multiplier)| *multiplier)
            .chain([
                CALIBRATION_FLOOR,
                DISAGREEMENT_FLOOR,
                OOD_FLOOR,
                REGIME_MISMATCH,
            ]);
        let mut seen = 0;
        for multiplier in all {
            seen += 1;
            assert!(multiplier <= Decimal::ONE, "{multiplier} would grow a size");
            assert!(
                multiplier.is_positive(),
                "{multiplier} would zero a size silently"
            );
        }
        assert_eq!(seen, 12, "premise: every schedule entry was enumerated");
    }

    #[test]
    fn an_unmeasured_platform_is_not_contracted() {
        let signals = ConfidenceSignals::default();
        let decision = signals
            .decide(&[thesis("h1", "obj-AAA", MarketRegime::Trending)])
            .unwrap();
        assert_eq!(decision.multiplier, Decimal::ONE);
        assert!(decision.contractions.is_empty());
    }

    #[test]
    fn decayed_calibration_shrinks_the_multiplier_and_recovery_only_restores_it() {
        let views = [thesis("h1", "obj-AAA", MarketRegime::Trending)];
        let mut signals = ConfidenceSignals::default();
        signals.observe_calibration(0.10, 40, [MarketRegime::Trending]);
        assert_eq!(
            signals.decide(&views).unwrap().multiplier,
            Decimal::ONE,
            "premise: a well-calibrated platform is not contracted"
        );
        signals.observe_calibration(0.32, 40, [MarketRegime::Trending]);
        let decayed = signals.decide(&views).unwrap();
        assert_eq!(decayed.multiplier, CALIBRATION_FLOOR);
        assert_eq!(decayed.contractions[0].trigger, Trigger::Calibration);
        // Recovery removes the contraction and nothing more.
        signals.observe_calibration(0.0, 40, [MarketRegime::Trending]);
        assert_eq!(signals.decide(&views).unwrap().multiplier, Decimal::ONE);
    }

    #[test]
    fn a_split_panel_shrinks_the_size_of_the_thesis_it_split_on() {
        let mut signals = ConfidenceSignals::default();
        signals.observe_dissent("h1", 0, 3);
        signals.observe_dissent("h2", 2, 3);
        let unanimous = signals
            .decide(&[thesis("h1", "obj-AAA", MarketRegime::Trending)])
            .unwrap();
        assert_eq!(unanimous.multiplier, Decimal::ONE, "premise");
        let split = signals
            .decide(&[thesis("h2", "obj-BBB", MarketRegime::Trending)])
            .unwrap();
        assert_eq!(split.multiplier, DISAGREEMENT_FLOOR);
        // A construction holding both takes the weakest, once.
        let both = signals
            .decide(&[
                thesis("h1", "obj-AAA", MarketRegime::Trending),
                thesis("h2", "obj-BBB", MarketRegime::Trending),
            ])
            .unwrap();
        assert_eq!(both.multiplier, DISAGREEMENT_FLOOR);
        assert_eq!(both.contractions.len(), 1);
    }

    #[test]
    fn an_out_of_distribution_instrument_shrinks_the_size() {
        let mut signals = ConfidenceSignals::default();
        signals.ood.insert("obj-AAA".to_string(), 3.0);
        signals.ood.insert("obj-QQQ".to_string(), f64::INFINITY);
        let views = |object: &str| [thesis("h", object, MarketRegime::Trending)];
        assert_eq!(
            signals.decide(&views("obj-BBB")).unwrap().multiplier,
            Decimal::ONE,
            "premise: an instrument with no shift is not contracted"
        );
        assert_eq!(
            signals.decide(&views("obj-AAA")).unwrap().multiplier,
            Decimal::from_raw(500_000_000)
        );
        assert_eq!(
            signals.decide(&views("obj-QQQ")).unwrap().multiplier,
            OOD_FLOOR,
            "a quantised feed is as far out of distribution as a series can be"
        );
    }

    #[test]
    fn a_regime_never_graded_in_shrinks_the_size() {
        let mut signals = ConfidenceSignals::default();
        signals.observe_calibration(0.10, 30, [MarketRegime::Trending, MarketRegime::Quiet]);
        let home = signals
            .decide(&[thesis("h", "obj-AAA", MarketRegime::Quiet)])
            .unwrap();
        assert_eq!(home.multiplier, Decimal::ONE, "premise: a graded regime");
        let away = signals
            .decide(&[thesis("h", "obj-AAA", MarketRegime::Crisis)])
            .unwrap();
        assert_eq!(away.multiplier, REGIME_MISMATCH);
        assert_eq!(away.contractions[0].trigger, Trigger::RegimeMismatch);
    }

    #[test]
    fn every_trigger_at_once_compounds_and_still_never_exceeds_one() {
        let mut signals = ConfidenceSignals::default();
        signals.observe_calibration(0.5, 10, [MarketRegime::Quiet]);
        signals.observe_dissent("h", 3, 3);
        signals.ood.insert("obj-AAA".to_string(), 10.0);
        let decision = signals
            .decide(&[thesis("h", "obj-AAA", MarketRegime::Crisis)])
            .unwrap();
        assert_eq!(decision.contractions.len(), 4);
        let expected = CALIBRATION_FLOOR * DISAGREEMENT_FLOOR * OOD_FLOOR * REGIME_MISMATCH;
        assert_eq!(decision.multiplier, expected);
        assert!(decision.multiplier < Decimal::ONE);
    }

    #[test]
    fn a_malformed_measurement_is_refused_rather_than_read_as_healthy() {
        assert!(calibration_multiplier(f64::NAN).is_err());
        assert!(calibration_multiplier(1.5).is_err());
        assert!(disagreement_multiplier(1.2).is_err());
        assert!(ood_multiplier(-1.0).is_err());
        assert!(ood_multiplier(f64::NAN).is_err());
        assert_eq!(ood_multiplier(f64::INFINITY).unwrap(), OOD_FLOOR);
    }
}
