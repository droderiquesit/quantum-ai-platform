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
    /// Mutation: not run (nothing was compiled).
    #[test]
    fn a_ledger_with_no_collector_url_exports_nothing() {
        let config = OpenObserveConfig::read(&|_| None).expect("an unset url is not an error");
        assert!(config.is_none());
    }

    /// The other half: a url and an organisation turn the drain on, so the
    /// check above is not passing because `read` always answers `None`.
    /// The earlier draft of this test supplied the url alone and failed,
    /// because the drain refuses a url with no organisation rather than
    /// guess one (`OpenObserveConfig::parse`); a url alone is the refusal,
    /// pinned below.
    /// Mutation: not run.
    #[test]
    fn a_ledger_with_a_collector_url_and_organisation_builds_a_drain_configuration() {
        let config = OpenObserveConfig::read(&|name| match name {
            n if n == URL_VARIABLE => Some("http://127.0.0.1:5080".to_string()),
            n if n == ORG_VARIABLE => Some("default".to_string()),
            _ => None,
        })
        .expect("a plain-http url with an organisation reads");
        assert!(config.is_some());
    }

    /// A url with no organisation stops the process rather than draining
    /// nowhere. Mutation: not run.
    #[test]
    fn a_ledger_with_a_collector_url_and_no_organisation_is_refused() {
        let refused = OpenObserveConfig::read(&|name| {
            (name == URL_VARIABLE).then(|| "http://127.0.0.1:5080".to_string())
        });
        assert!(refused.is_err());
    }
}
