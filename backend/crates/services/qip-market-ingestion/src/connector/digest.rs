//! What one successful poll actually fetched, hashed where the bytes exist.
//!
//! §22.3's data reference needs a content hash over "exactly the bytes read",
//! and there is one place in this platform where a vendor's bytes are in
//! hand together with the locator they came from and the instants the
//! decoded events describe: [`super::runtime::ConnectorRuntime::ingest`],
//! between the schema gate and the per-event gates. Everything downstream of
//! that seam sees mapped records, which are this platform's choice of fields
//! and not the vendor's bytes; hashing those would detect this code changing,
//! not the source revising.
//!
//! So the runtime records a [`FetchDigest`] there and carries it out on the
//! [`super::runtime::PollReport`]. The digest is a small record — a hash, a
//! length, a locator, a period, a bounded set of keys — and never the body:
//! the body is released to the loop and discarded, per §22.1's "transient"
//! class, and a digest that held it would be the raw copy that class exists
//! to forbid. This crate cannot name a data reference (it sits below
//! `qip-data-finder` on the dependency edge); the composition roots hand the
//! digest up to the kernel, which builds the reference against the source's
//! admission and keeps it in a bounded ledger.
//!
//! # Symbols and subjects
//!
//! The digest carries two key sets, because two different parties ask about
//! an extent. `symbols` are the source's own keys for the decoded events — a
//! trade id, `EUR/USD@2026-09-04`, a Kalshi ticker — which a reconciliation
//! against the vendor joins on. `subjects` are this platform's ids for what
//! the mapped records are about — an `ObjectId`, a macro series id — which
//! a research campaign and the reference ledger join on. The ledger held
//! only the first until 2026-09-12, and no campaign ever found a connector's
//! reference by it: a campaign asks by `ObjectId`, and a vendor's row key is
//! not one. A body whose events map to no subject at all yields no digest,
//! for the reason a body decoding to no events yields none: there is nothing
//! a reference could be found by.
//!
//! # What it costs the poll path
//!
//! One SHA-256 over a body that has already been parsed as JSON — linear in
//! a length the manifest's `max_events_per_batch` already bounds indirectly
//! and the transport bounds directly — a set of at most
//! `max_events_per_batch` keys, a set of at most as many subjects, and one
//! `map` per decoded event, duplicates and withheld ones included, which is
//! what makes a re-served table name the same subjects on every poll. No
//! allocation outlives the report, nothing blocks, and a digest that cannot
//! be formed leaves the poll exactly as it was: see
//! [`super::runtime::ConnectorRuntime::ingest`].

use super::envelope::RawEvent;
use super::manifest::{SchemaVersion, SourceManifest};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::Topic;
use serde::Serialize;
use std::collections::BTreeSet;

/// The content-hashed record of one delivered fetch.
///
/// Serialises, so a cycle line or a record can carry it, and deliberately
/// does not deserialise: [`FetchDigest::of`] is the one constructor, and it
/// hashes a body it was given, which is what lets a reference built from a
/// digest claim its hash was taken over real bytes. A `Deserialize` derive
/// would be a second constructor that takes any caller's word for the hash.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FetchDigest {
    source_id: String,
    /// The path and query the fetch was made against, as the transport put
    /// them on the wire — the same string a recorded fixture is filed under,
    /// and the locator a reference built from this digest carries.
    locator: String,
    /// SHA-256 of exactly the body the source served, lowercase hex.
    sha256: String,
    bytes: u64,
    /// The poll's horizon: when this platform read the bytes, not when the
    /// events occurred.
    retrieved_at: Timestamp,
    /// The earliest and latest event instants the body decoded to — what the
    /// bytes describe, as opposed to when they were read.
    period_start: Timestamp,
    period_end: Timestamp,
    /// The source's own keys for the decoded events — a currency pair, a
    /// trade id, a series code — bounded by the manifest's batch cap because
    /// the events were.
    symbols: BTreeSet<String>,
    /// This platform's ids for what the mapped records are about — what a
    /// reference built from this digest is found by. Bounded the same way.
    subjects: BTreeSet<String>,
    schema_version: SchemaVersion,
    /// The topic the source's records publish under, attached by the bridge
    /// that knows it ([`crate::connector_feed::ConnectorFeed`]); the runtime
    /// does not, and says so with `None` rather than guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    topic: Option<Topic>,
}

