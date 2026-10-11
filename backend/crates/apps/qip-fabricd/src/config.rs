//! Configuration loading and validation for the event fabric broker.
//! See ADR 0100 § 1 for this module's role.
//!
//! Every value arrives as a `QIP_EVENT_FABRIC_*` variable (ADR 0100 §1 fixes
//! the prefix) and every one that decides where records live, who may write
//! them or how a key routes is **required**. There is no default data
//! directory, catalogue, identities file, archive directory, listen address
//! or partition count: ADR 0100 §5 says "nothing rests on a broker default",
//! and a broker that started on a directory nobody chose would acknowledge
//! writes to a place nobody is backing up.
//!
//! [`Config::parse`] takes the variables as a map rather than reading the
//! environment, so the refusals below are tested without mutating process
//! state; [`Config::from_environment`] is the one line that reads it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use qip_core::error::{Error, Result};

/// Where partition segments, the leader epoch and consumer checkpoints live.
pub const DATA_DIR: &str = "QIP_EVENT_FABRIC_DATA_DIR";
/// The committed stream catalogue: stream policies and grants.
pub const CATALOGUE: &str = "QIP_EVENT_FABRIC_CATALOGUE";
/// The identities file: one `<identity> <sha256 of its token>` per line.
pub const IDENTITIES_FILE: &str = "QIP_EVENT_FABRIC_IDENTITIES_FILE";
/// Where sealed segments are archived to, when the archive is a directory.
/// Exactly one of this and [`ARCHIVE_BUCKET`] is set.
pub const ARCHIVE_DIR: &str = "QIP_EVENT_FABRIC_ARCHIVE_DIR";
/// The Cloud Storage landing bucket sealed segments are archived to, when
/// the archive is a bucket (FABRIC-8101, DATA-8124). Exactly one of this and
/// [`ARCHIVE_DIR`] is set.
pub const ARCHIVE_BUCKET: &str = "QIP_EVENT_FABRIC_ARCHIVE_BUCKET";
/// Optional: the key prefix inside [`ARCHIVE_BUCKET`] this broker writes
/// under, so one regional landing bucket can hold several brokers.
pub const ARCHIVE_PREFIX: &str = "QIP_EVENT_FABRIC_ARCHIVE_PREFIX";
/// The address the fabric protocol is served on.
pub const LISTEN: &str = "QIP_EVENT_FABRIC_LISTEN";
/// The address health and metrics are served on, never the protocol's own.
pub const HEALTH_LISTEN: &str = "QIP_EVENT_FABRIC_HEALTH_LISTEN";
/// How many partitions every stream is declared with.
pub const PARTITIONS: &str = "QIP_EVENT_FABRIC_PARTITIONS";
/// Optional: the size at which a partition's active segment is sealed.
pub const SEGMENT_BYTES: &str = "QIP_EVENT_FABRIC_SEGMENT_BYTES";
/// Optional: how often the archiver and the catalogue reload run.
pub const HOUSEKEEPING_MS: &str = "QIP_EVENT_FABRIC_HOUSEKEEPING_MS";

/// Optional: which managed consensus P0 and P1 are committed to (ADR 0111).
/// Unset is the local single-broker fabric, the default for development and
/// tests; `spanner` is the only other value, and anything else is refused
/// rather than read as either.
pub const MANAGED_LOG: &str = "QIP_EVENT_FABRIC_MANAGED_LOG";
/// With `spanner`: the database, `projects/<p>/instances/<i>/databases/<d>`.
pub const SPANNER_DATABASE: &str = "QIP_EVENT_FABRIC_SPANNER_DATABASE";
/// With a managed log: the region this broker's leases are keyed under.
pub const REGION: &str = "QIP_EVENT_FABRIC_REGION";
/// With a managed log: this instance's name, unique in its region (the zone
/// and the VM name, say). The lease holder is this joined to the process's
/// leader epoch.
pub const INSTANCE: &str = "QIP_EVENT_FABRIC_INSTANCE";
/// Optional, with a managed log: how long a lease lasts in the database's
/// time. The lease loop renews at a third of it.
pub const LEASE_TTL_MS: &str = "QIP_EVENT_FABRIC_LEASE_TTL_MS";
/// Optional: how many batches of each partition stay on this broker's disk
/// once their segments are archived and indexed; older history is read back
/// from the archive (FABRIC-8079). Unset keeps everything on disk.
pub const HOT_BATCHES: &str = "QIP_EVENT_FABRIC_HOT_BATCHES";

