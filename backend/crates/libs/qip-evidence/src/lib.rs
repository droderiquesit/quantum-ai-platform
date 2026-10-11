//! Evidence, and the fabric that weighs it (blueprint v12.0 §7).
//!
//! A library and not part of `qip-world-model`, because both sides of the
//! seam need it: ingestion builds an [`EvidenceRecord`] for every admitted
//! event and checks it before release, and the world model absorbs what
//! passed. Kept inside the world model, it forced the Lane 2 ingestion
//! service to link a cognitive slow-lane crate — the placement
//! `qip-acceptance`'s `lane_placement` suite refuses — and the alternative,
//! a second evidence type in ingestion, is how the reasoning engine came to
//! hold a thinner one the world model's checks never see. Pure data and
//! checks: no I/O, no clock, no environment.

pub mod evidence;
pub mod fabric;

pub use evidence::{
    AuthenticitySignal, EvidenceRecord, GeoPoint, GeographicConsistency, TemporalConsistency,
};
pub use fabric::{
    Authenticity, ClaimKey, Contest, EvidenceFabric, EvidenceVerdict, KeyOutcome, KnowledgeDelta,
    ProvenanceEdge, ProvenanceGraph, ProvenanceNode, SourceHistory,
};
