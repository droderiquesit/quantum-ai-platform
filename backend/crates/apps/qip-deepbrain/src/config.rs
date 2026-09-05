//! What the node reads from its environment before it will run.
//!
//! Every value has a default, and "safe" here means something different from
//! what it means on the fast path. The fast brain's dangerous misconfiguration
//! is a *loosened* ceiling, so its budget may only ever be tightened. This
//! node's dangerous misconfiguration is the opposite one: a deep brain given a
//! fast brain's cadence does not become quick, it becomes a loop that starts a
//! research cycle before the last one finished, bills for language-model calls
//! and finishes nothing. So the guard that is one-directional here is a *floor*
//! under the cycle interval, and it is a refusal rather than a clamp because an
//! operator who asked for a hundred-millisecond deep brain has misunderstood
//! which node they are configuring and should be told so.
//!
//! Nothing is read from a file in the repository and nothing is a credential.
//! Parsing takes a map rather than reading the process environment, so the
//! defaults and the refusals are asserted directly instead of by setting
//! variables in a process that other tests share.

use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_kernel::EventLogDestination;
use qip_reasoning_engine::providers::huggingface::{HF_TOKEN_VARIABLE, HuggingFaceToken};
use qip_storage::settings::StorageSettings;
use std::collections::BTreeMap;

/// Where the health surface binds when the environment does not say.
///
/// All interfaces, because a probe reaches the pod from outside it. The one
/// request that changes anything — the quiesce request — is refused unless it
/// came from loopback, so binding widely does not hand anyone a stop button.
pub const DEFAULT_HEALTH_ADDRESS: &str = "0.0.0.0:8080";

/// The cadence the deployment already asks for.
///
/// Five minutes, matching `cycle_interval_seconds` in the platform config map,
/// which `infrastructure/kubernetes/base/deepbrain.yaml` already passes to this
/// container. The default is that number rather than one invented here so the
/// binary and the manifest cannot disagree about what this node's cadence is.
pub const DEFAULT_CYCLE_INTERVAL: Duration = Duration::from_secs(300);

/// The shortest cycle interval this node will accept.
///
/// Not a performance target — a floor against a category error. A cycle on this
/// node runs discovery, causal reasoning, simulation, optimisation and
/// learning, and the agents it hosts may consult a language model; a second is
/// already far below anything it could finish in. What this refusal is really
/// protecting against is a deep brain configured from the fast brain's
/// numbers, which would spin, spend and produce nothing while reporting itself
/// perfectly healthy.
pub const MINIMUM_CYCLE_INTERVAL: Duration = Duration::from_secs(1);

/// How long the node waits for the flush on the way out.
///
/// Well inside the 120-second `terminationGracePeriodSeconds` the Deployment
/// grants, so the flush finishes or reports what it left behind before the
/// orchestrator stops waiting and sends `SIGKILL`.
pub const DEFAULT_SHUTDOWN_BUDGET: Duration = Duration::from_secs(30);

/// How many cycles pass between hand-overs to the chain archive.
///
/// Every cycle, which is where this node differs most sharply from the fast
/// brain's hundred. There the arithmetic is a real trade: cycles are
/// milliseconds apart, a store write between two of them is a measurable
/// fraction of the interval, and batching a hundred costs ten seconds of record
/// in a crash. Here a cycle is minutes and the archive write is milliseconds,
/// so batching would save nothing anybody could measure and would risk losing a
/// whole cycle of research — which on this node is the expensive thing in the
/// system, not a market tick that will be along again shortly.
pub const DEFAULT_ARCHIVE_EVERY: u64 = 1;

/// Consecutive failed cycles tolerated before the node reports itself unready.
///
/// Not one: a stage that fails once is a data source that timed out, and a node
/// that left rotation for it would flap. Not many either — a cycle that has
/// skipped a stage three times running is not researching, and the point of
/// having a readiness signal at all is that it can say so.
pub const DEFAULT_FAILURE_TOLERANCE: u32 = 3;

/// The environment variable this node reads for its cadence first.
pub const CYCLE_INTERVAL_VARIABLE: &str = "QIP_DEEPBRAIN_CYCLE_INTERVAL_SECS";

/// The platform-wide cadence variable, read when the node-specific one is
/// unset.
///
/// The Deployment already sets this one from the config map. Reading it means a
/// cluster that configured the platform's cadence once does not have to
/// discover that the deep brain wanted it spelled differently — and the
/// node-specific variable still wins, so a single node can be slowed down for
/// an experiment without editing a shared config map.
pub const SHARED_CYCLE_INTERVAL_VARIABLE: &str = "QIP_CYCLE_INTERVAL_SECONDS";

