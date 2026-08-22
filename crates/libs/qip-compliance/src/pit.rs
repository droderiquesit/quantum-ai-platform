//! Control 1 — point-in-time truth.
//!
//! Every backtest that beats its live counterpart does so for one of two
//! reasons, and by far the more common is that the research read a fact the
//! platform could not have known yet. [`PointInTime`] removes the opportunity
//! rather than warning about it: the reader is built by *discarding* facts
//! whose known-time is after the as-of, so the unknowable values are not in
//! the structure at all. There is no accessor that could return one because
//! there is nothing to return.
//!
//! That is the difference between this and a filter applied on read. A filter
//! is one forgotten call site away from leaking; a reader that never held the
//! future cannot leak however it is called.
//!
//! [`LeakageDetector`] covers the inputs that arrive from outside a reader —
//! a feature vector assembled by hand, a joined dataset, a model's covariates.
//! It answers "would this input have been knowable" for a set of stamped
//! values and names the ones that would not.

use qip_contracts::time::Stamped;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A reader over stamped facts that can only see what was knowable at `as_of`.
///
/// The as-of is fixed at construction and can only be moved *earlier*
/// ([`PointInTime::restrict_to`]). There is deliberately no method to widen it
/// and no method that returns a fact without regard to it — widening a reader
/// in place is indistinguishable, at the call site, from reading the future.
#[derive(Debug, Clone)]
pub struct PointInTime<T> {
    /// Only facts with `known_at <= as_of`, ordered by valid-time then
    /// known-time. Private: the invariant is that nothing else ever gets in.
    facts: Vec<Stamped<T>>,
    as_of: Timestamp,
    /// How many candidate facts were dropped as unknowable. A count, never a
    /// value — the number is a diagnostic, the values are the hazard.
    withheld: usize,
}

impl<T> PointInTime<T> {
    /// Build a reader as of a moment, discarding everything not yet knowable.
    ///
    /// Discarding at construction rather than filtering on read is the whole
    /// control: a reader physically does not hold the future.
    pub fn as_of(as_of: Timestamp, facts: impl IntoIterator<Item = Stamped<T>>) -> Self {
        let mut kept = Vec::new();
        let mut withheld = 0usize;
        for fact in facts {
            if fact.was_known_by(as_of) {
                kept.push(fact);
            } else {
                withheld += 1;
            }
        }
        kept.sort_by_key(|f| (f.valid_at().as_nanos(), f.known_at().as_nanos()));
        Self {
            facts: kept,
            as_of,
            withheld,
        }
    }

    /// The moment this reader reasons as of.
    pub const fn horizon(&self) -> Timestamp {
        self.as_of
    }

    /// How many facts were withheld as unknowable at [`PointInTime::horizon`].
    ///
    /// Non-zero is normal — it means the underlying store holds later facts.
    /// It is worth surfacing because a reader that withholds everything is
    /// usually an as-of that was computed wrongly rather than a quiet dataset.
    pub const fn withheld(&self) -> usize {
        self.withheld
    }

