//! What may be asked, and what makes two questions the same question.
//!
//! Three statistics, each with a sensitivity that can be written down in one
//! line, because a sensitivity nobody can write down is a noise scale nobody
//! can defend. Everything richer — a quantile, a correlation, a regression
//! across cells — has a sensitivity that depends on the data or on assumptions
//! this crate cannot check, and is deliberately absent rather than approximated.
//!
//! # The neighbour model, stated once
//!
//! Every sensitivity here is computed under one assumption about what a
//! neighbouring dataset is: **one cell's contribution is replaced by another
//! value in the declared range; the membership of the cohort is fixed and
//! public.**
//!
//! That is the honest model for this fabric, because the caller names the cells
//! it is asking about — membership is its own input, not a secret the fabric
//! could keep. What is secret is each cell's *number*. The consequence, stated
//! plainly because it is the kind of thing that gets assumed away: **this crate
//! does not hide which cells contributed to a release.** A cell that declines
//! to contribute is visible by its absence, and declining is therefore itself a
//! signal.

use crate::budget::Epsilon;
use crate::contribution::{CellId, CohortId};
use crate::noise::Sensitivity;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt;

/// The range a contribution is clamped into before it is used.
///
/// Declared by policy, from domain knowledge, before the data is seen. The
/// tempting alternative — take the range from the contributions — computes the
/// noise calibration from the two most disclosive statistics in the set (the
/// minimum and the maximum) and then publishes an answer calibrated by them.
///
/// Clamping costs something and it is not free to ignore: a contribution
/// outside the range is pulled to the edge, so a range chosen too narrow biases
/// the release toward the range. That is the trade. Widening the range to fit
/// the data is not the fix; it is the leak.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct Bounds {
    low: f64,
    high: f64,
}

impl Bounds {
    pub fn new(low: f64, high: f64) -> Result<Self> {
        if !low.is_finite() || !high.is_finite() {
            return Err(Error::invalid(
                "bounds must be finite; an infinite range is an unbounded sensitivity and an \
                 unbounded sensitivity cannot be noised",
            ));
        }
        if high <= low {
            return Err(Error::invalid(format!(
                "bounds [{low}, {high}] are not ordered; a range of zero width makes every \
                 contribution the same number and every release a constant"
            )));
        }
        if !(high - low).is_finite() {
            return Err(Error::invalid(format!(
                "bounds [{low}, {high}] have a width that overflows a float"
            )));
        }
        Ok(Self { low, high })
    }

    pub const fn low(self) -> f64 {
        self.low
    }

    pub const fn high(self) -> f64 {
        self.high
    }

    pub fn width(self) -> f64 {
        self.high - self.low
    }

    /// The largest absolute value a clamped contribution can take. Used to
    /// check, from public quantities alone, that a sum cannot overflow.
    pub fn largest_magnitude(self) -> f64 {
        self.low.abs().max(self.high.abs())
    }

    pub fn clamp(self, value: f64) -> f64 {
        value.clamp(self.low, self.high)
    }
}

impl fmt::Display for Bounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.low, self.high)
    }
}

/// The statistic a release reports.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Statistic {
    /// The total across contributing cells.
    Sum,
    /// The total divided by the contributor count.
    ///
    /// The divisor is the *true* count, un-noised, which is consistent with the
    /// neighbour model above: membership is public, so the count is not a
    /// secret this needs to protect. Under a model where a cell's presence is
    /// itself secret, this mean is **not** protected, because the denominator
    /// would then carry information the numerator's noise does not cover.
    Mean,
    /// How many contributing cells report a value above a threshold.
    ///
    /// A threshold sweep — the same count asked at fifty thresholds — is a
    /// legitimate-looking query family that reconstructs the whole
    /// distribution. Nothing here refuses it; every step spends budget from
    /// every cell in the cohort, so the sweep runs out. That is the only thing
    /// stopping it.
    CountAbove { threshold: f64 },
}

