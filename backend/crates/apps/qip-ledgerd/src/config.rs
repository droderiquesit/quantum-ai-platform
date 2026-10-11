//! Configuration loading and validation for the ledger (ADR 0100 §1).
//!
//! Every value arrives as a `QIP_LEDGER_*` variable, and every one that
//! decides where the ledger lives, what it reads or who it reads it as is
//! **required** — there is no default broker address, stream or data
//! directory. The binary used to hard-code its broker as `127.0.0.1:9090` in
//! `main`, a value no configuration could change and no deployment would
//! have answered on; a ledger that starts against a broker nobody chose is a
//! ledger that is quietly booking nothing.
//!
//! [`LedgerConfig::parse`] takes the variables as a map so its refusals are
//! tested without mutating process state; [`LedgerConfig::from_environment`]
//! is the one line that reads it, and is called only from `main`.
//!
//! The fabric bearer token is not in this struct. It is resolved by `main`
//! through `qip_core::secret` (so it may arrive as a mounted file) and handed
//! to the transport factory directly, so a `{:?}` of the configuration can
//! never print it. The Google credential the Spanner store presents is
//! resolved the same way, by `main`, into a `qip_storage::GcpAccess`.
//!
//! # Which store, and where it may be used (ADR 0113 §1)
//!
//! [`ENVIRONMENT`] is required and says whether this process is a
//! workstation run or a deployment. On a workstation the store defaults to
//! the local `DurableStore` under [`DATA_DIR`]; in a deployment it defaults
//! to Spanner, and `local` is **refused** — a deployed ledger posting to a
//! machine's disk is the first thing ADR 0113 says would make it wrong, and
//! one that cannot reach Spanner stops rather than falling back. The
//! environment is required rather than defaulted because a default of
//! "workstation" would let a deployment that forgot it post to a disk, and a
//! safety default here is the restrictive one.

use qip_core::deployment::{COMPUTE_ENGINE_PRODUCT_FILE, Deployment, DeploymentMarkers};
use qip_core::error::{Error, Result};
use qip_storage::gcp::spanner::{DEFAULT_TABLE, SpannerDatabase};
#[cfg(test)]
use qip_transport::tls::TLS_MODE_VARIABLE;
use qip_transport::tls::{PeerNames, TlsSource, link_source};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

/// Where the ledger's durable store lives, when the store is local.
pub const DATA_DIR: &str = "QIP_LEDGER_ARCHIVE_PATH";
/// `workstation`, `dev`, `staging` or `prod`. Required.
pub const ENVIRONMENT: &str = "QIP_LEDGER_ENVIRONMENT";
/// `local` or `spanner`. Defaults by environment; see the module docs.
pub const STORE: &str = "QIP_LEDGER_STORE";
/// The Google project holding the ledger's Spanner instance.
pub const SPANNER_PROJECT: &str = "QIP_LEDGER_SPANNER_PROJECT";
/// The Spanner instance.
pub const SPANNER_INSTANCE: &str = "QIP_LEDGER_SPANNER_INSTANCE";
/// The Spanner database.
pub const SPANNER_DATABASE: &str = "QIP_LEDGER_SPANNER_DATABASE";
/// Optional: the key-value table, `ledger_kv` when unset.
pub const SPANNER_TABLE: &str = "QIP_LEDGER_SPANNER_TABLE";

/// Whether this process is a workstation run or a deployment, as
/// [`ENVIRONMENT`] states it — the fact the store is chosen on (ADR 0113).
/// Distinct from [`qip_core::deployment::Deployment`], read from
/// [`DEPLOYMENT`], which decides the fabric link's security (ADR 0112).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerEnvironment {
    Workstation,
    Dev,
    Staging,
    Prod,
}

impl LedgerEnvironment {
    /// Every cloud environment, `dev` included: a ledger deployed anywhere
    /// is the ledger of record for that environment.
    pub fn is_deployed(self) -> bool {
        !matches!(self, Self::Workstation)
    }

    /// The value [`ENVIRONMENT`] spells this as.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workstation => "workstation",
            Self::Dev => "dev",
            Self::Staging => "staging",
            Self::Prod => "prod",
        }
    }
}

