//! Configuration loading and validation for the ledger.
//! See ADR 0100 § 1 for this module's role.
//!
//! Reads environment variables set by the composition root:
//! - `QIP_LEDGER_ACCOUNT_ID`: the account identifier for this ledger instance
//! - `QIP_LEDGER_ARCHIVE_PATH`: the directory where the ledger's durable store lives
//! - `QIP_LEDGER_LISTEN_ADDR`: the socket address for the read-side HTTP API
//! - `QIP_LEDGER_FABRIC_CONSUMER_GROUP`: the consumer group for P1 outcomes

use qip_core::error::{Error, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

/// Configuration for the ledger service, validated before start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerConfig {
    /// The account identifier for this ledger instance (non-empty string).
    pub account_id: String,
    /// Path to the directory where the ledger's durable store lives.
    pub archive_path: PathBuf,
    /// Socket address the read-side HTTP API will listen on.
    pub listen_addr: SocketAddr,
    /// Consumer group ID for reading P1 outcomes from the event fabric.
    pub fabric_consumer_group: String,
}

impl LedgerConfig {
    /// Load and validate configuration from environment variables.
    ///
    /// Returns an error if any required variable is missing or invalid.
    pub fn from_env() -> Result<Self> {
        let account_id = std::env::var("QIP_LEDGER_ACCOUNT_ID")
            .map_err(|_| Error::invalid("QIP_LEDGER_ACCOUNT_ID environment variable not set"))?;

        if account_id.is_empty() {
            return Err(Error::invalid("QIP_LEDGER_ACCOUNT_ID must not be empty"));
        }

        let archive_path_str = std::env::var("QIP_LEDGER_ARCHIVE_PATH")
            .map_err(|_| Error::invalid("QIP_LEDGER_ARCHIVE_PATH environment variable not set"))?;
        let archive_path = PathBuf::from(&archive_path_str);

        if !archive_path.exists() {
            return Err(Error::invalid(format!(
                "QIP_LEDGER_ARCHIVE_PATH {} does not exist",
                archive_path.display()
            )));
        }

        if !archive_path.is_dir() {
            return Err(Error::invalid(format!(
                "QIP_LEDGER_ARCHIVE_PATH {} is not a directory",
                archive_path.display()
            )));
        }

        let listen_addr_str = std::env::var("QIP_LEDGER_LISTEN_ADDR")
            .map_err(|_| Error::invalid("QIP_LEDGER_LISTEN_ADDR environment variable not set"))?;
        let listen_addr = SocketAddr::from_str(&listen_addr_str).map_err(|e| {
            Error::invalid(format!(
                "QIP_LEDGER_LISTEN_ADDR {} is not a valid socket address: {e}",
                listen_addr_str
            ))
        })?;

        let fabric_consumer_group =
            std::env::var("QIP_LEDGER_FABRIC_CONSUMER_GROUP").map_err(|_| {
                Error::invalid("QIP_LEDGER_FABRIC_CONSUMER_GROUP environment variable not set")
            })?;

        if fabric_consumer_group.is_empty() {
            return Err(Error::invalid(
                "QIP_LEDGER_FABRIC_CONSUMER_GROUP must not be empty",
            ));
        }

        Ok(Self {
            account_id,
            archive_path,
            listen_addr,
            fabric_consumer_group,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ledger_refuses_to_start_with_missing_config() {
        // Premise: account_id is required and must be non-empty.
        // We test that missing environment variables are refused.
        // We do not set any env vars to avoid issues with set_var.
        // The from_env() function checks for all required vars and rejects
        // any that are missing.
        let err = LedgerConfig::from_env();
        assert!(err.is_err(), "config with missing variables is refused");
        assert!(
            err.unwrap_err().message().contains("environment variable"),
            "error message references environment variables"
        );
    }
}
