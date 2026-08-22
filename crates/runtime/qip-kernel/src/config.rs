//! Platform configuration.
//!
//! Everything that varies between a test, a backtest and a deployment lives
//! here, and the defaults are the safe ones. In particular
//! [`PlatformConfig::autonomy_ceiling`] defaults to paper trading, so a
//! platform assembled from `Default::default()` cannot trade live no matter
//! what else is configured.

use qip_core::time::Duration;
use qip_optimization_engine::router::RoutingPolicy;
use qip_portfolio_engine::construction::Mandate;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_risk_engine::monitor::MonitorPolicy;
use serde::{Deserialize, Serialize};

/// How the platform is assembled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlatformConfig {
    /// Seed for every derived random stream. Fixing it makes a run replayable.
    pub seed: u64,
    /// The highest autonomy level this deployment may ever reach.
    ///
    /// Paper trading by default. A platform that was never explicitly
    /// configured for live trading cannot get there, and raising this is a
    /// deployment change rather than a runtime one.
    pub autonomy_ceiling: AutonomyLevel,
    /// Whether a quantum provider is attached at all.
    pub quantum_enabled: bool,
    pub mandate: Mandate,
    pub review: ReviewPolicy,
    pub routing: RoutingPolicy,
    pub monitor: MonitorPolicy,
    /// How long the platform waits between cycles.
    pub cycle_interval: Duration,
    /// Datasets licensed for use in a production investment decision.
    pub licensed_datasets: Vec<String>,
    /// How long an agent authorisation is valid before review.
    pub agent_review_interval: Duration,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            seed: 0x5EED,
            autonomy_ceiling: AutonomyLevel::PaperTrading,
            quantum_enabled: false,
            mandate: Mandate::default(),
            review: ReviewPolicy::default(),
            routing: RoutingPolicy::default(),
            monitor: MonitorPolicy::default(),
            cycle_interval: Duration::from_mins(5),
            licensed_datasets: Vec::new(),
            agent_review_interval: Duration::from_days(90),
        }
    }
}

impl PlatformConfig {
    /// A configuration whose ceiling permits live trading.
    ///
    /// A distinct constructor on purpose: a live-capable platform should be
    /// visibly different at the call site that assembles it.
    pub fn with_live_ceiling(mut self, ceiling: AutonomyLevel) -> Self {
        self.autonomy_ceiling = ceiling;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_quantum(mut self) -> Self {
        self.quantum_enabled = true;
        self
    }

    pub fn with_licensed_datasets(mut self, datasets: Vec<String>) -> Self {
        self.licensed_datasets = datasets;
        self
    }

    /// Whether this configuration could ever reach a live venue.
    pub fn permits_live_trading(&self) -> bool {
        self.autonomy_ceiling.is_live()
    }
}
