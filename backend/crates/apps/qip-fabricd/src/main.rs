//! Event fabric composition root's entry point.
//!
//! ADR 0100 assigns this binary the event-fabric broker's role: sole writer
//! of every partition's batch chain, running its own clock for segment roll
//! and archive (ADR 0100 §2). `qip_fabricd::start` is the composition and
//! holds the order every composition root here holds to — configuration
//! refused first, storage proven writable before a port is bound, serving
//! last (`.claude/rules/architecture/00-boundaries.md`). This file reads the
//! environment, calls it, and stops the process naming the reason when it
//! refuses.
//!
//! Under `QIP_TLS_MODE=cas` (ADR 0112 §2) this file also starts the
//! listener's certificate: named after the instance's service account,
//! requested from Certificate Authority Service with that account's
//! metadata-server token, over a key generated in memory, and renewed at
//! two-thirds of its life on a thread of its own. A CA that cannot be
//! reached at start does not stop the broker: it binds, refuses every
//! handshake, and answers `/health` with 503 naming the CA's error until a
//! certificate arrives.
//!
//! This binary refused to start outright until its `config`, `health` and
//! `archiver` modules existed, because serving on a configuration nobody
//! validated is the failure that order exists to close. It now starts only
//! on a configuration with no defaults in it. It is still not deployed:
//! `docs/adr/0010-what-gets-deployed.md` keeps it out of the image matrix
//! and every workload catalogue until its placement is decided (ADR 0099
//! C8).

use std::sync::Arc;

use qip_core::{Clock, SystemClock};
use qip_fabricd::config::Config;
use qip_storage::gcp::workload_certificate::workload_certificate;
use qip_transport::tls::TlsSource;

fn main() {
    let config = match Config::from_environment() {
        Ok(config) => config,
        Err(error) => refuse(&error),
    };
    match start(&config) {
        Ok(running) => {
            // The OpenObserve drain (ADR 0028, ADR 0156 §5.5): unset means this
            // broker's metrics and its produce spans stay local, as they
            // always have. Set means a thread exports them on an interval;
            // the handle must outlive `wait()` below, because dropping it
            // stops the thread. A configuration that does not parse stops
            // the process rather than draining nowhere.
            let _drain = match qip_fabricd::openobserve::from_env() {
                Ok(Some(drain)) => {
                    eprintln!("qip-fabricd: telemetry draining to {}", drain.describe());
                    match qip_fabricd::openobserve::spawn(
                        running.observability().clone(),
                        drain,
                        Arc::new(SystemClock),
                    ) {
                        Ok(handle) => Some(handle),
                        Err(error) => refuse(&error),
                    }
                }
                Ok(None) => None,
                Err(error) => refuse(&error),
            };
            // Printed from the configuration, never from `running`: the
            // broker holds the workload certificate's key, and no value read
            // out of a key-holding value reaches a log line.
            eprintln!(
                "qip-fabricd: serving the event-fabric protocol on {} and health on {}",
                config.listen, config.health_listen
            );
            running.wait();
        }
        Err(error) => refuse(&error),
    }
}

fn refuse(error: &qip_core::Error) -> ! {
    eprintln!("qip-fabricd: refusing to start. {error}");
    std::process::exit(1);
}

fn start(config: &Config) -> qip_core::Result<qip_fabricd::Running> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    if let Some(source) = &config.listener_tls {
        eprintln!("qip-fabricd: protocol listener over {}", source.describe());
    }
    match &config.listener_tls {
        Some(TlsSource::Issued(settings)) => {
            let issued = workload_certificate(settings, "qip-fabricd", Arc::clone(&clock))?;
            let status = issued.status();
            if let Some(error) = status.last_error() {
                eprintln!(
                    "qip-fabricd: no workload certificate yet; serving not-ready and \
                     retrying. The CA said: {error}"
                );
            }
            qip_fabricd::start_issued(config, clock, issued)
        }
        _ => qip_fabricd::start(config, clock),
    }
}
