//! What the node reads from its environment before it will run.
//!
//! Every value has a safe default, and "safe" here means the default cannot
//! make the node do more than it would otherwise do: the ceiling the
//! environment may set is a tighter one, never a looser one, and the run is
//! unbounded only because a node meant to stay up is the ordinary case. Nothing
//! is read from a file in the repository and nothing is a credential — this
//! node holds none, which is the deployment's half of the same guarantee the
//! roster check makes in the binary.
//!
//! Parsing takes a map rather than reading the process environment, so the
//! defaults and the refusals are asserted directly instead of by setting
//! variables in a process that other tests share.

use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_storage::settings::{ROOT_VARIABLE, StorageSettings, TARGET_VARIABLE};
use std::collections::BTreeMap;

use crate::roster::MAXIMUM_BUDGET;

/// Where the health surface binds when the environment does not say.
///
/// All interfaces, because a probe reaches the pod from outside it. The one
/// request that changes anything — the quiesce request — is refused unless it
/// came from loopback, so binding widely does not hand anyone a stop button.
pub const DEFAULT_HEALTH_ADDRESS: &str = "0.0.0.0:8080";

/// How often a cycle starts when the environment does not say.
pub const DEFAULT_CYCLE_INTERVAL: Duration = Duration::from_millis(100);

/// How long the node waits for the flush on the way out.
pub const DEFAULT_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// How many cycles pass between hand-overs to the chain archive.
///
/// The archive runs in the gap between cycles, never inside one, so this is not
/// a latency knob — it is how many cycles a crash takes with it. A hundred at
/// the default interval is ten seconds of record.
pub const DEFAULT_ARCHIVE_EVERY: u64 = 100;

/// Consecutive over-budget cycles tolerated before the node reports itself
/// unready.
///
/// Not one: a single slow cycle is a garbage collection or a noisy neighbour,
/// and a node that left rotation for one would flap. Not a large number either
/// — the point of a ceiling is that persistently missing it means something.
pub const DEFAULT_BREACH_TOLERANCE: u32 = 3;

/// Everything the node needs to run, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FastBrainConfig {
    /// Where the health surface binds.
    pub health_address: String,
    /// How often a cycle starts.
    pub cycle_interval: Duration,
    /// The longest a single cycle may take before it is a breach.
    pub cycle_budget: Duration,
    /// Stop cleanly after this many cycles. `None` runs until asked to stop.
    pub max_cycles: Option<u64>,
    /// Stop cleanly after this long. `None` runs until asked to stop.
    pub max_runtime: Option<Duration>,
    /// Consecutive breaches tolerated before the node reports itself unready.
    pub breach_tolerance: u32,
    /// Cycles between hand-overs to the chain archive. Zero archives only on
    /// the way out.
    pub archive_every: u64,
    /// Where the cycle journal goes on the way out.
    ///
    /// The same two variables every other binary reads, rather than one of this
    /// node's own: a deployment that configured storage once should not
    /// discover that the fast brain wanted it spelled differently. The
    /// refusals — an unrecognised target, a durable target with no root, a root
    /// beside the memory target — come from there and are not restated here.
    pub storage: StorageSettings,
    /// A recorded JSONL feed to replay instead of the synthetic exchange.
    pub replay_path: Option<String>,
    /// The synthetic exchange's seed, so a session is reproducible.
    pub seed: u64,
    /// How long the shutdown flush may take before the node gives up on it.
    pub shutdown_budget: Duration,
}

impl Default for FastBrainConfig {
    fn default() -> Self {
        Self {
            health_address: DEFAULT_HEALTH_ADDRESS.to_string(),
            cycle_interval: DEFAULT_CYCLE_INTERVAL,
            cycle_budget: MAXIMUM_BUDGET,
            max_cycles: None,
            max_runtime: None,
            breach_tolerance: DEFAULT_BREACH_TOLERANCE,
            archive_every: DEFAULT_ARCHIVE_EVERY,
            storage: StorageSettings::in_memory(),
            replay_path: None,
            seed: 20_260_822,
            shutdown_budget: DEFAULT_SHUTDOWN_BUDGET,
        }
    }
}

impl FastBrainConfig {
    /// Read the process environment.
    pub fn from_env() -> Result<Self> {
        Self::parse(&std::env::vars().collect())
    }

