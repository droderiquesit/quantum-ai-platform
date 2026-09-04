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
use qip_storage::settings::StorageSettings;
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
    /// A committed bitemporal tape to run through on its own clock instead
    /// of the synthetic exchange. Mutually exclusive with `replay_path`;
    /// `Feed::open` refuses the contradiction. See
    /// `qip_market_ingestion::tape` for why this is not the replay.
    pub tape_path: Option<String>,
    /// A licensed vendor to poll instead of either. `None` is the shipped
    /// state and the only state any environment in this repository configures.
    pub live_feed: Option<LiveFeedSettings>,
    /// A catalogued connector source, when the deployment names one. Mutually
    /// exclusive with `live_feed`; `Feed::open` refuses the contradiction.
    pub connector_feed: Option<ConnectorFeedSettings>,
    /// The synthetic exchange's seed, so a session is reproducible.
    pub seed: u64,
    /// How long the shutdown flush may take before the node gives up on it.
    pub shutdown_budget: Duration,
}

/// What a live market-data vendor needs before this node will open one.
///
/// Every field is required and none has a default. That is the whole design:
/// a partially configured live feed is the one failure mode worth refusing
/// outright, because the alternative is a node that starts, reports itself
/// healthy, silently falls back to a synthetic tape, and produces investment
/// decisions from generated prices while an operator reads a dashboard that
/// says the feed is live.
///
/// **No vendor is configured anywhere in this repository, deliberately.**
/// `infrastructure/kubernetes/base/egress.yaml` declines to allowlist a
/// market-data host for the same reason this struct has no default: there is
/// no vendor in the workspace to derive one from, and inventing a hostname
/// nobody holds a licence for would put it into a security control. Choosing
/// the vendor, licensing its data and holding its credential are decisions
/// this code cannot make. What it can do is make them configuration rather
/// than engineering, which is what this is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveFeedSettings {
    /// `http://host[:port]` of the **egress proxy**, never of the vendor.
    /// `qip_transport::http` speaks plaintext HTTP/1.1 and has no TLS stack,
    /// so an address pointing straight at a vendor would send the credential
    /// below across the internet in clear text.
    pub base_url: String,
    /// Path of the vendor's market-data endpoint under `base_url`.
    pub path: String,
    /// The vendor's symbols, in the vendor's own spelling.
    pub symbols: Vec<String>,
    /// Venue code stamped on every record from this feed.
    pub venue: String,
    /// The credential, already resolved. Read through `qip_core::secret`, so a
    /// deployment may supply it in a file rather than in the environment — a
    /// key in the environment is a key in `/proc/<pid>/environ`, every child
    /// process, and every crash dump.
    pub api_key: String,
    /// Header the credential travels in, since vendors disagree.
    pub api_key_header: String,
}

/// A catalogued connector source and the egress address to reach it through.
///
/// Two fields and no credential, because the sources this build carries are
/// unauthenticated by their manifests — `auth.scheme` is `none` — and the
/// licensing catalogue in [`crate::licensing`] is what decides whether the
/// source may be used at all. A future keyed source adds its credential to
/// the manifest's own auth scheme and resolves it through `qip_core::secret`,
/// not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorFeedSettings {
    /// The manifest's `source_id`, e.g. `coinbase-spot-ticker`. Must be
    /// catalogued or the feed refuses to open.
    pub source_id: String,
    /// `http://host[:port]` of the **egress proxy**, never of the vendor —
    /// the same rule, for the same plaintext transport, as the vendor path.
    pub base_url: String,
    /// Seed for the runtime's own jitter, from the platform seed.
    pub seed: u64,
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
            tape_path: None,
            live_feed: None,
            connector_feed: None,
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
        let max_runtime = match number(vars, "QIP_FASTBRAIN_MAX_RUNTIME_SECS")? {
            Some(seconds) => Some(Duration::from_secs(i64::try_from(seconds).map_err(
                |_| {
                    Error::invalid(format!(
                        "configuration: QIP_FASTBRAIN_MAX_RUNTIME_SECS is {seconds}, too large \
                         to express as a duration"
                    ))
                },
            )?)),
            None => None,
        };

        // Refused rather than clamped: a tolerance too large for `u32` is a
        // typo an operator would want back, not a value this node should
        // silently treat as "never leave rotation for breaches" by rounding
        // it down to `u32::MAX` — which is what capping it here would do in
        // effect, since no run breaches four billion consecutive cycles.
        let breach_tolerance = match number(vars, "QIP_FASTBRAIN_BREACH_TOLERANCE")? {
            Some(value) => u32::try_from(value).map_err(|_| {
                Error::invalid(format!(
                    "configuration: QIP_FASTBRAIN_BREACH_TOLERANCE is {value}, too large to \
                     express as a breach count"
                ))
            })?,
            None => defaults.breach_tolerance,
        };

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
            //
            // The variables are looked up in `vars` by the library rather
            // than read by it, so a managed target's credential is resolved
            // here, in the composition root, through `qip_core::secret`.
            storage: StorageSettings::from_env(&|name| text(vars, name))
                .map_err(|error| Error::invalid(format!("configuration: {}", error.message())))?,
            replay_path: text(vars, "QIP_FASTBRAIN_REPLAY_PATH"),
            tape_path: text(vars, "QIP_FASTBRAIN_TAPE_PATH"),
            live_feed: live_feed(vars)?,
            connector_feed: connector_feed(
                vars,
                number(vars, "QIP_FASTBRAIN_SEED")?.unwrap_or(defaults.seed),
            )?,
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

