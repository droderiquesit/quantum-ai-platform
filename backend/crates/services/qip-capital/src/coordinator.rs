//! The capital-society coordinator's arithmetic (R12-CAPITAL-9047, ADR
//! 0230): liquidity, collateral, funding and allocation proposals settled
//! against one grant in the survival waterfall's order.
//!
//! Independent proposers each sized against the same grant over-commit it:
//! each claim is reasonable alone and their sum is not. So the claims are
//! settled together, in one order, from one figure. Liquidity and collateral
//! come first, because a book that cannot meet a margin call or an exit
//! does not get to size anything else; funding obligations next, because
//! they are owed; allocation last, from what remains. A set that would sum
//! past the grant is cut from the bottom of that order, and the grant
//! itself is an input here and never an output, so nothing in this module
//! can widen it.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// What a proposal claims capital for, declared in the waterfall's order:
/// the derived `Ord` *is* the settlement order, so a new variant's position
/// in this list is the decision about where it is paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Claim {
    Liquidity,
    Collateral,
    Funding,
    Allocation,
}

impl Claim {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Liquidity => "liquidity",
            Self::Collateral => "collateral",
            Self::Funding => "funding",
            Self::Allocation => "allocation",
        }
    }
}

/// One proposer's claim on a grant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalProposal {
    pub claim: Claim,
    /// Who proposed it and for what, e.g. the strategy an allocation line
    /// funds. Orders proposals of one claim among themselves.
    pub source: String,
    pub amount: Decimal,
}

/// A proposal and what it was granted: its amount in full, less, or zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settled {
    pub proposal: CapitalProposal,
    pub granted: Decimal,
}

impl Settled {
    /// Whether the grant ran out before this proposal was paid in full.
    pub fn cut(&self) -> bool {
        self.granted < self.proposal.amount
    }
}

/// Every proposal settled against `grant`, in waterfall order (claim, then
/// source). The granted amounts never sum past `grant`; a proposal the
/// remainder cannot cover is cut to the remainder, and everything after it
/// gets nothing. A negative grant or proposal is refused rather than read as
/// zero: either is an accounting error upstream, and settling it would hide
/// where.
pub fn settle(grant: Decimal, mut proposals: Vec<CapitalProposal>) -> Result<Vec<Settled>> {
    if grant.is_negative() {
        return Err(Error::invalid(format!(
            "the grant is {grant}; a grant is zero or more, and a negative one is the \
             partition's accounting gone wrong, not a figure to settle against"
        )));
    }
    if let Some(bad) = proposals.iter().find(|p| p.amount.is_negative()) {
        return Err(Error::invalid(format!(
            "the {} proposal from {} claims {}; a claim is zero or more, so it is refused \
             rather than read as capital handed back",
            bad.claim.as_str(),
            bad.source,
            bad.amount
        )));
    }
    proposals.sort_by(|a, b| (a.claim, &a.source).cmp(&(b.claim, &b.source)));
    let mut remaining = grant;
    Ok(proposals
        .into_iter()
        .map(|proposal| {
            let granted = proposal.amount.min(remaining);
            remaining -= granted;
            Settled { proposal, granted }
        })
        .collect())
}

/// The sum of what a settlement granted.
pub fn granted_total(settled: &[Settled]) -> Decimal {
    settled
        .iter()
        .fold(Decimal::ZERO, |sum, line| sum + line.granted)
}