    /// Resolve a configuration from a set of variables.
    ///
    /// Every missing variable is a default rather than a refusal, which is the
    /// opposite of `qip-api`'s credential handling and for the same reason: an
    /// API that starts unauthenticated is worse than one that is down, and a
    /// fast-path node that will not start because nobody set a cycle interval
    /// is worse than one that starts on a sane one.
    pub fn parse(vars: &BTreeMap<String, String>) -> Result<Self> {
        let defaults = Self::default();

        let health_address =
            text(vars, "QIP_FASTBRAIN_HEALTH_ADDRESS").unwrap_or(defaults.health_address);

        let cycle_interval =
            millis(vars, "QIP_FASTBRAIN_CYCLE_INTERVAL_MS")?.unwrap_or(defaults.cycle_interval);
        if cycle_interval.as_nanos() <= 0 {
            return Err(Error::invalid(
                "configuration: QIP_FASTBRAIN_CYCLE_INTERVAL_MS is zero; a loop with no interval \
                 is a spin, not a clock",
            ));
        }

        let cycle_budget =
            millis(vars, "QIP_FASTBRAIN_CYCLE_BUDGET_MS")?.unwrap_or(defaults.cycle_budget);
        if cycle_budget.as_nanos() <= 0 {
            return Err(Error::invalid(
                "configuration: QIP_FASTBRAIN_CYCLE_BUDGET_MS is zero; every cycle would breach a \
                 ceiling nothing can meet",
            ));
        }
        // The environment may tighten the fast-path ceiling and may not loosen
        // it. A guarantee an operator can widen from a config map is not a
        // guarantee, and this is the same number the roster check enforces at
        // start-up.
        if cycle_budget > MAXIMUM_BUDGET {
            return Err(Error::denied(format!(
                "configuration: QIP_FASTBRAIN_CYCLE_BUDGET_MS asks for {cycle_budget:?}, beyond \
                 the {MAXIMUM_BUDGET:?} the fast path allows. The ceiling may be tightened here, \
                 never raised."
            )));
        }

        let max_cycles = number(vars, "QIP_FASTBRAIN_MAX_CYCLES")?;
        let max_runtime = number(vars, "QIP_FASTBRAIN_MAX_RUNTIME_SECS")?
            .map(|seconds| Duration::from_secs(seconds.min(i64::MAX as u64) as i64));

        let breach_tolerance = number(vars, "QIP_FASTBRAIN_BREACH_TOLERANCE")?
            .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
            .unwrap_or(defaults.breach_tolerance);

        let shutdown_budget =
            millis(vars, "QIP_FASTBRAIN_SHUTDOWN_BUDGET_MS")?.unwrap_or(defaults.shutdown_budget);

        let archive_every =
            number(vars, "QIP_FASTBRAIN_ARCHIVE_EVERY")?.unwrap_or(defaults.archive_every);

        Ok(Self {
            health_address,
            cycle_interval,
            cycle_budget,
            max_cycles,
            max_runtime,
            breach_tolerance,
            archive_every,
            // Prefixed so `main` can exit with the configuration code rather
            // than the general one: an orchestrator that cannot tell "deployed
            // wrong" from "broke" restarts the first forever.
            storage: StorageSettings::from_values(
                text(vars, TARGET_VARIABLE).as_deref(),
                text(vars, ROOT_VARIABLE).as_deref(),
            )
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?,
            replay_path: text(vars, "QIP_FASTBRAIN_REPLAY_PATH"),
            seed: number(vars, "QIP_FASTBRAIN_SEED")?.unwrap_or(defaults.seed),
            shutdown_budget,
        })
    }

    /// Whether the run stops on its own.
    ///
    /// Reported in the banner because "this node will stop by itself" and "this
    /// node stays up until something stops it" are different operational
    /// promises and an operator should not have to infer which one is in force.
    pub fn run_is_bounded(&self) -> bool {
        self.max_cycles.is_some() || self.max_runtime.is_some()
    }
}

