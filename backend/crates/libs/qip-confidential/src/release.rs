//! The release path: the one place an aggregate leaves, and the three gates it
//! passes on the way.
//!
//! A caller hands over a [`crate::query::Query`] and the cells' numbers, and
//! gets back a [`Release`] or a refusal. In order:
//!
//! 1. **The cohort threshold.** Fewer than [`Policy::min_contributors`]
//!    contributing cells and nothing is returned — no answer with a caveat, no
//!    answer with extra noise. A mean over two cells is very nearly the two
//!    numbers.
//! 2. **The differencing gate.** The threshold above is worth little on its
//!    own, because a caller who cannot ask about three cells directly can ask
//!    about eight and then about five, and subtract. So the threshold is
//!    applied to every *difference* a caller could form as well as to the query
//!    itself: two contributor sets whose symmetric difference is smaller than
//!    the threshold cannot both be answered.
//! 3. **The budget.** Every contributing cell is charged, and when a cell is
//!    out, questions that include it stop being answered. See
//!    [`crate::budget`].
//!
//! Then the statistic is computed over clamped values and noised at the scale
//! the sensitivity and epsilon imply.
//!
//! # A refusal never depends on the data
//!
//! Every refusal above is decided from things the caller already knows: which
//! cells are in the cohort, what has been asked before, what the ledger holds.
//! None of them looks at a value. That is deliberate — a refusal that depended
//! on the numbers would be a free answer, one bit per attempt, with no budget
//! charged for it. The one check that does look at a number, finiteness, is
//! done at the door in [`crate::contribution::Contribution::new`].
//!
//! # Where the differencing gate ends
//!
//! It compares the new query against every release **this gate** has recorded,
//! pairwise. Two limits follow, and both are real:
//!
//! * **Pairwise is not enough.** Three releases whose contributor sets are
//!   pairwise far apart can still isolate a single cell — the classic tracker:
//!   ask the whole set, then two disjoint halves of everything except one cell,
//!   and subtract. Every pair passes this gate and the combination recovers the
//!   cell. Nothing here detects it; the budget is what bounds it, and it bounds
//!   it only to the accuracy the noise allows. A test in
//!   `tests/differencing.rs` performs exactly this attack and shows it
//!   succeeding.
//! * **The history is the gate's own.** A second gate, or the same one after a
//!   restart, has no record of what was released before and will happily answer
//!   the other half of a differencing pair.

use crate::budget::{Budget, PrivacyLedger};
use crate::contribution::{CellId, CohortId, ContributionSet};
use crate::noise::{NoiseScale, noise_for, snap};
use crate::query::{Fingerprint, Query, Statistic};
use qip_core::error::{Error, Result};
use serde::Serialize;
use std::collections::BTreeSet;

/// The policy parameters a deployment sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Policy {
    min_contributors: usize,
    budget: Budget,
}

impl Policy {
    /// Five contributing cells, the floor used as standard practice in
    /// official statistics for suppressing a table cell.
    ///
    /// Worth being explicit about what this means in a seven-cell deployment,
    /// because the arithmetic is not obvious: with a threshold of five, any two
    /// answerable contributor sets drawn from seven cells differ by at most
    /// four cells, so the differencing gate admits **one** contributor set and
    /// refuses every other. In practice that is the intended shape — the
    /// fabric answers questions about the cells as a whole and refuses to carve
    /// them into regions. A deployment with more contributors gets more room;
    /// a deployment that wants regional slices has to lower the threshold and
    /// own what that means.
    pub const DEFAULT_MIN_CONTRIBUTORS: usize = 5;

    /// Below three, a threshold gates nothing. At two, either contributor
    /// subtracts its own number and reads the other's; at one there is nothing
    /// to hide behind at all. Three is not *safe* — two cells that talk to each
    /// other still recover the third — it is merely the smallest number that
    /// is not self-evidently pointless.
    pub const MINIMUM_CONTRIBUTORS: usize = 3;

    pub fn new(min_contributors: usize, budget: Budget) -> Result<Self> {
        if min_contributors < Self::MINIMUM_CONTRIBUTORS {
            return Err(Error::invalid(format!(
                "a cohort threshold of {min_contributors} does not gate anything; below {} a \
                 contributor recovers another contributor's number by subtracting its own",
                Self::MINIMUM_CONTRIBUTORS
            )));
        }
        Ok(Self {
            min_contributors,
            budget,
        })
    }

    pub const fn min_contributors(self) -> usize {
        self.min_contributors
    }

    pub const fn budget(self) -> Budget {
        self.budget
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_contributors: Self::DEFAULT_MIN_CONTRIBUTORS,
            budget: Budget::default(),
        }
    }
}

/// Where a release sits in one gate's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ReleaseId(u64);

impl ReleaseId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ReleaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "release {}", self.0)
    }
}

