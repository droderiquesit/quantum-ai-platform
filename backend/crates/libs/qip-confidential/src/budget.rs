//! The privacy budget: a ledger that only goes up.
//!
//! One release of one aggregate leaks a little about every cell that went into
//! it. A thousand releases of nearly the same aggregate leak the rows. The
//! difference between the two is not the size of any individual answer, it is
//! the *total*, and a total is only bounded by something that remembers.
//!
//! This is `qip_evolution::challenger::TrialLedger` applied to disclosure:
//! a running figure that has no method for lowering it, because every reason
//! anyone would want to lower it is a reason to distrust the answers that come
//! after. A trial count that can be reset launders selection bias; a privacy
//! budget that can be reset launders reconstruction, and it does it in the same
//! shape — a caller who has run out of budget is a caller who has already had
//! all the answers the policy was willing to give.
//!
//! # Why the account is a cell and not a cohort
//!
//! The natural-looking design is a budget per cohort — one label, one account,
//! one number to show an operator. It is the wrong design, and it fails
//! silently: a cell that appears in six cohorts pays into six separate accounts
//! and is protected by none of them, because what bounds the leakage about a
//! cell is the sum of the noise-scaled queries *that cell was in*, whatever
//! they were labelled. Composition sums over releases, not over labels.
//!
//! So the enforced account is the cell. Cohort totals are still kept, and
//! [`PrivacyLedger::spent_on_cohort`] reads them back, but they are reporting
//! only — nothing refuses on them.
//!
//! # Where this ledger's monotonicity ends
//!
//! It is in memory, and it lives as long as the [`crate::release::ReleaseGate`]
//! that owns it. There is no persistence here and no [`serde::Deserialize`],
//! so nothing in this crate can restore a smaller number over a larger one —
//! but a process restart starts a fresh ledger with the full budget, which is a
//! reset by other means. [`PrivacyLedger::report`] and
//! [`PrivacyLedger::absorb`] exist so a deployment can checkpoint the figures
//! somewhere durable and fold them back; `absorb` takes the *larger* of the two
//! figures for each cell, so a stale checkpoint cannot undo a spend.

