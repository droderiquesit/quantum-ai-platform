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

pub mod config;
pub mod cycle;
pub mod platform;

pub use config::PlatformConfig;
pub use cycle::{CycleReport, Stage, StageOutcome};
pub use platform::Platform;