/// The variable naming the event log's file, when a deployment wants one.
pub const EVENT_LOG_VARIABLE: &str = "QIP_DEEPBRAIN_EVENT_LOG";

/// The event log file's name under the storage root, when one is not named
/// explicitly.
pub const DEFAULT_EVENT_LOG_FILE: &str = "deepbrain-events.jsonl";

/// Which hosted language-model provider this node installs ahead of the
/// deterministic model (ADR 0037, decision 4). Unset means no hosted model
/// and every reasoning run narrates through templates, which is the state of
/// every deployment today. The one accepted value is [`HUGGING_FACE_PROVIDER`];
/// any other is refused by name rather than read as "none", because a
/// provider misspelled is a deployment that believes it is calling a model
/// and is not.
pub const LANGUAGE_MODEL_PROVIDER_VARIABLE: &str = "QIP_LANGUAGE_MODEL_PROVIDER";

/// The one provider this build can construct.
pub const HUGGING_FACE_PROVIDER: &str = "huggingface";

/// The model identifier as the router names it. Required when the provider
/// is set: there is no default model that is not a guess about price and
/// terms nobody read.
pub const LANGUAGE_MODEL_VARIABLE: &str = "QIP_LANGUAGE_MODEL";

/// `http://127.0.0.1:<port>` of the egress proxy's `huggingface` listener.
/// Required when the provider is set, and refused unless it is loopback: the
/// proxy is the only outbound path (ADR 0024), the transport has no TLS, and
/// a base URL naming anything else is a bearer token sent in clear text to
/// wherever it points.
pub const LANGUAGE_MODEL_BASE_URL_VARIABLE: &str = "QIP_LANGUAGE_MODEL_BASE_URL";

/// A hosted language model this node was told to install.
///
/// Carried on the configuration rather than assembled in `main`, so the
/// decision "which model narrates" is asserted by a test against the
/// variables rather than watched on a running process. The token is here
/// too, in a type that redacts in `Debug`; this struct therefore derives
/// neither `Serialize` nor `Deserialize`, and cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedLanguageModel {
    /// The model identifier as the router names it.
    pub model: String,
    /// The loopback listener, validated to be one.
    pub base_url: String,
    /// The credential, when the deployment supplied one. `None` is the
    /// built-dark state: the adapter is installed, reports itself
    /// unavailable naming the variable, and the chain falls through to the
    /// deterministic model.
    pub token: Option<HuggingFaceToken>,
}

/// Everything the node needs to run, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepBrainConfig {
    /// Where the health surface binds.
    pub health_address: String,
    /// How often a cycle starts.
    ///
    /// A cadence, not a deadline: a cycle that outruns it is reported and the
    /// next one starts immediately, because on this node a long cycle is
    /// research, not a fault.
    pub cycle_interval: Duration,
    /// Stop cleanly after this many cycles. `None` runs until asked to stop.
    pub max_cycles: Option<u64>,
    /// Stop cleanly after this long. `None` runs until asked to stop.
    pub max_runtime: Option<Duration>,
    /// Consecutive failed cycles tolerated before the node reports unready.
    pub failure_tolerance: u32,
    /// Cycles between hand-overs to the chain archive. Zero archives only on
    /// the way out.
    pub archive_every: u64,
    /// Where the archive and everything else durable goes.
    ///
    /// The same two variables every other binary reads. The refusals — an
    /// unrecognised target, a durable target with no root, a root beside the
    /// memory target — come from there and are not restated here.
    pub storage: StorageSettings,
    /// Where the kernel's own event log is written.
    ///
    /// Resolved here rather than left to the kernel's default because this is
    /// the deployment's decision, and because it is the one place a reader can
    /// see that this node's evidence has somewhere to go.
    pub event_log: EventLogDestination,
    /// How long the shutdown flush may take before the node gives up on it.
    pub shutdown_budget: Duration,
    /// A committed bitemporal tape to run through on its own clock instead
    /// of the synthetic exchange. Mutually exclusive with
    /// `QIP_DEEPBRAIN_REPLAY_PATH`, which `main` refuses beside it. See
    /// `qip_market_ingestion::tape` for why this is not the replay.
    pub tape_path: Option<String>,
    /// The hosted language model to install first in the fallback chain, when
    /// `QIP_LANGUAGE_MODEL_PROVIDER` names one. `None` is the deterministic
    /// model alone.
    pub language_model: Option<HostedLanguageModel>,
}