/// An aggregate that was allowed out, and everything a reader needs to know how
/// far to trust it.
///
/// The noise scale is on the record deliberately. A released number without its
/// scale invites being read as exact, and the whole point of this crate is that
/// it is not: the value is the truth plus a draw whose standard deviation is
/// [`Release::standard_deviation`]. Publishing the scale costs nothing — it is
/// computed from public quantities — and withholding it would only mislead the
/// people entitled to the answer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Release {
    id: ReleaseId,
    cohort: CohortId,
    statistic: Statistic,
    contributors: usize,
    value: f64,
    noise_scale: f64,
    epsilon: f64,
    fingerprint: Fingerprint,
}

impl Release {
    pub const fn id(&self) -> ReleaseId {
        self.id
    }

    pub fn cohort(&self) -> &CohortId {
        &self.cohort
    }

    pub const fn statistic(&self) -> Statistic {
        self.statistic
    }

    /// How many cells went into it. Never below the policy threshold.
    pub const fn contributors(&self) -> usize {
        self.contributors
    }

    /// The released figure: the statistic plus calibrated noise.
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// The `b` of the Laplace draw that was added.
    pub const fn noise_scale(&self) -> f64 {
        self.noise_scale
    }

    /// The spread of the noise on this figure, which is what a reader should
    /// quote alongside it.
    pub fn standard_deviation(&self) -> f64 {
        self.noise_scale * std::f64::consts::SQRT_2
    }

    /// What this release cost every cell that went into it.
    pub const fn epsilon(&self) -> f64 {
        self.epsilon
    }

    pub const fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }
}

/// One release as the gate remembers it: the answer, plus the contributor set
/// the differencing gate needs to compare future questions against.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseRecord {
    cells: BTreeSet<CellId>,
    release: Release,
}

impl ReleaseRecord {
    pub fn cells(&self) -> &BTreeSet<CellId> {
        &self.cells
    }

    pub fn release(&self) -> &Release {
        &self.release
    }
}

/// The thing that answers, or refuses.
///
/// One gate is one observation period over one body of data. That is not a
/// naming convention, it is the unit the budget is denominated in: the ledger
/// bounds what may be learned about a cell from *this* gate's releases, and a
/// second gate is a second budget.
///
/// Which leaves the honest caveat about time. If the underlying quantity has
/// genuinely changed — a new day's flow — then a new gate with a new budget is
/// right, because it is a different secret. If it has barely moved, it is one
/// secret measured twice and a fresh budget prices it as two. **This crate
/// cannot tell the difference and will not stop you.** A slowly varying
/// quantity asked about every hour for a week is a reconstruction, however
/// carefully each hour was budgeted.
///
/// `Debug` is written by hand rather than derived. The seed is key material —
/// see the module documentation on [`crate::noise`] — and a derived `Debug`
/// would print it in full the first time anything logs a gate for
/// troubleshooting, which is exactly the leak the crate's own threat model
/// names as undefended. The same convention is used for
/// `qip_brokers::credential::Secret` and `VenueCredential`, for the same
/// reason: a value that recovers every true release exactly must not be a
/// field a derive macro is free to print.
pub struct ReleaseGate {
    policy: Policy,
    seed: u64,
    ledger: PrivacyLedger,
    records: Vec<ReleaseRecord>,
    next_id: u64,
}

impl std::fmt::Debug for ReleaseGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseGate")
            .field("policy", &self.policy)
            .field("seed", &"<redacted>")
            .field("ledger", &self.ledger)
            .field("records", &self.records)
            .field("next_id", &self.next_id)
            .finish()
    }
}

impl ReleaseGate {
    /// A gate for one observation period.
    ///
    /// **Give each period its own seed.** Reusing one across periods makes the
    /// noise on a repeated question identical in both, so the difference of the
    /// two releases has no noise in it at all and hands over the change in the
    /// underlying value exactly. Using a fresh seed instead makes the two draws
    /// independent — which is right, and which is also what makes averaging
    /// across periods worth an attacker's while. Both directions are hazards;
    /// the fresh seed is the lesser one, and the budget does not cross periods
    /// to help.
    pub fn new(policy: Policy, seed: u64) -> Self {
        Self {
            policy,
            seed,
            ledger: PrivacyLedger::new(policy.budget()),
            records: Vec::new(),
            next_id: 1,
        }
    }

    pub const fn policy(&self) -> Policy {
        self.policy
    }

    pub const fn ledger(&self) -> &PrivacyLedger {
        &self.ledger
    }

    /// Everything this gate has let out, oldest first.
    pub fn records(&self) -> &[ReleaseRecord] {
        &self.records
    }

    /// Fold a checkpointed spend report back into this gate's ledger. Figures
    /// only ever rise; see [`PrivacyLedger::absorb`].
    pub fn absorb(&mut self, report: &crate::budget::SpendReport) {
        self.ledger.absorb(report);
    }

    /// Would this question be answered? Reads the gate; changes nothing.
    ///
    /// Exposed so a caller can plan a set of questions against a budget instead
    /// of discovering the refusal one release at a time. It reveals nothing a
    /// refusal would not have revealed, because no refusal here depends on the
    /// data.
    pub fn admits(&self, query: &Query, contributions: &ContributionSet) -> Result<()> {
        let cells: BTreeSet<CellId> = contributions.cells().cloned().collect();
        let fingerprint = query.fingerprint(&cells);
        if self.recorded(&fingerprint).is_some() {
            return Ok(());
        }
        self.check(query, &cells)
    }

