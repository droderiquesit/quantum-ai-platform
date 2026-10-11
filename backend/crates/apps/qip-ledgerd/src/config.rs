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

use qip_core::error::{Error, Result};
use qip_storage::gcp::spanner::{DEFAULT_TABLE, SpannerDatabase};
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

/// Whether this process is a workstation run or a deployment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deployment {
    Workstation,
    Dev,
    Staging,
    Prod,
}

impl Deployment {
    /// Every cloud environment, `dev` included: a ledger deployed anywhere
    /// is the ledger of record for that environment.
    pub fn is_deployed(self) -> bool {
        !matches!(self, Self::Workstation)
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
/// The fabric broker: `http://host:port`, scheme and authority only.
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
    /// Workstation or deployment.
    pub deployment: Deployment,
    /// Where the ledger's rows live.
    pub storage: LedgerStorage,
    /// Socket address the read API and health listen on.
    pub listen_addr: SocketAddr,
    /// Consumer group ID for reading P1 outcomes from the event fabric.
    pub fabric_consumer_group: String,
    /// `http://host:port` of the fabric broker.
    pub fabric_url: String,
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
        Self::parse(&std::env::vars().collect())
    }

    /// Validate `variables`, refusing the first thing wrong by name.
    pub fn parse(variables: &BTreeMap<String, String>) -> Result<Self> {
        let deployment = match required(
            variables,
            ENVIRONMENT,
            "`workstation` for a single-node development run, or the environment this ledger is \
             deployed in (`dev`, `staging`, `prod`)",
        )?
        .as_str()
        {
            "workstation" => Deployment::Workstation,
            "dev" => Deployment::Dev,
            "staging" => Deployment::Staging,
            "prod" => Deployment::Prod,
            other => {
                return Err(Error::invalid(format!(
                    "{ENVIRONMENT} is `{other}`; it must be `workstation`, `dev`, `staging` or \
                     `prod`"
                )));
            }
        };
        let storage = Self::storage(variables, deployment)?;
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
            "the broker's http://host:port (the egress-free in-VPC address)",
        )?;
        // The fabric transport speaks plaintext HTTP/1.1 and appends each
        // route's own path: a path here would be doubled, and https would
        // be refused by the client on the first call rather than here.
        let authority = fabric_url.strip_prefix("http://").ok_or_else(|| {
            Error::invalid(format!(
                "{FABRIC_URL} '{fabric_url}' must start with http://; the fabric client has no \
                 TLS stack and refuses https by name"
            ))
        })?;
        if authority.is_empty() || authority.contains('/') {
            return Err(Error::invalid(format!(
                "{FABRIC_URL} '{fabric_url}' must be scheme and authority only, with no path; \
                 every call appends its own route"
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
            deployment,
            storage,
            listen_addr,
            fabric_consumer_group,
            fabric_url,
            fabric_stream,
            fabric_partitions,
            poll_ms,
        })
    }

    /// The store `variables` select for `deployment`, refusing a local store
    /// in any deployment and a Spanner store that does not name its database.
    fn storage(
        variables: &BTreeMap<String, String>,
        deployment: Deployment,
    ) -> Result<LedgerStorage> {
        let chosen = match variables.get(STORE).map(|v| v.trim()) {
            None | Some("") if deployment.is_deployed() => "spanner",
            None | Some("") => "local",
            Some(other) => other,
        };
        match chosen {
            "local" if deployment.is_deployed() => Err(Error::invalid(format!(
                "{STORE}=local in the {deployment:?} environment; a deployed ledger is the \
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
        variables
    }

    /// R12-LEDGER-8007's check: a deployed ledger pointed at a non-Spanner
    /// store is refused, whether `local` is stated or merely defaulted to.
    /// The premise half: a workstation defaults to local, and every deployed
    /// environment with its database named loads as Spanner — so the
    /// refusals are about the store and not about a broken fixture.
    ///
    /// Mutation: drop the `if deployment.is_deployed()` guard on the
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
            assert!(config.deployment.is_deployed());
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
}
