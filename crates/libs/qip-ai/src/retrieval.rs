//! Document retrieval.
//!
//! Hybrid by default: BM25 over terms plus cosine over embeddings. Neither
//! alone is sufficient here. A pure vector search on a lexical embedder misses
//! exact identifiers — a CUSIP, a series code — that a research query hinges
//! on; pure BM25 misses paraphrase. Combining them recovers both, and the
//! weighting is explicit so the trade-off can be tuned per surface.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::embedding::{Embedder, Embedding};

/// A retrievable document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub text: String,
    /// When the document became true, for point-in-time filtering.
    pub occurred_at: Timestamp,
    /// Arbitrary tags for filtering: entity ids, topics, source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// Prior weight on the document's reliability, in `[0, 1]`.
    #[serde(default = "one")]
    pub reliability: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Embedding>,
}

fn one() -> f64 {
    1.0
}

impl Document {
    pub fn new(id: impl Into<String>, text: impl Into<String>, occurred_at: Timestamp) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            occurred_at,
            attributes: BTreeMap::new(),
            reliability: 1.0,
            embedding: None,
        }
    }

    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    pub fn with_reliability(mut self, reliability: f64) -> Self {
        self.reliability = reliability.clamp(0.0, 1.0);
        self
    }
}

/// One hit, with its score decomposed so a researcher can see why it matched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalResult {
    pub document: Document,
    pub score: f64,
    pub lexical_score: f64,
    pub vector_score: f64,
    /// Terms from the query that the document actually contains.
    pub matched_terms: Vec<String>,
}

/// How lexical and vector scores are combined.
#[derive(Clone, Copy, Debug)]
pub struct SearchWeights {
    pub lexical: f64,
    pub vector: f64,
    /// Weight on the document's own reliability.
    pub reliability: f64,
    /// Half-life of the recency decay, in days. Zero disables it.
    pub recency_half_life_days: f64,
    /// Combined score a document must clear to be returned at all.
    ///
    /// A hashing embedder assigns a small positive cosine to almost any pair of
    /// texts, so without a floor every query returns the whole corpus ordered by
    /// noise. Downstream that is worse than returning nothing: the reasoning
    /// stage would cite irrelevant documents as evidence and produce a
    /// confident thesis resting on them.
    pub minimum_score: f64,
}

impl Default for SearchWeights {
    fn default() -> Self {
        Self {
            lexical: 0.5,
            vector: 0.5,
            reliability: 0.3,
            // Two weeks: a research corpus where a month-old filing still
            // matters but a month-old wire story mostly does not.
            recency_half_life_days: 14.0,
            minimum_score: 0.15,
        }
    }
}

impl SearchWeights {
    /// Lexical only, for exact-identifier lookups.
    pub fn exact() -> Self {
        Self {
            lexical: 1.0,
            vector: 0.0,
            reliability: 0.0,
            recency_half_life_days: 0.0,
            minimum_score: 0.05,
        }
    }
}

