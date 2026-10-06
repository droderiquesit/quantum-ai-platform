//! Which source funds which requirement (CAPITAL-001).
//!
//! [`crate::allocation`] decides how much each strategy and cell should get.
//! It never says where the money comes from, and a grant that does not name
//! its source can be issued against capital that does not exist. This module
//! is the second question: given what grants and placements require and what
//! each source has, name the source of every unit.
//!
//! Two rules, both refusals.
//!
//! **A requirement is funded whole or not at all.** A requirement the
//! remaining sources cannot meet is refused and draws nothing. Funding the
//! part that fits would hand a strategy a grant smaller than the one it was
//! sized for under the same name, and the shortfall would surface as a
//! refused order rather than as a funding decision.
//!
//! **A financing source counts only where it is permitted.** A balance at a
//! counterparty the platform has no recorded permission to finance with
//! ([`crate::treasury::FinancingPermissions`]) is not available capital. It
//! is left out before anything is summed, so it cannot make a requirement
//! look fundable.
//!
//! The platform's own capital is drawn before any financing, because
//! financing creates an obligation and own capital does not. This is a plan:
//! it moves nothing, and nothing here contacts a counterparty.

use crate::treasury::{FinancingFunction, FinancingPermissions};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Where capital comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FundingSource {
    /// One of the platform's own books.
    OwnCapital {
        /// The book.
        book: String,
    },
    /// A financing function at a counterparty.
    Financing {
        /// The function used.
        function: FinancingFunction,
        /// The counterparty providing it.
        counterparty: String,
    },
}

impl FundingSource {
    fn is_own(&self) -> bool {
        matches!(self, Self::OwnCapital { .. })
    }
}

/// What one source has available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBalance {
    /// The source.
    pub source: FundingSource,
    /// What it can fund.
    pub available: Decimal,
}

/// What the capital is required for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequirementKind {
    /// A capital grant to a strategy.
    Grant,
    /// A placement at a venue or region.
    Placement,
}

/// One amount of capital one strategy and region requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingRequirement {
    /// Stable identity.
    pub id: String,
    /// Grant or placement.
    pub kind: RequirementKind,
    /// The strategy it is for.
    pub strategy: String,
    /// The region it is for.
    pub region: String,
    /// How much is required.
    pub amount: Decimal,
}

/// An amount taken from one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draw {
    /// The source drawn on.
    pub source: FundingSource,
    /// How much.
    pub amount: Decimal,
}

/// A requirement and the sources that fund it, summing exactly to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Funded {
    /// The requirement funded.
    pub requirement: String,
    /// The draws that fund it.
    pub draws: Vec<Draw>,
}

/// A requirement no source could meet, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingRefusal {
    /// The requirement refused.
    pub requirement: String,
    /// The figures that refused it.
    pub reason: String,
}

/// Every requirement, funded or refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingPlan {
    /// Requirements funded in full.
    pub funded: Vec<Funded>,
    /// Requirements refused whole.
    pub refused: Vec<FundingRefusal>,
}

/// Name a source for every requirement, in the order the requirements are
/// given, or refuse it.
///
/// `entity` and `jurisdiction` are who is financing and where, the other two
/// parts of the permission a financing source needs.
pub fn plan_funding(
    requirements: &[FundingRequirement],
    balances: &[SourceBalance],
    permissions: &FinancingPermissions,
    entity: &str,
    jurisdiction: &str,
) -> Result<FundingPlan> {
    let mut seen = BTreeSet::new();
    for balance in balances {
        if balance.available.is_negative() {
            return Err(Error::invalid(format!(
                "source {:?} has a negative balance of {}; reconcile it before planning \
                 against it",
                balance.source, balance.available
            )));
        }
        if !seen.insert(&balance.source) {
            return Err(Error::invalid(format!(
                "source {:?} is listed twice; merge the two balances, or its capital is \
                 planned against twice",
                balance.source
            )));
        }
    }

    // Own capital first, then financing, each in the order given. An
    // unpermitted financing source is dropped here, before any sum.
    let mut unpermitted = 0usize;
    let mut remaining: Vec<(FundingSource, Decimal)> = Vec::new();
    for own in [true, false] {
        for balance in balances.iter().filter(|b| b.source.is_own() == own) {
            if let FundingSource::Financing {
                function,
                counterparty,
            } = &balance.source
                && permissions
                    .require(*function, entity, counterparty, jurisdiction)
                    .is_err()
            {
                unpermitted += 1;
                continue;
            }
            remaining.push((balance.source.clone(), balance.available));
        }
    }

    let mut ids = BTreeSet::new();
    let mut plan = FundingPlan {
        funded: Vec::new(),
        refused: Vec::new(),
    };
    for requirement in requirements {
        if !requirement.amount.is_positive() {
            return Err(Error::invalid(format!(
                "requirement {} asks for {}; a requirement is a positive amount",
                requirement.id, requirement.amount
            )));
        }
        if !ids.insert(requirement.id.as_str()) {
            return Err(Error::invalid(format!(
                "requirement {} is listed twice; it would be funded twice",
                requirement.id
            )));
        }
        let on_hand = remaining
            .iter()
            .fold(Decimal::ZERO, |sum, (_, left)| sum + *left);
        if on_hand < requirement.amount {
            plan.refused.push(FundingRefusal {
                requirement: requirement.id.clone(),
                reason: format!(
                    "{} for {}/{} needs {} and the permitted sources hold {on_hand} \
                     ({unpermitted} financing source(s) left out for want of a recorded \
                     permission); fund a source, record the permission, or reduce the \
                     requirement",
                    requirement.id, requirement.strategy, requirement.region, requirement.amount
                ),
            });
            continue;
        }
        let mut needed = requirement.amount;
        let mut draws = Vec::new();
        for (source, left) in &mut remaining {
            let take = needed.min(*left);
            if take.is_positive() {
                *left -= take;
                needed -= take;
                draws.push(Draw {
                    source: source.clone(),
                    amount: take,
                });
            }
        }
        plan.funded.push(Funded {
            requirement: requirement.id.clone(),
            draws,
        });
    }
    Ok(plan)
}
