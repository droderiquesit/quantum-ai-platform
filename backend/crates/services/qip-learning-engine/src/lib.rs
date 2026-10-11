//! `qip-learning-engine` — LEARN.
//!
//! Closes the loop: what did the platform earn, was each thesis right, and
//! what should change as a result.
//!
//! Three commitments shape this crate:
//!
//! * **The attribution is exact.** [`attribution::Attribution::residual`] must
//!   be zero. An attribution that nearly adds up hides exactly the thing
//!   nobody understood, so the market-move term is defined residually and every
//!   other component is computed from its own inputs.
//! * **A thesis cannot be scored early.** Grading a thesis on the part of the
//!   path that happened to suit it is the easiest way to manufacture a track
//!   record, so [`evaluation::ThesisEvaluator::evaluate`] refuses until the
//!   horizon has closed.
//! * **Being right is not the same as being right for the stated reason.**
//!   [`evaluation::Verdict::RightForTheWrongReason`] is a distinct outcome and
//!   does not count towards a hit rate, because treating luck as skill
//!   reinforces the reasoning that produced it.
//!
//! [`feedback`] then turns that into changes: a calibration adjustment when
//! stated confidences do not match outcomes, and candidate lessons only where
//! a pattern holds across enough episodes from enough different agents.
//!
//! [`self_model`] keeps the per-component record those evaluations imply —
//! which detector, which analyst — as a bounded window with a stated
//! estimate, refused below a minimum sample, so the REASON stage can weight a
//! component by what it has actually got right.
//!
//! [`keyed_attribution`] joins the exact attribution to the seven keys §22
//! names — strategy, model, belief, evidence source, capital policy, risk
//! rule, execution path — and [`tournament`], [`forecast_market`] and
//! [`verifier`] are §9.8's Model Tournament, §22.1's synthetic-credit
//! Forecast Market and §9.11's adversarial verifier society. All of it is
//! advisory: nothing in this crate holds a handle to an order.

pub mod attribution;
pub mod evaluation;
pub mod feedback;
pub mod forecast_market;
pub mod keyed_attribution;
pub mod research;
pub mod self_model;
pub mod tournament;
pub mod verifier;

pub use attribution::{Attribution, Attributor, PositionAttribution, PositionPeriod, Source};
pub use evaluation::{
    Evaluation, EvaluationPolicy, Outcome, ThesisClaim, ThesisEvaluator, Verdict,
};
pub use feedback::{
    CalibrationReport, FeedbackEngine, FeedbackReport, LessonCandidate, PromotionBar,
};
pub use research::{MINIMUM_CLUSTER, ResearchLedger, ResearchPrompt};
pub use self_model::{
    CAPABILITY_WINDOW, Capability, CapabilityEstimate, ComponentKey, ComponentKind, MAX_COMPONENTS,
    MINIMUM_SAMPLE, ScoredOutcome, SelfModel,
};