impl FetchDigest {
    /// Digest `body` as fetched from `locator`, describing `events`.
    ///
    /// Refuses an empty locator and an empty body for the reasons
    /// `qip_financial::manifest::SourceManifest::of` refuses them — a hash of
    /// nothing is the same for every extent that never arrived — refuses a
    /// body that decoded to no events: a digest with no period describes no
    /// extent of the world, and the ledger keys on the period; and refuses
    /// an empty subject set, because a reference nothing can be found by is
    /// a reference that flags nothing.
    pub fn of(
        manifest: &SourceManifest,
        locator: &str,
        body: &[u8],
        events: &[RawEvent],
        subjects: impl IntoIterator<Item = String>,
        retrieved_at: Timestamp,
    ) -> Result<Self> {
        if locator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a fetch from `{}` has no locator, so what was hashed cannot be re-fetched to \
                 check it",
                manifest.source_id
            )));
        }
        if body.is_empty() {
            return Err(Error::invalid(format!(
                "the fetch from `{}` at {locator} carried no bytes; the SHA-256 of nothing is \
                 the same for every extent that never arrived",
                manifest.source_id
            )));
        }
        let (Some(period_start), Some(period_end)) = (
            events.iter().map(|event| event.event_time).min(),
            events.iter().map(|event| event.event_time).max(),
        ) else {
            return Err(Error::invalid(format!(
                "the fetch from `{}` at {locator} decoded to no events, so there is no period \
                 of the world for a digest to describe",
                manifest.source_id
            )));
        };
        let subjects: BTreeSet<String> = subjects.into_iter().collect();
        if subjects.is_empty() {
            return Err(Error::invalid(format!(
                "the fetch from `{}` at {locator} mapped to no record with a subject, so a \
                 reference to it could never be found by the id a campaign asks with",
                manifest.source_id
            )));
        }
        Ok(Self {
            source_id: manifest.source_id.clone(),
            locator: locator.to_string(),
            sha256: qip_core::sha256_hex(body),
            bytes: body.len() as u64,
            retrieved_at,
            period_start,
            period_end,
            symbols: events.iter().map(|event| event.key.clone()).collect(),
            subjects,
            schema_version: manifest.schema.version,
            topic: None,
        })
    }

    /// Attach the topic the source's records publish under.
    pub fn with_topic(mut self, topic: Topic) -> Self {
        self.topic = Some(topic);
        self
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    pub const fn retrieved_at(&self) -> Timestamp {
        self.retrieved_at
    }

    /// The earliest and latest event instants the body described.
    pub const fn period(&self) -> (Timestamp, Timestamp) {
        (self.period_start, self.period_end)
    }

    /// The source's own keys for the decoded events. For a reconciliation
    /// against the vendor; never what a reference is found by.
    pub fn symbols(&self) -> &BTreeSet<String> {
        &self.symbols
    }

    /// This platform's ids for what the mapped records are about — what a
    /// reference built from this digest names, and is found by.
    pub fn subjects(&self) -> &BTreeSet<String> {
        &self.subjects
    }

    pub const fn topic(&self) -> Option<Topic> {
        self.topic
    }

    /// One line for a cycle summary.
    pub fn describe(&self) -> String {
        format!(
            "{} fetched {} byte(s) from {} covering {} to {} ({} key(s), {} subject(s)), \
             sha256 {}",
            self.source_id,
            self.bytes,
            self.locator,
            self.period_start.to_rfc3339(),
            self.period_end.to_rfc3339(),
            self.symbols.len(),
            self.subjects.len(),
            &self.sha256[..12.min(self.sha256.len())]
        )
    }
}