/// Where the ledger's rows live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerStorage {
    /// The in-tree `DurableStore` in this directory. Workstation only.
    Local { archive_path: PathBuf },
    /// The ledger of record: one table in one Spanner database.
    Spanner {
        database: SpannerDatabase,
        table: String,
    },
}
/// The address the read API and health are served on.
pub const LISTEN: &str = "QIP_LEDGER_LISTEN_ADDR";
/// The consumer group the ledger joins on the outcomes stream.
pub const CONSUMER_GROUP: &str = "QIP_LEDGER_FABRIC_CONSUMER_GROUP";
/// Which deployment the ledger runs in (ADR 0110 decision 5, ADR 0112).
/// Required: it decides whether the fabric link may be plaintext.
pub const DEPLOYMENT: &str = "QIP_DEPLOYMENT";
/// Optional: comma-separated DNS names a client certificate must carry one
/// of to be served by the read API (ADR 0112), as `qip-fabricd`'s
/// `QIP_EVENT_FABRIC_ADMITTED_PEERS` does for the protocol listener. Refused
/// on a plaintext listener, where nothing could check it.
pub const ADMITTED_PEERS: &str = "QIP_LEDGER_ADMITTED_PEERS";
/// The fabric broker: `https://host:port` over mutual TLS (ADR 0112), or
/// `http://host:port` on a `local` deployment that chose plaintext; scheme
/// and authority only.
pub const FABRIC_URL: &str = "QIP_LEDGER_FABRIC_URL";
/// The P1 outcomes stream the ledger books from.
pub const FABRIC_STREAM: &str = "QIP_LEDGER_FABRIC_STREAM";
/// How many partitions that stream is declared with.
pub const FABRIC_PARTITIONS: &str = "QIP_LEDGER_FABRIC_PARTITIONS";
/// The bearer token the ledger presents to the broker (or `…_FILE`).
pub const FABRIC_TOKEN: &str = "QIP_LEDGER_FABRIC_TOKEN";
/// Optional: how long a partition worker waits after an empty fetch.
pub const POLL_MS: &str = "QIP_LEDGER_POLL_MS";

/// The idle wait when none is stated.
pub const DEFAULT_POLL_MS: u64 = 200;

/// Matches the broker's own ceiling (`qip_fabricd::config::MAXIMUM_PARTITIONS`).
pub const MAXIMUM_PARTITIONS: u32 = 1024;

/// Configuration for the ledger service, validated before start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerConfig {
    /// Workstation or deployment, from [`ENVIRONMENT`]; selects the store.
    pub environment: LedgerEnvironment,
    /// The deployment from [`DEPLOYMENT`]; decides whether the fabric link
    /// and the read API may be plaintext.
    pub deployment: Deployment,
    /// Where the ledger's rows live.
    pub storage: LedgerStorage,
    /// Socket address the read API and health listen on — mutual TLS
    /// whenever [`Self::workload_tls`] is `Some`.
    pub listen_addr: SocketAddr,
    /// Consumer group ID for reading P1 outcomes from the event fabric.
    pub fabric_consumer_group: String,
    /// The fabric broker's scheme and authority. The scheme agrees with
    /// [`Self::workload_tls`]: `https` when it is `Some`, `http` when `None`.
    pub fabric_url: String,
    /// This process's workload certificate (ADR 0112) — hand-placed files, or
    /// issued and renewed in place by CA Service under `cas` (§2) — and the one fact
    /// both of its links are secured by: the fabric client presents it, and
    /// the read API's listener serves under it and requires a client
    /// certificate chained to the same roots. `None` is plaintext on both,
    /// admitted only where [`DEPLOYMENT`] is `local` and
    /// `QIP_TLS_MODE=plaintext` says so.
    ///
    /// One field rather than one per link (it was `fabric_tls` while only the
    /// client used it): two settings for one process's identity could
    /// disagree, and the disagreement that matters — an encrypted fabric link
    /// beside a plaintext read API serving every balance — is exactly the one
    /// nobody would see on a dashboard that shows the fabric link.
    pub workload_tls: Option<TlsSource>,
    /// [`ADMITTED_PEERS`]: the workloads the read API serves, checked
    /// against each client certificate's DNS names. Empty admits any
    /// certificate the roots chain, which leaves the CA pool's template as
    /// the only identity control — `qip-fabricd`'s rule, mirrored.
    pub admitted_peers: Vec<String>,
    /// The P1 outcomes stream.
    pub fabric_stream: String,
    /// Partitions of that stream; one worker reads each.
    pub fabric_partitions: u32,
    /// Idle wait between empty fetches, in milliseconds.
    pub poll_ms: u64,
}

