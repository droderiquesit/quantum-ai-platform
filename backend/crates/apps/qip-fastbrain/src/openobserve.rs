//! This process's side of the OpenObserve drain (ADR 0028, ADR 0156 §5).
//!
//! The exporter itself lives once, in `qip-telemetry-drain`: it needs the
//! bounded HTTP client and the OTLP encoding of a metrics snapshot, a library
//! cannot host a thread that performs network I/O, and an app cannot depend on
//! another app. What stays here is the one thing a service crate may not do and
//! a composition root must: read the process environment. The fast brain hands
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
