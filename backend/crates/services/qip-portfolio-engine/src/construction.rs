//! Portfolio construction.
//!
//! Turns approved hypotheses into a sized [`crate::proposal::Proposal`]. The
//! sizing goes through the compute router, so the same concentration limits,
//! cardinality rules and cost model apply whichever solver runs.
//!
//! Two things this stage will not do. It will not size a position on a thesis
//! that has not been approved — the input is approved convictions, and there is
//! no path from an unreviewed opinion to a weight. And it will not quietly
//! relax a constraint to hit an exposure target: where a concentration cap
//! makes the target unreachable, the cap wins, the book is deliberately
//! under-invested, and the proposal says so in its own compromises.

use crate::proposal::{Proposal, ProposalLeg, ProposalStatus, Side};
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, ProposalId};
use qip_core::time::Timestamp;
use qip_core::{Currency, Decimal, Money};
use qip_optimization_engine::problem::{Objective, PortfolioConstraint, PortfolioProblem};
use qip_optimization_engine::router::{ComputeRouter, RoutingDecision};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One approved thesis, ready to be sized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovedThesis {
    pub hypothesis_id: String,
    pub object_id: ObjectId,
    /// Direction and strength, in `[-1, 1]`. Signed: negative is a short.
    pub conviction: f64,
    /// Expected return over the horizon, as a fraction. Computed upstream from
    /// the thesis, never chosen here.
    pub expected_return: f64,
    /// Reference price for sizing.
    pub price: Decimal,
}

impl ApprovedThesis {
    pub fn validate(&self) -> Result<()> {
        if !(-1.0..=1.0).contains(&self.conviction) {
            return Err(Error::invalid(format!(
                "thesis {} has conviction {} outside [-1, 1]",
                self.hypothesis_id, self.conviction
            )));
        }
        if self.price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "thesis {} has no usable price",
                self.hypothesis_id
            )));
        }
        if !self.expected_return.is_finite() {
            return Err(Error::numeric(format!(
                "thesis {} has a non-finite expected return",
                self.hypothesis_id
            )));
        }
        Ok(())
    }
}

/// The mandate the book is run under.
///
/// Every number here is a constraint someone agreed to. They are collected in
/// one place so a proposal can be checked against the mandate rather than
/// against whatever the sizing code happened to do.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mandate {
    /// Target gross exposure as a fraction of equity.
    pub target_gross: f64,
    /// Largest position, as a fraction of equity.
    pub position_cap: f64,
    /// Whether shorts are permitted.
    pub long_only: bool,
    /// Maximum number of holdings, when the mandate sets one.
    pub maximum_holdings: Option<usize>,
    /// Positions below this are not worth holding.
    pub minimum_position: f64,
    /// Cost per unit of turnover, in basis points.
    pub turnover_cost_bps: f64,
    /// Risk aversion for the mean-variance objective.
    pub risk_aversion: f64,
}

impl Default for Mandate {
    fn default() -> Self {
        Self {
            target_gross: 0.95,
            position_cap: 0.08,
            long_only: true,
            maximum_holdings: None,
            minimum_position: 0.005,
            turnover_cost_bps: 15.0,
            risk_aversion: 5.0,
        }
    }
}

impl Mandate {
    pub fn validate(&self) -> Result<()> {
        if self.position_cap <= 0.0 || self.position_cap > 1.0 {
            return Err(Error::invalid(
                "the position cap must be a fraction of equity in (0, 1]",
            ));
        }
        if self.target_gross <= 0.0 {
            return Err(Error::invalid("the gross target must be positive"));
        }
        if self.minimum_position >= self.position_cap {
            return Err(Error::invalid(
                "the minimum position is at or above the cap, so no position is permissible",
            ));
        }
        Ok(())
    }

    /// The gross exposure `n` names can reach under the cap.
    pub fn achievable_gross(&self, names: usize) -> f64 {
        self.target_gross.min(self.position_cap * names as f64)
    }
}

