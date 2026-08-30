//! Cross-cell insight, released through the confidential gate or not at all.
//!
//! The central plane holds every cell's positions, utilisation and breaks —
//! it has to, to enforce the capital envelopes. `qip-confidential` exists so
//! that what leaves the plane as *insight* — a number shown on a console, fed
//! to research, shared with another party — is an aggregate that no single
//! cell's book can be recovered from. Until this module, that crate was
//! reachable from nothing: fifty-four tests proving a gate nobody stood at.
//!
//! # The rule: no second door
//!
//! Everything this module exposes goes through [`ReleaseGate`]. There is no
//! method returning a per-cell breakdown, and none must ever be added here —
//! the plane itself has accessors for callers with an *operational* need for a
//! named cell's numbers (granting capital to a cell requires knowing that
//! cell's utilisation), and the difference is the point: an operational
//! reading names its cell and is audited as such, while an insight is
//! aggregate by construction. A module that offered both would invite the
//! caller to take the raw one because the gate refused.
//!
//! Refusals are therefore surfaced, not smoothed. Fewer cells reporting than
//! the cohort threshold means the number does not exist yet, and a console
//! rendering this should say "insufficient cells" rather than "0" — zero is a
//! number, and it is not the true one.
//!
//! # What the bounds are, and why they are not measured
//!
//! Every query's bounds come from the caller as policy, never from the data:
//! a bound computed from the observed values leaks through its own width.
//! The honest source for a gross-notional bound is the capital policy itself —
//! the largest gross limit any envelope grants is a number the platform
//! *issued*, not one it observed — and the docs on each method say which
//! policy number to use.

use qip_confidential::budget::Epsilon;
use qip_confidential::contribution::{CellId, Contribution, ContributionSet};
use qip_confidential::query::{Bounds, Query, Statistic};
use qip_confidential::release::{Policy, Release, ReleaseGate, ReleaseRecord};
use qip_core::Decimal;
use qip_core::error::{Error, Result};

use super::plane::CentralPlane;

/// The gate, standing between the plane's per-cell state and any aggregate
/// that leaves it.
#[derive(Debug)]
pub struct CellInsights {
    gate: ReleaseGate,
}

impl CellInsights {
    /// Build with the crate's default policy: a cohort threshold of five and
    /// one unit of privacy budget per cell.
    ///
    /// Five, against a platform that runs at most nine cells, means an
    /// insight exists only when most of the fleet is reporting — which is
    /// also the only time a cross-cell aggregate is worth reading.
    pub fn new(seed: u64) -> Self {
        Self {
            gate: ReleaseGate::new(Policy::default(), seed),
        }
    }

    pub fn with_policy(policy: Policy, seed: u64) -> Self {
        Self {
            gate: ReleaseGate::new(policy, seed),
        }
    }

    /// Mean gross notional per reporting cell.
    ///
    /// `bound` is the largest gross notional a single cell may hold — a
    /// policy number: use the largest envelope `gross_limit` the plane has
    /// granted, not a statistic of the books.
    pub fn mean_gross_notional(
        &mut self,
        plane: &CentralPlane,
        bound: Decimal,
        epsilon: Epsilon,
    ) -> Result<Release> {
        let query = Query::new(
            qip_confidential::contribution::CohortId::new("cells:gross-notional")?,
            Statistic::Mean,
            positive_bounds(bound)?,
            epsilon,
        )?;
        let contributions = contributions_from(plane.gross_notional_by_cell())?;
        self.gate.release(&query, &contributions)
    }

    /// Total realised loss across reporting cells.
    ///
    /// `bound` is the most a single cell can have lost — again a policy
    /// number: the sum of the loss limits its envelopes carry.
    pub fn total_realised_loss(
        &mut self,
        plane: &CentralPlane,
        bound: Decimal,
        epsilon: Epsilon,
    ) -> Result<Release> {
        let query = Query::new(
            qip_confidential::contribution::CohortId::new("cells:realised-loss")?,
            Statistic::Sum,
            positive_bounds(bound)?,
            epsilon,
        )?;
        let contributions = contributions_from(plane.realised_loss_by_cell())?;
        self.gate.release(&query, &contributions)
    }

    /// Every release ever made, for the audit surface. Records only —
    /// the raw contributions are not retained here at all.
    pub fn records(&self) -> &[ReleaseRecord] {
        self.gate.records()
    }
}

fn positive_bounds(bound: Decimal) -> Result<Bounds> {
    let high = bound.to_f64();
    if high <= 0.0 {
        return Err(Error::invalid(
            "an insight bound must be a positive policy number; zero width releases a constant",
        ));
    }
    Bounds::new(0.0, high)
}

fn contributions_from(
    per_cell: impl IntoIterator<Item = (String, Decimal)>,
) -> Result<ContributionSet> {
    ContributionSet::from_contributions(
        per_cell
            .into_iter()
            .map(|(cell, value)| Contribution::new(CellId::new(cell)?, value.to_f64()))
            .collect::<Result<Vec<_>>>()?,
    )
}
