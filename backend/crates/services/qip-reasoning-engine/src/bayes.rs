//! Belief updating.
//!
//! Confidence in a hypothesis is arithmetic over its evidence, not a number an
//! analyst or a model chose. That is the whole point: the same evidence gives
//! the same confidence every time, and a change in confidence can always be
//! traced to a change in evidence.
//!
//! The update is Bayesian in log-odds. A likelihood ratio per piece of
//! evidence, summed in log space, is both the standard formulation and the one
//! that makes the independence assumption visible — which matters, because the
//! assumption is usually the thing that is wrong.

use serde::{Deserialize, Serialize};

/// Convert a probability to log-odds, with the ends clamped.
///
/// Clamping keeps a single overwhelming piece of evidence from driving the
/// posterior to exactly 0 or 1, which no finite amount of evidence should do
/// and from which no later evidence could recover.
pub fn to_log_odds(probability: f64) -> f64 {
    const EPSILON: f64 = 1e-6;
    let p = probability.clamp(EPSILON, 1.0 - EPSILON);
    (p / (1.0 - p)).ln()
}

/// Inverse of [`to_log_odds`].
pub fn from_log_odds(log_odds: f64) -> f64 {
    if log_odds > 40.0 {
        return 1.0 - 1e-6;
    }
    if log_odds < -40.0 {
        return 1e-6;
    }
    let odds = log_odds.exp();
    odds / (1.0 + odds)
}

/// One evidence item's contribution to a belief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeliefContribution {
    pub evidence_id: String,
    /// Log-likelihood ratio contributed.
    pub log_likelihood_ratio: f64,
    /// The belief after applying this item, so the path is inspectable.
    pub running_probability: f64,
}

/// The result of updating a belief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeliefUpdate {
    pub prior: f64,
    pub posterior: f64,
    pub contributions: Vec<BeliefContribution>,
    /// Discount applied for correlated evidence, in `(0, 1]`.
    pub independence_discount: f64,
    /// Distinct origins the evidence came from.
    pub distinct_origins: usize,
}

impl BeliefUpdate {
    /// How far the evidence moved the belief.
    pub fn shift(&self) -> f64 {
        self.posterior - self.prior
    }