use crate::contribution::{CellId, CohortId};
use qip_core::error::{Error, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The privacy loss one release is priced at.
///
/// Bounded at both ends, and both ends prevent a specific failure.
///
/// **The floor.** Spend is accumulated in `f64`. An epsilon small enough to
/// vanish when added to the running total gives a caller unlimited releases
/// that cost nothing — the ledger stops moving while the answers keep coming,
/// which is the exact failure the ledger exists to prevent, arriving through
/// arithmetic rather than through a method call. [`Epsilon::MINIMUM`] is
/// thirteen orders of magnitude above the point where that starts, so the
/// accumulation is exact enough to be trusted for as long as the budget lasts.
///
/// **The ceiling.** Above it the noise is smaller than the quantity it is
/// supposed to hide and the mechanism is theatre. A caller who wants a
/// near-exact answer should be made to write down a number a reviewer will
/// query rather than a number that quietly disables the protection.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct Epsilon(f64);

impl Epsilon {
    /// Below this, the ledger's running total stops noticing the spend.
    pub const MINIMUM: f64 = 1e-3;
    /// Above this, the noise no longer hides anything worth hiding.
    pub const MAXIMUM: f64 = 4.0;

    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(Error::invalid("epsilon must be a finite number"));
        }
        if value < Self::MINIMUM {
            return Err(Error::invalid(format!(
                "epsilon {value} is below the floor of {}; a spend that small rounds away in the \
                 ledger's running total, which makes the budget unenforceable rather than generous",
                Self::MINIMUM
            )));
        }
        if value > Self::MAXIMUM {
            return Err(Error::invalid(format!(
                "epsilon {value} is above the ceiling of {}; at that scale the noise is smaller \
                 than the quantity it is meant to hide",
                Self::MAXIMUM
            )));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Epsilon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The total privacy loss any one cell may be exposed to before the fabric
/// stops answering.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct Budget(f64);

impl Budget {
    /// One unit of privacy loss per cell, per ledger.
    ///
    /// The defensible reading of this default is not "epsilon of one is safe";
    /// it is "a cell will be asked about a handful of times at a meaningful
    /// noise scale, and then this fabric will stop". A deployment that wants
    /// more releases has to write down a larger number and defend it.
    pub const DEFAULT: f64 = 1.0;
    /// Past this the guarantee is arithmetic rather than meaningful. The
    /// ceiling exists so that switching the control off has to be written as a
    /// number a reviewer will notice, rather than as `f64::MAX`.
    pub const MAXIMUM: f64 = 32.0;

    pub fn new(total: f64) -> Result<Self> {
        if !total.is_finite() {
            return Err(Error::invalid("a budget must be a finite number"));
        }
        if total < Epsilon::MINIMUM {
            return Err(Error::invalid(format!(
                "a budget of {total} is below the smallest spend that can be charged ({}), so no \
                 release could ever be made",
                Epsilon::MINIMUM
            )));
        }
        if total > Self::MAXIMUM {
            return Err(Error::invalid(format!(
                "a budget of {total} is above the ceiling of {}; a budget that large does not \
                 bound anything",
                Self::MAXIMUM
            )));
        }
        Ok(Self(total))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// Privacy loss a cell has already been exposed to.
///
/// A number that was **charged**, not asserted. The field is private and there
/// is no constructor: [`PrivacyLedger::spent`] is the only expression in the
/// platform that produces one, and it produces the ledger's own running total.
/// That is the whole defence against the cheapest way to get one more answer
/// out of an exhausted cohort, which is to write the spend back down.
///
/// ```compile_fail
/// # use qip_confidential::budget::SpentEpsilon;
/// // No constructor, and the field is private: a spend cannot be asserted,
/// // only charged.
/// let forged = SpentEpsilon(0.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct SpentEpsilon(f64);

impl SpentEpsilon {
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl fmt::Display for SpentEpsilon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A snapshot of what has been spent, for checkpointing and for the audit
/// trail.
///
/// `Serialize` and not `Deserialize`. Reading one of these back in would be the
/// missing constructor for [`SpentEpsilon`] and for the ledger itself: a
/// serialized report with the figures edited down is a reset with extra steps.
/// [`PrivacyLedger::absorb`] takes the report type directly, and takes the
/// larger of the two figures, so the only thing a caller can do with a report
/// is raise a spend.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SpendReport {
    per_cell: BTreeMap<CellId, f64>,
    releases_per_cell: BTreeMap<CellId, usize>,
    per_cohort: BTreeMap<CohortId, f64>,
}

impl SpendReport {
    pub fn cells(&self) -> impl Iterator<Item = (&CellId, f64)> {
        self.per_cell.iter().map(|(cell, spent)| (cell, *spent))
    }

    pub fn spent(&self, cell: &CellId) -> f64 {
        self.per_cell.get(cell).copied().unwrap_or(0.0)
    }

    pub fn releases(&self, cell: &CellId) -> usize {
        self.releases_per_cell.get(cell).copied().unwrap_or(0)
    }

    pub fn cohorts(&self) -> impl Iterator<Item = (&CohortId, f64)> {
        self.per_cohort
            .iter()
            .map(|(cohort, spent)| (cohort, *spent))
    }
}

/// Every cell's cumulative privacy loss. It only goes up.
///
/// There is no `reset`, no `refund`, no `set` and no `clear`, and that is the
/// entire mechanism. [`Self::spend`] is the only mutator that charges, and
/// [`Self::absorb`] is the only other mutator at all — it takes the maximum of
/// two figures, so it cannot lower one either.
#[derive(Clone, Debug, PartialEq)]
pub struct PrivacyLedger {
    budget: Budget,
    per_cell: BTreeMap<CellId, f64>,
    releases_per_cell: BTreeMap<CellId, usize>,
    per_cohort: BTreeMap<CohortId, f64>,
}

impl PrivacyLedger {
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            per_cell: BTreeMap::new(),
            releases_per_cell: BTreeMap::new(),
            per_cohort: BTreeMap::new(),
        }
    }

    pub const fn budget(&self) -> Budget {
        self.budget
    }

    /// Charge one release to every cell that went into it.
    ///
    /// Checked in full before anything is applied. A partial charge — some
    /// cells debited, then a refusal — would leave the ledger describing a
    /// release that never happened, and the only honest way to correct it
    /// would be to lower a figure, which is the one thing this type does not
    /// do.
    pub fn spend(
        &mut self,
        cells: &BTreeSet<CellId>,
        cohort: &CohortId,
        epsilon: Epsilon,
    ) -> Result<()> {
        if cells.is_empty() {
            return Err(Error::invalid(
                "a release with no contributing cells cannot be charged to anyone",
            ));
        }

        let mut exhausted = Vec::new();
        for cell in cells {
            let spent = self.per_cell.get(cell).copied().unwrap_or(0.0);
            if spent + epsilon.get() > self.budget.get() {
                exhausted.push(format!(
                    "{cell} (spent {spent}, remaining {})",
                    (self.budget.get() - spent).max(0.0)
                ));
            }
        }
        if !exhausted.is_empty() {
            return Err(Error::guard(format!(
                "the privacy budget of {} is exhausted for {} of {} cell(s): {}. A release costing \
                 {epsilon} cannot be charged, and the budget does not reset — these cells have \
                 already been asked about as often as the policy allows",
                self.budget.get(),
                exhausted.len(),
                cells.len(),
                exhausted.join(", ")
            )));
        }

        for cell in cells {
            let spent = self.per_cell.entry(cell.clone()).or_insert(0.0);
            *spent += epsilon.get();
            *self.releases_per_cell.entry(cell.clone()).or_insert(0) += 1;
        }
        *self.per_cohort.entry(cohort.clone()).or_insert(0.0) += epsilon.get();
        Ok(())
    }

    /// What a cell has been charged so far.
    pub fn spent(&self, cell: &CellId) -> SpentEpsilon {
        SpentEpsilon(self.per_cell.get(cell).copied().unwrap_or(0.0))
    }

    /// What is left before this cell stops being answerable about.
    pub fn remaining(&self, cell: &CellId) -> f64 {
        (self.budget.get() - self.spent(cell).get()).max(0.0)
    }

    /// Whether a release costing `epsilon` could be charged to every one of
    /// these cells. Reads the ledger; changes nothing.
    pub fn affordable(&self, cells: &BTreeSet<CellId>, epsilon: Epsilon) -> bool {
        !cells.is_empty()
            && cells
                .iter()
                .all(|cell| self.spent(cell).get() + epsilon.get() <= self.budget.get())
    }

    /// How many releases a cell has been part of. Reported rather than
    /// enforced: the epsilon total is what bounds the leakage, and a count of
    /// releases without their scales does not.
    pub fn releases(&self, cell: &CellId) -> usize {
        self.releases_per_cell.get(cell).copied().unwrap_or(0)
    }

    /// Spend attributed to a cohort label. Reporting only — see the module
    /// note on why the enforced account is the cell.
    pub fn spent_on_cohort(&self, cohort: &CohortId) -> SpentEpsilon {
        SpentEpsilon(self.per_cohort.get(cohort).copied().unwrap_or(0.0))
    }

    pub fn report(&self) -> SpendReport {
        SpendReport {
            per_cell: self.per_cell.clone(),
            releases_per_cell: self.releases_per_cell.clone(),
            per_cohort: self.per_cohort.clone(),
        }
    }

    /// Fold a checkpoint back in, taking the higher figure for each cell.
    ///
    /// The two ways the figures can disagree are a stale checkpoint and a
    /// release this process does not know it made. Taking the maximum is right
    /// for the first and safe for the second; taking the sum would double
    /// charge a cell for its own audit trail, and taking the checkpoint's
    /// figure would let a stale one undo a spend.
    pub fn absorb(&mut self, report: &SpendReport) {
        for (cell, spent) in &report.per_cell {
            let current = self.per_cell.entry(cell.clone()).or_insert(0.0);
            if *spent > *current {
                *current = *spent;
            }
        }
        for (cell, count) in &report.releases_per_cell {
            let current = self.releases_per_cell.entry(cell.clone()).or_insert(0);
            if *count > *current {
                *current = *count;
            }
        }
        for (cohort, spent) in &report.per_cohort {
            let current = self.per_cohort.entry(cohort.clone()).or_insert(0.0);
            if *spent > *current {
                *current = *spent;
            }
        }
    }
}
