//! Hosted language-model providers the reasoning service can call.
//!
//! In this crate rather than in `qip-ai` because a library performs no I/O
//! (`.claude/rules/architecture/00-boundaries.md`): `qip_ai::language`
//! defines the port and the deterministic stand-in, and an adapter that opens
//! a socket belongs in a service. One provider today, decided by ADR 0037.

pub mod huggingface;

pub use huggingface::{HuggingFaceConfig, HuggingFaceModel, HuggingFaceToken};
