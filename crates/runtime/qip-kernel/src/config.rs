//! Platform configuration.
//!
//! Everything that varies between a test, a backtest and a deployment lives
//! here, and the defaults are the safe ones. In particular
//! [`PlatformConfig::autonomy_ceiling`] defaults to paper trading, so a
//! platform assembled from `Default::default()` cannot trade live no matter
//! what else is configured.

use crate::central::CentralConfig;
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
    /// How the central plane is sized and bounded.
    ///
    /// `#[serde(default)]` so a configuration written before the central plane
    /// existed still deserialises: an operator's stored config should not stop
    /// being readable because the platform grew a subsystem it does not
    /// mention.
    #[serde(default)]
    pub central: CentralConfig,

    /// Who is accountable for what this deployment collects and registers.
    ///
    /// Carried here rather than derived because a registered data source needs
    /// a named owner before it may reach the mesh catalogue, and "whoever
    /// happened to run the process" is not a name anybody can be asked about
    /// six months later.
    #[serde(default = "default_owner")]
    pub owner: String,

    /// The user agent the data finder presents when it probes a source.
    ///
    /// Mandatory upstream and mandatory here: a publisher's only means of
    /// refusing a crawler is to name it in robots.txt, and a crawler that will
    /// not say who it is has taken that away from them.
    #[serde(default = "default_data_user_agent")]
    pub data_user_agent: String,

    /// How deep a chain observation has to be buried before the platform will
    /// read state derived from it.
    ///
    /// Stated in configuration because it is a risk appetite rather than a
    /// constant: the depth at which a reorg becomes tolerable differs by chain
    /// and by what the state is being used for.
    #[serde(default = "default_chain_confirmations")]
    pub chain_confirmations: u32,
}

/// The owner recorded against anything this deployment registers.
fn default_owner() -> String {
    "qip-platform".to_string()
}

/// The user agent the finder presents. Names the platform and the crate
/// version so a publisher reading its logs can identify and refuse it.
fn default_data_user_agent() -> String {
    format!("qip-data-finder/{}", env!("CARGO_PKG_VERSION"))
}

/// Twelve blocks: deep enough that a reorg past it is an incident rather than
/// a Tuesday, and shallow enough that state is readable within a few minutes
/// on a chain with a twelve-second block time.
fn default_chain_confirmations() -> u32 {
    12
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
            central: CentralConfig::default(),
            owner: default_owner(),
            data_user_agent: default_data_user_agent(),
            chain_confirmations: default_chain_confirmations(),
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

    /// Size and bound the central plane.
    pub fn with_central(mut self, central: CentralConfig) -> Self {
        self.central = central;
        self
    }

    /// Name who is accountable for what this deployment registers.
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = owner.into();
        self
    }

    /// Name the crawler, as robots.txt has to be able to.
    pub fn with_data_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.data_user_agent = user_agent.into();
        self
    }

    /// State how deep a block must be buried before its state may be read.
    pub fn with_chain_confirmations(mut self, blocks: u32) -> Self {
        self.chain_confirmations = blocks;
        self
    }

    /// Whether this configuration could ever reach a live venue.
    pub fn permits_live_trading(&self) -> bool {
        self.autonomy_ceiling.is_live()
    }
}
