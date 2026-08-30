//! Turning evaluations into changes.
//!
//! Scoring theses is only useful if something happens as a result. This module
//! produces the two things that should: a calibration adjustment when the
//! platform's stated confidences do not match its outcomes, and candidate
//! lessons when a pattern holds across enough independent episodes.
//!
//! The bar for a lesson is deliberately high. A rule learned from three
//! episodes is a coincidence with a narrative attached, and a platform that
//! promotes those will confidently apply them for years. [`LessonCandidate`]
//! carries its counter-examples so a rule that mostly holds is visibly a rule
//! that mostly holds.

use crate::evaluation::{Evaluation, Verdict};
use qip_ai::evaluation::{Calibration, PredictionOutcome};
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How the platform's confidence compares to its outcomes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub evaluated: usize,
    pub brier_score: f64,
    /// Factor to scale future confidences by. One means well calibrated.
    pub confidence_adjustment: f64,
    pub is_overconfident: bool,
    /// Hit rate by hypothesis class, where the class has enough history.
    pub hit_rate_by_class: BTreeMap<String, f64>,
    /// Hit rate by contributing agent.
    pub hit_rate_by_agent: BTreeMap<String, f64>,
    /// Classes and agents withheld for having too little history.
    pub withheld: Vec<String>,
}

impl CalibrationReport {
    /// Whether the adjustment is large enough to be worth applying.
    ///
    /// A 3% adjustment from forty observations is noise, and applying it would
    /// make the platform chase its own sampling error.
    pub fn is_material(&self) -> bool {
        (self.confidence_adjustment - 1.0).abs() > 0.10 && self.evaluated >= 50
    }

    pub fn summarise(&self) -> String {
        format!(
            "{} evaluation(s), Brier {:.3}, {} (adjustment {:.2}{})",
            self.evaluated,
            self.brier_score,
            if self.is_overconfident {
                "overconfident"
            } else {
                "reasonably calibrated"
            },
            self.confidence_adjustment,
            if self.is_material() {
                ", material"
            } else {
                ", not material"
            }
        )
    }
}

/// A pattern that might be worth remembering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LessonCandidate {
    /// When the pattern applies.
    pub applies_when: String,
    /// The pattern itself.
    pub statement: String,
    /// Evaluations supporting it.
    pub supporting: Vec<String>,
    /// Evaluations contradicting it. Kept: a rule that mostly holds should
    /// visibly be a rule that mostly holds.
    pub contradicting: Vec<String>,
    /// Distinct agents whose theses contributed, as a check on independence.
    ///
    /// Twelve episodes from one agent are twelve observations of that agent,
    /// not twelve observations of the world.
    pub distinct_contributors: usize,
    pub proposed_at: Timestamp,
}

impl LessonCandidate {
    pub fn support(&self) -> f64 {
        let for_it = self.supporting.len() as f64;
        let total = for_it + self.contradicting.len() as f64;
        if total <= 0.0 { 0.0 } else { for_it / total }
    }

    /// Laplace-smoothed support, which is what should actually be trusted.
    pub fn smoothed_support(&self) -> f64 {
        let for_it = self.supporting.len() as f64;
        let against = self.contradicting.len() as f64;
        (for_it + 1.0) / (for_it + against + 2.0)
    }

    pub fn episode_count(&self) -> usize {
        self.supporting.len() + self.contradicting.len()
    }
}

/// The bar a candidate must clear.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionBar {
    /// Supporting episodes required.
    pub minimum_support: usize,
    /// Smoothed support required.
    pub minimum_smoothed_support: f64,
    /// Distinct contributing agents required.
    ///
    /// More than one: a pattern seen only through one agent's theses is a fact
    /// about that agent.
    pub minimum_distinct_contributors: usize,
}

impl Default for PromotionBar {
    fn default() -> Self {
        Self {
            minimum_support: 8,
            minimum_smoothed_support: 0.70,
            minimum_distinct_contributors: 2,
        }
    }
}

impl PromotionBar {
    /// Whether a candidate clears the bar, and why not if it does not.
    pub fn assess(&self, candidate: &LessonCandidate) -> std::result::Result<(), String> {
        if candidate.supporting.len() < self.minimum_support {
            return Err(format!(
                "{} supporting episode(s), {} required; a rule learned from fewer is a coincidence with a narrative attached",
                candidate.supporting.len(),
                self.minimum_support
            ));
        }
        if candidate.smoothed_support() < self.minimum_smoothed_support {
            return Err(format!(
                "smoothed support {:.3} below the {:.3} required",
                candidate.smoothed_support(),
                self.minimum_smoothed_support
            ));
        }
        if candidate.distinct_contributors < self.minimum_distinct_contributors {
            return Err(format!(
                "{} distinct contributor(s), {} required; a pattern seen only through one agent's theses is a fact about that agent",
                candidate.distinct_contributors, self.minimum_distinct_contributors
            ));
        }
        Ok(())
    }
}

