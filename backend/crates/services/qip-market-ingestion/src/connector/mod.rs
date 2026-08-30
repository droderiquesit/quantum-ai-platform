//! The source-connector SDK: a new data source is a manifest plus a small
//! adapter.
//!
//! Before this module, adding a source meant edits scattered across the
//! platform — a bespoke config struct, a hand-written poll loop, a descriptor,
//! a retry ladder nobody else's source shared, and a set of point-in-time
//! rules re-derived each time and re-derived slightly differently. The rules
//! that were got right in `rest.rs` were got right *there*, and a second
//! source had to be trusted to reach the same conclusions.
//!
//! Here they are reached once. [`manifest::SourceManifest`] is the source as
//! configuration; [`runtime::ConnectorRuntime`] is every cross-cutting
//! behaviour, applied identically to every source; [`SourceConnector`] is the
//! small remainder that is genuinely specific — what to ask for, what came
//! back, and what platform record each event becomes.
//!
//! # The lifecycle, and where each stage lives
//!
//! | stage | trait method | what the runtime adds |
//! |---|---|---|
//! | connect | [`SourceConnector::connect`] | resolves the credential, runs one health request, fails loudly |
//! | health | [`SourceConnector::health_request`] | liveness from the heartbeat; no rate-limit token spent |
//! | fetch | [`SourceConnector::fetch_request`], [`SourceConnector::decode`] | admission, bounded jittered backoff, schema and version gates |
//! | map | [`SourceConnector::map`] | fingerprint, dedup, knowable-time withholding, envelope, quarantine |
//! | checkpoint | [`SourceConnector::advance`] | the cursor, bound to source and schema |
//! | resume | [`SourceConnector::resume`] | refuses another source's or another schema's cursor |
//! | shutdown | [`SourceConnector::shutdown`] | marks the runtime disconnected |
//!
//! # The two disciplines a connector author cannot opt out of
//!
//! **A credential is never in a manifest.** [`manifest::AuthSpec`] holds a
//! [`manifest::SecretRef`] — a variable name — and no field of any type here
//! can hold a credential value. The transport resolves it once and applies it
//! as it writes the request, so a connector never sees one and a serialised
//! request cannot carry one.
//!
//! **Three instants are not one.** Event time, ingest time and knowable time
//! are three fields on [`envelope::MarketEventEnvelope`], and the runtime
//! withholds a record until its knowable time rather than when it occurred.
//! See that module for why conflating them is the cheapest way to write a
//! backtest that reads the future.
//!
//! # Testing a connector without a socket
//!
//! [`emulator::SourceEmulator`] implements the same
//! [`transport::SourceTransport`] port as the real HTTP client and answers
//! from recorded fixtures, including the answers that are hard to arrange
//! against a live source: a 429 with a `retry-after`, a truncated body, a page
//! that repeats itself, a payload missing a field. [`harness::ContractHarness`]
//! runs any connector through the checks every connector must pass. Between
//! them, the whole suite runs with no network.
//!
//! # Where to start
//!
//! `docs/data-sources/connector-development.md`, and the two worked examples
//! in [`crate::connectors`].

pub mod backoff;
pub mod checkpoint;
pub mod dedup;
pub mod emulator;
pub mod envelope;
pub mod harness;
pub mod heartbeat;
pub mod manifest;
pub mod quarantine;
pub mod ratelimit;
pub mod runtime;
pub mod transport;
pub mod validate;

pub use backoff::{BackoffLadder, FailureKind, RetryDecision};
pub use checkpoint::{Checkpoint, Cursor, CursorPosition};
pub use dedup::{DedupWindow, EventFingerprint, Novelty};
pub use emulator::{RecordedExchange, SourceEmulator};
pub use envelope::{MarketEventEnvelope, RawEvent};
pub use harness::{ContractCheck, ContractHarness, ContractReport};
pub use heartbeat::{FeedHeartbeat, Liveness};
pub use manifest::{
    AssetClass, AuthScheme, AuthSpec, EndpointSpec, FieldKind, FieldSpec, Protocol, RateLimitSpec,
    Region, RetrySpec, SchemaContract, SchemaVersion, SecretRef, SourceManifest,
    UnknownFieldPolicy,
};
pub use quarantine::{Quarantine, QuarantineReason, QuarantinedEvent};
pub use ratelimit::{Admission, RateLimiter};
pub use runtime::{
    ConnectorHealth, ConnectorRuntime, PollOutcome, PollReport, RuntimeConfig, RuntimeStats,
};
pub use transport::{
    HttpSourceTransport, RequestPurpose, SourceRequest, SourceResponse, SourceTransport,
};
pub use validate::{SchemaGuard, SchemaOutcome, SchemaViolation};

use crate::adapter::SensedRecord;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_financial::quality::DataQuality;

