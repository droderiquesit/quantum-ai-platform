//! A learned embedding of market state (R12-TICK-2064, ADR 0182 decision 2).
//!
//! The fit is a principal-component projection computed in-tree: standardise
//! each feature, take the covariance, extract the leading directions by power
//! iteration with deflation. No new crate, no randomness: the same rows give
//! the same bytes, so the artefact's digest is a fact about the data rather
//! than about a run.
//!
//! **An embedding that has not beaten something is a claim nobody checked.**
//! [`StateEmbedder::evaluate`] runs a retrieval task on held-out states: find
//! the nearest training state in embedding space and read its recorded
//! outcome. The baseline is the same retrieval over the first `dimensions`
//! standardised raw features, the cheapest thing that needs no fit.
//! [`StateEmbedder::issue_card`] refuses to produce a card unless the
//! embedding's error is strictly lower, so an artefact that does not
//! outperform raw features cannot reach the registry through this door.
//!
//! The statistics are `f64`; nothing here is money.

use crate::registry::{EvaluationRecord, ModelCard, ModelClass};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{ModelId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Power-iteration steps per component. Fixed, so the fit is a function of
/// the rows alone.
const ITERATIONS: usize = 500;
/// An eigenvalue at or below this is a direction the data does not have;
/// asking for it would return noise dressed as a feature.
const MIN_EIGENVALUE: f64 = 1e-9;

/// The fitted projection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateEmbedder {
    features: Vec<String>,
    mean: Vec<f64>,
    scale: Vec<f64>,
    /// `dimensions` unit vectors, each as long as `features`.
    components: Vec<Vec<f64>>,
}

/// What the embedding scored against its baseline on held-out states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingEvaluation {
    pub held_out: usize,
    /// Mean absolute error of the outcome read from the embedding's nearest
    /// neighbour.
    pub embedding_error: f64,
    /// The same, retrieving over the first `dimensions` standardised raw
    /// features.
    pub baseline_error: f64,
}

impl EmbeddingEvaluation {
    /// Strictly lower error than the baseline. A tie is not a win.
    pub fn beats_baseline(&self) -> bool {
        self.embedding_error < self.baseline_error
    }
}

impl StateEmbedder {
    /// Fit `dimensions` components to `rows` (one state per row, one column
    /// per name in `features`).
    ///
    /// Refuses rather than repairs: ragged or non-finite rows, a constant
    /// column (it has no scale to standardise by), too few rows for the
    /// requested dimensions, and a request for more directions than the data
    /// has.
    pub fn fit(features: Vec<String>, rows: &[Vec<f64>], dimensions: usize) -> Result<Self> {
        let width = features.len();
        if dimensions == 0 || dimensions > width {
            return Err(Error::invalid(format!(
                "asked for {dimensions} dimensions of {width} features; ask for 1 to {width}"
            )));
        }
        if rows.len() <= width {
            return Err(Error::invalid(format!(
                "{} rows cannot fit a covariance over {width} features; supply more than {width}",
                rows.len()
            )));
        }
        for (index, row) in rows.iter().enumerate() {
            if row.len() != width || row.iter().any(|v| !v.is_finite()) {
                return Err(Error::invalid(format!(
                    "row {index} is ragged or not finite; every row needs {width} finite values"
                )));
            }
        }
        let n = rows.len() as f64;
        let mean: Vec<f64> = (0..width)
            .map(|c| rows.iter().map(|r| r[c]).sum::<f64>() / n)
            .collect();
        let mut scale = Vec::with_capacity(width);
        for c in 0..width {
            let var = rows.iter().map(|r| (r[c] - mean[c]).powi(2)).sum::<f64>() / n;
            if var <= 0.0 {
                return Err(Error::invalid(format!(
                    "feature `{}` is constant over the rows; drop it, it carries no state",
                    features[c]
                )));
            }
            scale.push(var.sqrt());
        }
        let z =
            |r: &[f64]| -> Vec<f64> { (0..width).map(|c| (r[c] - mean[c]) / scale[c]).collect() };
        let mut cov = vec![vec![0.0; width]; width];
        for r in rows {
            let v = z(r);
            for i in 0..width {
                for j in 0..width {
                    cov[i][j] += v[i] * v[j] / n;
                }
            }
        }
        let mut components: Vec<Vec<f64>> = Vec::with_capacity(dimensions);
        for k in 0..dimensions {
            // A start that cannot be orthogonal to every eigenvector in
            // practice, and is the same every run.
            let mut v: Vec<f64> = (0..width).map(|i| 1.0 + (i + k) as f64 * 0.37).collect();
            normalise(&mut v)?;
            let mut eigenvalue = 0.0;
            for _ in 0..ITERATIONS {
                let mut next: Vec<f64> = (0..width)
                    .map(|i| (0..width).map(|j| cov[i][j] * v[j]).sum())
                    .collect();
                for prior in &components {
                    let proj: f64 = next.iter().zip(prior).map(|(a, b)| a * b).sum();
                    for (x, p) in next.iter_mut().zip(prior) {
                        *x -= proj * p;
                    }
                }
                eigenvalue = norm(&next);
                if eigenvalue <= MIN_EIGENVALUE {
                    return Err(Error::invalid(format!(
                        "the data has no direction {} of variance; ask for fewer than {dimensions} \
                         dimensions",
                        k + 1
                    )));
                }
                for x in &mut next {
                    *x /= eigenvalue;
                }
                v = next;
            }
            if eigenvalue <= MIN_EIGENVALUE {
                return Err(Error::invalid("a component collapsed during the fit"));
            }
            components.push(v);
        }
        Ok(Self {
            features,
            mean,
            scale,
            components,
        })
    }

