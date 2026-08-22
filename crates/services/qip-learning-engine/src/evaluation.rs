//! Scoring theses against what happened.
//!
//! A hypothesis stated a direction, a magnitude and a horizon. Once the horizon
//! passes, all three can be checked, and [`ThesisEvaluator::evaluate`] checks
//! all three rather than only the first. A thesis that got the direction right
//! and the magnitude wrong by a factor of ten was not a good thesis, and
//! scoring on direction alone is how a platform convinces itself it is skilled.
//!
//! Two things this refuses to do:
//!
//! * **Score early.** A thesis whose horizon has not passed cannot be graded,
//!   because grading it on the part of the path that happened to suit it is
//!   the easiest way to manufacture a track record.
//! * **Grade on the outcome alone.** [`Verdict`] separates whether the thesis
//!   was right from whether the reasoning was sound. A thesis that was right
//!   for the wrong reason teaches nothing, and treating it as a success
//!   reinforces the reasoning that produced it.

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// What a thesis claimed, in the form the evaluator can check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThesisClaim {
    pub hypothesis_id: String,
    /// The hypothesis class, so lessons can be grouped.
    pub class: String,
    pub subject: String,
    /// When the claim was made.
    pub formed_at: Timestamp,
    /// When it should have resolved.
    pub resolves_at: Timestamp,
    /// Expected direction: positive or negative.
    pub direction: f64,
    /// Expected magnitude in basis points over the horizon.
    pub expected_move_bps: f64,
    /// Confidence at the time, in `[0, 1]`.
    pub confidence: f64,
    /// The falsifiers stated when the thesis was formed.
    pub falsifiers: Vec<String>,
    /// The agents that contributed.
    pub contributors: Vec<String>,
}

impl ThesisClaim {
    pub fn horizon(&self) -> Duration {
        self.resolves_at.since(self.formed_at)
    }

    pub fn is_resolvable(&self, now: Timestamp) -> bool {
        now >= self.resolves_at
    }
}

/// What actually happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub hypothesis_id: String,
    pub observed_at: Timestamp,
    /// Realised move in basis points over the same horizon.
    pub realised_move_bps: f64,
    /// P&L the thesis actually earned, from the attribution.
    pub realised_pnl: f64,
    /// Any falsifier that was observed.
    pub falsifiers_triggered: Vec<String>,
    /// Whether the mechanism's own observables appeared.
    ///
    /// The question that separates being right from being right for the stated
    /// reason.
    pub mechanism_confirmed: Option<bool>,
}

/// How a thesis is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Right direction, magnitude in the right range, mechanism confirmed.
    Vindicated,
    /// Right direction and the mechanism held, but the magnitude was well off.
    DirectionallyRight,
    /// Right outcome, wrong reason. Recorded distinctly because treating it as
    /// a success reinforces reasoning that did not work.
    RightForTheWrongReason,
    /// Wrong direction.
    Wrong,
    /// A falsifier fired. The strongest possible refutation: the thesis named
    /// this in advance as the thing that would make it wrong.
    Falsified,
    /// Nothing happened either way.
    Inconclusive,
}

impl Verdict {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vindicated => "vindicated",
            Self::DirectionallyRight => "directionally_right",
            Self::RightForTheWrongReason => "right_for_the_wrong_reason",
            Self::Wrong => "wrong",
            Self::Falsified => "falsified",
            Self::Inconclusive => "inconclusive",
        }
    }

    /// Whether the thesis should count as correct for a hit rate.
    ///
    /// `RightForTheWrongReason` deliberately does not: a hit rate that counts
    /// lucky outcomes measures luck.
    pub const fn counts_as_correct(&self) -> bool {
        matches!(self, Self::Vindicated | Self::DirectionallyRight)
    }

    /// Whether the reasoning behind it held up, regardless of the outcome.
    pub const fn reasoning_held(&self) -> bool {
        matches!(self, Self::Vindicated | Self::DirectionallyRight)
    }

    /// Whether this verdict is evidence about the process rather than noise.
    pub const fn is_informative(&self) -> bool {
        !matches!(self, Self::Inconclusive)
    }
}

/// One scored thesis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub hypothesis_id: String,
    pub class: String,
    pub verdict: Verdict,
    pub expected_move_bps: f64,
    pub realised_move_bps: f64,
    /// Realised over expected. One is exactly right.
    pub magnitude_ratio: f64,
    pub confidence: f64,
    pub realised_pnl: f64,
    pub contributors: Vec<String>,
    pub evaluated_at: Timestamp,
    /// Why this verdict, for the record.
    pub rationale: String,
}

impl Evaluation {
    /// The Brier component: squared error of the confidence against the
    /// outcome. Lower is better.
    pub fn brier_component(&self) -> f64 {
        let outcome = f64::from(u8::from(self.verdict.counts_as_correct()));
        (self.confidence - outcome).powi(2)
    }

    /// Whether the confidence was well above what the outcome justified.
    pub fn was_overconfident(&self) -> bool {
        !self.verdict.counts_as_correct() && self.confidence > 0.7
    }
}

