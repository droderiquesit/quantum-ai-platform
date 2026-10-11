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

use qip_core::deployment::Deployment;
use qip_core::error::{Error, Result};
use qip_transport::tls::{PeerNames, TLS_MODE_VARIABLE, TlsPaths, link_mode};

/// Which deployment this broker runs in (ADR 0110 decision 5, ADR 0112).
/// Required: it decides whether the protocol listener may be plaintext,
/// and an omission must not decide that.
pub const DEPLOYMENT: &str = "QIP_DEPLOYMENT";
/// The workload names a client certificate must be valid for, comma
/// separated (ADR 0112). Empty or unset admits any certificate the CA pool
/// issued, which is the pool template's identity control alone.
pub const ADMITTED_PEERS: &str = "QIP_EVENT_FABRIC_ADMITTED_PEERS";

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
    /// The protocol listener's workload certificate (ADR 0112): `Some` is
    /// mutual TLS with a client certificate required on every connection;
    /// `None` is plaintext, which [`Config::parse`] admits only when
    /// [`DEPLOYMENT`] is `local` and `QIP_TLS_MODE=plaintext` says so. The
    /// health listener is plaintext either way: a probe carries no
    /// certificate, and health on its own listener is FABRIC-073's point.
    pub listener_tls: Option<TlsPaths>,
    /// [`ADMITTED_PEERS`], validated as names a certificate can carry.
    pub admitted_peers: Vec<String>,
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
    use qip_storage::gcp::{
        ENDPOINT_VARIABLE, METADATA_VARIABLE, TOKEN_FILE_VARIABLE, TOKEN_VARIABLE,
    };
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
            // A literal token expires in about an hour and nothing in a
            // long-running broker would notice; the archive would start
            // failing an hour after start-up with every P0/P1 producer
            // waiting on it.
            if present(variables, TOKEN_VARIABLE).is_some() {
                return Err(Error::invalid(format!(
                    "{TOKEN_VARIABLE} is set for a bucket archive; a broker runs for longer \
                     than a literal token lives. Use {METADATA_VARIABLE}=1 or \
                     {TOKEN_FILE_VARIABLE}"
                )));
            }
            let endpoint = present(variables, ENDPOINT_VARIABLE).ok_or_else(|| {
                Error::invalid(format!(
                    "{ARCHIVE_BUCKET} is set but {ENDPOINT_VARIABLE} is not; set it to the \
                     http:// address of the TLS-terminating proxy in front of Cloud Storage"
                ))
            })?;
            let metadata_server = present(variables, METADATA_VARIABLE)
                .is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "yes" | "on"));
            let token_file = present(variables, TOKEN_FILE_VARIABLE).map(str::to_string);
            if metadata_server == token_file.is_some() {
                return Err(Error::invalid(format!(
                    "a bucket archive needs exactly one credential: {METADATA_VARIABLE}=1 on \
                     Google Cloud, or {TOKEN_FILE_VARIABLE} pointing at a file something keeps \
                     fresh"
                )));
            }
            Ok(ArchiveTarget::CloudStorage {
                bucket: bucket.to_string(),
                prefix: present(variables, ARCHIVE_PREFIX)
                    .unwrap_or_default()
                    .to_string(),
                endpoint: endpoint.to_string(),
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
        // ADR 0112: decided here, before anything is opened, from the
        // deployment and the TLS variables; the identity itself is loaded by
        // `start`, after storage is proven and before the port is bound.
        let deployment = Deployment::parse(present(variables, DEPLOYMENT))?;
        let listener_tls = link_mode(
            deployment,
            present(variables, TLS_MODE_VARIABLE),
            TlsPaths::from_lookup(&|name| present(variables, name).map(str::to_string))?.as_ref(),
        )?;
        let admitted_peers: Vec<String> = present(variables, ADMITTED_PEERS)
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        PeerNames::new(admitted_peers.clone())?;
        if listener_tls.is_none() && !admitted_peers.is_empty() {
            return Err(Error::invalid(format!(
                "{ADMITTED_PEERS} names workloads and the protocol listener is plaintext; a \
                 peer list nothing can check reads as a control. Configure the listener's \
                 certificate or unset {ADMITTED_PEERS}"
            )));
        }
        let housekeeping_ms =
            optional_number(variables, HOUSEKEEPING_MS)?.unwrap_or(DEFAULT_HOUSEKEEPING_MS);
        if housekeeping_ms == 0 {
            return Err(Error::invalid(format!(
                "{HOUSEKEEPING_MS} must be at least 1; zero would spin the archiver thread"
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
            listener_tls,
            admitted_peers,
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
            (DEPLOYMENT, "local"),
            (TLS_MODE_VARIABLE, "plaintext"),
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
            DEPLOYMENT,
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

    /// ADR 0112 at the broker. Mutation: let `plaintext` through outside
    /// `local`, or treat an unset mode with no certificate as plaintext, and
    /// one of these parses; this fails.
    #[test]
    fn a_broker_serves_plaintext_only_where_a_local_deployment_chose_it() {
        // Premise: the local plaintext fixture is accepted and is plaintext.
        assert_eq!(
            Config::parse(&complete())
                .expect("local plaintext")
                .listener_tls,
            None
        );
        let mut deployed = complete();
        deployed.insert(DEPLOYMENT.to_string(), "prod".to_string());
        let error = Config::parse(&deployed).expect_err("plaintext was admitted in prod");
        assert!(error.to_string().contains("mutual TLS"), "{error}");

        let mut unset = complete();
        unset.remove(TLS_MODE_VARIABLE);
        assert!(
            Config::parse(&unset).is_err(),
            "an unset mode with no certificate came up plaintext"
        );

        let mut tls = complete();
        tls.remove(TLS_MODE_VARIABLE);
        tls.insert(DEPLOYMENT.to_string(), "prod".to_string());
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
            (
                ADMITTED_PEERS,
                "edge-node.prod.qip.internal, ledgerd.prod.qip.internal",
            ),
        ] {
            tls.insert(name.to_string(), value.to_string());
        }
        let config = Config::parse(&tls).expect("a deployed broker with a certificate");
        assert!(config.listener_tls.is_some());
        assert_eq!(config.admitted_peers.len(), 2);
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
}