/// The lease TTL when none is stated.
pub const DEFAULT_LEASE_TTL_MS: u64 = 10_000;
/// The shortest lease accepted: below this a renewal at a third of it races
/// one Spanner round trip, and a healthy leader would depose itself.
pub const MINIMUM_LEASE_TTL_MS: u64 = 1_500;
/// The longest lease accepted: a dead leader holds its partitions this long
/// before a standby may take them, and every P0/P1 producer waits for it.
pub const MAXIMUM_LEASE_TTL_MS: u64 = 120_000;

/// Where the managed consensus lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagedBackend {
    /// A Spanner database reached through the TLS-terminating proxy, with a
    /// token from the metadata server or a token file — the same access the
    /// Cloud Storage archive uses, refused the same way.
    Spanner {
        database: String,
        endpoint: String,
        metadata_server: bool,
        token_file: Option<String>,
    },
}

/// ADR 0111's managed commit path, when configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedConfig {
    pub backend: ManagedBackend,
    pub region: String,
    pub instance: String,
    pub lease_ttl_ms: u64,
}

/// The most partitions a stream may be declared with. Every partition is a
/// directory, an open segment and a producer window; a count past this is a
/// typo, not a deployment.
pub const MAXIMUM_PARTITIONS: u32 = 1024;

/// The housekeeping interval when none is stated: the catalogue's own seal
/// cadence for the local slice (`seal_age_ms: 1000`).
pub const DEFAULT_HOUSEKEEPING_MS: u64 = 1000;

/// Where sealed segments go.
///
/// A directory stays the target for development and tests. A bucket is the
/// deployed target: content-addressed segment objects in a regional landing
/// bucket, reached over the in-VPC TLS-terminating proxy with a token from
/// the metadata server or a token file (`qip_storage::gcp`). There is no
/// fallback from one to the other — a broker told to archive to a bucket it
/// cannot reach refuses to start rather than writing to the container's
/// disk and acknowledging P0/P1 batches as archived on a copy that dies with
/// the pod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveTarget {
    Directory(PathBuf),
    CloudStorage {
        bucket: String,
        prefix: String,
        /// `http://` address of the TLS-terminating proxy (`QIP_GCP_ENDPOINT`).
        endpoint: String,
        /// Take tokens from the instance metadata server
        /// (`QIP_GCP_METADATA_SERVER`); otherwise `token_file`.
        metadata_server: bool,
        /// A token file something else keeps fresh (`QIP_GCP_TOKEN_FILE`).
        token_file: Option<String>,
    },
}

/// The broker's validated configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub data_dir: PathBuf,
    pub catalogue: PathBuf,
    pub identities_file: PathBuf,
    pub archive: ArchiveTarget,
    pub listen: String,
    pub health_listen: String,
    pub partitions: u32,
    pub segment_bytes: Option<u64>,
    pub housekeeping_ms: u64,
    /// `None` is the local single-broker fabric (ADR 0100); `Some` seats the
    /// broker at the managed consensus and requires every P0/P1 stream to
    /// be declared `managed_majority` (ADR 0111).
    pub managed: Option<ManagedConfig>,
    /// The hot window, in batches per partition, when retention is on.
    pub hot_batches: Option<u64>,
}