impl Statistic {
    /// A short stable tag, used in fingerprints and in refusal messages.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Mean => "mean",
            Self::CountAbove { .. } => "count_above",
        }
    }

    /// Refuses a statistic that cannot mean anything against these bounds.
    ///
    /// A threshold outside the declared range makes the count a constant — zero
    /// or the contributor count — for every possible dataset. Answering it
    /// would spend real budget on a number that carries no information, which
    /// is the worst possible trade and is almost always a caller's mistake
    /// about the units.
    pub fn validate(self, bounds: Bounds) -> Result<()> {
        match self {
            Self::Sum | Self::Mean => Ok(()),
            Self::CountAbove { threshold } => {
                if !threshold.is_finite() {
                    return Err(Error::invalid("a count threshold must be finite"));
                }
                if threshold < bounds.low() || threshold >= bounds.high() {
                    return Err(Error::invalid(format!(
                        "a threshold of {threshold} is outside the declared bounds {bounds}; \
                         after clamping the count would be the same number for every possible \
                         dataset, and answering it would spend budget on nothing"
                    )));
                }
                Ok(())
            }
        }
    }

    /// How much one cell can move this statistic, under the module's neighbour
    /// model.
    ///
    /// * `Sum` — the full width of the range: a cell can sit at either end.
    /// * `Mean` — the width over the contributor count, because the divisor is
    ///   fixed and public.
    /// * `CountAbove` — one: changing one cell's value flips at most one
    ///   indicator.
    pub fn sensitivity(self, bounds: Bounds, contributors: usize) -> Result<Sensitivity> {
        if contributors == 0 {
            return Err(Error::invalid(
                "a statistic over no contributors has no sensitivity and no meaning",
            ));
        }
        let value = match self {
            Self::Sum => bounds.width(),
            Self::Mean => bounds.width() / contributors as f64,
            Self::CountAbove { .. } => 1.0,
        };
        Sensitivity::new(value)
    }

    /// The true statistic over already-clamped values, in cell order.
    pub(crate) fn evaluate(self, values: &[f64]) -> Result<f64> {
        if values.is_empty() {
            return Err(Error::invalid("no contributions to evaluate"));
        }
        let result = match self {
            Self::Sum => values.iter().sum::<f64>(),
            Self::Mean => values.iter().sum::<f64>() / values.len() as f64,
            Self::CountAbove { threshold } => {
                values.iter().filter(|value| **value > threshold).count() as f64
            }
        };
        if !result.is_finite() {
            return Err(Error::numeric(format!(
                "the {} over {} contributions is not finite",
                self.tag(),
                values.len()
            )));
        }
        Ok(result)
    }

    /// The statistic's own bytes for the fingerprint, threshold included.
    fn fingerprint_bytes(self) -> Vec<u8> {
        let mut bytes = self.tag().as_bytes().to_vec();
        if let Self::CountAbove { threshold } = self {
            bytes.extend_from_slice(&threshold.to_bits().to_le_bytes());
        }
        bytes
    }
}

/// The identity of a question: which cells, which statistic, which range, at
/// which epsilon.
///
/// Two questions with the same fingerprint are the same question and get the
/// same answer. What is deliberately absent from it is documented in
/// [`Query::fingerprint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, for the audit trail.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

/// One question, ready to be asked of a [`crate::release::ReleaseGate`].
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    cohort: CohortId,
    statistic: Statistic,
    bounds: Bounds,
    epsilon: Epsilon,
}

impl Query {
    pub fn new(
        cohort: CohortId,
        statistic: Statistic,
        bounds: Bounds,
        epsilon: Epsilon,
    ) -> Result<Self> {
        statistic.validate(bounds)?;
        Ok(Self {
            cohort,
            statistic,
            bounds,
            epsilon,
        })
    }

    pub fn cohort(&self) -> &CohortId {
        &self.cohort
    }

    pub const fn statistic(&self) -> Statistic {
        self.statistic
    }

    pub const fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub const fn epsilon(&self) -> Epsilon {
        self.epsilon
    }

    /// The question's identity, over the cells that answered it.
    ///
    /// Included: the cells, the statistic and its parameters, the bounds, and
    /// epsilon. Every one of them changes either what is being measured or how
    /// hard it is being hidden.
    ///
    /// **Excluded: the cohort label.** It is a name a caller chose, and if it
    /// were in here then asking the identical question under two names would
    /// draw two independent noise values whose average has two-thirds the
    /// spread of either. Renaming a question is not asking a new one.
    pub fn fingerprint(&self, cells: &BTreeSet<CellId>) -> Fingerprint {
        let mut material = Vec::with_capacity(128);
        material.extend_from_slice(b"qip-confidential/question/v1");
        material.extend_from_slice(&self.statistic.fingerprint_bytes());
        material.extend_from_slice(&self.bounds.low().to_bits().to_le_bytes());
        material.extend_from_slice(&self.bounds.high().to_bits().to_le_bytes());
        material.extend_from_slice(&self.epsilon.get().to_bits().to_le_bytes());
        material.extend_from_slice(&(cells.len() as u64).to_le_bytes());
        for cell in cells {
            // Length-prefixed, so that {"ab", "c"} and {"a", "bc"} are not the
            // same question — two different cohorts sharing one fingerprint
            // would share one noise draw, and shared noise cancels.
            let bytes = cell.as_str().as_bytes();
            material.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            material.extend_from_slice(bytes);
        }
        Fingerprint(sha256(&material))
    }
}
