//! `qip-opportunity-engine` — the DISCOVER stage.
//!
//! The platform is not supposed to wait for a human to ask a question. This
//! stage watches everything it can observe, notices what is unusual, and turns
//! the important ones into ranked [`Opportunity`] records for the reasoning
//! agents to investigate.
//!
//! The hard part is not detection, it is *restraint*. A detector tuned to fire
//! often produces a queue nobody can work through, and a research organisation
//! that investigates everything investigates nothing carefully. Three things
//! keep the volume honest:
//!
//! * detectors report a **z-score and a base rate**, not a boolean, so how
//!   unusual something is survives into the ranking;
//! * ranking multiplies importance by **novelty**, so the fifth alert about the
//!   same instrument ranks far below the first;
//! * an opportunity carries the **investigation it requires**, so the cost of
//!   following it up is visible when deciding whether to.

pub mod detector;
pub mod engine;
pub mod opportunity;

pub use detector::{Anomaly, AnomalyKind, DetectionContext, Detector, DetectorRegistry};
pub use engine::{EngineConfig, OpportunityEngine};
pub use opportunity::{Investigation, Opportunity, OpportunityRank};