impl Default for DeepBrainConfig {
    fn default() -> Self {
        Self {
            health_address: DEFAULT_HEALTH_ADDRESS.to_string(),
            cycle_interval: DEFAULT_CYCLE_INTERVAL,
            max_cycles: None,
            max_runtime: None,
            failure_tolerance: DEFAULT_FAILURE_TOLERANCE,
            archive_every: DEFAULT_ARCHIVE_EVERY,
            storage: StorageSettings::in_memory(),
            event_log: EventLogDestination::InMemory,
            shutdown_budget: DEFAULT_SHUTDOWN_BUDGET,
            tape_path: None,
            language_model: None,
        }
    }
}

impl DeepBrainConfig {
    /// Read the process environment.
    pub fn from_env() -> Result<Self> {
        Self::parse(&std::env::vars().collect())
    }

    /// Resolve a configuration from a set of variables.
    ///
    /// A missing variable is a default rather than a refusal, with the single
    /// exception of the storage pair, whose refusals exist precisely because a
    /// deployment that believes it persists and does not passes every smoke
    /// test it has.
    pub fn parse(vars: &BTreeMap<String, String>) -> Result<Self> {
        let defaults = Self::default();

        let health_address =
            text(vars, "QIP_DEEPBRAIN_HEALTH_ADDRESS").unwrap_or(defaults.health_address);

        let cycle_interval = match seconds(vars, CYCLE_INTERVAL_VARIABLE)? {
            Some(interval) => interval,
            None => {
                seconds(vars, SHARED_CYCLE_INTERVAL_VARIABLE)?.unwrap_or(defaults.cycle_interval)
            }
        };
        if cycle_interval < MINIMUM_CYCLE_INTERVAL {
            return Err(Error::invalid(format!(
                "configuration: a cycle interval of {cycle_interval:?} is below the \
                 {MINIMUM_CYCLE_INTERVAL:?} floor. The deep brain runs discovery, causal \
                 reasoning, simulation, optimisation and learning, and may consult a language \
                 model; at this cadence it would start a cycle before the last one finished and \
                 bill for the privilege. This is the fast brain's cadence, and the fast brain is \
                 a different binary."
            )));
        }

        let max_cycles = number(vars, "QIP_DEEPBRAIN_MAX_CYCLES")?;
        let max_runtime = seconds(vars, "QIP_DEEPBRAIN_MAX_RUNTIME_SECS")?;

        let failure_tolerance = number(vars, "QIP_DEEPBRAIN_FAILURE_TOLERANCE")?
            .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
            .unwrap_or(defaults.failure_tolerance);

        let archive_every =
            number(vars, "QIP_DEEPBRAIN_ARCHIVE_EVERY")?.unwrap_or(defaults.archive_every);

        let shutdown_budget = seconds(vars, "QIP_DEEPBRAIN_SHUTDOWN_BUDGET_SECS")?
            .unwrap_or(defaults.shutdown_budget);

        // Prefixed so `main` can exit with the configuration code rather than
        // the general one: an orchestrator that cannot tell "deployed wrong"
        // from "broke" restarts the first forever.
        //
        // The variables are looked up in `vars` by the library rather than
        // read by it, so a managed target's credential is resolved here, in
        // the composition root, through `qip_core::secret`.
        let storage = StorageSettings::from_env(&|name| text(vars, name))
            .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;

        let event_log = event_log_destination(vars, &storage)?;

        let language_model = hosted_language_model(vars)?;

        Ok(Self {
            health_address,
            cycle_interval,
            max_cycles,
            max_runtime,
            failure_tolerance,
            archive_every,
            storage,
            event_log,
            shutdown_budget,
            tape_path: text(vars, "QIP_DEEPBRAIN_TAPE_PATH"),
            language_model,
        })
    }

    /// Whether the run stops on its own.
    ///
    /// In the banner because "this node will stop by itself" and "this node
    /// stays up until something stops it" are different operational promises,
    /// and an operator should not have to infer which one is in force.
    pub fn run_is_bounded(&self) -> bool {
        self.max_cycles.is_some() || self.max_runtime.is_some()
    }

