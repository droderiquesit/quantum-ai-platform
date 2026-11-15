//! The placement optimiser (R12-CAPITAL-4090, ADR 0686): which source funds
//! which requirement across currencies, venues, counterparties and legal
//! entities, cheapest carry first, with the ledger's balance as the ceiling
//! on every source.
//!
//! Optimisation that overrides the ledger invents capital, so the ledger is
//! an input here and never an output. A source offered above what the ledger
//! holds for it is refused outright rather than clamped to the ledger's
//! figure: a clamp would hide that two claims about one balance disagree.
//! Every plan is reconciled against the ledger before it is returned, and
//! [`reconcile`] is public so a caller that holds a plan from elsewhere can
//! put the same question to it.
//!
//! What this does not do, on purpose. It converts no currency: a conversion
//! needs a rate nobody has supplied, so a requirement with no same-currency
//! source is left unplaced and says so. It never splits across entities: a
//! draw from one legal entity's balance funds only that entity's
//! requirement. It moves nothing; a plan is a proposal and the coordinator
//! (`crate::coordinator`) still settles it against the grant.
//!
//! The method is greedy in the requirements' given order, each requirement
//! drawing its cheapest eligible sources first and funded whole or not at
//! all. That is not a global optimum when requirements compete for one cheap
//! source, and the caller's order is the priority between them; ADR 0686
//! records the limit.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One place capital sits and may be drawn from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSource {
    /// Stable identity, the key the ledger names it under.
    pub id: String,
    /// The currency it is held in.
    pub currency: String,
    /// The legal entity that holds it.
    pub entity: String,
    /// The counterparty it sits with.
    pub counterparty: String,
    /// The one venue it may fund, or `None` where it may fund any.
    pub venue: Option<String>,
    /// What the caller says is available.
    pub available: Decimal,
    /// Carry cost of drawing it, in basis points a year. Lower is drawn first.
    pub carry_bps: Decimal,
}

/// Capital one entity needs in one currency at one venue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementRequirement {
    pub id: String,
    pub currency: String,
    pub entity: String,
    pub venue: String,
    pub amount: Decimal,
}

/// An amount drawn from one source for one requirement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementDraw {
    pub requirement: String,
    pub source: String,
    pub amount: Decimal,
}

/// A requirement no eligible source could meet in full, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unplaced {
    pub requirement: String,
    pub reason: String,
}

/// Proposed placements and the requirements left unplaced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementPlan {
    pub draws: Vec<PlacementDraw>,
    pub unplaced: Vec<Unplaced>,
}

impl PlacementPlan {
    /// Total drawn from `source` across every requirement.
    pub fn drawn_from(&self, source: &str) -> Decimal {
        self.draws
            .iter()
            .filter(|draw| draw.source == source)
            .fold(Decimal::ZERO, |sum, draw| sum + draw.amount)
    }

    /// Total drawn for `requirement`.
    pub fn placed_for(&self, requirement: &str) -> Decimal {
        self.draws
            .iter()
            .filter(|draw| draw.requirement == requirement)
            .fold(Decimal::ZERO, |sum, draw| sum + draw.amount)
    }
}

