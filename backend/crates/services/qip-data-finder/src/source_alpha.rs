//! What a source adds beyond what the platform already has, and what it
//! earned once used (DATA-015).
//!
//! [`crate::scoring::SourceScores`] prices a source before it is used:
//! reliability, freshness, a uniqueness estimated from coverage metadata,
//! cost. Two things only evidence can say were missing. The first is
//! *incremental* information — a duplicate of a registered source predicts
//! nothing the incumbent did not, however fresh and reliable it is. The
//! second is *realised* contribution: whether the source's signal was
//! followed by the outcome it predicted. Both are statistics over supplied
//! series, so `f64`, and nothing here is money or settles anything.
//!
//! Pure functions plus a bounded history. No clock, no I/O. Nothing in a
//! composition root feeds this yet: the outcome series must come from the
//! LEARN stage's attribution, and that join is not built.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Observations needed before a correlation means anything.
pub const MIN_OBSERVATIONS: usize = 8;

/// Scores kept per source. Older ones are dropped, oldest first.
pub const HISTORY_BOUND: usize = 256;

fn pearson(left: &[f64], right: &[f64], what: &str) -> Result<f64> {
    if left.len() != right.len() {
        return Err(Error::invalid(format!(
            "{what}: the two series have {} and {} observations; align them before scoring",
            left.len(),
            right.len()
        )));
    }
    if left.len() < MIN_OBSERVATIONS {
        return Err(Error::invalid(format!(
            "{what}: {} observations is fewer than the {MIN_OBSERVATIONS} a correlation needs",
            left.len()
        )));
    }
    if left.iter().chain(right).any(|value| !value.is_finite()) {
        return Err(Error::invalid(format!(
            "{what}: a series holds a non-finite value"
        )));
    }
    let n = left.len() as f64;
    let mean_left = left.iter().sum::<f64>() / n;
    let mean_right = right.iter().sum::<f64>() / n;
    let (mut covariance, mut var_left, mut var_right) = (0.0, 0.0, 0.0);
    for (a, b) in left.iter().zip(right) {
        covariance += (a - mean_left) * (b - mean_right);
        var_left += (a - mean_left).powi(2);
        var_right += (b - mean_right).powi(2);
    }
    if var_left == 0.0 || var_right == 0.0 {
        return Err(Error::invalid(format!(
            "{what}: a constant series has no variation to correlate; a source that never moves \
             is not scored as if it had been measured"
        )));
    }
    Ok((covariance / (var_left.sqrt() * var_right.sqrt())).clamp(-1.0, 1.0))
}

/// The share of `candidate`'s variation the `incumbent` does not already
/// carry, in `[0, 1]`: `1 - r²`. A perfect duplicate — or a rescaled, shifted
/// one — scores 0; an unrelated series scores near 1.
pub fn incremental_information_gain(candidate: &[f64], incumbent: &[f64]) -> Result<f64> {
    let r = pearson(candidate, incumbent, "incremental information gain")?;
    Ok(1.0 - r * r)
}

/// A perfect fit's `R²` falls short of one by roundoff only; an incumbent
/// its predecessors explain past this adds no column worth fitting.
const COLLINEAR_R_SQUARED: f64 = 1.0 - 1e-9;