/// The proxy endpoint and the one lasting credential a Google API is reached
/// with, refused by name when either is missing or a literal token is given.
fn gcp_access(
    variables: &BTreeMap<String, String>,
    for_what: &str,
) -> Result<(String, bool, Option<String>)> {
    use qip_storage::gcp::{
        ENDPOINT_VARIABLE, METADATA_VARIABLE, TOKEN_FILE_VARIABLE, TOKEN_VARIABLE,
    };
    // A literal token expires in about an hour and nothing in a long-running
    // broker would notice; the archive or the managed log would start failing
    // an hour after start-up with every P0/P1 producer waiting on it.
    if present(variables, TOKEN_VARIABLE).is_some() {
        return Err(Error::invalid(format!(
            "{TOKEN_VARIABLE} is set for {for_what}; a broker runs for longer than a literal \
             token lives. Use {METADATA_VARIABLE}=1 or {TOKEN_FILE_VARIABLE}"
        )));
    }
    let endpoint = present(variables, ENDPOINT_VARIABLE).ok_or_else(|| {
        Error::invalid(format!(
            "{for_what} is configured but {ENDPOINT_VARIABLE} is not; set it to the http:// \
             address of the TLS-terminating proxy in front of Google"
        ))
    })?;
    let metadata_server = present(variables, METADATA_VARIABLE)
        .is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "yes" | "on"));
    let token_file = present(variables, TOKEN_FILE_VARIABLE).map(str::to_string);
    if metadata_server == token_file.is_some() {
        return Err(Error::invalid(format!(
            "{for_what} needs exactly one credential: {METADATA_VARIABLE}=1 on Google Cloud, or \
             {TOKEN_FILE_VARIABLE} pointing at a file something keeps fresh"
        )));
    }
    Ok((endpoint.to_string(), metadata_server, token_file))
}

/// A Spanner database path, refused unless it is exactly
/// `projects/<id>/instances/<id>/databases/<id>` in the characters those ids
/// allow — it is spliced into every request path.
fn spanner_database(raw: &str) -> Result<String> {
    let parts: Vec<&str> = raw.split('/').collect();
    let shaped = parts.len() == 6
        && parts[0] == "projects"
        && parts[2] == "instances"
        && parts[4] == "databases"
        && [parts[1], parts[3], parts[5]].iter().all(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        });
    if !shaped {
        return Err(Error::invalid(format!(
            "{SPANNER_DATABASE} must be projects/<project>/instances/<instance>/databases/<db> in \
             lower-case letters, digits, '-' and '_', not '{raw}'"
        )));
    }
    Ok(raw.to_string())
}

/// A label-safe name: lower-case letters, digits and `-`, as a region or an
/// instance name is. Both reach a lease row and a log line.
fn label_name(variables: &BTreeMap<String, String>, name: &str, what: &str) -> Result<String> {
    let value = required(variables, name, what)?;
    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(Error::invalid(format!(
            "{name} must be lower-case letters, digits and '-', not '{value}'"
        )));
    }
    Ok(value)
}