/// Propose placements for `requirements` from `sources`, bounded by
/// `ledger`, the authoritative balance per source id.
pub fn optimise(
    requirements: &[PlacementRequirement],
    sources: &[PlacementSource],
    ledger: &BTreeMap<String, Decimal>,
) -> Result<PlacementPlan> {
    let mut seen = BTreeSet::new();
    for source in sources {
        if !seen.insert(source.id.as_str()) {
            return Err(Error::invalid(format!(
                "source {} is offered twice; two lines for one balance would let it be drawn twice",
                source.id
            )));
        }
        if source.available.is_negative() || source.carry_bps.is_negative() {
            return Err(Error::invalid(format!(
                "source {} offers {} at {} bp; an amount and a carry cost are zero or more",
                source.id, source.available, source.carry_bps
            )));
        }
        let Some(held) = ledger.get(&source.id) else {
            return Err(Error::invalid(format!(
                "source {} is offered and the ledger holds no balance under that id; a source \
                 the ledger does not know is not capital",
                source.id
            )));
        };
        if source.available > *held {
            return Err(Error::invalid(format!(
                "source {} offers {} and the ledger holds {}; the ledger is authoritative, so \
                 the offer is refused rather than cut to it",
                source.id, source.available, held
            )));
        }
    }
    let mut seen = BTreeSet::new();
    for requirement in requirements {
        if !seen.insert(requirement.id.as_str()) {
            return Err(Error::invalid(format!(
                "requirement {} is listed twice; it would be funded twice",
                requirement.id
            )));
        }
        if requirement.amount.is_negative() {
            return Err(Error::invalid(format!(
                "requirement {} is {}; a requirement is zero or more",
                requirement.id, requirement.amount
            )));
        }
    }

    let mut remaining: BTreeMap<&str, Decimal> = sources
        .iter()
        .map(|source| (source.id.as_str(), source.available))
        .collect();
    let mut plan = PlacementPlan::default();
    for requirement in requirements {
        let mut eligible: Vec<&PlacementSource> = sources
            .iter()
            .filter(|source| {
                source.currency == requirement.currency
                    && source.entity == requirement.entity
                    && source
                        .venue
                        .as_ref()
                        .is_none_or(|venue| *venue == requirement.venue)
            })
            .collect();
        eligible.sort_by(|a, b| (a.carry_bps, &a.id).cmp(&(b.carry_bps, &b.id)));
        let capacity = eligible.iter().fold(Decimal::ZERO, |sum, source| {
            sum + remaining
                .get(source.id.as_str())
                .copied()
                .unwrap_or(Decimal::ZERO)
        });
        if capacity < requirement.amount {
            plan.unplaced.push(Unplaced {
                requirement: requirement.id.clone(),
                reason: format!(
                    "{} {} is required for {} at {} and the eligible sources of that currency, \
                     entity and venue hold {} between them; no currency is converted here, \
                     because no rate was supplied",
                    requirement.amount,
                    requirement.currency,
                    requirement.entity,
                    requirement.venue,
                    capacity
                ),
            });
            continue;
        }
        let mut needed = requirement.amount;
        for source in eligible {
            if needed.is_zero() {
                break;
            }
            let left = remaining
                .get_mut(source.id.as_str())
                .ok_or_else(|| Error::invalid(format!("source {} lost its balance", source.id)))?;
            let take = needed.min(*left);
            if take.is_zero() {
                continue;
            }
            *left -= take;
            needed -= take;
            plan.draws.push(PlacementDraw {
                requirement: requirement.id.clone(),
                source: source.id.clone(),
                amount: take,
            });
        }
    }
    reconcile(&plan, requirements, ledger)?;
    Ok(plan)
}

/// Whether `plan`'s totals reconcile to `ledger`: no source drawn past its
/// ledger balance, and every requirement either funded to the unit or not
/// drawn on at all. A plan that fails is refused whole.
pub fn reconcile(
    plan: &PlacementPlan,
    requirements: &[PlacementRequirement],
    ledger: &BTreeMap<String, Decimal>,
) -> Result<()> {
    let drawn_sources: BTreeSet<&str> = plan.draws.iter().map(|d| d.source.as_str()).collect();
    for source in drawn_sources {
        let held = ledger.get(source).copied().unwrap_or(Decimal::ZERO);
        let drawn = plan.drawn_from(source);
        if drawn > held {
            return Err(Error::invalid(format!(
                "the plan draws {drawn} from {source} and the ledger holds {held}; a placement \
                 that outruns the ledger is invented capital"
            )));
        }
    }
    for requirement in requirements {
        let placed = plan.placed_for(&requirement.id);
        if !placed.is_zero() && placed != requirement.amount {
            return Err(Error::invalid(format!(
                "requirement {} is {} and the plan places {}; a requirement is funded whole or \
                 not at all",
                requirement.id, requirement.amount, placed
            )));
        }
    }
    Ok(())
}
