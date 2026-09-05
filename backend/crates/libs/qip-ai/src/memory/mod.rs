//! Episodic memory — "have we seen this before, and what happened?"
//!
//! Streaming estimators forget by construction (blueprint §10). This module
//! is the platform's answer to the question a desk trader answers from
//! experience: an [`Episode`] is the blueprint's compressed episode vector —
//! the instrument, the regime in force, what the detectors found, where each
//! analyst stood, what the hypothesis claimed and at what confidence, what
//! was decided, and what followed once the claim resolved — and
//! [`EpisodicMemory`] retrieves the episodes nearest to a situation by
//! approximate nearest neighbour.
//!
//! Two properties are load-bearing and both are structural rather than
//! conventions:
//!
//! * **Bitemporal, with no leakage.** Every episode carries `at` (when the
//!   situation was true) and `known_at` (when its outcome became knowable).
//!   [`EpisodicMemory::recall`] takes the caller's `now` and returns only
//!   episodes whose `known_at` is strictly before it. A backtest replaying
//!   Monday cannot see Tuesday's resolution, because there is no read path
//!   that ignores `known_at`.
//! * **Bounded and deterministic.** Capacity is fixed at construction and
//!   eviction is oldest-first by `known_at`; the index examines at most a
//!   fixed number of candidates per query; and the locality-sensitive hash
//!   is seeded from a stated constant, so two processes built from the same
//!   episodes recall the same neighbours in the same order. A replay that
//!   reorders is not a replay.
//!
//! The feature embedding is computed in-tree from the episode's own fields
//! with no learned weights — see [`episode::EPISODE_DIMENSIONS`] for the
//! exact layout. That makes it honest about what it is: a fixed encoding
//! good enough to find "same instrument, same regime, same claim, similar
//! conviction" and nothing cleverer. A learned state vector can replace the
//! encoder later behind the same `Embedding` type without touching the
//! index.
//!
//! What this module deliberately does **not** do: change a confidence. The
//! blueprint routes episodic analogues into belief formation (§10.3), but in
//! this slice the nearest episodes and their outcome agreement are recorded
//! on the hypothesis as *precedent* — evidence context a reviewer can read —
//! and the confidence arithmetic in `qip-reasoning-engine` is untouched. The
//! route by which precedent could later bear on confidence is ADR 0005's
//! evidence-weighted update: a precedent digest would enter as an
//! `Evidence` item with its own diagnosticity, never as a multiplier applied
//! after review.

pub mod episode;
pub mod store;

pub use episode::{
    AnalystStance, ClaimRecord, DecisionTaken, EPISODE_DIMENSIONS, EPISODE_ENCODING, Episode,
    EpisodeOutcome, EpisodeQuery, FindingsSummary, RegimeLabel, StanceDirection,
};
pub use store::{EpisodicMemory, PrecedentDigest, Recall, Recalled};