/// ADR 0111's managed commit path, or `None` for the local fabric.
fn managed_config(variables: &BTreeMap<String, String>) -> Result<Option<ManagedConfig>> {
    let backend = match present(variables, MANAGED_LOG) {
        None => {
            // Fail closed on a half-configured managed log: settings for it
            // with the switch off mean somebody meant to turn it on, and a
            // broker that started local would acknowledge P0/P1 on one disk
            // while the deployment believed otherwise.
            for stray in [SPANNER_DATABASE, LEASE_TTL_MS] {
                if present(variables, stray).is_some() {
                    return Err(Error::invalid(format!(
                        "{stray} is set but {MANAGED_LOG} is not; set {MANAGED_LOG}=spanner to \
                         commit P0/P1 to the managed consensus, or unset {stray}"
                    )));
                }
            }
            return Ok(None);
        }
        Some("spanner") => {
            let database = spanner_database(&required(
                variables,
                SPANNER_DATABASE,
                "the Spanner database P0/P1 are committed to",
            )?)?;
            let (endpoint, metadata_server, token_file) =
                gcp_access(variables, "the Spanner managed log")?;
            ManagedBackend::Spanner {
                database,
                endpoint,
                metadata_server,
                token_file,
            }
        }
        Some(other) => {
            return Err(Error::invalid(format!(
                "{MANAGED_LOG} is '{other}'; the only managed log is 'spanner' (ADR 0111). Unset \
                 it for the local single-broker fabric"
            )));
        }
    };
    let region = label_name(
        variables,
        REGION,
        "the region this broker's leases are keyed in",
    )?;
    let instance = label_name(variables, INSTANCE, "this instance's name in its region")?;
    let lease_ttl_ms = optional_number(variables, LEASE_TTL_MS)?.unwrap_or(DEFAULT_LEASE_TTL_MS);
    if !(MINIMUM_LEASE_TTL_MS..=MAXIMUM_LEASE_TTL_MS).contains(&lease_ttl_ms) {
        return Err(Error::invalid(format!(
            "{LEASE_TTL_MS} must be between {MINIMUM_LEASE_TTL_MS} and {MAXIMUM_LEASE_TTL_MS}, \
             not {lease_ttl_ms}"
        )));
    }
    Ok(Some(ManagedConfig {
        backend,
        region,
        instance,
        lease_ttl_ms,
    }))
}

fn required(variables: &BTreeMap<String, String>, name: &str, what: &str) -> Result<String> {
    match variables.get(name).map(|value| value.trim()) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(Error::invalid(format!(
            "{name} is not set; set it to {what}. The event fabric has no default for it \
             (ADR 0100 §5: nothing rests on a broker default)"
        ))),
    }
}

fn optional_number(variables: &BTreeMap<String, String>, name: &str) -> Result<Option<u64>> {
    match variables.get(name).map(|value| value.trim()) {
        None | Some("") => Ok(None),
        Some(value) => value.parse::<u64>().map(Some).map_err(|_| {
            Error::invalid(format!(
                "{name} must be a whole number, not '{value}'; unset it to take the default"
            ))
        }),
    }
}