    /// Answer the question, or refuse it.
    ///
    /// A repeat of a question this gate has already answered returns that same
    /// release and charges nothing. The answer would have been identical
    /// anyway — the noise is a function of the question — so the record is not
    /// there to keep the answer stable; it is there so the second ask is not
    /// charged for information it did not receive.
    pub fn release(&mut self, query: &Query, contributions: &ContributionSet) -> Result<Release> {
        let cells: BTreeSet<CellId> = contributions.cells().cloned().collect();
        let fingerprint = query.fingerprint(&cells);

        if let Some(record) = self.recorded(&fingerprint) {
            return Ok(record.release.clone());
        }

        self.check(query, &cells)?;

        let contributors = cells.len();
        let bounds = query.bounds();
        let sensitivity = query.statistic().sensitivity(bounds, contributors)?;
        let scale = NoiseScale::calibrate(sensitivity, query.epsilon())?;

        // Charged before the answer is computed, so that no path exists on
        // which a value is produced and the ledger does not know about it.
        self.ledger.spend(&cells, query.cohort(), query.epsilon())?;

        let values = contributions.clamped(bounds);
        let truth = query.statistic().evaluate(&values)?;
        let noisy = snap(truth + noise_for(self.seed, &fingerprint, scale), scale);
        if !noisy.is_finite() {
            return Err(Error::numeric(
                "the noised release is not finite; the budget has been charged and nothing is \
                 released",
            ));
        }

        let release = Release {
            id: ReleaseId(self.next_id),
            cohort: query.cohort().clone(),
            statistic: query.statistic(),
            contributors,
            value: noisy,
            noise_scale: scale.get(),
            epsilon: query.epsilon().get(),
            fingerprint,
        };
        self.next_id += 1;
        self.records.push(ReleaseRecord {
            cells,
            release: release.clone(),
        });
        Ok(release)
    }

    fn recorded(&self, fingerprint: &Fingerprint) -> Option<&ReleaseRecord> {
        self.records
            .iter()
            .find(|record| record.release.fingerprint == *fingerprint)
    }

    /// The three gates, in the order that keeps a refusal free of data.
    fn check(&self, query: &Query, cells: &BTreeSet<CellId>) -> Result<()> {
        self.check_threshold(cells)?;
        self.check_differencing(cells)?;
        self.check_arithmetic(query, cells.len())?;
        self.check_budget(query, cells)
    }

    fn check_threshold(&self, cells: &BTreeSet<CellId>) -> Result<()> {
        let contributors = cells.len();
        if contributors < self.policy.min_contributors {
            return Err(Error::guard(format!(
                "refused: an aggregate over {contributors} cell(s) is below the cohort threshold \
                 of {}. A statistic over that few contributors is close enough to the \
                 contributions themselves that releasing it releases them",
                self.policy.min_contributors
            )));
        }
        Ok(())
    }

    fn check_differencing(&self, cells: &BTreeSet<CellId>) -> Result<()> {
        let threshold = self.policy.min_contributors;
        for record in &self.records {
            let difference = record.cells.symmetric_difference(cells).count();
            if difference == 0 || difference >= threshold {
                continue;
            }
            let differing: Vec<String> = record
                .cells
                .symmetric_difference(cells)
                .map(CellId::to_string)
                .collect();
            return Err(Error::guard(format!(
                "refused: this cohort of {} cell(s) differs from {} (over {} cell(s)) by only {} \
                 cell(s) — {}. Subtracting the two answers is an aggregate over those cells, \
                 which is below the cohort threshold of {threshold}; the threshold applies to \
                 differences a caller can compute as well as to questions a caller can ask",
                cells.len(),
                record.release.id,
                record.cells.len(),
                difference,
                differing.join(", ")
            )));
        }
        Ok(())
    }

    /// Refuses, from public quantities only, a request whose arithmetic cannot
    /// be carried out in a float. Checked against the bounds and the
    /// contributor count rather than against the contributions, so that the
    /// refusal still says nothing about the data.
    fn check_arithmetic(&self, query: &Query, contributors: usize) -> Result<()> {
        let largest = contributors as f64 * query.bounds().largest_magnitude();
        if !largest.is_finite() {
            return Err(Error::numeric(format!(
                "{contributors} contributions bounded by {} can sum past what a float holds",
                query.bounds()
            )));
        }
        Ok(())
    }

    fn check_budget(&self, query: &Query, cells: &BTreeSet<CellId>) -> Result<()> {
        if self.ledger.affordable(cells, query.epsilon()) {
            return Ok(());
        }
        // Re-run the spend's own accounting to produce the message, without
        // mutating anything: the ledger is the authority on what is left.
        let mut probe = self.ledger.clone();
        probe.spend(cells, query.cohort(), query.epsilon())
    }
}