/// The share of `candidate`'s variation that none of `incumbents` already
/// carries, together, in `[0, 1]`: `1 - R²` of `candidate` regressed on all
/// of them by ordinary least squares (R12-DATA-2022).
///
/// The pairwise [`incremental_information_gain`] measures a source against
/// one series. Measured against the own stream alone, two connectors that
/// duplicate each other both look new — each is independent of the stream
/// and neither is asked about the other — and the platform pays twice for
/// one piece of information. Here every series the platform already holds
/// is one column, so a duplicate of any of them, or of a combination of
/// them, scores near zero.
///
/// An incumbent the earlier columns already explain is left out rather than
/// fitted: it carries nothing the fit lacks, and its column would make the
/// normal equations singular. With one incumbent this equals the pairwise
/// gain. Refuses a constant candidate, misaligned series and too few
/// observations for the columns kept, as the pairwise score does.
pub fn incremental_information_gain_over(
    candidate: &[f64],
    incumbents: &[Vec<f64>],
) -> Result<f64> {
    let Some((first, rest)) = incumbents.split_first() else {
        return Err(Error::invalid(
            "incremental information gain: no incumbent series; a gain over nothing is not a measurement",
        ));
    };
    // The pairwise score checks lengths, finiteness, the minimum count and a
    // constant candidate; asking it first keeps those refusals word for word.
    let pairwise = incremental_information_gain(candidate, first)?;
    let mut kept: Vec<&[f64]> = vec![first.as_slice()];
    for series in rest {
        if series.len() != candidate.len() {
            return Err(Error::invalid(format!(
                "incremental information gain: an incumbent has {} observations and the \
                 candidate {}; align them before scoring",
                series.len(),
                candidate.len()
            )));
        }
        if series.iter().any(|v| !v.is_finite()) {
            return Err(Error::invalid(
                "incremental information gain: a series holds a non-finite value",
            ));
        }
        if r_squared(series, &kept)? < COLLINEAR_R_SQUARED {
            kept.push(series.as_slice());
        }
    }
    if kept.len() == 1 {
        return Ok(pairwise);
    }
    // R² is in [0, 1] by construction; the clamp removes roundoff only.
    Ok((1.0 - r_squared(candidate, &kept)?).clamp(0.0, 1.0))
}

/// `R²` of `y` on the columns, by ordinary least squares with an intercept.
fn r_squared(y: &[f64], columns: &[&[f64]]) -> Result<f64> {
    let rows: Vec<Vec<f64>> = (0..y.len())
        .map(|row| columns.iter().map(|column| column[row]).collect())
        .collect();
    let design = qip_numerics::matrix::Matrix::from_rows(&rows)?;
    Ok(qip_numerics::stats::ols(&design, y)?.r_squared)
}

/// How well the source's signal anticipated the outcome that followed it, in
/// `[-1, 1]` (the information coefficient). Negative means the source was
/// followed by the opposite of what it said, which is worth knowing and is
/// not clamped away.
pub fn realised_contribution(signal: &[f64], outcome: &[f64]) -> Result<f64> {
    pearson(signal, outcome, "realised contribution")
}

/// One evidence-based score of a source at an instant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceAlpha {
    pub at: Timestamp,
    pub incremental_gain: f64,
    pub realised_contribution: f64,
}

/// Each source's scores over time, bounded per source.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SourceAlphaHistory {
    by_source: BTreeMap<String, VecDeque<SourceAlpha>>,
}

impl SourceAlphaHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Score `source` against what the platform already has and against the
    /// outcome that followed, and keep the result.
    pub fn score(
        &mut self,
        source: &str,
        at: Timestamp,
        signal: &[f64],
        incumbent: &[f64],
        outcome: &[f64],
    ) -> Result<SourceAlpha> {
        self.score_over(source, at, signal, &[incumbent.to_vec()], outcome)
    }

    /// [`Self::score`] against every series the platform already holds, by
    /// [`incremental_information_gain_over`].
    pub fn score_over(
        &mut self,
        source: &str,
        at: Timestamp,
        signal: &[f64],
        incumbents: &[Vec<f64>],
        outcome: &[f64],
    ) -> Result<SourceAlpha> {
        if source.trim().is_empty() {
            return Err(Error::invalid("a score is recorded against a named source"));
        }
        let score = SourceAlpha {
            at,
            incremental_gain: incremental_information_gain_over(signal, incumbents)?,
            realised_contribution: realised_contribution(signal, outcome)?,
        };
        let series = self.by_source.entry(source.to_string()).or_default();
        if series.len() >= HISTORY_BOUND {
            series.pop_front();
        }
        series.push_back(score);
        Ok(score)
    }

    /// A source's scores, oldest first.
    pub fn history(&self, source: &str) -> impl Iterator<Item = &SourceAlpha> {
        self.by_source.get(source).into_iter().flatten()
    }
}
