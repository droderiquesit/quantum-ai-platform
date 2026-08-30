//! `qip-quant` — factors, signals and the strategy SDK.
//!
//! A [`signal::Signal`] is a scalar view on one instrument at one moment, with
//! a horizon and a confidence attached. A [`strategy::Strategy`] turns signals
//! and state into target weights. Keeping the two separate is what lets the
//! same signal be reused across strategies and evaluated on its own merits.
//!
//! Every signal declares the horizon it is meant for. A momentum signal
//! measured over twelve months says nothing about the next hour, and a
//! platform that mixes horizons without tracking them will size a slow signal
//! as though it were fast.

pub mod factor;
pub mod signal;
pub mod strategy;

pub use factor::{FactorDefinition, FactorLibrary, FactorScore};
pub use signal::{Horizon, Signal, SignalKind, SignalSet};
pub use strategy::{Strategy, StrategyContext, StrategyDecision, TargetWeights};
