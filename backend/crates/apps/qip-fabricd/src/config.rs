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
/// Where sealed segments are archived to.
pub const ARCHIVE_DIR: &str = "QIP_EVENT_FABRIC_ARCHIVE_DIR";
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

/// The broker's validated configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub data_dir: PathBuf,
    pub catalogue: PathBuf,
    pub identities_file: PathBuf,
    pub archive_dir: PathBuf,
    pub listen: String,
    pub health_listen: String,
    pub partitions: u32,
    pub segment_bytes: Option<u64>,
    pub housekeeping_ms: u64,
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
        let archive_dir = required(
            variables,
            ARCHIVE_DIR,
            "the directory sealed segments are archived to",
        )?;
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
        // An archive inside the data directory is deleted by the same
        // mistake, and filled by the same disk, as the segments it copies.
        if PathBuf::from(&archive_dir).starts_with(&data_dir) {
            return Err(Error::invalid(format!(
                "{ARCHIVE_DIR} ('{archive_dir}') is inside {DATA_DIR} ('{data_dir}'); archive \
                 to a directory outside the one being archived"
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
        Ok(Self {
            data_dir: PathBuf::from(data_dir),
            catalogue: PathBuf::from(catalogue),
            identities_file: PathBuf::from(identities_file),
            archive_dir: PathBuf::from(archive_dir),
            listen,
            health_listen,
            partitions,
            segment_bytes: optional_number(variables, SEGMENT_BYTES)?,
            housekeeping_ms,
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
}