/// An in-memory hybrid search index.
#[derive(Debug)]
pub struct SearchIndex {
    documents: Vec<Document>,
    /// Term to the document positions containing it.
    postings: BTreeMap<String, Vec<usize>>,
    /// Term frequency per document.
    term_frequencies: Vec<BTreeMap<String, u32>>,
    document_lengths: Vec<usize>,
    average_length: f64,
    embedder: Option<std::sync::Arc<dyn Embedder>>,
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchIndex {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            postings: BTreeMap::new(),
            term_frequencies: Vec::new(),
            document_lengths: Vec::new(),
            average_length: 0.0,
            embedder: None,
        }
    }

    /// Attach an embedder; documents added afterwards are embedded on insert.
    pub fn with_embedder(mut self, embedder: std::sync::Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    pub fn add(&mut self, mut document: Document) {
        if document.embedding.is_none()
            && let Some(embedder) = &self.embedder
        {
            document.embedding = Some(embedder.embed(&document.text));
        }

        let position = self.documents.len();
        let terms = tokenise(&document.text);
        let mut frequencies: BTreeMap<String, u32> = BTreeMap::new();
        for term in &terms {
            *frequencies.entry(term.clone()).or_insert(0) += 1;
        }
        for term in frequencies.keys() {
            self.postings
                .entry(term.clone())
                .or_default()
                .push(position);
        }

        self.document_lengths.push(terms.len());
        self.term_frequencies.push(frequencies);
        self.documents.push(document);
        self.average_length =
            self.document_lengths.iter().sum::<usize>() as f64 / self.documents.len() as f64;
    }

    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    pub fn get(&self, id: &str) -> Option<&Document> {
        self.documents.iter().find(|d| d.id == id)
    }

    /// Search with the default weights.
    pub fn search(&self, query: &str, limit: usize) -> Vec<RetrievalResult> {
        self.search_with(query, limit, SearchWeights::default(), None, &[])
    }

    /// Search restricted to documents that existed at `as_of`.
    ///
    /// This is the point-in-time guard for research: a thesis formed on a past
    /// date must not cite a document published after it.
    pub fn search_as_of(
        &self,
        query: &str,
        limit: usize,
        as_of: Timestamp,
    ) -> Vec<RetrievalResult> {
        self.search_with(query, limit, SearchWeights::default(), Some(as_of), &[])
    }

    /// Full search: weights, an optional point-in-time cutoff, and attribute
    /// filters that must all match.
    pub fn search_with(
        &self,
        query: &str,
        limit: usize,
        weights: SearchWeights,
        as_of: Option<Timestamp>,
        filters: &[(&str, &str)],
    ) -> Vec<RetrievalResult> {
        if self.documents.is_empty() || limit == 0 {
            return Vec::new();
        }
        let query_terms = tokenise(query);
        let query_embedding = self
            .embedder
            .as_ref()
            .filter(|_| weights.vector > 0.0)
            .map(|e| e.embed(query));

        let mut results: Vec<RetrievalResult> = Vec::new();
        for (position, document) in self.documents.iter().enumerate() {
            if let Some(cutoff) = as_of
                && document.occurred_at > cutoff
            {
                continue;
            }
            if !filters.iter().all(|(key, value)| {
                document.attributes.get(*key).map(String::as_str) == Some(*value)
            }) {
                continue;
            }

            let (lexical, matched) = self.bm25(position, &query_terms);
            let vector = match (&query_embedding, &document.embedding) {
                (Some(q), Some(d)) => f64::from(q.cosine_similarity(d)).max(0.0),
                _ => 0.0,
            };

            let recency = if weights.recency_half_life_days > 0.0 {
                let reference = as_of.unwrap_or_else(|| {
                    self.documents
                        .iter()
                        .map(|d| d.occurred_at)
                        .max()
                        .unwrap_or(document.occurred_at)
                });
                let age_days = reference.since(document.occurred_at).as_days_f64().max(0.0);
                0.5f64.powf(age_days / weights.recency_half_life_days)
            } else {
                1.0
            };

            let base = weights.lexical * normalise_bm25(lexical) + weights.vector * vector;
            if base <= weights.minimum_score {
                continue;
            }
            let score = base
                * (1.0 + weights.reliability * (document.reliability - 0.5))
                * recency.max(0.05);

            results.push(RetrievalResult {
                document: document.clone(),
                score,
                lexical_score: lexical,
                vector_score: vector,
                matched_terms: matched,
            });
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                // Ties break on document id so results are deterministic.
                .then_with(|| a.document.id.cmp(&b.document.id))
        });
        results.truncate(limit);
        results
    }

    /// Documents most similar to a given one, excluding itself.
    pub fn similar_to(&self, id: &str, limit: usize) -> Vec<RetrievalResult> {
        let Some(document) = self.get(id) else {
            return Vec::new();
        };
        let mut results = self.search_with(
            &document.text,
            limit + 1,
            SearchWeights {
                recency_half_life_days: 0.0,
                // A whole document as the query matches strongly; the floor can
                // be lower than for a short query without admitting noise.
                minimum_score: 0.10,
                ..SearchWeights::default()
            },
            None,
            &[],
        );
        results.retain(|r| r.document.id != id);
        results.truncate(limit);
        results
    }

    /// BM25 score for one document against the query terms.
    fn bm25(&self, position: usize, query_terms: &[String]) -> (f64, Vec<String>) {
        // Okapi BM25 with the conventional parameters.
        const K1: f64 = 1.2;
        const B: f64 = 0.75;

        let total = self.documents.len() as f64;
        let length = self.document_lengths[position] as f64;
        let frequencies = &self.term_frequencies[position];

        let mut score = 0.0;
        let mut matched = Vec::new();
        for term in query_terms {
            let Some(frequency) = frequencies.get(term) else {
                continue;
            };
            let containing = self.postings.get(term).map_or(0, Vec::len) as f64;
            // Smoothed inverse document frequency, never negative.
            let idf = ((total - containing + 0.5) / (containing + 0.5) + 1.0).ln();
            let tf = f64::from(*frequency);
            score += idf * (tf * (K1 + 1.0))
                / (tf + K1 * (1.0 - B + B * length / self.average_length.max(1.0)));
            matched.push(term.clone());
        }
        matched.dedup();
        (score, matched)
    }
}

/// Squash an unbounded BM25 score into `[0, 1)` so it can be blended with
/// cosine similarity, which is already bounded.
fn normalise_bm25(score: f64) -> f64 {
    if score <= 0.0 {
        return 0.0;
    }
    score / (1.0 + score)
}

fn tokenise(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| t.len() > 1)
        .collect()
}
