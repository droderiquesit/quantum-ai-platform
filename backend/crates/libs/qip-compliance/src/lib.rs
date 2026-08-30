//! `qip-compliance` — the cross-cutting governance and safety plane, enforced
//! rather than documented.
//!
//! Six controls apply to every subsystem rather than to one of them, and
//! `qip_contracts::governance` names them. This crate builds one enforcement
//! mechanism per control, and the test for each one demonstrates that the
//! *unsafe* action is impossible or refused — not that the safe path works.
//!
//! | Control | Mechanism | The unsafe action, and why it cannot happen |
//! |---|---|---|
//! | Point-in-time truth | [`pit::PointInTime`] | A reader is built by discarding facts whose known-time is after the as-of, so there is no future fact to return and no accessor that could return one. |
//! | Licensing | [`licensing::LicensedData`] | The value is private; every accessor takes a [`qip_contracts::Usage`] and the registry, and records the decision. Deriving carries the licence onto the derived value. |
//! | Model risk | [`model_risk::AdmittedOutput`] | No public constructor. The only source checks eligibility, a current risk file, the validated operating range, and an [`model_risk::Explanation`] that cannot exist unless it reconciles. |
//! | Capital approval | [`approval::ApprovedCapital`] | No public constructor and no `Deserialize`; the chain demands a named approver who is not the requester, fresh credentials, and two people above the threshold. |
//! | Signed provenance | [`artifacts::ArtifactStore`] | Nothing is stored whose bytes do not hash to its digest and whose signature does not verify. [`artifacts::ProvenanceChain`] names the exact digest where ancestry breaks. |
//! | Kill switch | [`incident::IncidentLog`] | Tripping needs no authority; clearing needs a named operator with a fresh credential and a stated reason, and leaves a record. |
//!
//! [`plane::CompliancePlane`] composes all six and produces a
//! [`plane::ComplianceReport`] enumerating, for each control, whether it is
//! enforced and by what — the artifact that makes "fully compliant" a thing a
//! test can check rather than a claim in a document.
//!
//! Three house rules hold throughout. **No ambient clock**: every check takes
//! the timestamp it should reason at, so a replay reproduces the same
//! decisions. **Exact money**: limits and contributions are
//! [`qip_core::Decimal`]. **Refusals name things**: every denial quotes the
//! dataset, the model, the person or the digest at issue, because a control
//! that refuses without saying what to fix gets switched off.
//!
//! The honest gaps are recorded on [`plane::ControlStatus::caveats`] and read
//! back by [`plane::ComplianceReport::caveats`]; the largest is that artifact
//! signing is symmetric — see [`signing`].

pub mod approval;
pub mod artifacts;
pub mod incident;
pub mod licensing;
pub mod model_risk;
pub mod pit;
pub mod plane;
pub mod signing;

pub use approval::{
    ApprovalChain, ApprovedCapital, CapitalRequest, GrantRecord, MAXIMUM_CREDENTIAL_AGE,
    OperatorCredential,
};
pub use artifacts::{
    ArtifactStore, ChainBreak, ChainNode, ProvenanceChain, RawDataset, StoredArtifact,
};
pub use incident::{Clearance, Halt, HaltScope, Incident, IncidentLog, ResponsePolicy};
pub use licensing::{EntitlementCheck, EntitlementRegistry, LicensedData};
pub use model_risk::{
    AdmissionRecord, AdmittedOutput, Contribution, Explanation, ModelReview, ModelRiskFile,
    ModelRiskRegister, PerformanceBoundary, ValidationEvidence, ValidationKind,
};
pub use pit::{LeakageDetector, LeakageFinding, LeakageReport, PointInTime};
pub use plane::{CompliancePlane, ComplianceReport, ControlStatus};
pub use signing::SigningKey;
