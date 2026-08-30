//! `qip-entity-resolution` — turning many providers' views of the world into
//! one set of canonical entities.
//!
//! The same company arrives as "Northwind Semiconductor Corporation" from a
//! filing, "Northwind Semi" from a wire story, "NWSC" from a market feed and a
//! bare LEI from a reference file. Unless those collapse onto one entity, the
//! knowledge graph fragments and no reasoning over it can work.
//!
//! Two properties matter more than raw accuracy. The first is that every link
//! is **explainable**: a decision carries the evidence that produced it, so a
//! wrong merge can be found and understood rather than guessed at. The second
//! is that the resolver **refuses to guess**. A match it is unsure of becomes
//! [`Decision::Ambiguous`] and is held for review, because a wrong merge is far
//! more damaging than a missed one — it silently attributes one company's news
//! to another's securities.

pub mod entity;
pub mod matching;
pub mod resolver;

pub use entity::{Entity, EntityKind, EntityRecord};
pub use matching::{MatchEvidence, name_similarity, normalise_name};
pub use resolver::{Decision, ResolutionReport, Resolver, ResolverConfig};
