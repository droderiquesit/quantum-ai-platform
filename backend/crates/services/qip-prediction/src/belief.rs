//! The platform's own belief about an event, and what it makes a contract worth.
//!
//! [`crate::pricing`] reads a probability *out of* a market price. This module
//! runs the other way: a distribution over an event's outcomes (from a
//! branching development tree, or updated by a dated arrival of information)
//! and the fair value each outcome's contract has under it. A price that does
//! not cite the distribution it came from cannot be attributed afterwards, so
//! [`Valuation`] carries the distribution's digest.
//!
//! Arithmetic is `Decimal`. Rounding residue from products and normalisation
//! is placed on the most probable outcome, deterministically, so a
//! distribution always sums to exactly one rather than "close enough".

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::market::{EventMarket, OutcomeId};
use crate::pricing::Probability;

/// A probability for every outcome of one event, summing to exactly one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutcomeDistribution {
    probabilities: BTreeMap<OutcomeId, Probability>,
    as_of: Timestamp,
    /// What produced it: `"stated"`, a tree, or the arrival it was updated on.
    basis: String,
}

impl OutcomeDistribution {
    /// Refuses fewer than two outcomes and any total other than exactly one.
    /// A total of 0.98 is not a distribution that needs rescaling, it is an
    /// outcome nobody listed.
    pub fn new(
        probabilities: BTreeMap<OutcomeId, Probability>,
        as_of: Timestamp,
        basis: impl Into<String>,
    ) -> Result<Self> {
        if probabilities.len() < 2 {
            return Err(Error::invalid(
                "an outcome distribution needs at least two outcomes",
            ));
        }
        let total: Decimal = probabilities.values().map(Probability::value).sum();
        if total != Decimal::ONE {
            return Err(Error::invalid(format!(
                "outcome probabilities sum to {total}, not 1; list the missing outcome or fix the weights"
            )));
        }
        Ok(Self {
            probabilities,
            as_of,
            basis: basis.into(),
        })
    }

    /// Normalise non-negative weights, putting rounding residue on the most
    /// probable outcome (ties to the lowest id). Refuses a zero total.
    fn from_weights(
        weights: BTreeMap<OutcomeId, Decimal>,
        as_of: Timestamp,
        basis: String,
    ) -> Result<Self> {
        let total: Decimal = weights.values().copied().sum();
        if !total.is_positive() {
            return Err(Error::invalid(
                "every outcome has zero weight, so no distribution can be formed",
            ));
        }
        let mut shares = BTreeMap::new();
        for (id, weight) in weights {
            let share = weight
                .checked_div(total)
                .ok_or_else(|| Error::numeric("a weight could not be normalised"))?;
            shares.insert(id, share);
        }
        let assigned: Decimal = shares.values().copied().sum();
        let mut top: Option<(OutcomeId, Decimal)> = None;
        for (id, share) in &shares {
            if top.as_ref().is_none_or(|(_, held)| *share > *held) {
                top = Some((id.clone(), *share));
            }
        }
        let (top, _) = top.ok_or_else(|| Error::invalid("no outcomes to normalise"))?;
        if let Some(share) = shares.get_mut(&top) {
            *share += Decimal::ONE - assigned;
        }
        let mut probabilities = BTreeMap::new();
        for (id, share) in shares {
            probabilities.insert(id, Probability::new(share)?);
        }
        Self::new(probabilities, as_of, basis)
    }

    pub fn probability(&self, outcome: &OutcomeId) -> Option<Probability> {
        self.probabilities.get(outcome).copied()
    }

    pub const fn as_of(&self) -> Timestamp {
        self.as_of
    }

    pub fn basis(&self) -> &str {
        &self.basis
    }

    pub fn outcomes(&self) -> impl Iterator<Item = (&OutcomeId, &Probability)> {
        self.probabilities.iter()
    }

    /// Identity of this belief, so a valuation can cite exactly it.
    pub fn digest(&self) -> String {
        let mut canonical = format!("{}|{}", self.as_of.to_rfc3339(), self.basis);
        for (id, p) in &self.probabilities {
            canonical.push_str(&format!("|{id}={}", p.value()));
        }
        qip_core::sha256_hex(canonical.as_bytes())
    }

    /// Update on one dated arrival of information (Bayes' rule).
    ///
    /// `as_of` is when the platform forms the new belief. Refuses an arrival
    /// not yet knowable then (leakage), an `as_of` before the prior's, and a
    /// likelihood set that does not cover exactly this event's outcomes. The
    /// arrival's name becomes the new belief's basis, so the update is
    /// attributable.
    pub fn update(&self, arrival: &Arrival, as_of: Timestamp) -> Result<Self> {
        if arrival.knowable_at > as_of {
            return Err(Error::invalid(format!(
                "arrival {} became knowable at {}, after {}; a belief cannot use information it could not have had",
                arrival.name,
                arrival.knowable_at.to_rfc3339(),
                as_of.to_rfc3339()
            )));
        }
        if as_of < self.as_of {
            return Err(Error::invalid(
                "an update cannot be dated before the belief it revises",
            ));
        }
        if arrival.likelihoods.keys().ne(self.probabilities.keys()) {
            return Err(Error::invalid(format!(
                "arrival {} gives likelihoods for different outcomes than this event has",
                arrival.name
            )));
        }
        let mut weights = BTreeMap::new();
        for (id, prior) in &self.probabilities {
            let likelihood = arrival.likelihoods[id];
            if likelihood.is_negative() {
                return Err(Error::invalid(format!(
                    "likelihood {likelihood} for {id} is negative"
                )));
            }
            let weight = prior
                .value()
                .checked_mul(likelihood)
                .ok_or_else(|| Error::numeric("a posterior weight overflowed"))?;
            weights.insert(id.clone(), weight);
        }
        Self::from_weights(weights, as_of, format!("updated on {}", arrival.name))
    }
}

