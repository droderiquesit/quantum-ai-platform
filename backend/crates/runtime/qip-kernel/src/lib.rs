//! `qip-kernel` — the composition root.
//!
//! One place knows how the pieces fit together, which is what keeps the pieces
//! themselves free of assumptions about each other. [`Platform`] owns every
//! stage and [`Platform::run_cycle`] is one pass through SENSE → UNDERSTAND →
//! DISCOVER → REASON → SIMULATE → DECIDE → ACT → LEARN.
//!
//! A cycle never panics and never stops early. A stage that fails records its
//! problem in the [`cycle::CycleReport`] and the cycle continues, because the
//! learning stage is what would eventually notice that a stage keeps failing.
//!
//! [`config::PlatformConfig::default`] cannot trade live: the autonomy ceiling
//! starts at paper trading and nothing after assembly can raise it. A
//! live-capable platform is assembled through a different, visible call.
//!
//! [`central`] is the other half of the same composition root. The loop above
//! is what one process does with the market in front of it;
//! [`central::CentralPlane`] is what the platform does about strategies and
//! capital across every edge cell — research, approval gates, allocation,
//! aggregate exposure and the six governance controls. It is additive: a
//! platform that never touches it runs exactly the cycle it always did.
//!
//! # What this process contains
//!
//! A crate is not a service until a process ships it. Eight further service
//! crates are composed onto [`Platform`] and *called* by it, so a deployed
//! binary contains the capability rather than merely depending on the code:
//! the autonomous data finder and the mesh catalogue it feeds, the chain
//! observer and its confirmation discipline, the prediction record, the
//! durable event journal, the outcome capture and counterfactual twin, the
//! capital demand forecaster, and the cost meter that charges every cycle for
//! having been run. See [`platform`].

pub mod central;
pub mod config;
pub mod cycle;
pub mod platform;

pub use central::{CellReport, CentralPlane, StrategyDna, StrategyFactory};
pub use config::{EventLogDestination, PlatformConfig};
pub use cycle::{CycleReport, Stage, StageOutcome};
pub use platform::{
    ChainAbsorption, CycleJournalEntry, Platform, RecordedPrediction, SourceAssessment,
};
