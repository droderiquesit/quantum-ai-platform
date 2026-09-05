//! `qip-ai` — model infrastructure.
//!
//! Four things live here, and one boundary.
//!
//! * [`embedding`] turns text into vectors and [`retrieval`] finds relevant
//!   documents, combining lexical and vector scoring.
//! * [`registry`] is model governance: every model the platform uses has a
//!   card recording its version, training data, evaluation and owner, and every
//!   investment decision references the models that produced it.
//! * [`evaluation`] scores probabilistic predictions and detects drift.
//! * [`language`] is the port for language models.
//! * [`memory`] is episodic memory: compressed episodes with outcomes, and a
//!   bounded, deterministic approximate-nearest-neighbour recall over them,
//!   readable only from the instant each became knowable.
//!
//! The boundary is in [`language`]: a language model may produce narrative,
//! structure and claims, but never a number that a calculation depends on.
//! [`language::NumericGuard`] enforces that mechanically rather than by
//! convention, because the failure mode — a plausible hallucinated price
//! flowing into a position size — is silent and expensive.

pub mod embedding;
pub mod evaluation;
pub mod language;
pub mod memory;
pub mod registry;
pub mod retrieval;

pub use embedding::{Embedder, Embedding, HashingEmbedder};
pub use evaluation::{Calibration, DriftReport, PredictionOutcome};
pub use language::{
    Completion, DeterministicModel, LanguageModel, ModelRequest, NumericGuard, OutputSchema,
};
pub use registry::{ModelCard, ModelRegistry, ModelStage};
pub use retrieval::{Document, RetrievalResult, SearchIndex};
