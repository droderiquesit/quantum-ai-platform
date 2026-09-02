//! The cost engine: two deductions, ready to hand to a [`NetEdge`].
//!
//! Everything else in this crate exists to make these two numbers real. The
//! engine itself is small on purpose — it takes what a decision spent on
//! compute and what it spent on data, and produces the two
//! [`qip_contracts::Deduction`]s that make an edge calculation account for
//! having been performed at all.
//!
//! # Units
//!
//! Both deductions come out in whatever currency the tier costs and
//! subscriptions were stated in. A [`NetEdge`] is denominated in units of the
//! instrument the opportunity started from, and for a dollar-quoted book those
//! are the same thing. For anything else they are not, and a caller must
//! convert before deducting. This is stated rather than checked because it
//! cannot be checked here: subtracting dollars from an edge quoted in units of
//! a token is a units bug that produces a plausible number, and no assertion in
//! this crate can tell the two apart.
//!
//! # This engine reports; the bounds live in the router
//!
//! There is deliberately no "spent so far" figure here for a caller to compare
//! against the value at stake before escalating. One existed, and nothing
//! compared it: the bound on cumulative spend is [`crate::Router::escalate`]
//! refusing past [`crate::EscalationLimits::maximum_spend`], and the bound on a
//! rung against the value at stake is [`crate::RoutingPolicy::affordable`]
//! inside [`crate::Router::assess`]. A second figure of the same sum, owned
//! here and consulted nowhere, read as a control and was not.

use crate::data::{DataCostModel, DataReads};
use crate::ledger::ComputeLedger;
use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_core::error::Result;

/// Turns what a decision spent into what its edge has to survive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CostEngine {
    data: DataCostModel,
}

impl CostEngine {
    pub fn new(data: DataCostModel) -> Self {
        Self { data }
    }

    pub fn data(&self) -> &DataCostModel {
        &self.data
    }

    /// The compute and data deductions for one decision, in that order.
    ///
    /// Returned as a pair rather than applied, because the edge belongs to the
    /// caller and a cost engine that could mutate one would be a component that
    /// changes a number after it has been reported.
    ///
    /// Both are produced even when they are zero. A zero deduction with a basis
    /// is a claim someone can argue with; an absent one is indistinguishable
    /// from a cost nobody thought about, which is precisely what
    /// [`NetEdge::require_complete`] exists to catch.
    pub fn cost_deductions(
        &self,
        compute: &ComputeLedger,
        reads: &DataReads,
    ) -> Result<(Deduction, Deduction)> {
        let compute_cost = compute.total_cost();
        let compute_deduction =
            Deduction::new(DeductionKind::ComputeCost, compute_cost, compute.basis())?;

        let (data_cost, data_basis) = self.data.charge(reads)?;
        let data_deduction = Deduction::new(DeductionKind::DataCost, data_cost, data_basis)?;

        Ok((compute_deduction, data_deduction))
    }

    /// Apply both deductions to an edge.
    ///
    /// A convenience with a purpose: the pair is easy to build and easy to hand
    /// on somewhere it is only ever displayed. This is the call that puts them
    /// where they change the answer.
    pub fn charge_edge(
        &self,
        edge: NetEdge,
        compute: &ComputeLedger,
        reads: &DataReads,
    ) -> Result<NetEdge> {
        let (compute_deduction, data_deduction) = self.cost_deductions(compute, reads)?;
        Ok(edge.deduct(compute_deduction).deduct(data_deduction))
    }
}
