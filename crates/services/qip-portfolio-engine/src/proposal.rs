//! Trade proposals.
//!
//! A [`Proposal`] is what the construction stage produces and what the risk and
//! compliance functions veto. It is deliberately not an order: it carries
//! intent, the reasoning behind it, and the checks it has passed, and nothing
//! turns it into an order except an explicit authorised step in the ACT stage.
//!
//! Every proposal states the theses it expresses. A trade nobody can trace to a
//! reviewed hypothesis is a trade nobody reviewed.

use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, ProposalId};
use qip_core::time::Timestamp;
use qip_core::{Decimal, Money};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which way a leg goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    pub const fn sign(&self) -> i32 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
}

/// One instrument's worth of a proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposalLeg {
    pub object_id: ObjectId,
    pub side: Side,
    /// Units to trade. Always positive; the side carries the direction.
    pub quantity: Decimal,
    /// Reference price the sizing was computed at.
    pub reference_price: Decimal,
    /// Weight before and after, as fractions of equity.
    pub current_weight: f64,
    pub target_weight: f64,
    /// Estimated cost of the leg in basis points.
    pub estimated_cost_bps: f64,
    /// The hypotheses this leg expresses.
    pub hypotheses: Vec<String>,
}

impl ProposalLeg {
    pub fn notional(&self) -> Decimal {
        self.quantity * self.reference_price
    }

    pub fn weight_change(&self) -> f64 {
        self.target_weight - self.current_weight
    }
}

/// Where a proposal is in its life.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Constructed, not yet checked.
    Draft,
    /// Passed risk and compliance.
    Approved { at: Timestamp, by: Vec<String> },
    /// Blocked. The reason names the control and the limit.
    Vetoed {
        at: Timestamp,
        by: String,
        reason: String,
    },
    /// Approved and handed to execution.
    Released { at: Timestamp },
    /// Superseded before it was released.
    Withdrawn { at: Timestamp, reason: String },
}

impl ProposalStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved { .. } => "approved",
            Self::Vetoed { .. } => "vetoed",
            Self::Released { .. } => "released",
            Self::Withdrawn { .. } => "withdrawn",
        }
    }

    /// Whether the proposal may be turned into orders.
    ///
    /// Only `Approved`. A draft has not been checked and a vetoed proposal has
    /// been checked and refused; neither is a step away from execution.
    pub const fn is_releasable(&self) -> bool {
        matches!(self, Self::Approved { .. })
    }
}

/// A proposed change to the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub proposal_id: ProposalId,
    pub created_at: Timestamp,
    /// The point in time the sizing reasoned as of.
    pub as_of: Timestamp,
    pub status: ProposalStatus,
    pub legs: Vec<ProposalLeg>,
    /// Equity the weights are fractions of.
    pub equity: Money,
    /// Gross and net exposure the proposal would produce.
    pub target_gross: f64,
    pub target_net: f64,
    /// Fraction of the book that must trade.
    pub turnover: f64,
    /// Total estimated cost, in basis points of equity.
    pub estimated_cost_bps: f64,
    /// Why this proposal, in a sentence for the decision record.
    pub rationale: String,
    /// Everything the construction had to compromise on.
    pub compromises: Vec<String>,
    /// Checks that have passed, by control name.
    pub checks_passed: Vec<String>,
}

impl Proposal {
    pub fn is_empty(&self) -> bool {
        self.legs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.legs.len()
    }

    /// Total absolute notional traded.
    pub fn traded_notional(&self) -> Decimal {
        self.legs
            .iter()
            .map(|leg| leg.notional())
            .fold(Decimal::ZERO, |a, b| a + b)
    }

    pub fn leg(&self, object_id: &ObjectId) -> Option<&ProposalLeg> {
        self.legs.iter().find(|leg| &leg.object_id == object_id)
    }