/// What the construction produced, and how.
#[derive(Clone, Debug)]
pub struct ConstructionOutcome {
    pub proposal: Proposal,
    /// The routing decision that sized it, so the solver choice is auditable.
    pub routing: RoutingDecision,
}

/// Builds proposals from approved theses.
#[derive(Debug)]
pub struct PortfolioConstructor {
    mandate: Mandate,
    router: ComputeRouter,
}

impl PortfolioConstructor {
    pub fn new(mandate: Mandate, router: ComputeRouter) -> Result<Self> {
        mandate.validate()?;
        Ok(Self { mandate, router })
    }

    pub fn mandate(&self) -> Mandate {
        self.mandate
    }

    /// Build a proposal.
    ///
    /// `covariance` must be in the same asset order as `theses`. `current`
    /// holds the weights the book already has, so the turnover is real rather
    /// than assumed from a flat start.
    pub fn construct(
        &self,
        theses: &[ApprovedThesis],
        covariance: &[Vec<f64>],
        current: &BTreeMap<String, f64>,
        equity: Money,
        as_of: Timestamp,
        now: Timestamp,
        proposal_id: ProposalId,
    ) -> Result<ConstructionOutcome> {
        if now < as_of {
            return Err(Error::invalid(format!(
                "constructing at {now} for an as-of time of {as_of}, which is in its future"
            )));
        }
        if theses.is_empty() {
            return Err(Error::invalid(
                "no approved thesis to express; construction does not create views",
            ));
        }
        for thesis in theses {
            thesis.validate()?;
        }
        if equity.amount <= Decimal::ZERO {
            return Err(Error::invalid("cannot size a proposal against no equity"));
        }

        let assets: Vec<String> = theses
            .iter()
            .map(|t| t.object_id.as_str().to_string())
            .collect();
        let n = assets.len();

        // A short with a long-only mandate is not sized down, it is dropped:
        // silently flipping it to a zero weight would hide that the thesis was
        // unusable under this mandate.
        let mut dropped = Vec::new();
        let floor = if self.mandate.long_only {
            0.0
        } else {
            -self.mandate.position_cap
        };
        let lower: Vec<f64> = vec![floor; n];
        let upper: Vec<f64> = theses
            .iter()
            .map(|t| {
                if self.mandate.long_only && t.conviction < 0.0 {
                    dropped.push(t.hypothesis_id.clone());
                    0.0
                } else {
                    self.mandate.position_cap
                }
            })
            .collect();

        let achievable = self.mandate.achievable_gross(n);
        let mut problem = PortfolioProblem::new(assets.clone(), covariance.to_vec())?
            .with_objective(Objective::MeanVariance)
            .with_expected_returns(theses.iter().map(|t| t.expected_return).collect())
            .with_risk_aversion(self.mandate.risk_aversion)
            .with_bounds(lower, upper)
            .with_constraint(PortfolioConstraint::equals(
                "budget",
                vec![1.0; n],
                achievable,
            ));

        if let Some(maximum) = self.mandate.maximum_holdings {
            problem = problem.with_cardinality(maximum.min(n));
        }
        if self.mandate.turnover_cost_bps > 0.0 {
            let current_vector: Vec<f64> = assets
                .iter()
                .map(|id| current.get(id).copied().unwrap_or(0.0))
                .collect();
            problem = problem
                .with_turnover_penalty(current_vector, self.mandate.turnover_cost_bps / 10_000.0);
        }

        let routing = self.router.solve(&problem)?;

        // Build the legs. Anything below the minimum is dropped rather than
        // held: a 0.1% position costs the same in operational overhead as a 5%
        // one and contributes nothing.
        let equity_value = equity.amount.to_f64();
        let mut legs = Vec::new();
        let mut turnover = 0.0;
        for (index, thesis) in theses.iter().enumerate() {
            let target = routing.weights.get(index).copied().unwrap_or(0.0);
            let target = if target.abs() < self.mandate.minimum_position {
                0.0
            } else {
                target
            };
            let held = current
                .get(thesis.object_id.as_str())
                .copied()
                .unwrap_or(0.0);
            let change = target - held;
            turnover += change.abs();
            if change.abs() < 1e-9 {
                continue;
            }

            let price = thesis.price.to_f64();
            let units = (change * equity_value / price).abs();
            let Some(quantity) = Decimal::from_f64(units) else {
                return Err(Error::numeric(format!(
                    "the quantity for {} is not representable",
                    thesis.object_id.as_str()
                )));
            };
            if quantity <= Decimal::ZERO {
                continue;
            }

            legs.push(ProposalLeg {
                object_id: thesis.object_id.clone(),
                side: if change > 0.0 { Side::Buy } else { Side::Sell },
                quantity,
                reference_price: thesis.price,
                current_weight: held,
                target_weight: target,
                estimated_cost_bps: self.mandate.turnover_cost_bps,
                hypotheses: vec![thesis.hypothesis_id.clone()],
            });
        }
        // Turnover is half the sum of absolute weight changes: the fraction of
        // the book that must trade, not twice it.
        turnover *= 0.5;

        let target_gross: f64 = legs.iter().map(|leg| leg.target_weight.abs()).sum();
        let target_net: f64 = legs.iter().map(|leg| leg.target_weight).sum();

        let mut compromises = Vec::new();
        if achievable < self.mandate.target_gross - 1e-9 {
            compromises.push(format!(
                "{n} name(s) against a {:.1}% cap reach {:.1}% gross, short of the {:.1}% target; the cap wins and the book is deliberately under-invested",
                self.mandate.position_cap * 100.0,
                achievable * 100.0,
                self.mandate.target_gross * 100.0
            ));
        }
        if !dropped.is_empty() {
            compromises.push(format!(
                "{} short thesis(es) dropped under a long-only mandate: {}",
                dropped.len(),
                dropped.join(", ")
            ));
        }
        for run in &routing.runs {
            for relaxation in &run.relaxations {
                compromises.push(format!("{}: {relaxation}", run.solver.as_str()));
            }
        }
        if let Some(chosen) = routing.run_for(routing.chosen)
            && !chosen.feasible
        {
            compromises.push(format!(
                "no feasible sizing was found; {} is breached by {:.4}",
                chosen.violated_constraint, chosen.worst_violation
            ));
        }

        let proposal = Proposal {
            proposal_id,
            created_at: now,
            as_of,
            status: ProposalStatus::Draft,
            legs,
            equity,
            target_gross,
            target_net,
            turnover,
            estimated_cost_bps: turnover * self.mandate.turnover_cost_bps * 2.0,
            rationale: format!(
                "expresses {} approved thesis(es) at {:.1}% gross, sized by {}: {}",
                theses.len(),
                target_gross * 100.0,
                routing.chosen.as_str(),
                routing.rationale
            ),
            compromises,
            checks_passed: Vec::new(),
        };
        proposal.validate()?;

        Ok(ConstructionOutcome { proposal, routing })
    }

    /// An empty proposal, for a cycle with nothing to do.
    ///
    /// Distinct from an error: having no approved theses is a normal state and
    /// should not read as a failure.
    pub fn nothing_to_do(
        &self,
        proposal_id: ProposalId,
        equity: Money,
        as_of: Timestamp,
        now: Timestamp,
        reason: impl Into<String>,
    ) -> Proposal {
        Proposal {
            proposal_id,
            created_at: now,
            as_of,
            status: ProposalStatus::Draft,
            legs: Vec::new(),
            equity,
            target_gross: 0.0,
            target_net: 0.0,
            turnover: 0.0,
            estimated_cost_bps: 0.0,
            rationale: reason.into(),
            compromises: Vec::new(),
            checks_passed: Vec::new(),
        }
    }
}

/// The base currency proposals are denominated in when none is stated.
pub const DEFAULT_CURRENCY: Currency = Currency::USD;