fn required(variables: &BTreeMap<String, String>, name: &str, what: &str) -> Result<String> {
    match variables.get(name).map(|value| value.trim()) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(Error::invalid(format!(
            "{name} is not set; set it to {what}. The ledger has no default for it"
        ))),
    }
}

impl LedgerConfig {
    /// Read the process environment. Called from `main` only.
    pub fn from_environment() -> Result<Self> {
        let variables: BTreeMap<String, String> = std::env::vars().collect();
        // The ledger runs on Compute Engine, where the DMI product name is
        // the marker that refuses a `local` an `add-metadata` edit set.
        let markers = DeploymentMarkers::observe(
            &|name| variables.get(name).cloned(),
            std::fs::read_to_string(COMPUTE_ENGINE_PRODUCT_FILE)
                .ok()
                .as_deref(),
        );
        Self::parse_with(&variables, &markers)
    }

    /// Validate `variables`, refusing the first thing wrong by name, with
    /// the deployment markers observed from `variables` alone — the form a
    /// test drives. The process goes through [`Self::parse_with`] with the
    /// host's DMI product name as well.
    pub fn parse(variables: &BTreeMap<String, String>) -> Result<Self> {
        Self::parse_with(
            variables,
            &DeploymentMarkers::observe(&|name| variables.get(name).cloned(), None),
        )
    }

