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
        if source.trim().is_empty() {
            return Err(Error::invalid("a score is recorded against a named source"));
        }
        let score = SourceAlpha {
            at,
            incremental_gain: incremental_information_gain(signal, incumbent)?,
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