    /// The one thing the two durability banners cannot say between them.
    ///
    /// This node has two places evidence goes and they are configured
    /// separately: the kernel's event log, which is a file path, and the chain
    /// archive, which is a key-value store. `StorageSettings::banner_lines`
    /// describes the second and [`EventLogDestination::describe`] describes the
    /// first, and a reader who sees "NOTHING SURVIVES A RESTART" from one and a
    /// file path from the other is entitled to wonder which is true.
    ///
    /// Both are. The combination is reachable — a named log file beside the
    /// default memory store — and it is worth a sentence rather than an
    /// inference, because what it means is subtle: the log keeps accumulating
    /// across restarts while the archive begins again empty each time, so the
    /// two stop agreeing about how much history exists.
    ///
    /// Extracted here rather than written inline in the banner so the condition
    /// is asserted rather than eyeballed on a running process.
    pub fn durability_note(&self) -> Option<&'static str> {
        match (
            self.event_log.survives_this_process(),
            self.storage.is_durable(),
        ) {
            (true, false) => Some(
                "the event log outlives this process but the chain archive does not; a \
                 restart keeps appending to the log file and starts the archive again from \
                 nothing",
            ),
            _ => None,
        }
    }
}

/// Where the event log goes, given the variables and the resolved storage.
///
/// Three cases, and the middle one is the reason this is a function with a name
/// rather than three lines inside `parse`:
///
/// * A path was named. It is used, whatever the storage target is — a
///   deployment that named a file meant it.
/// * No path, but storage is durable. The log goes beside the durable store,
///   because a node that was configured to keep its archive and silently threw
///   away the log the archive is made of would be keeping half an audit trail.
/// * No path and no durable storage. In memory, and the banner says so. The
///   alternative — defaulting to a file — writes to a directory nobody chose,
///   which on a read-only root filesystem fails at start-up and on a writable
///   one quietly fills a container's ephemeral disk.
fn event_log_destination(
    vars: &BTreeMap<String, String>,
    storage: &StorageSettings,
) -> Result<EventLogDestination> {
    if let Some(path) = text(vars, EVENT_LOG_VARIABLE) {
        return Ok(EventLogDestination::file(path));
    }
    if storage.is_durable() && !storage.root().as_os_str().is_empty() {
        return Ok(EventLogDestination::file(
            storage.root().join(DEFAULT_EVENT_LOG_FILE),
        ));
    }
    Ok(EventLogDestination::InMemory)
}

