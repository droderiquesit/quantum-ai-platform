//! Text embeddings.
//!
//! The in-tree [`HashingEmbedder] is a feature-hashing bag-of-n-grams with
//! sub-linear term weighting and L2 normalisation. It is honestly a *lexical*
//! embedder, not a learned semantic one: it will match "revenue guidance cut"
//! to "guidance for revenue reduced" but not to "outlook lowered". That is
//! enough for retrieval over the platform's own corpus, it is deterministic,
//! and it needs no credentials.
//!
//! A production deployment should register a learned embedding model through
//! [`Embedder`] — the interface is the same, and everything downstream is
//! written against it. See `docs/operations/external-dependencies.md`.

use qip_core::hash::Hasher256;
use serde::{Deserialize, Serialize};

/// A dense vector representation of a text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    pub values: Vec<f32>,
    /// Model that produced it, so a stale index can be detected after a model
    /// change rather than silently returning nonsense neighbours.
    pub model: String,
}

impl Embedding {
    pub fn new(values: Vec<f32>, model: impl Into<String>) -> Self {
        Self {
            values,
            model: model.into(),
        }
    }

    pub fn dimensions(&self) -> usize {
        self.values.len()
    }

    /// Cosine similarity in `[-1, 1]`. Zero for a dimension or model mismatch —
    /// comparing vectors from different models is meaningless, not merely
    /// inaccurate.
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        if self.values.len() != other.values.len() || self.model != other.model {
            return 0.0;
        }
        let dot: f32 = self
            .values
            .iter()
            .zip(&other.values)
            .map(|(a, b)| a * b)
            .sum();
        let norm_a: f32 = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        let norm_b: f32 = other.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm_a <= 0.0 || norm_b <= 0.0 {
            return 0.0;
        }
        (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
    }

    /// Euclidean distance.
    pub fn distance(&self, other: &Self) -> f32 {
        if self.values.len() != other.values.len() {
            return f32::INFINITY;
        }
        self.values
            .iter()
            .zip(&other.values)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt()
    }

    /// Scale to unit length.
    pub fn normalised(&self) -> Self {
        let norm: f32 = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm <= 0.0 {
            return self.clone();
        }
        Self {
            values: self.values.iter().map(|v| v / norm).collect(),
            model: self.model.clone(),
        }
    }

    /// Element-wise mean of several embeddings, for a centroid.
    pub fn centroid(embeddings: &[Embedding]) -> Option<Embedding> {
        let first = embeddings.first()?;
        let dimensions = first.dimensions();
        if embeddings.iter().any(|e| e.dimensions() != dimensions) {
            return None;
        }
        let mut sum = vec![0.0f32; dimensions];
        for embedding in embeddings {
            for (slot, value) in sum.iter_mut().zip(&embedding.values) {
                *slot += value;
            }
        }
        let count = embeddings.len() as f32;
        Some(Embedding {
            values: sum.into_iter().map(|v| v / count).collect(),
            model: first.model.clone(),
        })
    }
}

/// Turns text into an embedding.
pub trait Embedder: Send + Sync + std::fmt::Debug {
    fn model_name(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed(&self, text: &str) -> Embedding;

    /// Embed a batch. Provided so a remote model can amortise round trips.
    fn embed_batch(&self, texts: &[String]) -> Vec<Embedding> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

/// Feature-hashing embedder over word and character n-grams.
#[derive(Debug, Clone)]
pub struct HashingEmbedder {
    dimensions: usize,
    /// Length of the character n-grams, which give robustness to inflection
    /// and to the misspellings that appear in real filings.
    char_gram: usize,
    /// Whether to include word bigrams, which capture short phrases.
    use_bigrams: bool,
}

impl Default for HashingEmbedder {
    fn default() -> Self {
        Self {
            dimensions: 512,
            char_gram: 4,
            use_bigrams: true,
        }
    }
}

impl HashingEmbedder {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions: dimensions.max(16),
            ..Self::default()
        }
    }

    /// Split into lowercase alphanumeric tokens, dropping stop words.
    ///
    /// Stop words carry no discriminating signal and, under feature hashing,
    /// actively crowd the vector: every document would share their buckets.
    fn tokenise(text: &str) -> Vec<String> {
        const STOP_WORDS: [&str; 30] = [
            "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for", "with",
            "by", "from", "as", "is", "are", "was", "were", "be", "been", "it", "its", "that",
            "this", "these", "those", "will", "has",
        ];
        text.split(|c: char| !c.is_alphanumeric())
            .map(|t| t.to_ascii_lowercase())
            .filter(|t| t.len() > 1 && !STOP_WORDS.contains(&t.as_str()))
            .collect()
    }

    /// Map a feature to a bucket and a sign.
    ///
    /// The sign is what keeps hash collisions from systematically inflating a
    /// bucket: colliding features cancel on average instead of accumulating.
    fn bucket(&self, feature: &str) -> (usize, f32) {
        let mut hasher = Hasher256::new();
        hasher.update(feature.as_bytes());
        let digest = hasher.finish();
        let index = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize
            % self.dimensions;
        let sign = if digest[4] % 2 == 0 { 1.0 } else { -1.0 };
        (index, sign)
    }

    fn accumulate(&self, values: &mut [f32], feature: &str, weight: f32) {
        let (index, sign) = self.bucket(feature);
        values[index] += sign * weight;
    }
}

impl Embedder for HashingEmbedder {
    fn model_name(&self) -> &str {
        "hashing-ngram-v1"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, text: &str) -> Embedding {
        let mut values = vec![0.0f32; self.dimensions];
        let tokens = Self::tokenise(text);

        // Sub-linear term weighting: the tenth occurrence of a word says far
        // less than the first, so raw counts over-weight repetition.
        let mut counts: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
        for token in &tokens {
            *counts.entry(token.as_str()).or_insert(0) += 1;
        }
        for (token, count) in &counts {
            let weight = 1.0 + (f64::from(*count)).ln() as f32;
            self.accumulate(&mut values, token, weight);

            // Character n-grams, at lower weight.
            if token.len() > self.char_gram {
                let bytes: Vec<char> = token.chars().collect();
                for window in bytes.windows(self.char_gram) {
                    let gram: String = window.iter().collect();
                    self.accumulate(&mut values, &gram, 0.3);
                }
            }
        }

        if self.use_bigrams {
            for pair in tokens.windows(2) {
                self.accumulate(&mut values, &format!("{}_{}", pair[0], pair[1]), 0.7);
            }
        }

        Embedding::new(values, self.model_name()).normalised()
    }
}