/// Thresholds the evaluator applies.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationPolicy {
    /// Move below which nothing meaningful happened, in basis points.
    ///
    /// Without a floor, a one-basis-point move in the right direction counts
    /// as a correct call, and the hit rate becomes a coin flip dressed up.
    pub noise_floor_bps: f64,
    /// Magnitude ratio range within which a thesis is fully vindicated.
    ///
    /// Half to double: a thesis predicting 100bp that delivered 60 was a good
    /// call, and one that delivered 1000 got lucky rather than being right.
    pub vindicated_range: (f64, f64),
}

impl Default for EvaluationPolicy {
    fn default() -> Self {
        Self {
            noise_floor_bps: 25.0,
            vindicated_range: (0.5, 2.0),
        }
    }
}

/// Scores theses.
#[derive(Debug, Default)]
pub struct ThesisEvaluator {
    policy: EvaluationPolicy,
}

impl ThesisEvaluator {
    pub fn new(policy: EvaluationPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> EvaluationPolicy {
        self.policy
    }

    /// Score one thesis against its outcome.
    pub fn evaluate(
        &self,
        claim: &ThesisClaim,
        outcome: &Outcome,
        now: Timestamp,
    ) -> Result<Evaluation> {
        if claim.hypothesis_id != outcome.hypothesis_id {
            return Err(Error::invalid(format!(
                "outcome for {} cannot score claim {}",
                outcome.hypothesis_id, claim.hypothesis_id
            )));
        }
        // The refusal that stops a track record being manufactured.
        if !claim.is_resolvable(now) {
            return Err(Error::invalid(format!(
                "thesis {} resolves at {} and cannot be scored at {now}; grading it on the part of the path that happened to suit it is the easiest way to invent a track record",
                claim.hypothesis_id, claim.resolves_at
            )));
        }
        if outcome.observed_at < claim.resolves_at {
            return Err(Error::invalid(format!(
                "the outcome for {} was observed at {} before the horizon closed at {}",
                claim.hypothesis_id, outcome.observed_at, claim.resolves_at
            )));
        }

        let expected = claim.expected_move_bps.abs();
        let signed_realised = outcome.realised_move_bps * claim.direction.signum();
        let magnitude_ratio = if expected > 1e-9 {
            signed_realised / expected
        } else {
            0.0
        };

        let (verdict, rationale) = if !outcome.falsifiers_triggered.is_empty() {
            (
                Verdict::Falsified,
                format!(
                    "the thesis named {} in advance as what would make it wrong, and it happened",
                    outcome.falsifiers_triggered.join(", ")
                ),
            )
        } else if outcome.realised_move_bps.abs() < self.policy.noise_floor_bps {
            (
                Verdict::Inconclusive,
                format!(
                    "the realised move of {:.0}bp is below the {:.0}bp noise floor; nothing happened either way",
                    outcome.realised_move_bps, self.policy.noise_floor_bps
                ),
            )
        } else if signed_realised <= 0.0 {
            (
                Verdict::Wrong,
                format!(
                    "predicted {:+.0}bp and got {:+.0}bp: the direction was wrong",
                    claim.expected_move_bps, outcome.realised_move_bps
                ),
            )
        } else if outcome.mechanism_confirmed == Some(false) {
            (
                Verdict::RightForTheWrongReason,
                format!(
                    "the move of {:+.0}bp went the predicted way, but the mechanism's own observables did not appear; the outcome was right and the reasoning was not",
                    outcome.realised_move_bps
                ),
            )
        } else {
            let (low, high) = self.policy.vindicated_range;
            if magnitude_ratio >= low && magnitude_ratio <= high {
                (
                    Verdict::Vindicated,
                    format!(
                        "predicted {:.0}bp and got {:.0}bp, a ratio of {magnitude_ratio:.2} inside [{low}, {high}]",
                        expected, signed_realised
                    ),
                )
            } else {
                (
                    Verdict::DirectionallyRight,
                    format!(
                        "the direction held but the magnitude was {magnitude_ratio:.2}x the {:.0}bp predicted; a thesis right about direction and wrong about size by that much is not a good thesis",
                        expected
                    ),
                )
            }
        };

        Ok(Evaluation {
            hypothesis_id: claim.hypothesis_id.clone(),
            class: claim.class.clone(),
            verdict,
            expected_move_bps: claim.expected_move_bps,
            realised_move_bps: outcome.realised_move_bps,
            magnitude_ratio,
            confidence: claim.confidence,
            realised_pnl: outcome.realised_pnl,
            contributors: claim.contributors.clone(),
            evaluated_at: now,
            rationale,
        })
    }

    /// Score a batch, skipping the ones that cannot yet be scored.
    ///
    /// Returns the evaluations and the ids that were skipped, so an unscored
    /// thesis is visible rather than silently absent.
    pub fn evaluate_all(
        &self,
        claims: &[ThesisClaim],
        outcomes: &[Outcome],
        now: Timestamp,
    ) -> (Vec<Evaluation>, Vec<String>) {
        let mut evaluations = Vec::new();
        let mut skipped = Vec::new();
        for claim in claims {
            let Some(outcome) = outcomes
                .iter()
                .find(|o| o.hypothesis_id == claim.hypothesis_id)
            else {
                skipped.push(claim.hypothesis_id.clone());
                continue;
            };
            match self.evaluate(claim, outcome, now) {
                Ok(evaluation) => evaluations.push(evaluation),
                Err(_) => skipped.push(claim.hypothesis_id.clone()),
            }
        }
        (evaluations, skipped)
    }
}