    /// Facts that were knowable, oldest valid-time first.
    pub fn known(&self) -> &[Stamped<T>] {
        &self.facts
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// The most recent knowable fact, by valid-time.
    pub fn latest(&self) -> Option<&Stamped<T>> {
        self.facts.last()
    }

    /// The most recent knowable fact, or an error naming the horizon.
    ///
    /// The error names the as-of because "no data" and "no data *yet*" are
    /// different bugs and the caller cannot tell them apart otherwise.
    pub fn require_latest(&self) -> Result<&Stamped<T>> {
        self.latest().ok_or_else(|| {
            Error::not_found(format!(
                "no fact was knowable as of {} ({} later fact(s) withheld)",
                self.as_of, self.withheld
            ))
        })
    }

    /// The fact in force at a valid-time: the latest knowable fact whose
    /// valid-time is at or before `valid_at`.
    ///
    /// Both times are honoured. Filtering on valid-time alone is the classic
    /// leak; filtering on known-time alone answers a different question.
    pub fn in_force_at(&self, valid_at: Timestamp) -> Option<&Stamped<T>> {
        self.facts.iter().rev().find(|f| f.valid_at() <= valid_at)
    }

    /// [`PointInTime::in_force_at`], or an error naming both times.
    pub fn require_in_force_at(&self, valid_at: Timestamp) -> Result<&Stamped<T>> {
        self.in_force_at(valid_at).ok_or_else(|| {
            Error::not_found(format!(
                "nothing was in force at {valid_at} that was knowable by {}",
                self.as_of
            ))
        })
    }

    /// Narrow the reader to an earlier as-of.
    ///
    /// Only narrowing exists. A widening method would let a caller holding a
    /// reader promote it into one that sees the future, which is exactly the
    /// mistake the type is built to prevent — so the operation is refused, and
    /// the refusal names both horizons.
    pub fn restrict_to(self, earlier: Timestamp) -> Result<Self> {
        if earlier > self.as_of {
            return Err(Error::guard(format!(
                "a point-in-time reader as of {} cannot be widened to {earlier}; \
                 build a new reader if a later horizon is genuinely intended",
                self.as_of
            )));
        }
        Ok(Self::as_of(earlier, self.facts))
    }

    /// The worst known-time lag among the facts this reader can see.
    ///
    /// A statistic about the feed, not about a value, so it is safe to expose:
    /// it is computed only from facts that were already knowable.
    pub fn worst_latency(&self) -> Duration {
        self.facts
            .iter()
            .map(Stamped::latency)
            .max()
            .unwrap_or(Duration::ZERO)
    }
}

/// One input that would not have been knowable at the as-of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeakageFinding {
    /// What the input was called, so the finding points at a column or feature
    /// rather than at an anonymous row.
    pub input: String,
    pub known_at: Timestamp,
    pub as_of: Timestamp,
    /// How far into the future the input reaches.
    pub ahead_by: Duration,
}

impl LeakageFinding {
    pub fn describe(&self) -> String {
        format!(
            "input `{}` became known at {} which is {:?} after the as-of {}",
            self.input, self.known_at, self.ahead_by, self.as_of
        )
    }
}

/// What an audit of a set of inputs concluded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeakageReport {
    inspected: usize,
    findings: Vec<LeakageFinding>,
}

impl LeakageReport {
    pub fn inspected(&self) -> usize {
        self.inspected
    }

    pub fn findings(&self) -> &[LeakageFinding] {
        &self.findings
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Turn a dirty report into an error naming every leaked input.
    ///
    /// Naming all of them rather than the first: a fix that addresses one leak
    /// and leaves three is worse than no fix, because the run now looks clean.
    pub fn require_clean(&self) -> Result<()> {
        if self.findings.is_empty() {
            return Ok(());
        }
        let detail: Vec<String> = self.findings.iter().map(LeakageFinding::describe).collect();
        Err(Error::guard(format!(
            "{} of {} inputs would not have been knowable: {}",
            self.findings.len(),
            self.inspected,
            detail.join("; ")
        )))
    }
}

/// Checks that a set of inputs would have been knowable at an as-of.
///
/// The reader above protects data that comes through it. This protects data
/// that does not — a hand-assembled feature vector, a join whose right side
/// carries its own timestamps, a covariate someone added to a model.
#[derive(Clone, Copy, Debug)]
pub struct LeakageDetector {
    as_of: Timestamp,
}

impl LeakageDetector {
    pub const fn new(as_of: Timestamp) -> Self {
        Self { as_of }
    }

    pub const fn as_of(&self) -> Timestamp {
        self.as_of
    }

    /// Inspect one input. `None` means it was knowable.
    pub fn inspect<T>(&self, input: &str, fact: &Stamped<T>) -> Option<LeakageFinding> {
        if fact.was_known_by(self.as_of) {
            return None;
        }
        Some(LeakageFinding {
            input: input.to_string(),
            known_at: fact.known_at(),
            as_of: self.as_of,
            ahead_by: fact.known_at().since(self.as_of),
        })
    }

    /// Inspect a labelled set of inputs.
    pub fn audit<'a, T: 'a>(
        &self,
        inputs: impl IntoIterator<Item = (&'a str, &'a Stamped<T>)>,
    ) -> LeakageReport {
        let mut report = LeakageReport::default();
        for (label, fact) in inputs {
            report.inspected += 1;
            if let Some(finding) = self.inspect(label, fact) {
                report.findings.push(finding);
            }
        }
        report
    }
}
