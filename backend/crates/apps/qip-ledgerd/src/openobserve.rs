//! This process's side of the OpenObserve drain (ADR 0028, ADR 0156 §5).
//!
//! The exporter itself lives once, in `qip-telemetry-drain`: it needs the
//! bounded HTTP client and the OTLP encoding of a metrics snapshot, a library
//! cannot host a thread that performs network I/O, and an app cannot depend on
//! another app. What stays here is the one thing a service crate may not do and
//! a composition root must: read the process environment. The ledger hands
//! the drain a lookup over its four documented variables and nothing else, so
//! the drain cannot be made to read a variable it does not name.
//!
//! Nothing is pointed at a collector by this module: `QIP_OPENOBSERVE_URL`
//! stays unset in every deployment until a collector is vendored, attested and
//! listened for (ADR 0156 §5.4).

pub use qip_telemetry_drain::{
    BACKOFF_CEILING, CREDENTIAL_VARIABLE, DEFAULT_INTERVAL, DrainHandle, ExportPass,
    INTERVAL_VARIABLE, ORG_VARIABLE, OpenObserveConfig, SignalOutcome, URL_VARIABLE, VARIABLES,
    export_once, record, spawn,
};

use qip_core::error::Result;

/// The drain's configuration from the process environment, or `None` when
/// `{URL_VARIABLE}` is unset and this process exports nothing.
///
/// The lookup answers [`VARIABLES`] and the credential's `_FILE` spelling and
/// refuses every other name, so a typo in the drain cannot quietly start
/// reading something else from the environment.
pub fn from_env() -> Result<Option<OpenObserveConfig>> {
    let file_variable = format!("{CREDENTIAL_VARIABLE}{}", qip_core::secret::FILE_SUFFIX);
    OpenObserveConfig::read(&|name| {
        (VARIABLES.contains(&name) || name == file_variable).then(|| std::env::var(name).ok())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger started with no collector configured must export nothing: the
    /// drain is opt-in, and a default that opened a connection would make
    /// every workstation run depend on a collector nobody vendored.
    #[test]
    fn a_ledger_with_no_collector_url_exports_nothing() {
        let config = OpenObserveConfig::read(&|_| None).expect("an unset url is not an error");
        assert!(config.is_none());
    }

    /// The other half: a url alone is enough to turn the drain on, so the
    /// check above is not passing because `read` always answers `None`.
    #[test]
    fn a_ledger_with_a_collector_url_builds_a_drain_configuration() {
        let config = OpenObserveConfig::read(&|name| {
            (name == URL_VARIABLE).then(|| "http://127.0.0.1:5080".to_string())
        })
        .expect("a plain-http url reads");
        assert!(config.is_some());
    }
}