    /// Target weights, for a risk check.
    pub fn target_weights(&self) -> BTreeMap<String, f64> {
        self.legs
            .iter()
            .map(|leg| (leg.object_id.as_str().to_string(), leg.target_weight))
            .collect()
    }

    /// Every hypothesis this proposal expresses.
    pub fn hypotheses(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .legs
            .iter()
            .flat_map(|leg| leg.hypotheses.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Approve, recording who signed off.
    ///
    /// Requires at least two controls. A single approver is a single point of
    /// failure, and the platform has both a risk and a compliance function
    /// precisely so that neither is one.
    pub fn approve(&mut self, at: Timestamp, by: Vec<String>) -> Result<()> {
        if !matches!(self.status, ProposalStatus::Draft) {
            return Err(Error::denied(format!(
                "proposal {} is {} and cannot be approved",
                self.proposal_id.as_str(),
                self.status.as_str()
            )));
        }
        if by.len() < 2 {
            return Err(Error::denied(format!(
                "proposal {} was approved by {} control(s); risk and compliance must both sign",
                self.proposal_id.as_str(),
                by.len()
            )));
        }
        self.checks_passed = by.clone();
        self.status = ProposalStatus::Approved { at, by };
        Ok(())
    }

    /// Veto. Always permitted, from any state before release, because a
    /// control that can be locked out by timing is not a control.
    pub fn veto(&mut self, at: Timestamp, by: impl Into<String>, reason: impl Into<String>) {
        if matches!(self.status, ProposalStatus::Released { .. }) {
            return;
        }
        self.status = ProposalStatus::Vetoed {
            at,
            by: by.into(),
            reason: reason.into(),
        };
    }

    /// Release to execution. Only from `Approved`.
    pub fn release(&mut self, at: Timestamp) -> Result<()> {
        if !self.status.is_releasable() {
            return Err(Error::denied(format!(
                "proposal {} is {} and cannot be released",
                self.proposal_id.as_str(),
                self.status.as_str()
            )));
        }
        self.status = ProposalStatus::Released { at };
        Ok(())
    }

    pub fn withdraw(&mut self, at: Timestamp, reason: impl Into<String>) {
        self.status = ProposalStatus::Withdrawn {
            at,
            reason: reason.into(),
        };
    }

    pub fn validate(&self) -> Result<()> {
        if self.rationale.trim().is_empty() {
            return Err(Error::invalid(format!(
                "proposal {} states no rationale",
                self.proposal_id.as_str()
            )));
        }
        if self.created_at < self.as_of {
            return Err(Error::invalid(format!(
                "proposal {} was created at {} for an as-of time of {}, which is in its future",
                self.proposal_id.as_str(),
                self.created_at,
                self.as_of
            )));
        }
        for leg in &self.legs {
            if leg.quantity <= Decimal::ZERO {
                return Err(Error::invalid(format!(
                    "leg for {} has a non-positive quantity; the side carries the direction",
                    leg.object_id.as_str()
                )));
            }
            if leg.reference_price <= Decimal::ZERO {
                return Err(Error::invalid(format!(
                    "leg for {} has no usable reference price",
                    leg.object_id.as_str()
                )));
            }
            // The rule that makes every trade traceable.
            if leg.hypotheses.is_empty() {
                return Err(Error::invalid(format!(
                    "leg for {} expresses no hypothesis; a trade nobody can trace to a reviewed thesis is a trade nobody reviewed",
                    leg.object_id.as_str()
                )));
            }
            // The side must agree with the weight change, or the proposal says
            // one thing and does another.
            let change = leg.weight_change();
            let expected = if change > 0.0 { Side::Buy } else { Side::Sell };
            if change.abs() > 1e-12 && leg.side != expected {
                return Err(Error::invalid(format!(
                    "leg for {} is a {} but the weight moves {:+.4}",
                    leg.object_id.as_str(),
                    leg.side.as_str(),
                    change
                )));
            }
        }
        Ok(())
    }
}