/// Resolve a live vendor, or refuse a half-configured one.
///
/// Absent is `None` — no vendor is configured in any environment here, and
/// that is the shipped state. **Partially present is an error**, not a
/// fallback, and that asymmetry is the entire point of this function.
///
/// A node that fell back to the synthetic exchange because one of six
/// variables was missing would start, report itself healthy, and produce
/// investment decisions from generated prices while its dashboard said the
/// feed was live. Silence is the failure that costs the most here, because
/// nothing downstream can tell a synthetic tape from a licensed one once the
/// records look the same. `is_production_grade` comes off the adapter's own
/// descriptor for the same reason.
///
/// The credential is read through `qip_core::secret`, which accepts the
/// `_FILE` indirection the Secret Manager CSI driver projects, so a deployment
/// never has to put a key in the environment.
fn connector_feed(
    vars: &BTreeMap<String, String>,
    seed: u64,
) -> Result<Option<ConnectorFeedSettings>> {
    const SOURCE: &str = "QIP_CONNECTOR_SOURCE";
    const BASE_URL: &str = "QIP_CONNECTOR_BASE_URL";

    let source = text(vars, SOURCE);
    let base_url = text(vars, BASE_URL);
    match (source, base_url) {
        (None, None) => Ok(None),
        // Half a configuration is refused for the same reason half a vendor
        // is: the silent alternative is the synthetic exchange wearing a
        // configured look.
        (Some(_), None) => Err(Error::invalid(format!(
            "{SOURCE} is set and {BASE_URL} is not. A connector source needs the egress \
             proxy's address; set both, or neither"
        ))),
        (None, Some(_)) => Err(Error::invalid(format!(
            "{BASE_URL} is set and {SOURCE} is not. An egress address with no source names \
             nothing to fetch; set both, or neither"
        ))),
        (Some(source_id), Some(base_url)) => {
            if base_url.starts_with("https://") {
                return Err(Error::invalid(format!(
                    "{BASE_URL} is {base_url}. `qip_transport::http` speaks plaintext \
                     HTTP/1.1 and has no TLS stack: point this at the egress proxy, which \
                     terminates TLS to the vendor, never at the vendor itself"
                )));
            }
            Ok(Some(ConnectorFeedSettings {
                source_id,
                base_url,
                seed,
            }))
        }
    }
}

