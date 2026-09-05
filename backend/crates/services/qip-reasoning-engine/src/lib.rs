//! `qip-reasoning-engine` — REASON.
//!
//! Turns findings from the agent organisation into hypotheses that have been
//! attacked before anyone acts on them.
//!
//! The design commitment here is that **confidence is arithmetic**. A
//! [`hypothesis::Hypothesis`] cannot be constructed with a confidence; the only
//! constructor computes it from the evidence via [`bayes::update`], and
//! `validate` rejects a hypothesis whose stated confidence has drifted from
//! what its evidence implies. Raising confidence therefore requires better
//! evidence, which is the only mechanism that should raise it.
//!
//! Three further properties are structural rather than procedural:
//!
//! * **Correlated evidence cannot masquerade as independent confirmation.**
//!   [`evidence::EvidenceSet::independent_weight`] groups by origin first, and
//!   the belief update discounts by the effective sample size.
//! * **Point-in-time throughout.** Evidence is filtered to the hypothesis's
//!   as-of time before anything is computed, so a backtested thesis cannot
//!   rest on evidence that did not exist yet.
//! * **Every hypothesis is attacked.** [`redteam::RedTeam`] runs twelve named
//!   checks against the thesis's own structure. It is deterministic on
//!   purpose: a language model asked whether a well-written thesis is
//!   convincing will usually say yes.
//!
//! A hosted language model, when one is configured, is reached through
//! [`providers`] — for narrative only, behind `NumericGuard`, and only from
//! the deep brain (ADR 0037).

pub mod bayes;
pub mod engine;
pub mod evidence;
pub mod hypothesis;
pub mod providers;
pub mod redteam;

pub use bayes::{BaseRate, BeliefUpdate, EvidenceStrength};
pub use engine::{ReasoningEngine, ReasoningOutcome, SynthesisInput};
pub use evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
pub use hypothesis::{
    CausalChain, CausalStep, Claim, Hypothesis, HypothesisDraft, HypothesisStatus,
};
pub use providers::{HuggingFaceConfig, HuggingFaceModel, HuggingFaceToken};
pub use redteam::{Challenge, ChallengeKind, RedTeam, ReviewOutcome, ReviewPolicy, Severity};