/// The hosted language model the variables describe, or `None` when the
/// provider is unset.
///
/// Every refusal is a configuration fault, prefixed so `main` exits with the
/// configuration code. The shape is deliberately all-or-nothing once the
/// provider is named: a model with no listener, or a listener with no model,
/// is a deployment half-configured, and a process that started on half of it
/// would narrate through templates while its manifest said otherwise.
fn hosted_language_model(vars: &BTreeMap<String, String>) -> Result<Option<HostedLanguageModel>> {
    let Some(provider) = text(vars, LANGUAGE_MODEL_PROVIDER_VARIABLE) else {
        // Unset is the deterministic model alone. A model or a listener named
        // without a provider is refused, not ignored: a deployment that set
        // two of three variables did not mean "no hosted model".
        for variable in [LANGUAGE_MODEL_VARIABLE, LANGUAGE_MODEL_BASE_URL_VARIABLE] {
            if text(vars, variable).is_some() {
                return Err(Error::invalid(format!(
                    "configuration: {variable} is set and {LANGUAGE_MODEL_PROVIDER_VARIABLE} \
                     is not. Set the provider to `{HUGGING_FACE_PROVIDER}` to install the hosted \
                     model, or unset {variable} to narrate through the deterministic model"
                )));
            }
        }
        return Ok(None);
    };
    if provider != HUGGING_FACE_PROVIDER {
        return Err(Error::invalid(format!(
            "configuration: {LANGUAGE_MODEL_PROVIDER_VARIABLE} is `{provider}`, which this build \
             cannot construct. The one provider is `{HUGGING_FACE_PROVIDER}` (ADR 0037); unset \
             the variable to narrate through the deterministic model"
        )));
    }
    let model = text(vars, LANGUAGE_MODEL_VARIABLE).ok_or_else(|| {
        Error::invalid(format!(
            "configuration: {LANGUAGE_MODEL_PROVIDER_VARIABLE} is `{HUGGING_FACE_PROVIDER}` and \
             {LANGUAGE_MODEL_VARIABLE} is not set. Name the model as the router's catalogue \
             spells it; there is no default that is not a guess about price and terms"
        ))
    })?;
    let base_url = text(vars, LANGUAGE_MODEL_BASE_URL_VARIABLE).ok_or_else(|| {
        Error::invalid(format!(
            "configuration: {LANGUAGE_MODEL_PROVIDER_VARIABLE} is `{HUGGING_FACE_PROVIDER}` and \
             {LANGUAGE_MODEL_BASE_URL_VARIABLE} is not set. Point it at the egress proxy's \
             `huggingface` listener, http://127.0.0.1:<port>; the proxy is the only path out"
        ))
    })?;
    if !(base_url.starts_with("http://127.0.0.1:") || base_url.starts_with("http://localhost:")) {
        return Err(Error::invalid(format!(
            "configuration: {LANGUAGE_MODEL_BASE_URL_VARIABLE} is {base_url}. It must be \
             http://127.0.0.1:<port> or http://localhost:<port>, the egress proxy's \
             `huggingface` listener beside this process. The proxy is the only path to \
             router.huggingface.co: this transport has no TLS, and a base URL naming anything \
             else is a bearer token sent in clear text to wherever it points (ADR 0024)"
        )));
    }
    // Through `qip_core::secret`, so the deployment may mount the credential
    // as a file rather than set it: a credential in the environment is in
    // `/proc/<pid>/environ`, in every child process and in every crash dump.
    // The file variable's name is composed from `FILE_SUFFIX` rather than
    // written out, because a `"QIP_…"` literal here is exactly what
    // `manifest_wiring`'s walk counts as a variable this binary reads.
    let token = qip_core::secret::resolve_from(
        HF_TOKEN_VARIABLE,
        text(vars, HF_TOKEN_VARIABLE),
        text(
            vars,
            &format!("{HF_TOKEN_VARIABLE}{}", qip_core::secret::FILE_SUFFIX),
        ),
    )
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?
    .map(HuggingFaceToken::new)
    .transpose()
    .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?;
    Ok(Some(HostedLanguageModel {
        model,
        base_url,
        token,
    }))
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

fn seconds(vars: &BTreeMap<String, String>, name: &str) -> Result<Option<Duration>> {
    Ok(number(vars, name)?.map(|value| Duration::from_secs(value.min(i64::MAX as u64) as i64)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_storage::settings::{ROOT_VARIABLE, TARGET_VARIABLE};

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn an_empty_environment_yields_a_node_that_cycles_at_the_cadence_the_config_map_asks_for() {
        let config = DeepBrainConfig::parse(&vars(&[])).expect("defaults are valid");
        assert_eq!(config.health_address, DEFAULT_HEALTH_ADDRESS);
        assert_eq!(config.cycle_interval, Duration::from_secs(300));
        assert!(!config.run_is_bounded());
        assert!(
            !config.storage.is_durable(),
            "an unconfigured node must not claim to persist anything"
        );
        assert_eq!(
            config.event_log,
            EventLogDestination::InMemory,
            "an unconfigured node chose a file on disk to write its evidence to"
        );
    }

    #[test]
    fn the_cadence_this_node_defaults_to_is_the_one_the_deployment_already_passes_it() {
        // The manifest sets QIP_CYCLE_INTERVAL_SECONDS from the config map, and
        // the default above is that same number. Asserting they agree is what
        // stops the binary and the deployment drifting into two cadences.
        let from_manifest =
            DeepBrainConfig::parse(&vars(&[(SHARED_CYCLE_INTERVAL_VARIABLE, "300")]))
                .expect("the platform cadence is valid");
        assert_eq!(from_manifest.cycle_interval, DEFAULT_CYCLE_INTERVAL);
    }

    #[test]
    fn a_node_specific_cadence_overrides_the_platform_wide_one() {
        // So one node can be slowed for an experiment without editing a config
        // map every other workload reads.
        let config = DeepBrainConfig::parse(&vars(&[
            (SHARED_CYCLE_INTERVAL_VARIABLE, "300"),
            (CYCLE_INTERVAL_VARIABLE, "900"),
        ]))
        .expect("both cadences are valid");
        assert_eq!(config.cycle_interval, Duration::from_secs(900));
    }

    #[test]
    fn the_environment_may_slow_this_node_down_without_limit() {
        // The opposite of the fast brain's rule, and deliberately so: a longer
        // cycle here is a more careful one, and nothing about it is unsafe.
        let config = DeepBrainConfig::parse(&vars(&[(CYCLE_INTERVAL_VARIABLE, "86400")]))
            .expect("a daily cadence is a legitimate deep brain");
        assert_eq!(config.cycle_interval, Duration::from_secs(86_400));
    }

    #[test]
    fn a_fast_brain_cadence_is_refused_and_the_refusal_says_it_is_the_wrong_binary() {
        let refusal = DeepBrainConfig::parse(&vars(&[(CYCLE_INTERVAL_VARIABLE, "0")]))
            .expect_err("a zero interval is not a deep brain cadence");
        assert!(
            refusal.message().contains("different binary"),
            "the refusal does not tell the operator what they have confused: {}",
            refusal.message()
        );
        assert!(refusal.message().starts_with("configuration:"));
    }

    #[test]
    fn a_value_that_is_not_a_number_names_itself_in_the_refusal() {
        let refusal = DeepBrainConfig::parse(&vars(&[("QIP_DEEPBRAIN_MAX_CYCLES", "soon")]))
            .expect_err("`soon` is not a cycle count");
        assert!(
            refusal.message().contains("QIP_DEEPBRAIN_MAX_CYCLES")
                && refusal.message().contains("soon"),
            "the refusal names neither the variable nor the value: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_variable_set_to_the_empty_string_is_treated_as_unset_rather_than_as_an_empty_path() {
        let config = DeepBrainConfig::parse(&vars(&[
            (EVENT_LOG_VARIABLE, "   "),
            ("QIP_DEEPBRAIN_HEALTH_ADDRESS", ""),
        ]))
        .expect("blank values fall back to defaults");
        assert_eq!(config.event_log, EventLogDestination::InMemory);
        assert_eq!(config.health_address, DEFAULT_HEALTH_ADDRESS);
    }

    #[test]
    fn a_named_event_log_path_is_used_whatever_the_storage_target_is() {
        let config =
            DeepBrainConfig::parse(&vars(&[(EVENT_LOG_VARIABLE, "/var/lib/qip/log.jsonl")]))
                .expect("naming a log file is valid");
        assert_eq!(
            config.event_log.path(),
            Some(std::path::Path::new("/var/lib/qip/log.jsonl"))
        );
        assert!(
            config.event_log.survives_power_loss(),
            "a deployment that named a file got the fast, lossy answer by default"
        );
    }

    #[test]
    fn a_durable_store_puts_the_event_log_beside_it_rather_than_keeping_it_in_memory() {
        // The half-audit-trail case: keeping the archive and discarding the log
        // the archive is made of.
        let config = DeepBrainConfig::parse(&vars(&[
            (TARGET_VARIABLE, "engine"),
            (ROOT_VARIABLE, "/var/lib/qip"),
        ]))
        .expect("a durable target with a root is valid");
        assert!(
            config.storage.is_durable(),
            "the premise: storage is durable"
        );
        assert_eq!(
            config.event_log.path(),
            Some(
                std::path::Path::new("/var/lib/qip")
                    .join(DEFAULT_EVENT_LOG_FILE)
                    .as_path()
            )
        );
    }

    #[test]
    fn the_storage_refusals_every_other_binary_gets_are_the_ones_this_node_gets() {
        // Not restated here, and asserted here so a change to them is a change
        // to this node too rather than a divergence nobody noticed.
        let refusal = DeepBrainConfig::parse(&vars(&[(TARGET_VARIABLE, "engine")]))
            .expect_err("a durable target with no root has no default directory");
        assert!(
            refusal.message().starts_with("configuration:")
                && refusal.message().contains(ROOT_VARIABLE),
            "the refusal does not name the missing variable: {}",
            refusal.message()
        );

        let refusal = DeepBrainConfig::parse(&vars(&[(ROOT_VARIABLE, "/var/lib/qip")]))
            .expect_err("a root beside the memory target is a belief about persistence");
        assert!(
            refusal.message().contains("lose everything at the restart"),
            "the refusal does not say what it is protecting: {}",
            refusal.message()
        );
    }

    #[test]
    fn either_bound_alone_makes_the_run_a_bounded_one() {
        let by_cycles = DeepBrainConfig::parse(&vars(&[("QIP_DEEPBRAIN_MAX_CYCLES", "3")]))
            .expect("a cycle bound is valid");
        assert_eq!(by_cycles.max_cycles, Some(3));
        assert!(by_cycles.run_is_bounded());

        let by_time = DeepBrainConfig::parse(&vars(&[("QIP_DEEPBRAIN_MAX_RUNTIME_SECS", "600")]))
            .expect("a time bound is valid");
        assert_eq!(by_time.max_runtime, Some(Duration::from_secs(600)));
        assert!(by_time.run_is_bounded());
    }

    #[test]
    fn a_log_that_outlives_the_process_beside_an_archive_that_does_not_is_called_out() {
        // Reachable, and quiet: the storage banner says nothing survives while
        // the event log line names a file, and neither one owns the sentence
        // that reconciles them.
        let mixed = DeepBrainConfig::parse(&vars(&[(EVENT_LOG_VARIABLE, "/tmp/qip-events.jsonl")]))
            .expect("a named log beside the memory store is valid");
        assert!(
            mixed.event_log.survives_this_process() && !mixed.storage.is_durable(),
            "the premise: the log outlives the process and the archive does not"
        );
        let note = mixed
            .durability_note()
            .expect("the mismatch is not called out anywhere");
        assert!(note.contains("starts the archive again from nothing"));
    }

    #[test]
    fn a_configuration_whose_two_stores_agree_says_nothing_extra() {
        // No note when there is nothing surprising, so the note keeps meaning
        // something when it does appear.
        assert!(
            DeepBrainConfig::default().durability_note().is_none(),
            "an all-in-memory node was given a warning about a mismatch it does not have"
        );

        let durable = DeepBrainConfig::parse(&vars(&[
            (TARGET_VARIABLE, "engine"),
            (ROOT_VARIABLE, "/var/lib/qip"),
        ]))
        .expect("a durable configuration is valid");
        assert!(
            durable.durability_note().is_none(),
            "a node whose log and archive both persist was warned that they disagree"
        );
    }

    /// Shaped like a token and not one.
    const TEST_TOKEN: &str = "hf_not_a_real_token_for_this_test";

    fn hosted(pairs: &[(&str, &str)]) -> Vec<(&'static str, &'static str)> {
        let mut all = vec![
            (LANGUAGE_MODEL_PROVIDER_VARIABLE, HUGGING_FACE_PROVIDER),
            (LANGUAGE_MODEL_VARIABLE, "example-org/example-model"),
            (LANGUAGE_MODEL_BASE_URL_VARIABLE, "http://127.0.0.1:9106"),
            (HF_TOKEN_VARIABLE, TEST_TOKEN),
        ];
        for (name, value) in pairs {
            all.retain(|(existing, _)| existing != name);
            if !value.is_empty() {
                // Leaked deliberately: test-only, a handful of short strings.
                all.push((
                    Box::leak((*name).to_string().into_boxed_str()),
                    Box::leak((*value).to_string().into_boxed_str()),
                ));
            }
        }
        all
    }

    #[test]
    fn with_no_provider_variable_no_hosted_language_model_is_configured() {
        // The state of every deployment today, and the default this node must
        // keep: ADR 0037 is built dark. The failure this prevents is a
        // default provider appearing, so an unconfigured node reaches for a
        // credential it was never given.
        let config = DeepBrainConfig::parse(&vars(&[])).expect("defaults are valid");
        assert!(config.language_model.is_none());
    }

    #[test]
    fn the_hugging_face_provider_with_model_listener_and_token_configures_the_hosted_model() {
        let config = DeepBrainConfig::parse(&vars(&hosted(&[]))).expect("fully configured");
        assert!(
            !format!("{config:?}").contains(TEST_TOKEN),
            "the configuration's Debug output carries the credential"
        );
        let model = config
            .language_model
            .expect("the provider was named, so a hosted model is configured");
        assert_eq!(model.model, "example-org/example-model");
        assert_eq!(model.base_url, "http://127.0.0.1:9106");
        assert!(model.token.is_some(), "the token was set and not resolved");
    }

    #[test]
    fn the_token_may_arrive_through_the_file_indirection_and_not_through_both_at_once() {
        // The Secret Manager mount projects a file (ADR 0024); a root that read
        // only the environment would report a mounted credential absent.
        let dir =
            std::env::temp_dir().join(format!("qip-deepbrain-hf-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("token");
        std::fs::write(&path, format!("{TEST_TOKEN}\n")).expect("the token file is written");
        let file_variable = format!("{HF_TOKEN_VARIABLE}{}", qip_core::secret::FILE_SUFFIX);
        let path_text = path.to_string_lossy().to_string();

        let config = DeepBrainConfig::parse(&vars(&hosted(&[
            (HF_TOKEN_VARIABLE, ""),
            (&file_variable, &path_text),
        ])))
        .expect("a token mounted as a file resolves");
        let model = config.language_model.expect("configured");
        assert_eq!(
            model.token,
            Some(HuggingFaceToken::new(TEST_TOKEN).expect("well formed")),
            "the file's contents were not the token, or its trailing newline was kept"
        );

        let refusal = DeepBrainConfig::parse(&vars(&hosted(&[(&file_variable, &path_text)])))
            .expect_err("both spellings set at once is an ambiguity, not a choice");
        assert!(
            refusal.message().starts_with("configuration:")
                && refusal.message().contains("both set"),
            "{}",
            refusal.message()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_model_listener_or_token_is_refused_or_reported_by_name() {
        // The failure this prevents: a half-configured provider starting on
        // the half it has, narrating through templates while its manifest
        // says a model is installed. The model and the listener are refusals;
        // the token's absence is the built-dark state and is reported by the
        // adapter, not refused here (see `language.rs`).
        for missing in [LANGUAGE_MODEL_VARIABLE, LANGUAGE_MODEL_BASE_URL_VARIABLE] {
            let refusal = DeepBrainConfig::parse(&vars(&hosted(&[(missing, "")])))
                .err()
                .unwrap_or_else(|| panic!("a provider without {missing} was admitted"));
            assert!(
                refusal.message().starts_with("configuration:")
                    && refusal.message().contains(missing),
                "the refusal does not name {missing}: {}",
                refusal.message()
            );
        }
        let dark = DeepBrainConfig::parse(&vars(&hosted(&[(HF_TOKEN_VARIABLE, "")])))
            .expect("a provider without a credential is the built-dark state");
        assert!(dark.language_model.expect("configured").token.is_none());
    }

    #[test]
    fn a_base_url_that_is_not_loopback_is_refused_and_the_refusal_names_the_proxy() {
        // The failure this prevents: the adapter pointed at the vendor, or at
        // any address off this instance, so a bearer token leaves in clear
        // text. The proxy is the only path (ADR 0024).
        for bad in [
            "http://router.huggingface.co/",
            "http://10.0.0.5:9106",
            "https://127.0.0.1:9106",
            "http://127.0.0.1",
        ] {
            let refusal =
                DeepBrainConfig::parse(&vars(&hosted(&[(LANGUAGE_MODEL_BASE_URL_VARIABLE, bad)])))
                    .err()
                    .unwrap_or_else(|| panic!("{bad} was admitted as a listener"));
            assert!(
                refusal.message().starts_with("configuration:")
                    && refusal.message().contains("proxy")
                    && refusal.message().contains(LANGUAGE_MODEL_BASE_URL_VARIABLE),
                "the refusal of {bad} does not name the proxy or the variable: {}",
                refusal.message()
            );
        }
        // And the two loopback spellings are admitted, or the gate refuses
        // everything and proves nothing.
        for good in ["http://127.0.0.1:9106", "http://localhost:9106"] {
            DeepBrainConfig::parse(&vars(&hosted(&[(LANGUAGE_MODEL_BASE_URL_VARIABLE, good)])))
                .unwrap_or_else(|error| panic!("{good} was refused: {}", error.message()));
        }
    }

    #[test]
    fn an_unknown_provider_is_refused_by_name_rather_than_read_as_none() {
        // The failure this prevents: a misspelled provider silently meaning
        // "no hosted model", so a deployment believes it is calling one.
        let refusal = DeepBrainConfig::parse(&vars(&hosted(&[(
            LANGUAGE_MODEL_PROVIDER_VARIABLE,
            "openai",
        )])))
        .expect_err("`openai` is not a provider this build constructs");
        assert!(
            refusal.message().starts_with("configuration:")
                && refusal.message().contains("`openai`")
                && refusal.message().contains(HUGGING_FACE_PROVIDER),
            "the refusal does not name the value or the one accepted one: {}",
            refusal.message()
        );
        // A model named with no provider is the same half-configuration.
        let refusal = DeepBrainConfig::parse(&vars(&[(
            LANGUAGE_MODEL_VARIABLE,
            "example-org/example-model",
        )]))
        .expect_err("a model without a provider was admitted");
        assert!(
            refusal.message().contains(LANGUAGE_MODEL_PROVIDER_VARIABLE),
            "{}",
            refusal.message()
        );
    }

    #[test]
    fn this_node_hands_every_cycle_to_the_archive_rather_than_batching_them() {
        // The fast brain batches a hundred because a store write is a
        // measurable fraction of a millisecond-scale interval. Here it is not,
        // and what batching would risk is a whole cycle of research.
        assert_eq!(DeepBrainConfig::default().archive_every, 1);
    }
}