fn live_feed(vars: &BTreeMap<String, String>) -> Result<Option<LiveFeedSettings>> {
    const BASE_URL: &str = "QIP_MARKET_DATA_BASE_URL";
    const PATH: &str = "QIP_MARKET_DATA_PATH";
    const SYMBOLS: &str = "QIP_MARKET_DATA_SYMBOLS";
    const VENUE: &str = "QIP_MARKET_DATA_VENUE";
    const KEY: &str = "QIP_MARKET_DATA_KEY";
    const HEADER: &str = "QIP_MARKET_DATA_KEY_HEADER";

    let api_key = qip_core::secret::resolve_from(
        KEY,
        text(vars, KEY),
        text(vars, &format!("{KEY}{}", qip_core::secret::FILE_SUFFIX)),
    )?;

    let present: Vec<&str> = [BASE_URL, PATH, SYMBOLS, VENUE, HEADER]
        .into_iter()
        .filter(|name| text(vars, name).is_some())
        .chain(api_key.as_ref().map(|_| KEY))
        .collect();
    if present.is_empty() {
        return Ok(None);
    }

    let require = |name: &str| -> Result<String> {
        text(vars, name).ok_or_else(|| {
            Error::invalid(format!(
                "a live market-data feed is partly configured — {} — and {name} is missing. A \
                 half-configured feed is refused rather than quietly replaced by the synthetic \
                 exchange, because a node trading generated prices while reporting a live feed \
                 is the failure nothing downstream can detect. Set every variable, or none.",
                present.join(", ")
            ))
        })
    };

    let base_url = require(BASE_URL)?;
    // The transport has no TLS stack, so `https` is refused at construction
    // anyway; saying so here names the deployment mistake instead of surfacing
    // it as a connection error at the first poll.
    if base_url.starts_with("https://") {
        return Err(Error::invalid(format!(
            "{BASE_URL} is {base_url}. `qip_transport::http` speaks plaintext HTTP/1.1 and has \
             no TLS stack: point this at the in-cluster egress proxy, which terminates TLS to \
             the vendor, never at the vendor itself"
        )));
    }

    let symbols: Vec<String> = require(SYMBOLS)?
        .split(',')
        .map(|symbol| symbol.trim().to_string())
        .filter(|symbol| !symbol.is_empty())
        .collect();
    if symbols.is_empty() {
        return Err(Error::invalid(format!(
            "{SYMBOLS} names no symbol this node could ask for"
        )));
    }

    Ok(Some(LiveFeedSettings {
        base_url,
        path: require(PATH)?,
        symbols,
        venue: require(VENUE)?,
        api_key: api_key.ok_or_else(|| {
            Error::invalid(format!(
                "a live market-data feed is configured and no credential was resolved. Set {KEY}, \
                 or {KEY}{} to the path the Secret Manager CSI driver projects it to.",
                qip_core::secret::FILE_SUFFIX
            ))
        })?,
        api_key_header: require(HEADER)?,
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

fn millis(vars: &BTreeMap<String, String>, name: &str) -> Result<Option<Duration>> {
    let Some(value) = number(vars, name)? else {
        return Ok(None);
    };
    // Refused rather than clamped: a value too large to express as a signed
    // millisecond count is a caller mistake (a stray digit, a units error),
    // and silently capping it at `i64::MAX` milliseconds — nearly three
    // hundred million years — would hide that mistake behind a number nobody
    // asked for.
    let millis = i64::try_from(value).map_err(|_| {
        Error::invalid(format!(
            "configuration: {name} is {value}, too large to express as a millisecond duration"
        ))
    })?;
    Ok(Some(Duration::from_millis(millis)))
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
    fn a_cycle_interval_too_large_to_express_as_a_signed_duration_is_refused_rather_than_capped() {
        // One past `i64::MAX`: a real number, parseable as `u64`, that the
        // duration this config produces cannot hold without silently
        // rounding it down to `i64::MAX` milliseconds. A cap here would be a
        // caller's typo surviving as "run forever, near enough" instead of
        // being refused.
        let asked = (i64::MAX as u64) + 1;
        let refusal = FastBrainConfig::parse(&vars(&[(
            "QIP_FASTBRAIN_CYCLE_INTERVAL_MS",
            &asked.to_string(),
        )]))
        .expect_err("a millisecond count beyond i64::MAX was accepted and silently capped");
        assert!(
            refusal
                .message()
                .contains("QIP_FASTBRAIN_CYCLE_INTERVAL_MS")
                && refusal.message().contains("too large"),
            "the refusal does not name the variable or the size problem: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_max_runtime_too_large_to_express_as_a_signed_duration_is_refused_rather_than_capped() {
        let asked = (i64::MAX as u64) + 1;
        let refusal = FastBrainConfig::parse(&vars(&[(
            "QIP_FASTBRAIN_MAX_RUNTIME_SECS",
            &asked.to_string(),
        )]))
        .expect_err("a runtime bound beyond i64::MAX seconds was accepted and silently capped");
        assert!(
            refusal.message().contains("QIP_FASTBRAIN_MAX_RUNTIME_SECS")
                && refusal.message().contains("too large"),
            "the refusal does not name the variable or the size problem: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_breach_tolerance_too_large_to_express_as_a_u32_is_refused_rather_than_capped() {
        // One past `u32::MAX`. Capping this to `u32::MAX` would read as
        // "never leave rotation for consecutive breaches", a materially
        // different policy than the one asked for, chosen silently.
        let asked = (u32::MAX as u64) + 1;
        let refusal = FastBrainConfig::parse(&vars(&[(
            "QIP_FASTBRAIN_BREACH_TOLERANCE",
            &asked.to_string(),
        )]))
        .expect_err("a breach tolerance beyond u32::MAX was accepted and silently capped");
        assert!(
            refusal.message().contains("QIP_FASTBRAIN_BREACH_TOLERANCE")
                && refusal.message().contains("too large"),
            "the refusal does not name the variable or the size problem: {}",
            refusal.message()
        );
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

#[cfg(test)]
mod live_feed_tests {
    use super::*;

    fn full() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "QIP_MARKET_DATA_BASE_URL",
                "http://qip-egress.qip.svc.cluster.local:9105",
            ),
            ("QIP_MARKET_DATA_PATH", "/v1/quotes"),
            ("QIP_MARKET_DATA_SYMBOLS", "AAPL,MSFT"),
            ("QIP_MARKET_DATA_VENUE", "XNAS"),
            ("QIP_MARKET_DATA_KEY", "not-a-key"),
            ("QIP_MARKET_DATA_KEY_HEADER", "x-api-key"),
        ]
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn no_live_vendor_is_configured_and_that_is_not_an_error() {
        // The shipped state, and the state every environment in this
        // repository is in. A node with no vendor runs the synthetic exchange
        // and says so at start-up; it does not refuse to start.
        let config = FastBrainConfig::parse(&BTreeMap::new()).expect("an empty environment parses");
        assert!(config.live_feed.is_none());
    }

    #[test]
    fn a_fully_configured_vendor_is_accepted_with_every_field_carried_through() {
        // The premise for every refusal test below: the complete set is
        // genuinely accepted. A resolver that refused everything would pass
        // all the negative cases and be useless.
        let config = FastBrainConfig::parse(&map(&full())).expect("a complete vendor parses");
        let live = config.live_feed.expect("the vendor was resolved");
        assert_eq!(live.symbols, vec!["AAPL".to_string(), "MSFT".to_string()]);
        assert_eq!(live.venue, "XNAS");
        assert_eq!(live.api_key_header, "x-api-key");
        assert!(live.base_url.starts_with("http://"));
    }

    #[test]
    fn a_half_configured_vendor_is_refused_rather_than_silently_replaced() {
        // The failure this resolver exists to prevent, checked one missing
        // variable at a time. A node that fell back to the synthetic exchange
        // because one of six variables was absent would start, report itself
        // healthy, and produce investment decisions from generated prices
        // while its dashboard said the feed was live — and nothing downstream
        // can tell the two tapes apart once the records look the same.
        for omitted in [
            "QIP_MARKET_DATA_BASE_URL",
            "QIP_MARKET_DATA_PATH",
            "QIP_MARKET_DATA_SYMBOLS",
            "QIP_MARKET_DATA_VENUE",
            "QIP_MARKET_DATA_KEY",
            "QIP_MARKET_DATA_KEY_HEADER",
        ] {
            let partial: Vec<(&str, &str)> = full()
                .into_iter()
                .filter(|(name, _)| *name != omitted)
                .collect();
            // Premise: five of the six are still present, so this is a
            // half-configured vendor and not an unconfigured one.
            assert_eq!(partial.len(), 5, "the fixture stopped being partial");

            let error = FastBrainConfig::parse(&map(&partial))
                .expect_err(&format!("omitting {omitted} was accepted"));
            assert!(
                error.message().contains(omitted)
                    || error.message().contains("credential was resolved"),
                "omitting {omitted} produced a message that does not name it: {}",
                error.message()
            );
        }
    }

    #[test]
    fn a_vendor_address_over_tls_is_refused_by_name() {
        // `qip_transport::http` has no TLS stack, so `https` fails at
        // construction anyway. Refusing it here names the deployment mistake
        // — the address should be the in-cluster egress proxy, which
        // terminates TLS to the vendor — instead of surfacing it as a
        // connection error at the first poll, hours later.
        let mut pairs = full();
        pairs[0].1 = "https://vendor.example.com";
        let error = FastBrainConfig::parse(&map(&pairs)).expect_err("https was accepted");
        assert!(
            error.message().contains("egress proxy"),
            "the refusal does not point at the proxy: {}",
            error.message()
        );
    }

    #[test]
    fn a_vendor_naming_no_symbol_is_refused() {
        let mut pairs = full();
        pairs[2].1 = " , ,";
        let error = FastBrainConfig::parse(&map(&pairs)).expect_err("an empty symbol list parsed");
        assert!(error.message().contains("names no symbol"));
    }
}