    /// Validate `variables` against what the root observed of its platform:
    /// [`DEPLOYMENT`]=`local` is refused wherever `markers` is not empty.
    pub fn parse_with(
        variables: &BTreeMap<String, String>,
        markers: &DeploymentMarkers,
    ) -> Result<Self> {
        let environment = match required(
            variables,
            ENVIRONMENT,
            "`workstation` for a single-node development run, or the environment this ledger is \
             deployed in (`dev`, `staging`, `prod`)",
        )?
        .as_str()
        {
            "workstation" => LedgerEnvironment::Workstation,
            "dev" => LedgerEnvironment::Dev,
            "staging" => LedgerEnvironment::Staging,
            "prod" => LedgerEnvironment::Prod,
            other => {
                return Err(Error::invalid(format!(
                    "{ENVIRONMENT} is `{other}`; it must be `workstation`, `dev`, `staging` or \
                     `prod`"
                )));
            }
        };
        let storage = Self::storage(variables, environment)?;
        let listen_text = required(variables, LISTEN, "the read API's socket address")?;
        let listen_addr = SocketAddr::from_str(&listen_text).map_err(|e| {
            Error::invalid(format!(
                "{LISTEN} '{listen_text}' is not a socket address: {e}"
            ))
        })?;
        let fabric_consumer_group = required(
            variables,
            CONSUMER_GROUP,
            "the consumer group the ledger joins",
        )?;
        let fabric_url = required(
            variables,
            FABRIC_URL,
            "the broker's https://host:port (the egress-free in-VPC address)",
        )?;
        // ADR 0112: the link's security is decided from the deployment and
        // the TLS variables, and the URL's scheme must agree with it — an
        // http address on a TLS link is not upgraded, and an https address
        // on a plaintext link is not downgraded. The transport appends each
        // route's own path, so a path here would be doubled.
        let deployment =
            Deployment::parse_observed(variables.get(DEPLOYMENT).map(String::as_str), markers)?;
        // Two settings now describe one fact — whether this is a workstation
        // — and two claims about the same fact will disagree. Each relaxes a
        // different guard: `workstation` admits the local disk store, `local`
        // admits a plaintext fabric link and a reproducible key. Left
        // independent, `prod` + `local` is a ledger of record reading
        // outcomes over plaintext, and `workstation` + `prod` is a deployed
        // process posting to a machine's disk. Neither is guessed at; both
        // are refused, naming the pair.
        if environment.is_deployed() == (deployment == Deployment::Local) {
            return Err(Error::invalid(format!(
                "{ENVIRONMENT} is `{}` but {DEPLOYMENT} is `{}`; they must agree on whether this \
                 is a workstation — `workstation` goes with `local`, and every deployed \
                 environment with the deployment it runs in — because one relaxes the store \
                 and the other relaxes the fabric link, and a mismatch would relax one in a \
                 deployment",
                environment.as_str(),
                deployment.as_str()
            )));
        }
        // `cas` (ADR 0112 §2) is validated here with the rest; `main` starts
        // the issuance it describes.
        let workload_tls = link_source(deployment, &|name| variables.get(name).cloned())?;
        let scheme = if workload_tls.is_some() {
            "https://"
        } else {
            "http://"
        };
        let authority = fabric_url.strip_prefix(scheme).ok_or_else(|| {
            Error::invalid(format!(
                "{FABRIC_URL} '{fabric_url}' must start with {scheme}: the fabric link is {}",
                if workload_tls.is_some() {
                    "mutual TLS (ADR 0112)"
                } else {
                    "plaintext, which only a local deployment may choose"
                }
            ))
        })?;
        if authority.is_empty() || authority.contains('/') {
            return Err(Error::invalid(format!(
                "{FABRIC_URL} '{fabric_url}' must be scheme and authority only, with no path; \
                 every call appends its own route"
            )));
        }
        let admitted_peers: Vec<String> = variables
            .get(ADMITTED_PEERS)
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        // Validated here, before anything is opened, so a name no
        // certificate can carry stops the start rather than the bind.
        PeerNames::new(admitted_peers.clone())?;
        if workload_tls.is_none() && !admitted_peers.is_empty() {
            return Err(Error::invalid(format!(
                "{ADMITTED_PEERS} names workloads and the read API is plaintext; a peer list \
                 nothing can check reads as a control. Configure the workload certificate or \
                 unset {ADMITTED_PEERS}"
            )));
        }
        let fabric_stream = required(variables, FABRIC_STREAM, "the P1 outcomes stream's name")?;
        let partitions_text = required(
            variables,
            FABRIC_PARTITIONS,
            "how many partitions the outcomes stream is declared with",
        )?;
        let fabric_partitions = partitions_text.parse::<u32>().map_err(|_| {
            Error::invalid(format!(
                "{FABRIC_PARTITIONS} must be a whole number, not '{partitions_text}'"
            ))
        })?;
        if fabric_partitions == 0 || fabric_partitions > MAXIMUM_PARTITIONS {
            return Err(Error::invalid(format!(
                "{FABRIC_PARTITIONS} must be between 1 and {MAXIMUM_PARTITIONS}, not \
                 {fabric_partitions}"
            )));
        }
        let poll_ms = match variables.get(POLL_MS).map(|v| v.trim()) {
            None | Some("") => DEFAULT_POLL_MS,
            Some(text) => text.parse::<u64>().map_err(|_| {
                Error::invalid(format!(
                    "{POLL_MS} must be a whole number of milliseconds, not '{text}'"
                ))
            })?,
        };
        if poll_ms == 0 {
            return Err(Error::invalid(format!(
                "{POLL_MS} must be at least 1; zero would spin every partition worker against \
                 the broker"
            )));
        }
        Ok(Self {
            environment,
            deployment,
            storage,
            listen_addr,
            fabric_consumer_group,
            fabric_url,
            workload_tls,
            admitted_peers,
            fabric_stream,
            fabric_partitions,
            poll_ms,
        })
    }