/// The stable contract every data source is reached through.
///
/// Six of the nine methods have a default that is right for a plain REST
/// source, so a new connector is usually three: what to ask for, what the
/// answer contained, and what each event becomes. The defaults are on the
/// trait rather than in a base struct so that overriding one is a visible
/// decision in the connector's own file.
///
/// # Why `decode` and `map` are separate
///
/// `decode` produces [`RawEvent`]s — the source's own key, the source's own
/// event time, the source's own bytes. `map` turns one of those into a
/// platform record. The split is what lets deduplication fingerprint what the
/// *source* sent rather than what this code chose to keep: a connector that
/// dropped a field and then fingerprinted its own output would give two
/// genuinely different events one identity.
///
/// # Why nothing here takes a transport
///
/// A connector describes requests; it does not make them. That is what allows
/// the same connector to be driven against a socket, against recorded
/// fixtures, and through [`harness::ContractHarness`], with no branch inside
/// the connector deciding which — a branch that would inevitably be the one
/// that let a recorded price reach production.
pub trait SourceConnector: std::fmt::Debug {
    /// The manifest this connector was built from.
    fn manifest(&self) -> &manifest::SourceManifest;

    /// Lifecycle: **connect**. Whatever the source needs before the first
    /// fetch — a session, a symbol list, a subscription. Default: nothing.
    fn connect(&mut self, at: Timestamp) -> Result<()> {
        let _ = at;
        Ok(())
    }

    /// Lifecycle: **health**. A request that proves the source is alive
    /// without fetching data. Default: the manifest's health path.
    fn health_request(&self) -> transport::SourceRequest {
        transport::SourceRequest::get(self.manifest().endpoint.health_path()).for_health()
    }

    /// Lifecycle: **fetch**. What to ask for, given where we got to.
    ///
    /// Default: the manifest's path and fixed query. A source with a cursor
    /// overrides this to add it.
    fn fetch_request(&self, cursor: &checkpoint::Cursor) -> Result<transport::SourceRequest> {
        let _ = cursor;
        let endpoint = &self.manifest().endpoint;
        let mut request = transport::SourceRequest::get(&endpoint.path);
        request.query = endpoint.query.clone();
        Ok(request)
    }

    /// The schema version the payload declares, where a source declares one.
    ///
    /// Default `None`: most public endpoints version their URL rather than
    /// their body. Returning a version here is what turns a source's own
    /// announcement of a breaking change into a refusal instead of a batch of
    /// records that look ordinary and are wrong.
    fn declared_version(&self, payload: &serde_json::Value) -> Option<manifest::SchemaVersion> {
        let _ = payload;
        None
    }

    /// The events one response carries.
    ///
    /// Each [`RawEvent`] states the source's own key and the instant the fact
    /// was true in the world — taken from the payload, never from a clock.
    fn decode(
        &self,
        payload: &serde_json::Value,
        cursor: &checkpoint::Cursor,
    ) -> Result<Vec<envelope::RawEvent>>;

    /// One event as a platform record.
    ///
    /// Refuse rather than guess. A symbol with no instrument behind it, an
    /// interval this connector cannot name, a field of the wrong type — each
    /// is an error here, and the runtime quarantines it with the reason,
    /// which is how a source that has started sending something new becomes
    /// visible instead of becoming a gap.
    ///
    /// `ingest_time` is the caller's horizon, and it is what a record carrying
    /// its own `Provenance` must stamp as its ingestion instant — never a
    /// clock read, so the same fetch replayed produces the same record.
    fn map(&self, event: &envelope::RawEvent, ingest_time: Timestamp) -> Result<SensedRecord>;

    /// What this connector knows about the quality of one event.
    ///
    /// Default: [`DataQuality::clean`] — a measurement with nothing wrong
    /// with it. A connector that interpolated a gap, or that received a value
    /// the source flagged as provisional, overrides this and says so, because
    /// `DataQuality::default` asserts a perfect measurement and an unstated
    /// quality would arrive downstream as one.
    fn quality_of(&self, event: &envelope::RawEvent) -> DataQuality {
        let _ = event;
        runtime::measured_quality()
    }

    /// Lifecycle: **checkpoint**. Where to resume from after this batch.
    ///
    /// Default: the newest event time the batch carried, which never moves
    /// backwards. A source with an opaque continuation token overrides this to
    /// carry the token instead.
    fn advance(
        &self,
        cursor: &checkpoint::Cursor,
        position: checkpoint::CursorPosition,
        accepted: u64,
    ) -> checkpoint::Cursor {
        cursor.advanced_to(position, accepted)
    }

    /// Lifecycle: **resume**. Default: the checkpoint's own check, which
    /// refuses another source's cursor and an incompatible schema version.
    fn resume(&mut self, checkpoint: &checkpoint::Checkpoint) -> Result<checkpoint::Cursor> {
        checkpoint.resume_into(self.manifest())
    }

    /// Lifecycle: **shutdown**. Release whatever `connect` acquired.
    fn shutdown(&mut self, at: Timestamp) -> Result<()> {
        let _ = at;
        Ok(())
    }
}
