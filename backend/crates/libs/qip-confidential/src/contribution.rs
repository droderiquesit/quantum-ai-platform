//! What a cell hands over, and what the fabric protects.
//!
//! The unit of protection here is **one cell's contribution to one
//! statistic** — not one client, not one order, not one fill. A cell reduces
//! its own book to a single number and submits that number; the raw rows never
//! leave the cell. Everything else in this crate is arithmetic on top of that
//! reduction, and every guarantee it states is a guarantee about a cell's
//! number rather than about a client's row.
//!
//! That distinction is load bearing and is stated again in [`crate`]: if a
//! cell's contribution *is* one client's position, then that client is
//! protected exactly as well as the cell is and no better.

use qip_core::error::{Error, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

/// One regional cell, by the name the central plane knows it by.
///
/// A newtype rather than a `String` because the two identifiers in this crate —
/// the cell and the cohort — are both strings and are never interchangeable: a
/// cohort label is public and decorative, a cell name indexes a privacy budget.
/// Swapping them would silently charge the wrong account.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct CellId(String);

impl CellId {
    /// Refuses an empty name: an unnamed cell cannot be charged for what it
    /// spends, and a budget nobody is charged for is not a budget.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("a cell id cannot be empty"));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The label a release is filed under.
///
/// Deliberately *not* a privacy boundary. It names the question for whoever
/// reads the audit trail; it does not scope the budget (see
/// [`crate::budget::PrivacyLedger`]) and it does not scope the differencing
/// gate (see [`crate::release::ReleaseGate`]), because in both cases scoping by
/// a caller-chosen label would let a caller escape the control by renaming.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CohortId(String);

impl CohortId {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid("a cohort id cannot be empty"));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CohortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One cell's number for one question.
///
/// The finiteness check is here, at construction, rather than in the release
/// path, and that placement is the point. A refusal inside
/// [`crate::release::ReleaseGate::release`] that depended on the *values* would
/// be a side channel: the caller learns one bit about the data from the fact
/// that the release was refused, and learns it for free, without spending any
/// budget. Every refusal in the release path therefore depends only on things
/// the caller already knows — which cells are in the cohort, what was asked
/// before, what remains in the ledger — and a NaN is rejected at the door.
#[derive(Clone, Debug, PartialEq)]
pub struct Contribution {
    cell: CellId,
    value: f64,
}

impl Contribution {
    pub fn new(cell: CellId, value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "cell {cell} contributed a non-finite value; a contribution must be a number \
                 before it reaches a release, not after"
            )));
        }
        Ok(Self { cell, value })
    }

    pub fn cell(&self) -> &CellId {
        &self.cell
    }

    pub fn value(&self) -> f64 {
        self.value
    }
}

/// The set of contributions one release is computed over.
///
/// Two properties, both of which a `Vec<Contribution>` would quietly fail to
/// have.
///
/// **One entry per cell.** A cell that appears twice is counted twice by the
/// threshold gate and weighted twice in the statistic, which breaks the
/// sensitivity analysis the noise is calibrated from: the noise would be sized
/// for a contribution that can move the sum by one range, and the doubled cell
/// can move it by two. [`Self::insert`] refuses the second entry and names the
/// cell rather than overwriting the first, because a caller that submitted a
/// cell twice has a bug and the release it was about to get would not be the
/// release it thinks it asked for.
///
/// **A fixed iteration order.** Floating-point addition is not associative, so
/// the same contributions submitted in a different order produce a different
/// sum in the last bits, a different noisy release, and a platform that cannot
/// reproduce its own audit trail. Ordering by cell id fixes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContributionSet {
    by_cell: BTreeMap<CellId, f64>,
}

impl ContributionSet {
    pub fn new() -> Self {
        Self {
            by_cell: BTreeMap::new(),
        }
    }

    /// Add one cell's number. Refuses a cell already present.
    pub fn insert(&mut self, contribution: Contribution) -> Result<()> {
        let Contribution { cell, value } = contribution;
        if self.by_cell.contains_key(&cell) {
            return Err(Error::invalid(format!(
                "cell {cell} contributed twice to the same release; a repeated cell is counted \
                 twice by the threshold and weighted twice in the statistic"
            )));
        }
        self.by_cell.insert(cell, value);
        Ok(())
    }

    /// Build a set from an iterator, refusing duplicates the same way.
    pub fn from_contributions(
        contributions: impl IntoIterator<Item = Contribution>,
    ) -> Result<Self> {
        let mut set = Self::new();
        for contribution in contributions {
            set.insert(contribution)?;
        }
        Ok(set)
    }

    /// How many distinct cells are in the set. This is the number the
    /// threshold gate compares against, and it is public: the caller chose the
    /// cohort, so the caller already knows it.
    pub fn contributors(&self) -> usize {
        self.by_cell.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_cell.is_empty()
    }

    /// The cells, in the order everything downstream uses.
    pub fn cells(&self) -> impl Iterator<Item = &CellId> {
        self.by_cell.keys()
    }

    /// One cell's number, if it is in the set.
    ///
    /// Present because the caller assembling the set is the aggregation
    /// process, which already holds every value in memory — see the security
    /// boundary in [`crate`]. Hiding the accessor would hide nothing.
    pub fn value(&self, cell: &CellId) -> Option<f64> {
        self.by_cell.get(cell).copied()
    }

    /// The values, clamped into the declared range, in cell order.
    pub(crate) fn clamped(&self, bounds: crate::query::Bounds) -> Vec<f64> {
        self.by_cell.values().map(|v| bounds.clamp(*v)).collect()
    }
}
