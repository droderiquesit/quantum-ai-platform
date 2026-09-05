//! The central plane: the half of `docs/adr/0008-edge-cells-decide-alone.md`
//! that is allowed to be slow.
//!
//! An edge cell decides alone, against a grant it already holds. Everything
//! that decides *whether* it should hold one lives here: the walk from
//! candidate to scaled, the evidence each rung demands, the capital that sizes
//! a promotion, the aggregate exposure no single cell can see, and the six
//! governance controls that apply across both halves.
//!
//! Nothing in this module implements a gate, an allocator, a concentration
//! measure or a control. Those exist, complete and tested, in `qip-lifecycle`,
//! `qip-capital` and `qip-compliance`. What did not exist is the composition:
//!
//! * [`StrategyFactory`] walks a registered candidate up the ladder using
//!   [`qip_lifecycle::attempt_promotion`], keeps the whole walk in a
//!   [`qip_lifecycle::LifecycleLedger`], and runs
//!   [`qip_lifecycle::DemotionMonitor`] on a review tick. It cannot skip a
//!   rung or forget an approval, because
//!   [`qip_lifecycle::AuthorisedPromotion`] has no representation for either.
//! * [`StrategyDna`] is the sealed bundle the centre ships to a cell. Its one
//!   constructor refuses a strategy that is not at a capital-holding stage and
//!   refuses a grant whose terms differ from the ones a human approved.
//! * [`CentralPlane`] owns the factory, the allocator, the compliance plane
//!   and the cross-cell exposure aggregate; issues envelopes; ingests
//!   [`CellReport`]s; and turns a concentration finding into
//!   [`qip_capital::RecallOrder`]s.
//!
//! # The two signatures on one grant
//!
//! A grant issued here carries two independent records, and it is worth
//! stating plainly why rather than letting a reader discover it:
//!
//! * [`qip_capital::EnvelopeIssuer`] signs the [`qip_contracts::CapitalEnvelope`]
//!   the *cell* enforces. That signature is the one an edge cell checks, and
//!   the issuer is what caps every grant at
//!   [`qip_capital::MAXIMUM_ENVELOPE_VALIDITY`].
//! * [`qip_compliance::ApprovalChain`] produces the
//!   [`qip_compliance::ApprovedCapital`] the *governance plane* records. That
//!   value has no public constructor, so a component written against it cannot
//!   be handed a grant nobody approved.
//!
//! They are two trust roots today because the two crates hold two keys.
//! [`CentralPlane::issue`] refuses unless both cover an identical
//! [`qip_contracts::CapitalEnvelope::signing_payload`], so the governance
//! record and the cell's bound cannot drift apart — but a production
//! deployment should collapse them into one asymmetric key, and until it does,
//! rotating one key and not the other leaves a grant that verifies at the cell
//! and not in the audit trail.

pub mod dna;
pub mod factory;
pub mod foundry;
pub mod insights;
pub mod learning;
pub mod models;
pub mod plane;
pub mod realised;
pub mod regions;
pub mod whitelist;

pub use dna::{DnaPayload, StrategyDna};
pub use factory::{StrategyCandidate, StrategyFactory, StrategyReview};
pub use learning::{
    CellOutcome, DispositionInstruction, DispositionOutcome, DispositionRefused,
    DispositionedPosition, LearningReport, LearningVerdict, PositionDiscrepancy,
    RetirementDisposition, StrategyLearning,
};
pub use plane::{
    AbsorbedFill, BreakDirection, BreakOrigin, CellIngestion, CellReport, CentralConfig,
    CentralPlane, IssuedCapital, ReconciliationBreak, capital_subject,
};
pub use realised::{REALISED_SESSIONS, RealisedSeries, RealisedSession};
pub use regions::{RegionMembership, RegionShare, RegionShares};
pub use whitelist::{
    ArbitragePolicy, WhitelistIssue, WhitelistOutcome, WhitelistedMarket, WhitelistedVenue,
};