    /// The store `variables` select for `environment`, refusing a local store
    /// in any deployment and a Spanner store that does not name its database.
    fn storage(
        variables: &BTreeMap<String, String>,
        environment: LedgerEnvironment,
    ) -> Result<LedgerStorage> {
        let chosen = match variables.get(STORE).map(|v| v.trim()) {
            None | Some("") if environment.is_deployed() => "spanner",
            None | Some("") => "local",
            Some(other) => other,
        };
        match chosen {
            "local" if environment.is_deployed() => Err(Error::invalid(format!(
                "{STORE}=local in the {environment:?} environment; a deployed ledger is the \
                 ledger of record and lives in Spanner (ADR 0113). The local store is for a \
                 workstation run only — set {STORE}=spanner and the {SPANNER_PROJECT}, \
                 {SPANNER_INSTANCE} and {SPANNER_DATABASE} it needs"
            ))),
            "local" => Ok(LedgerStorage::Local {
                archive_path: PathBuf::from(required(
                    variables,
                    DATA_DIR,
                    "the directory the ledger's durable store lives in",
                )?),
            }),
            "spanner" => {
                let database = SpannerDatabase::new(
                    &required(variables, SPANNER_PROJECT, "the ledger's Google project")?,
                    &required(variables, SPANNER_INSTANCE, "the ledger's Spanner instance")?,
                    &required(variables, SPANNER_DATABASE, "the ledger's Spanner database")?,
                )?;
                let table = match variables.get(SPANNER_TABLE).map(|v| v.trim()) {
                    None | Some("") => DEFAULT_TABLE.to_string(),
                    Some(table) => table.to_string(),
                };
                Ok(LedgerStorage::Spanner { database, table })
            }
            other => Err(Error::invalid(format!(
                "{STORE} is `{other}`; it must be `local` (workstation only) or `spanner`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> BTreeMap<String, String> {
        [
            (ENVIRONMENT, "workstation"),
            (DATA_DIR, "/var/lib/ledger"),
            (LISTEN, "127.0.0.1:7200"),
            (CONSUMER_GROUP, "ledger"),
            (FABRIC_URL, "http://fabricd.internal:7100"),
            (FABRIC_STREAM, "reflex-outcomes"),
            (FABRIC_PARTITIONS, "4"),
            (DEPLOYMENT, "local"),
            (TLS_MODE_VARIABLE, "plaintext"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
    }

    /// The failure this prevents: a ledger that starts against a broker, a
    /// stream or a directory nobody chose — as it did when `main` hard-coded
    /// `127.0.0.1:9090`. Each required variable is removed in turn and the
    /// refusal must name it. Mutation: give `FABRIC_URL` a default in
    /// `parse` — its iteration is accepted and this fails.
    #[test]
    fn a_ledger_refuses_to_start_without_each_setting_it_has_no_default_for() {
        let config = LedgerConfig::parse(&complete()).expect("premise: the complete set loads");
        assert_eq!(config.fabric_partitions, 4);
        assert_eq!(config.poll_ms, DEFAULT_POLL_MS);
        assert_eq!(config.fabric_url, "http://fabricd.internal:7100");
        for name in [
            ENVIRONMENT,
            DATA_DIR,
            LISTEN,
            CONSUMER_GROUP,
            FABRIC_URL,
            FABRIC_STREAM,
            FABRIC_PARTITIONS,
        ] {
            let mut variables = complete();
            variables.remove(name);
            let error = LedgerConfig::parse(&variables).expect_err("a missing setting is refused");
            assert!(
                error.to_string().contains(&format!("{name} is not set")),
                "{name}: {error}"
            );
        }
    }

    /// The failure this prevents: an `add-metadata` edit setting
    /// `QIP_DEPLOYMENT=local` (and the workstation store beside it) on the
    /// ledger's VM brought it up reading outcomes over plaintext. Mutation:
    /// pass `&DeploymentMarkers::none()` to `parse_observed` in `parse_with`
    /// and the relabelled ledger parses; this fails.
    #[test]
    fn a_ledger_that_can_see_it_is_deployed_refuses_local() {
        // Premise: the workstation fixture parses with no marker.
        assert!(LedgerConfig::parse_with(&complete(), &DeploymentMarkers::none()).is_ok());
        let on_compute_engine =
            DeploymentMarkers::observe(&|_| None, Some("Google Compute Engine\n"));
        let error = LedgerConfig::parse_with(&complete(), &on_compute_engine)
            .expect_err("local was admitted on a Compute Engine guest");
        assert!(
            error.to_string().contains("can see it is deployed"),
            "{error}"
        );
    }

    /// A broker address with TLS or a path, a partition count of zero or past
    /// the broker's own ceiling, and a zero poll interval are refused rather
    /// than corrected.
    #[test]
    fn a_ledger_refuses_an_https_or_pathed_broker_and_an_impossible_partition_count() {
        for bad in [
            "https://fabricd:7100",
            "http://fabricd:7100/v1",
            "http://",
            "fabricd:7100",
        ] {
            let mut variables = complete();
            variables.insert(FABRIC_URL.to_string(), bad.to_string());
            assert!(LedgerConfig::parse(&variables).is_err(), "url = {bad}");
        }
        for bad in ["0", "1025", "four"] {
            let mut variables = complete();
            variables.insert(FABRIC_PARTITIONS.to_string(), bad.to_string());
            assert!(
                LedgerConfig::parse(&variables).is_err(),
                "partitions = {bad}"
            );
        }
        let mut variables = complete();
        variables.insert(POLL_MS.to_string(), "0".to_string());
        assert!(LedgerConfig::parse(&variables).is_err());
    }

    fn deployed(environment: &str) -> BTreeMap<String, String> {
        let mut variables = complete();
        variables.insert(ENVIRONMENT.to_string(), environment.to_string());
        variables.insert(SPANNER_PROJECT.to_string(), "qip-dev".to_string());
        variables.insert(SPANNER_INSTANCE.to_string(), "ledger".to_string());
        variables.insert(SPANNER_DATABASE.to_string(), "positions".to_string());
        // A deployed ledger runs in the deployment of the same name and so
        // reaches the fabric over mutual TLS; `complete`'s `local` plaintext
        // link is the contradiction `parse` refuses.
        let deployment = match environment {
            "staging" => "stage",
            other => other,
        };
        variables.insert(DEPLOYMENT.to_string(), deployment.to_string());
        variables.remove(TLS_MODE_VARIABLE);
        for (name, value) in [
            (
                qip_transport::tls::TLS_CERTIFICATE_CHAIN_VARIABLE,
                "/etc/qip/tls/chain.pem",
            ),
            (
                qip_transport::tls::TLS_PRIVATE_KEY_VARIABLE,
                "/run/qip/tls/key.pem",
            ),
            (
                qip_transport::tls::TLS_TRUST_ROOTS_VARIABLE,
                "/etc/qip/tls/roots.pem",
            ),
        ] {
            variables.insert(name.to_string(), value.to_string());
        }
        variables.insert(
            FABRIC_URL.to_string(),
            "https://fabricd.internal:7100".to_string(),
        );
        variables
    }

    /// ADR 0113 and ADR 0112 read two settings for one fact. The failure
    /// this prevents: `QIP_LEDGER_ENVIRONMENT=prod` with `QIP_DEPLOYMENT=local`
    /// loaded — the ledger of record taking outcomes over a plaintext link —
    /// and `workstation` with `prod` would put a deployed ledger on a disk.
    /// The premise half: each agreeing pair loads, so the refusals are about
    /// the pair and not a broken fixture.
    ///
    /// Mutation: delete the agreement check in `parse` — `prod` + `local`
    /// loads and the first refusal's `expect_err` fires.
    #[test]
    fn a_ledger_refuses_an_environment_and_a_deployment_that_disagree_about_a_workstation() {
        LedgerConfig::parse(&complete()).expect("premise: workstation + local loads");
        LedgerConfig::parse(&deployed("prod")).expect("premise: prod + prod loads");

        let mut plaintext_record = deployed("prod");
        plaintext_record.insert(DEPLOYMENT.to_string(), "local".to_string());
        plaintext_record.insert(TLS_MODE_VARIABLE.to_string(), "plaintext".to_string());
        plaintext_record.insert(
            FABRIC_URL.to_string(),
            "http://fabricd.internal:7100".to_string(),
        );
        let error = LedgerConfig::parse(&plaintext_record)
            .expect_err("a prod ledger on a local deployment is refused");
        assert!(
            error.to_string().contains("they must agree"),
            "refused for the wrong reason: {error}"
        );

        let mut disk_in_prod = deployed("prod");
        disk_in_prod.insert(ENVIRONMENT.to_string(), "workstation".to_string());
        let error = LedgerConfig::parse(&disk_in_prod)
            .expect_err("a workstation ledger in the prod deployment is refused");
        assert!(
            error.to_string().contains("they must agree"),
            "refused for the wrong reason: {error}"
        );
    }

    /// R12-LEDGER-8007's check: a deployed ledger pointed at a non-Spanner
    /// store is refused, whether `local` is stated or merely defaulted to.
    /// The premise half: a workstation defaults to local, and every deployed
    /// environment with its database named loads as Spanner — so the
    /// refusals are about the store and not about a broken fixture.
    ///
    /// Mutation: drop the `if environment.is_deployed()` guard on the
    /// `"local"` arm of `LedgerConfig::storage` — `dev` with `STORE=local`
    /// loads and the second loop fails.
    #[test]
    fn a_deployed_ledger_refuses_the_local_store_and_defaults_to_spanner() {
        let workstation = LedgerConfig::parse(&complete()).expect("premise: workstation loads");
        assert_eq!(
            workstation.storage,
            LedgerStorage::Local {
                archive_path: PathBuf::from("/var/lib/ledger")
            }
        );
        for environment in ["dev", "staging", "prod"] {
            let config = LedgerConfig::parse(&deployed(environment))
                .unwrap_or_else(|e| panic!("premise: {environment} with Spanner loads: {e}"));
            assert!(config.environment.is_deployed());
            assert_eq!(
                config.storage,
                LedgerStorage::Spanner {
                    database: SpannerDatabase::new("qip-dev", "ledger", "positions")
                        .expect("plain names"),
                    table: DEFAULT_TABLE.to_string(),
                },
                "{environment} defaults to Spanner"
            );
        }
        for environment in ["dev", "staging", "prod"] {
            let mut variables = deployed(environment);
            variables.insert(STORE.to_string(), "local".to_string());
            let error = LedgerConfig::parse(&variables)
                .expect_err("a deployed ledger on the local store is refused");
            assert!(
                error.to_string().contains("ADR 0113"),
                "{environment}: refused for the wrong reason: {error}"
            );
        }
        let mut unnamed = deployed("prod");
        unnamed.remove(SPANNER_DATABASE);
        let error = LedgerConfig::parse(&unnamed).expect_err("a Spanner store needs its database");
        assert!(
            error
                .to_string()
                .contains(&format!("{SPANNER_DATABASE} is not set"))
        );
        let mut unknown = complete();
        unknown.insert(ENVIRONMENT.to_string(), "production".to_string());
        assert!(LedgerConfig::parse(&unknown).is_err());
    }

    /// ADR 0112 at the ledger's fabric client. Mutation: let `plaintext`
    /// through outside `local`, or stop checking the URL's scheme against
    /// the link, and one of these parses; this fails.
    #[test]
    fn a_ledger_reaches_the_fabric_in_plaintext_only_where_a_local_deployment_chose_it() {
        // Built on a `staging` ledger in the `stage` deployment so that the
        // environment and the deployment agree, and every refusal below is
        // about the link rather than the pair.
        let mut plaintext = deployed("staging");
        for name in [
            qip_transport::tls::TLS_CERTIFICATE_CHAIN_VARIABLE,
            qip_transport::tls::TLS_PRIVATE_KEY_VARIABLE,
            qip_transport::tls::TLS_TRUST_ROOTS_VARIABLE,
        ] {
            plaintext.remove(name);
        }
        plaintext.insert(TLS_MODE_VARIABLE.to_string(), "plaintext".to_string());
        plaintext.insert(
            FABRIC_URL.to_string(),
            "http://fabricd.internal:7100".to_string(),
        );
        let error = LedgerConfig::parse(&plaintext).expect_err("plaintext in stage");
        assert!(
            !error.to_string().contains("they must agree"),
            "refused for the pair, not the link: {error}"
        );

        let mut tls = deployed("staging");
        // An http address on a TLS link is refused, not upgraded.
        tls.insert(
            FABRIC_URL.to_string(),
            "http://fabricd.internal:7100".to_string(),
        );
        assert!(LedgerConfig::parse(&tls).is_err(), "http on a TLS link");
        tls.insert(
            FABRIC_URL.to_string(),
            "https://fabricd.internal:7100".to_string(),
        );
        let config = LedgerConfig::parse(&tls).expect("https on a TLS link");
        assert!(matches!(config.workload_tls, Some(TlsSource::Files(_))));

        // The other direction: an https address on a local plaintext link is
        // refused at start-up rather than at the first call, where the
        // plaintext client would refuse the scheme. Until the round-2
        // mutation pass this half was absent, and a `parse` that accepted
        // `https://` whatever the link passed the test.
        LedgerConfig::parse(&complete()).expect("premise: local plaintext over http loads");
        let mut https_on_plaintext = complete();
        https_on_plaintext.insert(
            FABRIC_URL.to_string(),
            "https://fabricd.internal:7100".to_string(),
        );
        let error =
            LedgerConfig::parse(&https_on_plaintext).expect_err("https on a plaintext link");
        assert!(
            error.to_string().contains("must start with http://"),
            "refused for another reason: {error}"
        );
    }

    /// A reader list on a plaintext read API is refused: nothing could check
    /// a certificate that is never presented, and a list nobody enforces
    /// reads as a control. On a TLS listener the list is parsed, trimmed and
    /// kept, and a name no certificate can carry is refused.
    ///
    /// Mutation: delete the plaintext refusal in `parse` — the first
    /// `expect_err` fires.
    #[test]
    fn a_reader_list_is_refused_on_a_plaintext_read_api_and_kept_on_a_mutual_tls_one() {
        let mut plaintext = complete();
        plaintext.insert(
            ADMITTED_PEERS.to_string(),
            "reader.test.qip.internal".to_string(),
        );
        let error = LedgerConfig::parse(&plaintext)
            .expect_err("a reader list on a plaintext read API was accepted");
        assert!(
            error
                .to_string()
                .contains(&format!("{ADMITTED_PEERS} names workloads")),
            "refused for the wrong reason: {error}"
        );
        // Premise: without the list, the same plaintext configuration loads.
        assert!(
            LedgerConfig::parse(&complete())
                .expect("the plaintext set loads")
                .admitted_peers
                .is_empty()
        );

        let mut tls = deployed("staging");
        tls.insert(
            FABRIC_URL.to_string(),
            "https://fabricd.internal:7100".to_string(),
        );
        tls.insert(
            ADMITTED_PEERS.to_string(),
            " reader.test.qip.internal , api.test.qip.internal ,".to_string(),
        );
        let config = LedgerConfig::parse(&tls).expect("a reader list on a TLS read API");
        assert_eq!(
            config.admitted_peers,
            vec![
                "reader.test.qip.internal".to_string(),
                "api.test.qip.internal".to_string()
            ]
        );
        tls.insert(ADMITTED_PEERS.to_string(), "not a name".to_string());
        assert!(
            LedgerConfig::parse(&tls).is_err(),
            "a name no certificate can carry was admitted"
        );
    }

    /// ADR 0112 §2 at the ledger: `cas` is an issued source whose URL must
    /// be `https`, and its lifetime is bounded at parse. Mutation: parse the
    /// link with `link_mode` again and the first parse fails; drop the
    /// bounds and the ten-minute lifetime is accepted.
    #[test]
    fn a_ledger_on_certificate_authority_service_is_validated_before_it_starts() {
        let mut cas = deployed("staging");
        for name in [
            qip_transport::tls::TLS_CERTIFICATE_CHAIN_VARIABLE,
            qip_transport::tls::TLS_PRIVATE_KEY_VARIABLE,
        ] {
            cas.remove(name);
        }
        for (name, value) in [
            (TLS_MODE_VARIABLE, "cas"),
            (
                qip_transport::cas::CA_POOL_VARIABLE,
                "projects/p/locations/europe-west2/caPools/qip-stage-internal",
            ),
            (
                qip_transport::cas::GOOGLE_TLS_ROOTS_VARIABLE,
                "/etc/qip/tls/google-roots.pem",
            ),
            (FABRIC_URL, "https://fabricd.internal:7100"),
        ] {
            cas.insert(name.to_string(), value.to_string());
        }
        let config = LedgerConfig::parse(&cas).expect("a ledger on CA Service");
        assert!(matches!(config.workload_tls, Some(TlsSource::Issued(_))));
        cas.insert(
            qip_transport::cas::LIFETIME_VARIABLE.to_string(),
            "600".to_string(),
        );
        assert!(
            LedgerConfig::parse(&cas).is_err(),
            "a ten-minute lifetime was accepted"
        );
    }
}