/// A named piece of information with how likely each outcome made it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arrival {
    pub name: String,
    pub knowable_at: Timestamp,
    /// P(this information | outcome), per outcome. Not a distribution.
    pub likelihoods: BTreeMap<OutcomeId, Decimal>,
}

/// One root-to-outcome path: the branch labels taken, the outcome reached and
/// the joint probability of the path.
pub type Path = (Vec<String>, OutcomeId, Decimal);

/// An event as branching intermediate developments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Development {
    /// The event ends in this outcome.
    Ends(OutcomeId),
    /// A development that goes one of several ways, each with the probability
    /// of that way *given* the path so far.
    Branches(Vec<Branch>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub label: String,
    pub conditional: Probability,
    pub then: Development,
}

impl Development {
    /// Every root-to-outcome path with its joint probability, in tree order.
    pub fn paths(&self) -> Result<Vec<Path>> {
        let mut out = Vec::new();
        self.walk(&mut Vec::new(), Decimal::ONE, &mut out)?;
        Ok(out)
    }

    fn walk(&self, trail: &mut Vec<String>, joint: Decimal, out: &mut Vec<Path>) -> Result<()> {
        match self {
            Self::Ends(outcome) => out.push((trail.clone(), outcome.clone(), joint)),
            Self::Branches(branches) => {
                let total: Decimal = branches.iter().map(|b| b.conditional.value()).sum();
                if branches.len() < 2 || total != Decimal::ONE {
                    return Err(Error::invalid(format!(
                        "after {trail:?} the branches need at least two ways and conditionals summing to 1, not {total}"
                    )));
                }
                for branch in branches {
                    trail.push(branch.label.clone());
                    let next = joint
                        .checked_mul(branch.conditional.value())
                        .ok_or_else(|| Error::numeric("a joint probability overflowed"))?;
                    branch.then.walk(trail, next, out)?;
                    trail.pop();
                }
            }
        }
        Ok(())
    }

    /// The outcome distribution the tree implies: each outcome's paths summed.
    /// Refuses a tree naming an outcome `market` does not have, since a belief
    /// about a different event prices nothing.
    pub fn outcome_distribution(
        &self,
        market: &EventMarket,
        as_of: Timestamp,
    ) -> Result<OutcomeDistribution> {
        let mut weights: BTreeMap<OutcomeId, Decimal> = market
            .kind
            .outcomes()
            .into_iter()
            .map(|o| (o.id.clone(), Decimal::ZERO))
            .collect();
        for (_, outcome, joint) in self.paths()? {
            let slot = weights.get_mut(&outcome).ok_or_else(|| {
                Error::invalid(format!(
                    "the tree ends in {outcome}, which market {} does not have",
                    market.market_id
                ))
            })?;
            *slot += joint;
        }
        OutcomeDistribution::from_weights(weights, as_of, "development tree".to_string())
    }
}

/// What each outcome's contract is worth under one cited belief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Valuation {
    pub market_id: ObjectId,
    /// Digest of the [`OutcomeDistribution`] used.
    pub distribution: String,
    pub as_of: Timestamp,
    pub fair_values: BTreeMap<OutcomeId, Decimal>,
}

/// Fair value of each outcome's contract: probability times the payoff.
///
/// Refuses a distribution whose outcomes are not exactly the market's, so a
/// belief about another event cannot price this one.
pub fn fair_values(market: &EventMarket, belief: &OutcomeDistribution) -> Result<Valuation> {
    let payoff = market.proposition.settlement.payoff;
    let mut values = BTreeMap::new();
    for outcome in market.kind.outcomes() {
        let p = belief.probability(&outcome.id).ok_or_else(|| {
            Error::invalid(format!(
                "the belief has no probability for {}; it describes a different event",
                outcome.id
            ))
        })?;
        let value = p
            .value()
            .checked_mul(payoff)
            .ok_or_else(|| Error::numeric("a fair value overflowed"))?;
        values.insert(outcome.id.clone(), value);
    }
    if values.len() != belief.probabilities.len() {
        return Err(Error::invalid(
            "the belief names outcomes this market does not have",
        ));
    }
    Ok(Valuation {
        market_id: market.market_id.clone(),
        distribution: belief.digest(),
        as_of: belief.as_of(),
        fair_values: values,
    })
}
