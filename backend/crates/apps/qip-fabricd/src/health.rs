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

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use qip_observability::metrics::Metrics;
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
        }
    }

    pub fn storage_proven(&self) {
        self.storage_proven.store(true, Ordering::SeqCst);
    }

    pub fn set_serving(&self, serving: bool) {
        self.serving.store(serving, Ordering::SeqCst);
    }

    /// Ready means both: storage proven writable and the protocol served.
    pub fn is_ready(&self) -> bool {
        self.storage_proven.load(Ordering::SeqCst) && self.serving.load(Ordering::SeqCst)
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
        });
        Response::json(if ready { 200 } else { 503 }, body.to_string())
    }
}
