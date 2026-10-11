//! The research lane's records (§11.3–11.5): which research domain a routed
//! problem belongs to and the classical check that domain requires, what each
//! candidate contributed as an ensemble *feature*, and the boundary at which a
//! QPU result would count as advantage.
//!
//! Three statements the blueprint makes, each turned into a value a reader of
//! the journal can check rather than a sentence in a design document:
//!
//! * **§11.3** — every quantum research domain has a "required classical
//!   check". [`ResearchDomain`] is that table, and the router writes the
//!   domain's check into the compiled task's verification list, so the check
//!   a candidate was held to is on the record beside the candidate.
//! * **§11.4** — classical, quantum-inspired and QPU outputs "become ensemble
//!   features rather than direct authority". [`EnsembleFeature`] is one per
//!   candidate: its gap to the baseline and how far its selection agrees
//!   with the baseline's. Nothing reads a feature as a decision; the
//!   decision is still the margin rule in the router.
//! * **§11.5** — today "QPU jobs remain asynchronous research workloads" and
//!   "quantum never establishes truth". [`AdvantageStanding`] states, per
//!   decision, whether a QPU result would count as advantage and every
//!   condition that is not met. On a simulator it never counts, and the
//!   record says so in those words rather than leaving the reader to infer
//!   it from a flag.
//!
//! None of this changes what the cycle sizes against. It is the evidence
//! that makes a later claim about a substrate checkable.

use crate::compute::Substrate;
use serde::{Deserialize, Serialize};

/// The research domains of §11.3, each with its required classical check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchDomain {
    ForecastingFeatureSpaces,
    WorldModelSubsetSelection,
    HedgeBasketSearch,
    CapitalPlacement,
    ArbitragePathSearch,
    ModelSocietySelection,
    SimulationSampling,
}

impl ResearchDomain {
    pub const ALL: [Self; 7] = [
        Self::ForecastingFeatureSpaces,
        Self::WorldModelSubsetSelection,
        Self::HedgeBasketSearch,
        Self::CapitalPlacement,
        Self::ArbitragePathSearch,
        Self::ModelSocietySelection,
        Self::SimulationSampling,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ForecastingFeatureSpaces => "forecasting_feature_spaces",
            Self::WorldModelSubsetSelection => "world_model_subset_selection",
            Self::HedgeBasketSearch => "hedge_basket_search",
            Self::CapitalPlacement => "capital_placement",
            Self::ArbitragePathSearch => "arbitrage_path_search",
            Self::ModelSocietySelection => "model_society_selection",
            Self::SimulationSampling => "simulation_sampling",
        }
    }

    /// The classical check a candidate in this domain must pass before it is
    /// used, in the blueprint's terms.
    pub const fn required_check(&self) -> &'static str {
        match self {
            Self::ForecastingFeatureSpaces => {
                "out-of-time calibration against a strong classical model"
            }
            Self::WorldModelSubsetSelection => {
                "causal and evidence coherence, re-run for reproducibility"
            }
            Self::HedgeBasketSearch => "constraints, Greeks, liquidity and executable cost",
            Self::CapitalPlacement => {
                "risk and capital invariants and transaction costs, re-scored on the real problem"
            }
            Self::ArbitragePathSearch => "live executable edge and bounded-loss saga feasibility",
            Self::ModelSocietySelection => "forecast score uplift and compute economics",
            Self::SimulationSampling => "classical verification and bias analysis",
        }
    }

    /// Whether a router in this workspace routes problems of this domain.
    /// Only capital placement: the portfolio allocation family is the one the
    /// foundry registers. The other six are declared so that their check is
    /// fixed before anything routes them, not invented by whoever does.
    pub const fn is_routed(&self) -> bool {
        matches!(self, Self::CapitalPlacement)
    }
}

