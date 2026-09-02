//! `qip-lifecycle` — evidence and approval gates.
//!
//! A strategy walks candidate → holdout → paper → shadow → pilot → scaled, and
//! can be pushed back to any of them, or retired, from anywhere.
//! [`qip_contracts::gate::GateStage`] already encodes the ladder and already
//! refuses to skip a rung. This crate is the evidence each rung demands and
//! the record of who decided what.
//!
//! Four commitments hold throughout:
//!
//! * **One failing check fails the gate.** [`gates`] never scores. A weighted
//!   score would let a strong backtest outweigh a leakage finding, and a
//!   leakage finding is precisely the thing that makes the backtest
//!   meaningless — there is nothing for it to be weighed against.
//! * **Gates recompute, they do not trust.** The holdout gate takes the return
//!   series and the cross-validation parameters and re-derives the deflated
//!   Sharpe and the fold structure using
//!   [`qip_simulation_engine::validation`]. A submitted Sharpe ratio is an
//!   assertion; a submitted return series is auditable.
//! * **Promotion to capital is unrepresentable without two people.**
//!   [`ledger::AuthorisedPromotion`] computes its target rung from
//!   [`qip_contracts::gate::GateStage::next`] and refuses a rung that
//!   [`qip_contracts::gate::GateStage::requires_human_approval`] without a
//!   dual [`qip_contracts::governance::Approval`]. The check cannot be
//!   forgotten at a call site, because the call site cannot build the value.
//! * **Demotion needs no authority at all.** [`LifecycleLedger::demote`] takes
//!   no approver and no credential; [`demotion::DemotionMonitor`] fires
//!   without a human. This mirrors the kill switch in
//!   `qip_risk_engine::autonomy` and rests on the same asymmetry: a false
//!   demotion costs a day of missed opportunity, a missed one costs the book.
//!
//! Nothing here reads a clock or draws a random number. Every entry point
//! takes the instant it is reasoning about as a [`qip_core::Timestamp`], so a
//! replay of the same events produces the same ledger.
//!
//! # Walking a strategy up the ladder
//!
//! ```
//! use qip_contracts::gate::GateStage;
//! use qip_contracts::signal::StrategyId;
//! use qip_core::Timestamp;
//! use qip_lifecycle::{LifecycleLedger, StrategyEvidence, attempt_promotion};
//!
//! let mut ledger = LifecycleLedger::new();
//! let strategy = StrategyId::new("momentum-v3");
//! let now = Timestamp::from_secs(1_700_000_000);
//!
//! // With no evidence, the first rung refuses.
//! let refused = attempt_promotion(
//!     &mut ledger,
//!     &strategy,
//!     &StrategyEvidence::new(),
//!     None,
//!     "promoting on the research write-up",
//!     now,
//! );
//! assert!(refused.is_err());
//! assert_eq!(ledger.stage_of(&strategy), GateStage::Candidate);
//!
//! // Anyone may push it down or retire it, with nobody's approval.
//! ledger
//!     .retire(&strategy, "risk-monitor", "the venue delisted the universe", now)
//!     .expect("a demotion needs no authority");
//! assert_eq!(ledger.stage_of(&strategy), GateStage::Retired);
//! ```

pub mod band;
pub mod demotion;
pub mod evidence;
pub mod gates;
pub mod ledger;
pub mod scoring;
pub mod trials;

pub use band::{BandMethod, BandVerdict, HoldoutBand};
pub use demotion::{
    DemotionMonitor, DemotionPolicy, DemotionTrigger, LiveObservation, PilotBaseline,
};
pub use evidence::{
    CrossValidationRun, EvidencePackage, FeatureTiming, HoldoutEvidence, KillCondition,
    LeakageAudit, PaperEvidence, PilotEvidence, ScaledEvidence, ShadowDecision, ShadowEvidence,
    StrategyEvidence,
};
pub use gates::{
    Admission, Gate, HoldoutGate, HoldoutPolicy, PaperGate, PaperPolicy, PilotGate, PilotPolicy,
    ScaledGate, ScaledPolicy, ShadowGate, ShadowPolicy, gate_for,
};
pub use ledger::{AuthorisedPromotion, LedgerEntry, LifecycleLedger, attempt_promotion};
pub use trials::{
    DEFAULT_QUARTERLY_BUDGET, Quarter, StrategyFamily, TrialAccount, TrialBook, TrialEvent,
    TrialRecord,
};