    /// The single largest contribution by absolute log-likelihood ratio.
    ///
    /// If one item dominates, the thesis is really about that one item, and
    /// the review should be about whether that item is right.
    pub fn dominant(&self) -> Option<&BeliefContribution> {
        self.contributions.iter().max_by(|a, b| {
            a.log_likelihood_ratio
                .abs()
                .partial_cmp(&b.log_likelihood_ratio.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Fraction of the total absolute evidence weight carried by one item.
    pub fn dominance(&self) -> f64 {
        let total: f64 = self
            .contributions
            .iter()
            .map(|c| c.log_likelihood_ratio.abs())
            .sum();
        if total <= 1e-12 {
            return 0.0;
        }
        self.dominant()
            .map(|c| c.log_likelihood_ratio.abs() / total)
            .unwrap_or(0.0)
    }
}

/// One item's evidence strength, as the belief updater sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceStrength {
    pub id: String,
    /// Signed weight in `[-1, 1]`: positive supports, negative contradicts.
    pub signed_weight: f64,
    /// Origin, for the independence discount.
    pub origin: String,
}

/// Maximum log-likelihood ratio a single piece of evidence may contribute.
///
/// One filing, however good, should not take a belief from 10% to 99%. Capping
/// per item is what keeps the update honest when a weight is misjudged.
const MAX_LLR_PER_ITEM: f64 = 2.0;

/// Update a prior with a body of evidence.
///
/// Correlated evidence is discounted before it is applied: the discount is
/// `sqrt(distinct_origins / items)`, which is 1 when every item is independent
/// and falls toward `1/sqrt(n)` when they all trace to one source. That is the
/// standard correction for the effective sample size of correlated
/// observations, and it is applied here rather than left to the caller because
/// a caller who forgets produces an overconfident thesis that looks rigorous.
pub fn update(prior: f64, evidence: &[EvidenceStrength]) -> BeliefUpdate {
    if evidence.is_empty() {
        return BeliefUpdate {
            prior,
            posterior: prior,
            contributions: Vec::new(),
            independence_discount: 1.0,
            distinct_origins: 0,
        };
    }

    let mut origins: Vec<&str> = evidence.iter().map(|e| e.origin.as_str()).collect();
    origins.sort_unstable();
    origins.dedup();
    let distinct = origins.len();
    let discount = (distinct as f64 / evidence.len() as f64).sqrt();

    let mut log_odds = to_log_odds(prior);
    let mut contributions = Vec::with_capacity(evidence.len());
    for item in evidence {
        // Map a signed weight in [-1, 1] onto a bounded log-likelihood ratio.
        // `atanh` is the natural choice: it is the log-odds of a probability,
        // it is odd, and it is near-linear for small weights so ordinary
        // evidence behaves the way an analyst would expect.
        let w = item.signed_weight.clamp(-0.999, 0.999);
        let llr = (w.atanh() * discount).clamp(-MAX_LLR_PER_ITEM, MAX_LLR_PER_ITEM);
        log_odds += llr;
        contributions.push(BeliefContribution {
            evidence_id: item.id.clone(),
            log_likelihood_ratio: llr,
            running_probability: from_log_odds(log_odds),
        });
    }

    BeliefUpdate {
        prior,
        posterior: from_log_odds(log_odds),
        contributions,
        independence_discount: discount,
        distinct_origins: distinct,
    }
}

/// Attenuate an evidence-driven belief toward its prior.
///
/// Used to fold the mechanism's own plausibility into a hypothesis's
/// confidence. A weak causal story means the evidence tells you *less*, not
/// that the claim is less likely than its base rate — so the belief is pulled
/// back toward the prior rather than scaled toward zero.
///
/// Multiplying the posterior by the chain confidence, the obvious alternative,
/// is wrong in a way that matters: with a prior of 0.25 and a three-step
/// mechanism it returns 0.22, asserting the claim is less likely than the base
/// rate purely because its explanation is long. Blending in log-odds space is
/// monotone in both inputs, returns the posterior at `weight = 1`, and returns
/// the prior at `weight = 0`.
pub fn attenuate(prior: f64, posterior: f64, weight: f64) -> f64 {
    let w = weight.clamp(0.0, 1.0);
    let prior_odds = to_log_odds(prior);
    let posterior_odds = to_log_odds(posterior);
    from_log_odds(prior_odds + w * (posterior_odds - prior_odds))
}

/// A base rate for a class of hypothesis.
///
/// Priors are declared per hypothesis class and reviewed, rather than chosen
/// per thesis by whoever is writing it. A prior chosen to suit the conclusion
/// is not a prior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseRate {
    /// The class this rate applies to, e.g. `earnings-surprise-persists`.
    pub class: String,
    /// The historical frequency.
    pub rate: f64,
    /// Observations the rate is based on.
    pub observations: usize,
    /// Where the rate came from.
    pub source: String,
}

impl BaseRate {
    pub fn new(
        class: impl Into<String>,
        rate: f64,
        observations: usize,
        source: impl Into<String>,
    ) -> Self {
        Self {
            class: class.into(),
            rate: rate.clamp(0.0, 1.0),
            observations,
            source: source.into(),
        }
    }

    /// The rate shrunk toward one half by its own sample size.
    ///
    /// A base rate from twelve observations is barely a base rate; shrinking
    /// stops it anchoring a thesis more firmly than it deserves.
    pub fn shrunk(&self) -> f64 {
        const PSEUDO_COUNTS: f64 = 20.0;
        let n = self.observations as f64;
        (self.rate * n + 0.5 * PSEUDO_COUNTS) / (n + PSEUDO_COUNTS)
    }

    /// Whether the rate rests on enough history to be worth using directly.
    pub fn is_well_established(&self) -> bool {
        self.observations >= 50
    }
}