fn present<'a>(variables: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    variables
        .get(name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Exactly one archive target, refused by name when there are none or two.
fn archive_target(variables: &BTreeMap<String, String>, data_dir: &str) -> Result<ArchiveTarget> {
    match (
        present(variables, ARCHIVE_DIR),
        present(variables, ARCHIVE_BUCKET),
    ) {
        (Some(_), Some(_)) => Err(Error::invalid(format!(
            "both {ARCHIVE_DIR} and {ARCHIVE_BUCKET} are set; set exactly one. Archiving to \
             whichever a match arm happened to prefer would leave the other believed in"
        ))),
        (None, None) => Err(Error::invalid(format!(
            "{ARCHIVE_DIR} is not set; set it to the directory sealed segments are archived \
             to, or set {ARCHIVE_BUCKET} to a Cloud Storage landing bucket. The event fabric \
             has no default for it (ADR 0100 §5: nothing rests on a broker default)"
        ))),
        (Some(directory), None) => {
            // An archive inside the data directory is deleted by the same
            // mistake, and filled by the same disk, as the segments it copies.
            if PathBuf::from(directory).starts_with(data_dir) {
                return Err(Error::invalid(format!(
                    "{ARCHIVE_DIR} ('{directory}') is inside {DATA_DIR} ('{data_dir}'); archive \
                     to a directory outside the one being archived"
                )));
            }
            Ok(ArchiveTarget::Directory(PathBuf::from(directory)))
        }
        (None, Some(bucket)) => {
            let (endpoint, metadata_server, token_file) =
                gcp_access(variables, &format!("{ARCHIVE_BUCKET} (a bucket archive)"))?;
            Ok(ArchiveTarget::CloudStorage {
                bucket: bucket.to_string(),
                prefix: present(variables, ARCHIVE_PREFIX)
                    .unwrap_or_default()
                    .to_string(),
                endpoint,
                metadata_server,
                token_file,
            })
        }
    }
}

impl Config {
    /// Read the process environment. The only place this binary does.
    pub fn from_environment() -> Result<Self> {
        Self::parse(&std::env::vars().collect())
    }

    /// Validate `variables`, refusing the first thing wrong by name.
    pub fn parse(variables: &BTreeMap<String, String>) -> Result<Self> {
        let data_dir = required(
            variables,
            DATA_DIR,
            "the directory partitions are stored in",
        )?;
        let catalogue = required(variables, CATALOGUE, "the stream catalogue file")?;
        let identities_file = required(
            variables,
            IDENTITIES_FILE,
            "the identities file of token digests",
        )?;
        let archive = archive_target(variables, &data_dir)?;
        let listen = required(variables, LISTEN, "the address the protocol is served on")?;
        let health_listen = required(
            variables,
            HEALTH_LISTEN,
            "the address health and metrics are served on",
        )?;
        // FABRIC-073: health must not share the protocol's listener, or a
        // data plane that stops answering takes its own outage report down
        // with it. Port 0 asks the kernel for a free port, so two zeros are
        // two different ports and are not refused.
        if listen == health_listen && !listen.ends_with(":0") {
            return Err(Error::invalid(format!(
                "{LISTEN} and {HEALTH_LISTEN} are both '{listen}'; health is served on its own \
                 listener so that it still answers when the protocol's does not"
            )));
        }
        let partitions_text = required(
            variables,
            PARTITIONS,
            "how many partitions each stream is declared with",
        )?;
        let partitions = partitions_text.parse::<u32>().map_err(|_| {
            Error::invalid(format!(
                "{PARTITIONS} must be a whole number, not '{partitions_text}'"
            ))
        })?;
        if partitions == 0 || partitions > MAXIMUM_PARTITIONS {
            return Err(Error::invalid(format!(
                "{PARTITIONS} must be between 1 and {MAXIMUM_PARTITIONS}, not {partitions}"
            )));
        }
        let housekeeping_ms =
            optional_number(variables, HOUSEKEEPING_MS)?.unwrap_or(DEFAULT_HOUSEKEEPING_MS);
        if housekeeping_ms == 0 {
            return Err(Error::invalid(format!(
                "{HOUSEKEEPING_MS} must be at least 1; zero would spin the archiver thread"
            )));
        }
        let hot_batches = optional_number(variables, HOT_BATCHES)?;
        if hot_batches == Some(0) {
            return Err(Error::invalid(format!(
                "{HOT_BATCHES} must be at least 1; unset it to keep every segment on disk"
            )));
        }
        Ok(Self {
            data_dir: PathBuf::from(data_dir),
            catalogue: PathBuf::from(catalogue),
            identities_file: PathBuf::from(identities_file),
            archive,
            listen,
            health_listen,
            partitions,
            segment_bytes: optional_number(variables, SEGMENT_BYTES)?,
            housekeeping_ms,
            managed: managed_config(variables)?,
            hot_batches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> BTreeMap<String, String> {
        [
            (DATA_DIR, "/var/lib/fabric/data"),
            (CATALOGUE, "/etc/fabric/streams.json"),
            (IDENTITIES_FILE, "/etc/fabric/identities"),
            (ARCHIVE_DIR, "/var/lib/fabric/archive"),
            (LISTEN, "127.0.0.1:7100"),
            (HEALTH_LISTEN, "127.0.0.1:7101"),
            (PARTITIONS, "4"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
    }

    /// The failure this prevents: a broker that starts on a directory, a
    /// catalogue or a partition count nobody chose. Each required variable
    /// is removed in turn and the refusal must name that variable.
    #[test]
    fn a_broker_refuses_to_start_without_each_setting_it_has_no_default_for() {
        let config = Config::parse(&complete()).expect("premise: the complete set is accepted");
        assert_eq!(config.partitions, 4);
        assert_eq!(config.housekeeping_ms, DEFAULT_HOUSEKEEPING_MS);
        for name in [
            DATA_DIR,
            CATALOGUE,
            IDENTITIES_FILE,
            ARCHIVE_DIR,
            LISTEN,
            HEALTH_LISTEN,
            PARTITIONS,
        ] {
            let mut variables = complete();
            variables.remove(name);
            let error = Config::parse(&variables).expect_err("a missing setting is refused");
            assert!(
                error.to_string().contains(&format!("{name} is not set")),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn a_broker_refuses_health_on_the_protocol_listener_an_archive_inside_its_data_and_no_partitions()
     {
        let mut shared = complete();
        shared.insert(HEALTH_LISTEN.to_string(), "127.0.0.1:7100".to_string());
        let error = Config::parse(&shared).expect_err("a shared listener is refused");
        assert!(error.to_string().contains("its own listener"), "{error}");

        let mut both = complete();
        both.insert(ARCHIVE_BUCKET.to_string(), "landing-eu".to_string());
        let error = Config::parse(&both).expect_err("two archive targets are refused");
        assert!(error.to_string().contains("set exactly one"), "{error}");

        let mut nested = complete();
        nested.insert(
            ARCHIVE_DIR.to_string(),
            "/var/lib/fabric/data/archive".to_string(),
        );
        let error = Config::parse(&nested).expect_err("a nested archive is refused");
        assert!(error.to_string().contains("is inside"), "{error}");

        for bad in ["0", "1025", "four"] {
            let mut variables = complete();
            variables.insert(PARTITIONS.to_string(), bad.to_string());
            assert!(Config::parse(&variables).is_err(), "partitions = {bad}");
        }
    }

    fn bucket() -> BTreeMap<String, String> {
        let mut variables = complete();
        variables.remove(ARCHIVE_DIR);
        for (name, value) in [
            (ARCHIVE_BUCKET, "qip-fabric-landing-europe-west2"),
            (ARCHIVE_PREFIX, "fabricd-0"),
            (qip_storage::gcp::ENDPOINT_VARIABLE, "http://127.0.0.1:8443"),
            (qip_storage::gcp::METADATA_VARIABLE, "1"),
        ] {
            variables.insert(name.to_string(), value.to_string());
        }
        variables
    }

    /// FABRIC-8101 / DATA-8124: a bucket is a selectable archive target, and
    /// one that could not be reached for longer than an hour, or at all, is
    /// refused at start rather than discovered by the first sealed segment.
    /// Mutation: in `archive_target`, accept a bucket with no credential —
    /// the credential-less iteration is accepted and this fails.
    #[test]
    fn a_bucket_archive_is_accepted_with_an_endpoint_and_one_lasting_credential_and_refused_otherwise()
     {
        let config = Config::parse(&bucket()).expect("premise: a complete bucket archive loads");
        assert_eq!(
            config.archive,
            ArchiveTarget::CloudStorage {
                bucket: "qip-fabric-landing-europe-west2".to_string(),
                prefix: "fabricd-0".to_string(),
                endpoint: "http://127.0.0.1:8443".to_string(),
                metadata_server: true,
                token_file: None,
            }
        );
        let directory = Config::parse(&complete()).expect("the directory archive still loads");
        assert!(matches!(directory.archive, ArchiveTarget::Directory(_)));

        for (remove, insert) in [
            (Some(qip_storage::gcp::ENDPOINT_VARIABLE), None),
            (Some(qip_storage::gcp::METADATA_VARIABLE), None),
            (
                None,
                Some((qip_storage::gcp::TOKEN_FILE_VARIABLE, "/run/token")),
            ),
            (
                None,
                Some((qip_storage::gcp::TOKEN_VARIABLE, "ya29.literal")),
            ),
        ] {
            let mut variables = bucket();
            if let Some(name) = remove {
                variables.remove(name);
            }
            if let Some((name, value)) = insert {
                variables.insert(name.to_string(), value.to_string());
            }
            assert!(
                Config::parse(&variables).is_err(),
                "removed {remove:?}, inserted {insert:?}"
            );
        }
    }

    fn spanner() -> BTreeMap<String, String> {
        let mut variables = complete();
        for (name, value) in [
            (MANAGED_LOG, "spanner"),
            (
                SPANNER_DATABASE,
                "projects/qip-dev/instances/fabric-ew2/databases/fabric",
            ),
            (REGION, "europe-west2"),
            (INSTANCE, "fabricd-ew2-a"),
            (qip_storage::gcp::ENDPOINT_VARIABLE, "http://127.0.0.1:8443"),
            (qip_storage::gcp::METADATA_VARIABLE, "1"),
        ] {
            variables.insert(name.to_string(), value.to_string());
        }
        variables
    }

    /// ADR 0111: the managed commit path is off unless asked for, and asked
    /// for it is complete or refused. The failure this prevents is a broker
    /// that half-reads a Spanner configuration and starts local, answering
    /// P0/P1 producers from one disk while the deployment believes they
    /// survive a zone.
    ///
    /// Mutation: in `managed_config`, return `Ok(None)` for an unknown
    /// `MANAGED_LOG` value — the `raft` iteration is accepted and this fires.
    /// Mutation: drop the stray-setting check — a database with the switch
    /// off is accepted and this fires.
    #[test]
    fn the_managed_log_is_local_by_default_complete_when_asked_for_and_refused_half_configured() {
        let local = Config::parse(&complete()).expect("premise: the local fabric loads");
        assert_eq!(local.managed, None);
        assert_eq!(local.hot_batches, None);

        let managed = Config::parse(&spanner()).expect("a complete Spanner path loads");
        assert_eq!(
            managed.managed,
            Some(ManagedConfig {
                backend: ManagedBackend::Spanner {
                    database: "projects/qip-dev/instances/fabric-ew2/databases/fabric".to_string(),
                    endpoint: "http://127.0.0.1:8443".to_string(),
                    metadata_server: true,
                    token_file: None,
                },
                region: "europe-west2".to_string(),
                instance: "fabricd-ew2-a".to_string(),
                lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            })
        );

        let mut unknown = spanner();
        unknown.insert(MANAGED_LOG.to_string(), "raft".to_string());
        assert!(Config::parse(&unknown).is_err(), "an unknown managed log");

        let mut stray = complete();
        stray.insert(
            SPANNER_DATABASE.to_string(),
            "projects/p/instances/i/databases/d".to_string(),
        );
        let error = Config::parse(&stray).expect_err("a database with the switch off");
        assert!(error.to_string().contains(MANAGED_LOG), "{error}");

        for name in [
            SPANNER_DATABASE,
            REGION,
            INSTANCE,
            qip_storage::gcp::ENDPOINT_VARIABLE,
            qip_storage::gcp::METADATA_VARIABLE,
        ] {
            let mut variables = spanner();
            variables.remove(name);
            assert!(Config::parse(&variables).is_err(), "without {name}");
        }
        for (name, bad) in [
            (SPANNER_DATABASE, "projects/p/instances/i"),
            (SPANNER_DATABASE, "projects/p/instances/i/databases/d?x=1"),
            (REGION, "Europe West"),
            (LEASE_TTL_MS, "100"),
            (LEASE_TTL_MS, "999999"),
            (qip_storage::gcp::TOKEN_VARIABLE, "ya29.literal"),
        ] {
            let mut variables = spanner();
            variables.insert(name.to_string(), bad.to_string());
            assert!(Config::parse(&variables).is_err(), "{name} = {bad}");
        }

        let mut zero = complete();
        zero.insert(HOT_BATCHES.to_string(), "0".to_string());
        assert!(Config::parse(&zero).is_err(), "a zero hot window");
        let mut hot = complete();
        hot.insert(HOT_BATCHES.to_string(), "64".to_string());
        assert_eq!(
            Config::parse(&hot).expect("a hot window").hot_batches,
            Some(64)
        );
    }
}