/// One candidate's contribution as an ensemble feature (§11.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnsembleFeature {
    pub substrate: Substrate,
    /// The router rung that produced it.
    pub solver: String,
    pub objective: f64,
    /// Improvement over the classical baseline, as a fraction; zero for the
    /// baseline itself.
    pub gap_to_baseline: f64,
    /// Jaccard overlap between the names this candidate holds and the names
    /// the baseline holds: one where they agree on the selection, zero where
    /// they share nothing. Disagreement with a good objective is the
    /// interesting feature; agreement says the substrate added nothing new.
    pub selection_overlap: f64,
    /// Whether the candidate was feasible on the real problem.
    pub feasible: bool,
}

/// Jaccard overlap of the non-zero positions of two weight vectors. Two empty
/// selections agree completely.
pub fn selection_overlap(left: &[f64], right: &[f64]) -> f64 {
    let held = |weights: &[f64], index: usize| weights.get(index).is_some_and(|w| w.abs() > 1e-9);
    let n = left.len().max(right.len());
    let mut both = 0usize;
    let mut either = 0usize;
    for index in 0..n {
        let (l, r) = (held(left, index), held(right, index));
        if l && r {
            both += 1;
        }
        if l || r {
            either += 1;
        }
    }
    if either == 0 {
        1.0
    } else {
        both as f64 / either as f64
    }
}

/// Whether a QPU result would count as advantage on this decision (§11.5),
/// and every condition that is not met.
///
/// `counts` is true only when every condition holds. Even then the answer
/// establishes nothing about the market: a proposal sized from it still
/// passes the deterministic pre-trade gate like any other ("quantum never
/// establishes truth and never bypasses deterministic financial controls").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AdvantageStanding {
    pub counts: bool,
    /// The unmet conditions, in the order [`AdvantageBoundary::assess`]
    /// checks them. Empty only when `counts` is true.
    pub unmet: Vec<String>,
}

/// The conditions under which a QPU answer counts as advantage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdvantageBoundary {
    /// Trials at this problem size before a win rate means anything.
    pub minimum_trials: u32,
    /// The measured win rate at or above which the QPU is ahead more often
    /// than not.
    pub minimum_win_rate: f64,
}

impl Default for AdvantageBoundary {
    fn default() -> Self {
        Self {
            minimum_trials: 30,
            minimum_win_rate: 0.5,
        }
    }
}

/// What [`AdvantageBoundary::assess`] needs to know about one decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdvantageEvidence {
    /// Whether a verified QPU answer was the one used.
    pub qpu_answer_used: bool,
    /// `Some(true)` for a simulator, `Some(false)` for hardware, `None` when
    /// no provider is attached.
    pub simulated: Option<bool>,
    pub trials: u32,
    pub wins: u32,
}

impl AdvantageBoundary {
    pub fn assess(&self, evidence: AdvantageEvidence) -> AdvantageStanding {
        let mut unmet = Vec::new();
        match evidence.simulated {
            None => unmet.push("no quantum provider is attached".to_string()),
            Some(true) => unmet.push(
                "the provider is a simulator: its answers validate a formulation and are never \
                 evidence of advantage"
                    .to_string(),
            ),
            Some(false) => {}
        }
        if !evidence.qpu_answer_used {
            unmet.push(
                "no verified QPU answer beat the classical baseline by the margin on this decision"
                    .to_string(),
            );
        }
        if evidence.trials < self.minimum_trials {
            unmet.push(format!(
                "{} trial(s) at this size, short of the {} a win rate needs to mean anything",
                evidence.trials, self.minimum_trials
            ));
        } else {
            let rate = f64::from(evidence.wins) / f64::from(evidence.trials.max(1));
            if rate < self.minimum_win_rate {
                unmet.push(format!(
                    "a measured win rate of {rate:.3} at this size, below {:.3}",
                    self.minimum_win_rate
                ));
            }
        }
        AdvantageStanding {
            counts: unmet.is_empty(),
            unmet,
        }
    }
}