/// A non-empty value, trimmed. Empty is treated as unset: a variable set to the
/// empty string in a manifest is a variable somebody forgot to fill in.
fn text(vars: &BTreeMap<String, String>, name: &str) -> Option<String> {
    vars.get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn number(vars: &BTreeMap<String, String>, name: &str) -> Result<Option<u64>> {
    let Some(raw) = text(vars, name) else {
        return Ok(None);
    };
    raw.parse::<u64>().map(Some).map_err(|_| {
        Error::invalid(format!(
            "configuration: {name} is not a non-negative whole number: {raw}"
        ))
    })
}

fn millis(vars: &BTreeMap<String, String>, name: &str) -> Result<Option<Duration>> {
    Ok(number(vars, name)?.map(|value| Duration::from_millis(value.min(i64::MAX as u64) as i64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn an_empty_environment_yields_a_node_that_serves_on_all_interfaces_and_never_stops_itself() {
        let config = FastBrainConfig::parse(&vars(&[])).expect("defaults are valid");
        assert_eq!(config.health_address, DEFAULT_HEALTH_ADDRESS);
        assert_eq!(config.cycle_interval, DEFAULT_CYCLE_INTERVAL);
        assert_eq!(config.cycle_budget, MAXIMUM_BUDGET);
        assert!(!config.run_is_bounded());
        assert!(
            !config.storage.is_durable(),
            "an unconfigured node must not claim to persist anything"
        );
        assert!(config.replay_path.is_none());
    }

    #[test]
    fn the_environment_may_tighten_the_fast_path_ceiling() {
        let config = FastBrainConfig::parse(&vars(&[("QIP_FASTBRAIN_CYCLE_BUDGET_MS", "5")]))
            .expect("a tighter ceiling is permitted");
        assert_eq!(config.cycle_budget, Duration::from_millis(5));
        assert!(config.cycle_budget < MAXIMUM_BUDGET);
    }

    #[test]
    fn the_environment_may_not_raise_the_fast_path_ceiling_above_what_the_roster_check_enforces() {
        let asked = MAXIMUM_BUDGET.as_millis() + 1;
        let refusal = FastBrainConfig::parse(&vars(&[(
            "QIP_FASTBRAIN_CYCLE_BUDGET_MS",
            &asked.to_string(),
        )]))
        .expect_err("a ceiling above the fast-path maximum is not configuration, it is a waiver");
        assert!(
            refusal.message().contains("never raised"),
            "the refusal does not say the ceiling is one-directional: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_zero_cycle_interval_is_refused_because_it_describes_a_spin_rather_than_a_clock() {
        let refusal = FastBrainConfig::parse(&vars(&[("QIP_FASTBRAIN_CYCLE_INTERVAL_MS", "0")]))
            .expect_err("a zero interval is not a schedule");
        assert!(refusal.message().contains("spin"), "{}", refusal.message());
    }

    #[test]
    fn a_value_that_is_not_a_number_names_itself_in_the_refusal() {
        let refusal = FastBrainConfig::parse(&vars(&[("QIP_FASTBRAIN_MAX_CYCLES", "soon")]))
            .expect_err("`soon` is not a cycle count");
        assert!(
            refusal.message().contains("QIP_FASTBRAIN_MAX_CYCLES")
                && refusal.message().contains("soon"),
            "the refusal names neither the variable nor the value: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_variable_set_to_the_empty_string_is_treated_as_unset_rather_than_as_an_empty_path() {
        let config = FastBrainConfig::parse(&vars(&[
            ("QIP_FASTBRAIN_REPLAY_PATH", "   "),
            ("QIP_FASTBRAIN_HEALTH_ADDRESS", ""),
        ]))
        .expect("blank values fall back to defaults");
        assert!(config.replay_path.is_none());
        assert_eq!(config.health_address, DEFAULT_HEALTH_ADDRESS);
    }

    #[test]
    fn the_storage_refusals_every_other_binary_gets_are_the_ones_this_node_gets() {
        // Not restated here, and asserted here so that a change to them is a
        // change to this node too rather than a divergence nobody noticed.
        let refusal = FastBrainConfig::parse(&vars(&[(TARGET_VARIABLE, "engine")]))
            .expect_err("a durable target with no root has no default directory");
        assert!(
            refusal.message().starts_with("configuration:")
                && refusal.message().contains(ROOT_VARIABLE),
            "the refusal does not name the missing variable: {}",
            refusal.message()
        );

        let refusal = FastBrainConfig::parse(&vars(&[(ROOT_VARIABLE, "/var/lib/qip")]))
            .expect_err("a root beside the memory target is a belief about persistence");
        assert!(
            refusal.message().starts_with("configuration:")
                && refusal.message().contains("lose everything at the restart"),
            "the refusal does not say what it is protecting: {}",
            refusal.message()
        );
    }

    #[test]
    fn either_bound_alone_makes_the_run_a_bounded_one() {
        let by_cycles = FastBrainConfig::parse(&vars(&[("QIP_FASTBRAIN_MAX_CYCLES", "3")]))
            .expect("a cycle bound is valid");
        assert_eq!(by_cycles.max_cycles, Some(3));
        assert!(by_cycles.run_is_bounded());

        let by_time = FastBrainConfig::parse(&vars(&[("QIP_FASTBRAIN_MAX_RUNTIME_SECS", "30")]))
            .expect("a time bound is valid");
        assert_eq!(by_time.max_runtime, Some(Duration::from_secs(30)));
        assert!(by_time.run_is_bounded());
    }
}
