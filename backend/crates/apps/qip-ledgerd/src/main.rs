//! Ledger composition root's entry point.
//!
//! ADR 0100 assigns this binary the ledger's role: sole writer of the chain
//! of double-entry postings for fills, and its own read-side API (ADR 0100
//! §2). `qip_ledgerd::start` is the composition and holds the order — storage
//! proven writable, port bound, partitions consumed, and only then ready;
//! the crate documentation says why each step is where it is. This file
//! reads the environment (the only place this binary does), resolves the
//! fabric bearer token through `qip_core::secret` so it may arrive as a
//! mounted file, calls `start`, and stops the process naming the reason when
//! it refuses.
//!
//! The comments here used to list "bind the listen address" and "start
//! consuming" as steps while the code beneath them did neither and exited
//! with the read API's unconditional error.

use std::sync::Arc;

use qip_core::SystemClock;
use qip_ledgerd::config::{FABRIC_TOKEN, LedgerConfig, LedgerStorage};
use qip_ledgerd::consumer::{Connect, thread_sleeper};
use qip_storage::{GcpAccess, gcp};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport};
use qip_transport::tls::{LinkSecurity, WorkloadIdentity};

fn main() {
    let started = LedgerConfig::from_environment().and_then(|config| {
        // Resolved once, here, to fail at start on a missing or malformed
        // token; each worker's transport resolves its own copy from the same
        // two values so the token is never cloned through a `BearerToken`.
        let direct = std::env::var(FABRIC_TOKEN).ok();
        let path = std::env::var(format!("{FABRIC_TOKEN}{}", qip_core::secret::FILE_SUFFIX)).ok();
        BearerToken::resolve(FABRIC_TOKEN, direct.clone(), path.clone())?;
        // ADR 0112: the fabric link is mutual TLS with this workload's
        // certificate unless a local deployment chose plaintext. Loaded once,
        // here, so a certificate that will not load stops the ledger before
        // a worker starts; there is no fallback.
        let link = match &config.fabric_tls {
            Some(paths) => LinkSecurity::MutualTls(Arc::new(WorkloadIdentity::from_files(paths)?)),
            None => LinkSecurity::Plaintext,
        };
        eprintln!("qip-ledgerd: fabric link {}", link.describe());
        let url = config.fabric_url.clone();
        let connect: Connect = Arc::new(move || {
            let token = BearerToken::resolve(FABRIC_TOKEN, direct.clone(), path.clone())?;
            let transport: Box<dyn FabricTransport + Send> =
                Box::new(HttpTransport::over(&link, &url, token)?);
            Ok(transport)
        });
        let clock: Arc<dyn qip_core::Clock> = Arc::new(SystemClock);
        // The Google endpoint and credential the Spanner store is reached
        // through, read only when that is the store: a workstation run on
        // the local store must not be refused over a credential it was
        // never going to present. The access token goes through
        // `qip_core::secret`, so it may arrive as a mounted file.
        let access = match &config.storage {
            LedgerStorage::Spanner { .. } => {
                let plain = |name: &str| std::env::var(name).ok();
                let token = qip_core::secret::resolve_from(
                    gcp::TOKEN_VARIABLE,
                    plain(gcp::TOKEN_VARIABLE),
                    plain(&format!(
                        "{}{}",
                        gcp::TOKEN_VARIABLE,
                        qip_core::secret::FILE_SUFFIX
                    )),
                )?;
                GcpAccess::from_values(
                    plain(gcp::ENDPOINT_VARIABLE).as_deref(),
                    plain(gcp::METADATA_VARIABLE).as_deref(),
                    plain(gcp::TOKEN_FILE_VARIABLE).as_deref(),
                    token.as_deref(),
                    clock.clone(),
                )?
            }
            LedgerStorage::Local { .. } => GcpAccess::unconfigured(),
        };
        qip_ledgerd::start_with_access(&config, clock, thread_sleeper(), connect, access)
    });
    match started {
        Ok(running) => {
            eprintln!(
                "qip-ledgerd: serving the ledger read API and health on {}",
                running.address()
            );
            running.wait();
        }
        Err(error) => {
            eprintln!("qip-ledgerd: refusing to start. {error}");
            std::process::exit(1);
        }
    }
}