/// What the learning stage produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedbackReport {
    pub calibration: CalibrationReport,
    /// Candidates that cleared the bar.
    pub promoted: Vec<LessonCandidate>,
    /// Candidates that did not, and why.
    pub rejected: Vec<(LessonCandidate, String)>,
    /// Agents whose recent record argues for a reduced weight, and the factor.
    pub agent_weight_adjustments: BTreeMap<String, f64>,
}

impl FeedbackReport {
    /// Whether anything here should change how the platform behaves.
    pub fn has_actions(&self) -> bool {
        self.calibration.is_material()
            || !self.promoted.is_empty()
            || !self.agent_weight_adjustments.is_empty()
    }
}

/// Minimum resolved episodes before a per-agent or per-class rate is reported.
///
/// Ten is not statistical significance; it is the point below which reporting
/// a rate at all would mislead.
const MINIMUM_FOR_A_RATE: usize = 10;

/// Turns evaluations into calibration and lessons.
#[derive(Debug, Default)]
pub struct FeedbackEngine {
    bar: PromotionBar,
}

impl FeedbackEngine {
    pub fn new(bar: PromotionBar) -> Self {
        Self { bar }
    }

    pub fn bar(&self) -> PromotionBar {
        self.bar
    }

    /// Assess how well the platform's confidences matched its outcomes.
    pub fn calibrate(&self, evaluations: &[Evaluation]) -> Result<CalibrationReport> {
        let informative: Vec<&Evaluation> = evaluations
            .iter()
            .filter(|e| e.verdict.is_informative())
            .collect();
        if informative.is_empty() {
            return Err(Error::invalid(
                "no informative evaluation to calibrate against",
            ));
        }

        let outcomes: Vec<PredictionOutcome> = informative
            .iter()
            .map(|e| {
                PredictionOutcome::new(e.confidence, e.verdict.counts_as_correct())
                    .in_cohort(e.class.clone())
            })
            .collect();
        let calibration = Calibration::assess(&outcomes, 5);

        let mut withheld = Vec::new();
        let hit_rate_by_class = self.rates(
            informative.iter().map(|e| (e.class.clone(), e.verdict)),
            &mut withheld,
            "class",
        );
        let hit_rate_by_agent = self.rates(
            informative.iter().flat_map(|e| {
                e.contributors
                    .iter()
                    .map(move |agent| (agent.clone(), e.verdict))
            }),
            &mut withheld,
            "agent",
        );

        Ok(CalibrationReport {
            evaluated: informative.len(),
            brier_score: calibration.brier_score,
            confidence_adjustment: calibration.confidence_adjustment(),
            is_overconfident: calibration.is_overconfident(0.05),
            hit_rate_by_class,
            hit_rate_by_agent,
            withheld,
        })
    }

