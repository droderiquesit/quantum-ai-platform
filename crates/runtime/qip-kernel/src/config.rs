//! Platform configuration.
//!
//! Everything that varies between a test, a backtest and a deployment lives
//! here, and the defaults are the safe ones. In particular
//! [`PlatformConfig::autonomy_ceiling`] defaults to paper trading, so a
//! platform assembled from `Default::default()` cannot trade live no matter
//! what else is configured.

use crate::central::CentralConfig;
use qip_core::error::Result;
use qip_core::time::Duration;
use qip_events::log::{Durability, EventLog};
use qip_optimization_engine::router::RoutingPolicy;
use qip_portfolio_engine::construction::Mandate;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk_engine::autonomy::AutonomyLevel;
use qip_risk_engine::monitor::MonitorPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the platform's event log is kept.
///
/// The log is the platform's evidence: every decision a cycle reached, chained
/// so that removing one breaks the chain at that point. Until this existed the
/// kernel built that log in memory and nothing could say otherwise, so a
/// process that wanted an event record that outlived it had to copy the log
/// out at a cycle boundary — which leaves every append between two boundaries
/// with nowhere to be if the process dies. Naming the destination here moves
/// the decision to where the log is built.
///
/// It is a configuration value rather than an injected [`EventLog`] because a
/// log carries state — its sequence and its chain tail — and a caller holding
/// one could append to it beside the platform. Records interleaved from two
/// writers still chain and are no longer a record of what the platform did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLogDestination {
    /// Held in this process and nowhere else. The default, because it is the
    /// only destination that cannot be wrong about itself: a file path that
    /// turns out to be unwritable is discovered at assembly, and a default
    /// path would be one nobody chose.
    #[default]
    InMemory,
    /// Appended to a JSONL file, which is also read back at assembly.
    ///
    /// Reading it back is the point: the chain continues from the last record
    /// the file holds rather than starting again at sequence one, so the
    /// evidence spans restarts of the process instead of being one run's
    /// worth each time.
    File {
        path: PathBuf,
        /// Whether an appended record is on the platter before the append
        /// returns. Defaulted so a configuration written without it opts into
        /// the safe answer rather than the fast one.
        #[serde(default)]
        durability: Durability,
    },
}