    pub fn dimensions(&self) -> usize {
        self.components.len()
    }

    /// The fitted directions, each a unit vector over the standardised
    /// features. Read-only so a test or reviewer can check they are
    /// orthogonal: two copies of one direction is a 2-d embedding of 1-d
    /// information.
    pub fn components(&self) -> &[Vec<f64>] {
        &self.components
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    fn standardise(&self, state: &[f64]) -> Result<Vec<f64>> {
        if state.len() != self.features.len() || state.iter().any(|v| !v.is_finite()) {
            return Err(Error::invalid(format!(
                "a state needs {} finite values, one per feature; got {}",
                self.features.len(),
                state.len()
            )));
        }
        Ok((0..state.len())
            .map(|c| (state[c] - self.mean[c]) / self.scale[c])
            .collect())
    }

    /// The embedding of one market state.
    pub fn embed(&self, state: &[f64]) -> Result<Vec<f64>> {
        let z = self.standardise(state)?;
        Ok(self
            .components
            .iter()
            .map(|c| c.iter().zip(&z).map(|(a, b)| a * b).sum())
            .collect())
    }

    /// SHA-256 over the artefact's serialised form: what a card binds to.
    pub fn digest(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| Error::invalid(format!("embedding artefact did not serialise: {e}")))?;
        Ok(sha256_hex(&bytes))
    }

    /// Score the embedding against its baseline: for each `held_out` state,
    /// read the outcome of its nearest `train` state, once in embedding space
    /// and once over the first `dimensions` standardised raw features.
    /// Each pair is `(state, outcome)`.
    pub fn evaluate(
        &self,
        train: &[(Vec<f64>, f64)],
        held_out: &[(Vec<f64>, f64)],
    ) -> Result<EmbeddingEvaluation> {
        if train.is_empty() || held_out.is_empty() {
            return Err(Error::invalid(
                "an evaluation needs training and held-out states; both were not supplied",
            ));
        }
        let k = self.dimensions();
        let mut indexed = Vec::with_capacity(train.len());
        for (state, outcome) in train {
            let z = self.standardise(state)?;
            indexed.push((self.embed(state)?, z[..k].to_vec(), *outcome));
        }
        let (mut emb_err, mut base_err) = (0.0, 0.0);
        for (state, outcome) in held_out {
            let z = self.standardise(state)?;
            let e = self.embed(state)?;
            let by_embedding = nearest(&indexed, |t| distance(&t.0, &e));
            let by_baseline = nearest(&indexed, |t| distance(&t.1, &z[..k]));
            emb_err += (by_embedding - outcome).abs();
            base_err += (by_baseline - outcome).abs();
        }
        let n = held_out.len() as f64;
        Ok(EmbeddingEvaluation {
            held_out: held_out.len(),
            embedding_error: emb_err / n,
            baseline_error: base_err / n,
        })
    }

    /// The card for this embedding, or a refusal.
    ///
    /// Refuses unless `evaluation` beats its baseline and `dimensions`
    /// matches this embedding. The card names the artefact digest, the
    /// baseline it was judged against, and says in `limitations` what the
    /// wire allows it for: retrieval and features, never a decision.
    pub fn issue_card(
        &self,
        model_id: ModelId,
        version: &str,
        owner: &str,
        dataset: &str,
        evaluation: &EmbeddingEvaluation,
        at: Timestamp,
    ) -> Result<ModelCard> {
        if !evaluation.beats_baseline() {
            return Err(Error::denied(format!(
                "no card is issued: the embedding's error {} does not beat the raw-feature \
                 baseline's {}; an embedding that outperforms nothing is not a model",
                evaluation.embedding_error, evaluation.baseline_error
            )));
        }
        let mut metrics = BTreeMap::new();
        metrics.insert("embedding_error".to_string(), evaluation.embedding_error);
        metrics.insert("baseline_error".to_string(), evaluation.baseline_error);
        metrics.insert("held_out".to_string(), evaluation.held_out as f64);
        let mut card = ModelCard::new(model_id, "market_state_embedding", version, owner, at)
            .with_class(ModelClass::Generic)
            .with_purpose(
                "fixed-dimension embedding of market state, for retrieval and features only",
            )
            .with_features(self.features.clone())
            .with_training_data(vec![dataset.to_string()])
            .with_benchmark("raw_first_k_nearest_neighbour")
            .with_limitation("not a decision input: retrieval and features only (ADR 0182)");
        card.parameters
            .insert("dimensions".to_string(), self.dimensions().to_string());
        card.parameters
            .insert("method".to_string(), "pca_power_iteration".to_string());
        card.artifact_digest = Some(self.digest()?);
        card.evaluations.push(EvaluationRecord {
            evaluated_at: at,
            dataset: dataset.to_string(),
            metrics,
            passed: true,
        });
        Ok(card)
    }
}

fn norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn normalise(v: &mut [f64]) -> Result<()> {
    let n = norm(v);
    if n <= 0.0 {
        return Err(Error::invalid("a zero vector cannot be normalised"));
    }
    for x in v.iter_mut() {
        *x /= n;
    }
    Ok(())
}

fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>()
}

/// Outcome of the training state closest by `dist`; the first wins a tie, so
/// the answer is a function of the order given.
fn nearest(
    indexed: &[(Vec<f64>, Vec<f64>, f64)],
    dist: impl Fn(&(Vec<f64>, Vec<f64>, f64)) -> f64,
) -> f64 {
    let mut best = (f64::INFINITY, 0.0);
    for t in indexed {
        let d = dist(t);
        if d < best.0 {
            best = (d, t.2);
        }
    }
    best.1
}
