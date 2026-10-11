//! Health check and readiness reporting for the event fabric broker.
//! See ADR 0100 § 1 for this module's role.
//!
//! FABRIC-073: the fabric's health must never depend on the fabric's own
//! topics. So this module holds a few atomics the composition root sets
//! directly, is served from a listener of its own, and has no path to the
//! broker at all: it does not fetch, it does not read a stream, and the
//! protocol handler is not among the things it can name. When the data
//! plane stops, [`Health::respond`] is what still answers, and it answers
//! `503` with `"serving": false` rather than going quiet with it.
//!
//! `/metrics` is served here too, for the reason
//! `qip_transport::event_fabric::protocol` gives for leaving it off the
//! protocol: an operator's probe should not go through the refusal machinery
//! a producer's traffic does.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use qip_observability::metrics::Metrics;
use qip_transport::cas::RenewalStatus;
use qip_transport::server::{Method, Request, Response};

/// What the broker process knows about its own readiness.
#[derive(Debug)]
pub struct Health {
    leader_epoch: u64,
    /// Set once the data directory has been written to and every declared
    /// partition has recovered. Never set before either.
    storage_proven: AtomicBool,
    /// Whether the protocol listener is accepting requests.
    serving: AtomicBool,
    archive_passes: AtomicU64,
    /// Why the last archive pass failed, or `None` when it did not.
    archive_error: Mutex<Option<String>>,
    /// Why the catalogue on disk is not the one in force, or `None` when it
    /// is. A catalogue the broker refused changes nothing about who may do
    /// what, and this is where an operator finds out it was refused.
    catalogue_error: Mutex<Option<String>>,
    /// The last lease pass, for a broker seated at the managed consensus
    /// (ADR 0111): partitions led, followed and failed, and the first
    /// failure. `None` on the local fabric, where there are no leases.
    leases: Mutex<Option<serde_json::Value>>,
    /// The listener's issued certificate, under `QIP_TLS_MODE=cas` (ADR
    /// 0112 §2). When set, the broker is ready only while it holds a current
    /// certificate — read at the probe, not from a flag the renewal loop
    /// last wrote, so a certificate that lapses between two renewal attempts
    /// reads as not-ready at once.
    certificate: OnceLock<Arc<RenewalStatus>>,
}

fn set(slot: &Mutex<Option<String>>, value: Option<String>) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = value;
}

fn get(slot: &Mutex<Option<String>>) -> Option<String> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

impl Health {
    pub fn new(leader_epoch: u64) -> Self {
        Self {
            leader_epoch,
            storage_proven: AtomicBool::new(false),
            serving: AtomicBool::new(false),
            archive_passes: AtomicU64::new(0),
            archive_error: Mutex::new(None),
            catalogue_error: Mutex::new(None),
            leases: Mutex::new(None),
            certificate: OnceLock::new(),
        }
    }

    /// Record one lease pass. A partition this instance cannot tend is named
    /// here, because from outside it looks like a P0/P1 producer being
    /// refused with nothing near the producer saying why.
    pub fn lease_pass(
        &self,
        report: &qip_core::Result<qip_streaming::event_fabric::broker::LeaseReport>,
    ) {
        let body = match report {
            Ok(report) => serde_json::json!({
                "leading": report.leading.len(),
                "following": report.following.len(),
                "failed": report.failed.len(),
                "first_failure": report.failed.values().next(),
            }),
            Err(error) => serde_json::json!({ "error": error.to_string() }),
        };
        *self.leases.lock().unwrap_or_else(|e| e.into_inner()) = Some(body);
    }

    pub fn storage_proven(&self) {
        self.storage_proven.store(true, Ordering::SeqCst);
    }

    pub fn set_serving(&self, serving: bool) {
        self.serving.store(serving, Ordering::SeqCst);
    }

    /// Gate readiness on an issued certificate from now on. Once only: a
    /// broker has one listener and one identity.
    pub fn require_certificate(&self, status: Arc<RenewalStatus>) {
        let _ = self.certificate.set(status);
    }

    /// Ready means all of: storage proven writable, the protocol served,
    /// and — under `cas` — a certificate the listener can present now. A
    /// broker without one refuses every handshake, and a probe told it was
    /// ready would route producers at a listener that turns them all away.
    pub fn is_ready(&self) -> bool {
        self.storage_proven.load(Ordering::SeqCst)
            && self.serving.load(Ordering::SeqCst)
            && self
                .certificate
                .get()
                .is_none_or(|status| status.is_current())
    }

    /// Record how one archive pass went.
    pub fn archive_pass(&self, outcome: &qip_core::Result<u64>) {
        self.archive_passes.fetch_add(1, Ordering::SeqCst);
        set(
            &self.archive_error,
            outcome.as_ref().err().map(ToString::to_string),
        );
    }