impl EventLogDestination {
    /// Write the log to `path`, synchronously.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File {
            path: path.into(),
            durability: Durability::Synchronous,
        }
    }

    /// Trade the durability guarantee for throughput, deliberately.
    ///
    /// A no-op on [`EventLogDestination::InMemory`], which has nothing to
    /// flush: silently rewriting it into a file destination would invent a
    /// path the caller never named.
    pub fn with_durability(self, durability: Durability) -> Self {
        match self {
            Self::InMemory => Self::InMemory,
            Self::File { path, .. } => Self::File { path, durability },
        }
    }

    /// The path this destination writes to, if it writes to one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::InMemory => None,
            Self::File { path, .. } => Some(path),
        }
    }

    /// Whether a record this log has accepted outlives the process that wrote
    /// it.
    ///
    /// True for either file destination: bytes the operating system has
    /// accepted are in the file after `kill -9`, whatever the platter holds.
    pub fn survives_this_process(&self) -> bool {
        matches!(self, Self::File { .. })
    }

    /// Whether a record this log has accepted outlives the machine.
    ///
    /// Deliberately separate from
    /// [`EventLogDestination::survives_this_process`], because the two answers
    /// differ for exactly the destination somebody picks to go faster: an
    /// OS-buffered file survives `kill -9` and does not survive a power cut. A
    /// single predicate answering both questions would be wrong about the one
    /// case that loses records, and a banner reading it would say so.
    pub fn survives_power_loss(&self) -> bool {
        match self {
            Self::InMemory => false,
            Self::File { durability, .. } => durability.survives_power_loss(),
        }
    }

    /// Build the log.
    ///
    /// Fallible, and called during assembly rather than at the first append,
    /// because an unwritable directory or a corrupt line in an existing log is
    /// a deployment fault: discovering it at the first append means the
    /// process is already running and already believed.
    pub fn open(&self) -> Result<EventLog> {
        match self {
            Self::InMemory => Ok(EventLog::in_memory()),
            Self::File { path, durability } => {
                Ok(EventLog::open(path)?.with_durability(*durability))
            }
        }
    }

    /// One line for a start-up banner.
    pub fn describe(&self) -> String {
        match self {
            Self::InMemory => "in memory; nothing appended to it survives this process".to_string(),
            Self::File { path, durability } => format!(
                "{} ({})",
                path.display(),
                match durability {
                    Durability::Synchronous => "on the platter before an append returns",
                    Durability::OsBuffered =>
                        "buffered by the operating system; a power cut loses the last records",
                }
            ),
        }
    }
}

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

    /// Where the event log is written.
    ///
    /// In memory unless a deployment says otherwise, so nothing that assembled
    /// a platform before this field existed starts writing to a disk it never
    /// asked for. `#[serde(default)]` for the same reason a stored
    /// configuration keeps deserialising: an operator's config should not stop
    /// being readable because the kernel grew a field it does not mention.
    #[serde(default)]
    pub event_log: EventLogDestination,

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
            event_log: EventLogDestination::default(),
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

    /// Say where the event log goes.
    pub fn with_event_log(mut self, destination: EventLogDestination) -> Self {
        self.event_log = destination;
        self
    }

    /// Write the event log to a file, so the chain spans restarts.
    ///
    /// Named separately from [`PlatformConfig::with_event_log`] because this is
    /// the case a deployment actually wants, and a call site that says it in
    /// one line is one a reviewer can see.
    pub fn with_event_log_file(self, path: impl Into<PathBuf>) -> Self {
        self.with_event_log(EventLogDestination::file(path))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_platform_configured_by_nobody_writes_its_event_log_nowhere() {
        // The default has to be the destination that cannot be wrong about
        // itself. A default path would be one no operator chose, and a
        // deployment would inherit a disk it never asked for.
        let config = PlatformConfig::default();
        assert_eq!(config.event_log, EventLogDestination::InMemory);
        assert!(config.event_log.path().is_none());
        assert!(
            !config.event_log.survives_this_process(),
            "an unconfigured platform must not claim to keep anything"
        );
    }

    #[test]
    fn an_os_buffered_file_survives_the_process_and_does_not_survive_the_machine() {
        // The two questions a banner asks are genuinely different for exactly
        // this destination, which is why they are two predicates: bytes the
        // operating system accepted are in the file after `kill -9` and are
        // gone after a power cut.
        let buffered = EventLogDestination::file("/tmp/qip-events.jsonl")
            .with_durability(Durability::OsBuffered);
        assert!(buffered.survives_this_process());
        assert!(!buffered.survives_power_loss());

        let synchronous = EventLogDestination::file("/tmp/qip-events.jsonl");
        assert!(synchronous.survives_this_process());
        assert!(
            synchronous.survives_power_loss(),
            "the plain file constructor must be the safe one, not the fast one"
        );
    }

    #[test]
    fn asking_an_in_memory_destination_for_durability_does_not_invent_a_path_for_it() {
        let asked = EventLogDestination::InMemory.with_durability(Durability::Synchronous);
        assert_eq!(asked, EventLogDestination::InMemory);
        assert!(
            asked.path().is_none(),
            "a durability request rewrote an in-memory log into a file nobody named"
        );
    }

    #[test]
    fn a_stored_configuration_written_before_this_field_existed_still_deserialises() {
        // The compatibility this field exists under: an operator's config is
        // not supposed to stop being readable because the kernel grew a field
        // it does not mention.
        let stored = serde_json::to_value(PlatformConfig::default())
            .expect("the default configuration serialises");
        let mut object = match stored {
            serde_json::Value::Object(map) => map,
            other => panic!("a configuration is a JSON object, not {other}"),
        };
        assert!(
            object.remove("event_log").is_some(),
            "the premise: the field is in the serialised form to be removed"
        );

        let restored: PlatformConfig = serde_json::from_value(serde_json::Value::Object(object))
            .expect("a configuration without the field is still a configuration");
        assert_eq!(restored.event_log, EventLogDestination::InMemory);
    }

    #[test]
    fn a_file_destination_written_without_a_durability_opts_into_the_safe_answer() {
        let restored: EventLogDestination =
            serde_json::from_str(r#"{"file":{"path":"/var/lib/qip/events.jsonl"}}"#)
                .expect("a file destination without a durability is valid");
        assert!(
            restored.survives_power_loss(),
            "an omitted durability defaulted to the fast answer rather than the safe one"
        );
    }

    #[test]
    fn the_banner_line_for_a_buffered_file_says_what_a_power_cut_takes() {
        let described = EventLogDestination::file("/var/lib/qip/events.jsonl")
            .with_durability(Durability::OsBuffered)
            .describe();
        assert!(
            described.contains("/var/lib/qip/events.jsonl") && described.contains("power cut"),
            "an operator reading this cannot tell what it loses: {described}"
        );
        assert!(
            EventLogDestination::InMemory
                .describe()
                .contains("survives this process"),
            "the in-memory banner does not say that nothing outlives the process"
        );
    }

    #[test]
    fn the_builder_puts_the_destination_where_the_platform_will_read_it() {
        let config = PlatformConfig::default().with_event_log_file("/var/lib/qip/events.jsonl");
        assert_eq!(
            config.event_log.path(),
            Some(Path::new("/var/lib/qip/events.jsonl"))
        );
        assert!(config.event_log.survives_power_loss());
    }
}
