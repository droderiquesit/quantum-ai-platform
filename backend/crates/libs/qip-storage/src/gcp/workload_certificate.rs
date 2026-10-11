//! A Compute Engine workload's certificate from Certificate Authority
//! Service (ADR 0112 §2), composed from the two halves that live in two
//! crates: the metadata server, which is this crate's, and the CA Service
//! client and renewal loop, which are `qip-transport`'s.
//!
//! One function rather than the same twenty lines in each of
//! `qip-fabricd`, `qip-edge-node` and `qip-ledgerd`: three copies of
//! "which account am I, and who signs my request" are three places for the
//! name on the certificate and the account requesting it to drift apart.

use crate::gcp::auth::MetadataServerTokens;
use qip_core::Clock;
use qip_core::error::Result;
use qip_transport::cas::{CasIdentity, CasSettings, WorkloadSubject};
use std::sync::Arc;

/// Name the certificate after the instance's service account, request it
/// with that account's metadata-server token, and start the renewal loop.
///
/// Stops the caller only when this process cannot say who it is — no
/// service account, or a metadata server that will not answer — because
/// nothing can be requested without a name. A CA that refuses or cannot be
/// reached does not stop it: the returned identity presents nothing and its
/// status says why, and the loop retries (see [`CasIdentity::start`]).
pub fn workload_certificate(
    settings: &CasSettings,
    workload: &str,
    clock: Arc<dyn Clock>,
) -> Result<CasIdentity> {
    workload_certificate_from(
        settings,
        workload,
        MetadataServerTokens::new(Arc::clone(&clock)),
        clock,
    )
}

/// [`workload_certificate`] against a given metadata source — a test's
/// loopback server.
pub fn workload_certificate_from(
    settings: &CasSettings,
    workload: &str,
    metadata: MetadataServerTokens,
    clock: Arc<dyn Clock>,
) -> Result<CasIdentity> {
    let email = metadata.service_account_email()?;
    let subject = WorkloadSubject::from_service_account(&email)?;
    CasIdentity::start(settings, workload, subject, Arc::new(metadata), clock)
}