    /// Build candidate lessons from a set of evaluations.
    ///
    /// The patterns looked for are deliberately narrow and stated, rather than
    /// discovered: a search over arbitrary groupings of a few hundred
    /// evaluations will always find something, and what it finds will be the
    /// grouping, not a lesson.
    pub fn propose_lessons(
        &self,
        evaluations: &[Evaluation],
        at: Timestamp,
    ) -> Vec<LessonCandidate> {
        let mut candidates = Vec::new();

        // Pattern one: a hypothesis class that systematically over- or
        // under-shoots its own magnitude estimate.
        let mut by_class: BTreeMap<&str, Vec<&Evaluation>> = BTreeMap::new();
        for evaluation in evaluations.iter().filter(|e| e.verdict.is_informative()) {
            by_class
                .entry(evaluation.class.as_str())
                .or_default()
                .push(evaluation);
        }
        for (class, group) in by_class {
            let overshooting: Vec<&&Evaluation> = group
                .iter()
                .filter(|e| e.magnitude_ratio < 0.5 && e.magnitude_ratio > 0.0)
                .collect();
            if overshooting.len() * 2 > group.len() && overshooting.len() >= 3 {
                candidates.push(LessonCandidate {
                    applies_when: format!("class={class}"),
                    statement: format!(
                        "theses of class {class} overstate their expected move: {} of {} delivered less than half what was predicted",
                        overshooting.len(),
                        group.len()
                    ),
                    supporting: overshooting
                        .iter()
                        .map(|e| e.hypothesis_id.clone())
                        .collect(),
                    contradicting: group
                        .iter()
                        .filter(|e| e.magnitude_ratio >= 0.5)
                        .map(|e| e.hypothesis_id.clone())
                        .collect(),
                    distinct_contributors: distinct_contributors(&group),
                    proposed_at: at,
                });
            }
        }

        // Pattern two: theses that were right for the wrong reason. If this is
        // common the platform is being rewarded for reasoning that does not
        // work, which is worse than being wrong.
        let lucky: Vec<&Evaluation> = evaluations
            .iter()
            .filter(|e| e.verdict == Verdict::RightForTheWrongReason)
            .collect();
        if lucky.len() >= 3 {
            let informative_count = evaluations
                .iter()
                .filter(|e| e.verdict.is_informative())
                .count()
                .max(1);
            candidates.push(LessonCandidate {
                applies_when: String::new(),
                statement: format!(
                    "{} of {informative_count} theses were right for the wrong reason: the mechanisms are not doing the work the outcomes are credited to",
                    lucky.len()
                ),
                supporting: lucky.iter().map(|e| e.hypothesis_id.clone()).collect(),
                contradicting: evaluations
                    .iter()
                    .filter(|e| e.verdict == Verdict::Vindicated)
                    .map(|e| e.hypothesis_id.clone())
                    .collect(),
                distinct_contributors: distinct_contributors(&lucky),
                proposed_at: at,
            });
        }

        // Pattern three: falsifiers that actually fire. A thesis class whose
        // stated falsifiers keep triggering is one whose authors know what
        // would refute them and keep publishing anyway.
        let falsified: Vec<&Evaluation> = evaluations
            .iter()
            .filter(|e| e.verdict == Verdict::Falsified)
            .collect();
        if falsified.len() >= 3 {
            candidates.push(LessonCandidate {
                applies_when: String::new(),
                statement: format!(
                    "{} theses were refuted by falsifiers they named in advance; the falsifier should be checked before the thesis is published, not after",
                    falsified.len()
                ),
                supporting: falsified.iter().map(|e| e.hypothesis_id.clone()).collect(),
                contradicting: Vec::new(),
                distinct_contributors: distinct_contributors(&falsified),
                proposed_at: at,
            });
        }

        candidates
    }

    /// The full feedback pass.
    pub fn process(&self, evaluations: &[Evaluation], at: Timestamp) -> Result<FeedbackReport> {
        let calibration = self.calibrate(evaluations)?;

        let mut promoted = Vec::new();
        let mut rejected = Vec::new();
        for candidate in self.propose_lessons(evaluations, at) {
            match self.bar.assess(&candidate) {
                Ok(()) => promoted.push(candidate),
                Err(reason) => rejected.push((candidate, reason)),
            }
        }

        // An agent whose hit rate is well below even money, over enough
        // episodes to mean something, has its findings discounted. Never to
        // zero: an agent that is reliably wrong still carries information, and
        // silencing it discards that.
        let mut agent_weight_adjustments = BTreeMap::new();
        for (agent, rate) in &calibration.hit_rate_by_agent {
            if *rate < 0.4 {
                agent_weight_adjustments.insert(agent.clone(), (rate / 0.5).clamp(0.3, 1.0));
            }
        }

        Ok(FeedbackReport {
            calibration,
            promoted,
            rejected,
            agent_weight_adjustments,
        })
    }

    /// Hit rates per key, withholding any key with too little history.
    fn rates<I: Iterator<Item = (String, Verdict)>>(
        &self,
        items: I,
        withheld: &mut Vec<String>,
        label: &str,
    ) -> BTreeMap<String, f64> {
        let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for (key, verdict) in items {
            let entry = counts.entry(key).or_insert((0, 0));
            entry.1 += 1;
            if verdict.counts_as_correct() {
                entry.0 += 1;
            }
        }
        let mut rates = BTreeMap::new();
        for (key, (correct, total)) in counts {
            if total < MINIMUM_FOR_A_RATE {
                withheld.push(format!("{label}={key} ({total} episode(s))"));
                continue;
            }
            rates.insert(key, correct as f64 / total as f64);
        }
        rates
    }
}

/// Distinct agents across a group of evaluations.
fn distinct_contributors<T: std::ops::Deref<Target = Evaluation>>(group: &[T]) -> usize {
    let mut agents: Vec<&str> = group
        .iter()
        .flat_map(|e| e.contributors.iter().map(String::as_str))
        .collect();
    agents.sort_unstable();
    agents.dedup();
    agents.len()
}