    /// Record whether the catalogue on disk is the one in force.
    pub fn catalogue_check(&self, outcome: &qip_core::Result<()>) {
        set(
            &self.catalogue_error,
            outcome.as_ref().err().map(ToString::to_string),
        );
    }

    /// How many archive passes have finished, succeeded or not.
    pub fn archive_passes(&self) -> u64 {
        self.archive_passes.load(Ordering::SeqCst)
    }

    /// Answer one request on the health listener.
    ///
    /// `GET /metrics` is the registry's Prometheus exposition. Every other
    /// path is the readiness body, `200` when ready and `503` when not, so a
    /// probe that only reads the status line is still told the truth.
    pub fn respond(&self, metrics: &Metrics, request: &Request) -> Response {
        if !matches!(request.method, Method::Get | Method::Head) {
            return Response::json(405, r#"{"error":"the health listener answers GET only"}"#);
        }
        if request.path == "/metrics" {
            return Response::new(
                200,
                "text/plain; version=0.0.4",
                metrics.snapshot().to_prometheus().into_bytes(),
            );
        }
        let ready = self.is_ready();
        let body = serde_json::json!({
            "service": "qip-fabricd",
            "status": if ready { "ready" } else { "not_ready" },
            "storage_proven": self.storage_proven.load(Ordering::SeqCst),
            "serving": self.serving.load(Ordering::SeqCst),
            "leader_epoch": self.leader_epoch,
            "archive_passes": self.archive_passes(),
            "archive_error": get(&self.archive_error),
            "catalogue_error": get(&self.catalogue_error),
            "leases": self.leases.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "workload_certificate": self.certificate.get().map(|status| status.to_json()),
        });
        Response::json(if ready { 200 } else { 503 }, body.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_transport::tls::WorkloadIdentity;
    use std::collections::BTreeMap;

    /// A CA certificate `openssl req -x509` made for this test; its key was
    /// deleted unread. Public material: it only lets an identity exist with
    /// roots and no certificate, which is the state under test.
    const FIXTURE_ROOT: &str = "-----BEGIN CERTIFICATE-----
MIIBqzCCAVOgAwIBAgIUYT43nsbEcR277bc6CUkHAo5ZbjowCgYIKoZIzj0EAwIw
IzEhMB8GA1UEAwwYcWlwIHJlYWRpbmVzcyBmaXh0dXJlIENBMCAXDTI2MTAxMTEx
MDEwOFoYDzIxMjYwOTE3MTEwMTA4WjAjMSEwHwYDVQQDDBhxaXAgcmVhZGluZXNz
IGZpeHR1cmUgQ0EwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAATQZpXAff4IZ68a
48wN/d71sb6foGUd8b+/60TuL8tLKMU3W8WOGutofeSFvdbum2VcgSIKSG5iAW4B
oZz5lLOuo2MwYTAdBgNVHQ4EFgQUW/eoZJFiL/VbokTiODWFqzpfZx8wHwYDVR0j
BBgwFoAUW/eoZJFiL/VbokTiODWFqzpfZx8wDwYDVR0TAQH/BAUwAwEB/zAOBgNV
HQ8BAf8EBAMCAQYwCgYIKoZIzj0EAwIDRgAwQwIfAxe48XHniN3HVZAeq+yZXgUP
y+tWW22AN6xQlVPo3gIgJH4TQbQ5DbBEaFi8NBD1YPhsi93qd8LFwl7M9NuVuYo=
-----END CERTIFICATE-----
";

    fn probe() -> Request {
        Request {
            method: Method::Get,
            path: "/health".to_string(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body: Vec::new(),
            peer: "127.0.0.1:1".to_string(),
        }
    }

    /// ADR 0112's fail-closed rule at the probe: a broker on CA Service that
    /// holds no current certificate refuses every handshake, so it must not
    /// read as ready. Mutation: leave the certificate out of `is_ready` and
    /// the second assertion fails.
    #[test]
    fn a_broker_without_a_current_certificate_is_not_ready() {
        let health = Health::new(1);
        health.storage_proven();
        health.set_serving(true);
        assert!(health.is_ready(), "premise: ready with no certificate gate");
        let identity = WorkloadIdentity::awaiting_issuance(
            FIXTURE_ROOT.as_bytes(),
            Arc::new(qip_core::SystemClock),
        )
        .expect("the fixture root loads");
        health.require_certificate(Arc::new(RenewalStatus::new(Arc::new(identity))));
        assert!(!health.is_ready(), "ready with no certificate to present");
        let response = health.respond(&Metrics::new("qip-fabricd"), &probe());
        assert_eq!(response.status, 503);
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("json");
        assert_eq!(body["workload_certificate"]["current"], false);
    }
}
