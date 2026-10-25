//! `qip-language-providers` — the hosted language-model adapters.
//!
//! A crate of its own, linked by `qip-deepbrain` alone (ADR 0138), because
//! the property that a generalist model never reaches the fast path or the
//! order path (R12-MODEL-4002, ADR 0008) is a property of who *links* the
//! adapter, not only of who constructs it. While the adapter lived in
//! `qip-reasoning-engine`, `qip-fastbrain` linked it through `qip-kernel`, and
//! a one-line construction in the fast brain would have compiled.
//! `lane_placement.rs` pins the closure; `model_slow_lane_confinement.rs`
//! pins the construction site.
//!
//! Not in `qip-ai` because a library performs no I/O
//! (`.claude/rules/architecture/00-boundaries.md`): `qip_ai::language`
//! defines the port and the deterministic stand-in, and an adapter that opens
//! a socket belongs in a service. One provider today, decided by ADR 0037.

pub mod huggingface;

pub use huggingface::{HuggingFaceConfig, HuggingFaceModel, HuggingFaceToken};
