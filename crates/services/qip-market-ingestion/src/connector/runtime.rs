//! The runtime that drives one connector through the lifecycle.
//!
//! This is where the promise in [`super`] is kept. Everything that is the same
//! for every source — admission, backoff, schema checking, deduplication,
//! knowable-time withholding, quarantine, heartbeat, cursor advance — happens
//! here, once. A connector supplies only what is different about its source:
//! the request to make, the events in a response, and the record each event
//! becomes.
//!
//! # Nothing here reads a clock or draws entropy from the environment
//!
//! `at` comes from the caller and the jitter comes from a seeded RNG, so the
//! same poll replayed against the same fixtures produces the same envelopes,
//! the same backoff schedule and the same quarantine entries. That is what
//! makes an incident reproducible from a log rather than described from
//! memory.
//!
//! # A poll degrades; a connect fails
//!
//! [`ConnectorRuntime::connect`] is loud: a missing credential or a rejected
//! one is an error, so a bad rollout fails while somebody is watching it.
//! [`ConnectorRuntime::poll`] is quiet: a source that is down produces a
//! report saying so and a dead letter, not an error that stops the ingestion
//! loop for every other source in the process.

use super::SourceConnector;
use super::backoff::{BackoffLadder, FailureKind, RetryDecision};
use super::checkpoint::{Checkpoint, Cursor, CursorPosition};
use super::dedup::{DedupWindow, Novelty};
use super::envelope::{MarketEventEnvelope, RawEvent};
use super::heartbeat::{FeedHeartbeat, Liveness};
use super::manifest::SourceManifest;
use super::quarantine::{Quarantine, QuarantineReason};
use super::ratelimit::{Admission, RateLimiter};
use super::transport::{SourceResponse, SourceTransport};
use super::validate::SchemaGuard;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_financial::quality::DataQuality;
use qip_transport::{Sleeper, ThreadSleeper};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How the runtime is sized and seeded.
///
/// Separate from the manifest because these are deployment decisions, not
/// facts about the source: how much memory this process will spend remembering
/// fingerprints, and which seed spreads this replica's retries against its
/// siblings'.
#[derive(Clone)]
pub struct RuntimeConfig {
    /// Seeds the backoff jitter. Two replicas of one connector must use
    /// different seeds, or a rollout has them all retrying on the same
    /// millisecond.
    pub seed: u64,
    /// Fingerprints remembered. Sized for the source's replay behaviour: at
    /// least one poll window's worth of events, with room for a resume to
    /// re-read the boundary.
    pub dedup_capacity: usize,
    /// Dead letters held in memory before the oldest is dropped.
    pub quarantine_capacity: usize,
    /// Where a backoff interval is spent. [`ThreadSleeper`] in production;
    /// `qip_transport::RecordingSleeper` under test, so a test asserts the
    /// schedule instead of spending it.
    pub sleeper: Arc<dyn Sleeper>,
}

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfig")
            .field("seed", &self.seed)
            .field("dedup_capacity", &self.dedup_capacity)
            .field("quarantine_capacity", &self.quarantine_capacity)
            .finish_non_exhaustive()
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            seed: 0x5150_1ee7_c0ff_ee01,
            dedup_capacity: 8_192,
            quarantine_capacity: 256,
            sleeper: Arc::new(ThreadSleeper),
        }
    }
}

impl RuntimeConfig {
    pub fn seeded(seed: u64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }

    pub fn with_sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    pub const fn with_dedup_capacity(mut self, capacity: usize) -> Self {
        self.dedup_capacity = capacity;
        self
    }

    pub const fn with_quarantine_capacity(mut self, capacity: usize) -> Self {
        self.quarantine_capacity = capacity;
        self
    }
}

/// What one poll did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PollOutcome {
    /// The source answered and its events were processed.
    Delivered,
    /// The rate limiter refused. No request was made and nothing is lost;
    /// the caller waits until `until` and asks again.
    Deferred { until: Timestamp },
    /// Every attempt the manifest permits failed. A dead letter records why.
    Refused,
}

impl PollOutcome {
    pub const fn delivered(&self) -> bool {
        matches!(self, Self::Delivered)
    }
}

/// The result of one poll.
#[derive(Clone, Debug)]
pub struct PollReport {
    pub outcome: PollOutcome,
    /// Events that passed every gate and are knowable now.
    pub admitted: Vec<MarketEventEnvelope>,
    /// Events already seen inside the dedup window.
    pub duplicates: u64,
    /// Events that exist and that the deployment is not yet entitled to see.
    /// Not a loss: the next poll's window covers them again.
    pub withheld: u64,
    pub quarantined: u64,
    /// Requests made, including the first. Above one means retries happened.
    pub attempts: u32,
    /// What the backoff spent inside this poll.
    pub waited: Duration,
    pub liveness: Liveness,
}

impl PollReport {
    fn empty(outcome: PollOutcome, liveness: Liveness) -> Self {
        Self {
            outcome,
            admitted: Vec::new(),
            duplicates: 0,
            withheld: 0,
            quarantined: 0,
            attempts: 0,
            waited: Duration::ZERO,
            liveness,
        }
    }
}

/// Everything this runtime has done, for metrics and for a test that asserts
/// a gate ran rather than assuming it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStats {
    pub polls: u64,
    pub requests: u64,
    pub deferrals: u64,
    pub retries: u64,
    pub refusals: u64,
    pub admitted: u64,
    pub duplicates: u64,
    pub withheld: u64,
    pub quarantined: u64,
}

/// A source's liveness and reachability, answered without fetching data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorHealth {
    pub source_id: String,
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub latency: Duration,
    pub liveness: Liveness,
    pub detail: String,
}

impl ConnectorHealth {
    /// Whether this source should be believed right now.
    pub const fn is_serving(&self) -> bool {
        self.reachable && !self.liveness.is_alarming()
    }
}

/// One connector's cross-cutting machinery.
#[derive(Debug)]
pub struct ConnectorRuntime {
    manifest: SourceManifest,
    guard: SchemaGuard,
    limiter: RateLimiter,
    ladder: BackoffLadder,
    dedup: DedupWindow,
    quarantine: Quarantine,
    heartbeat: FeedHeartbeat,
    cursor: Cursor,
    stats: RuntimeStats,
    sleeper: Arc<dyn Sleeper>,
    connected: bool,
}

impl ConnectorRuntime {
    pub fn new(manifest: SourceManifest, config: RuntimeConfig) -> Result<Self> {
        manifest.validate()?;
        let guard = SchemaGuard::new(&manifest.source_id, manifest.schema.clone());
        let limiter = RateLimiter::new(manifest.rate_limit)?;
        let ladder = BackoffLadder::new(manifest.retry.policy(), config.seed)?;
        let dedup = DedupWindow::new(config.dedup_capacity)?;
        let quarantine = Quarantine::new(&manifest.source_id, config.quarantine_capacity)?;
        let heartbeat = FeedHeartbeat::new(&manifest.source_id, manifest.freshness_sla());
        Ok(Self {
            manifest,
            guard,
            limiter,
            ladder,
            dedup,
            quarantine,
            heartbeat,
            cursor: Cursor::beginning(),
            stats: RuntimeStats::default(),
            sleeper: config.sleeper,
            connected: false,
        })
    }

    pub const fn manifest(&self) -> &SourceManifest {
        &self.manifest
    }

    pub const fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    pub const fn stats(&self) -> RuntimeStats {
        self.stats
    }

    pub const fn quarantine(&self) -> &Quarantine {
        &self.quarantine
    }

    pub const fn heartbeat(&self) -> &FeedHeartbeat {
        &self.heartbeat
    }

    pub const fn dedup(&self) -> &DedupWindow {
        &self.dedup
    }

    pub const fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    pub const fn is_connected(&self) -> bool {
        self.connected
    }

    /// Lifecycle: **connect**. Loud on purpose.
    ///
    /// Runs the connector's own preparation and then one health request, so a
    /// missing endpoint, a rejected credential or a source that is simply not
    /// there fails during the rollout rather than an hour later inside a poll
    /// loop, where it looks like a feed that has gone quiet.
    pub fn connect(
        &mut self,
        connector: &mut dyn SourceConnector,
        transport: &mut dyn SourceTransport,
        at: Timestamp,
    ) -> Result<ConnectorHealth> {
        connector.connect(at)?;
        let health = self.health(connector, transport, at)?;
        if !health.reachable {
            return Err(Error::unavailable(format!(
                "`{}` did not answer its health check through {}: {}",
                self.manifest.source_id,
                transport.describe(),
                health.detail
            )));
        }
        self.connected = true;
        Ok(health)
    }

    /// Lifecycle: **health**. One cheap request, no data fetched.
    ///
    /// Deliberately does not consume a rate-limit token: a limiter that
    /// refused the health check would make a saturated feed indistinguishable
    /// from a dead one, which is precisely when the answer matters.
    pub fn health(
        &mut self,
        connector: &mut dyn SourceConnector,
        transport: &mut dyn SourceTransport,
        at: Timestamp,
    ) -> Result<ConnectorHealth> {
        let request = connector.health_request();
        let liveness = self.heartbeat.liveness(at);
        match transport.request(&request, at) {
            Ok(response) => Ok(ConnectorHealth {
                source_id: self.manifest.source_id.clone(),
                reachable: response.is_success(),
                status: Some(response.status),
                latency: response.latency,
                liveness,
                detail: if response.is_success() {
                    format!(
                        "answered HTTP {} in {:?}",
                        response.status, response.latency
                    )
                } else {
                    format!(
                        "answered HTTP {}: {}",
                        response.status,
                        response.body_excerpt()
                    )
                },
            }),
            Err(error) => Ok(ConnectorHealth {
                source_id: self.manifest.source_id.clone(),
                reachable: false,
                status: None,
                latency: Duration::ZERO,
                liveness,
                detail: error.message().to_string(),
            }),
        }
    }

    /// Lifecycle: **fetch**. One admission, up to the manifest's attempts, one
    /// batch through every gate.
    pub fn poll(
        &mut self,
        connector: &mut dyn SourceConnector,
        transport: &mut dyn SourceTransport,
        at: Timestamp,
    ) -> Result<PollReport> {
        self.stats.polls = self.stats.polls.saturating_add(1);
        if let Admission::Deferred { until, .. } = self.limiter.admit(at) {
            self.stats.deferrals = self.stats.deferrals.saturating_add(1);
            return Ok(PollReport::empty(
                PollOutcome::Deferred { until },
                self.heartbeat.liveness(at),
            ));
        }

        let request = connector.fetch_request(&self.cursor)?;
        let mut attempts = 0u32;
        let mut waited = Duration::ZERO;
        let mut now = at;
        let response = loop {
            attempts = attempts.saturating_add(1);
            self.stats.requests = self.stats.requests.saturating_add(1);
            let failure = match transport.request(&request, now) {
                Ok(response) if response.is_success() => break Some(response),
                Ok(response) => self.classify(&response, now),
                Err(error) => FailureKind::Transient {
                    detail: error.message().to_string(),
                },
            };
            match self.ladder.failed(&failure) {
                RetryDecision::Retry { after, .. } => {
                    self.stats.retries = self.stats.retries.saturating_add(1);
                    self.sleeper.sleep(after);
                    waited = waited + after;
                    now = now.saturating_add(after);
                }
                RetryDecision::GiveUp { attempts, reason } => {
                    self.stats.refusals = self.stats.refusals.saturating_add(1);
                    self.stats.quarantined = self.stats.quarantined.saturating_add(1);
                    self.quarantine.hold(
                        request.target(),
                        None,
                        QuarantineReason::RetriesExhausted {
                            attempts,
                            detail: reason,
                        },
                        failure.detail(),
                        now,
                    );
                    break None;
                }
            }
        };

        let Some(response) = response else {
            let mut report = PollReport::empty(PollOutcome::Refused, self.heartbeat.liveness(now));
            report.attempts = attempts;
            report.waited = waited;
            report.quarantined = 1;
            return Ok(report);
        };
        self.ladder.succeeded();

        let mut report = self.ingest(connector, &response, &request.target(), now);
        report.attempts = attempts;
        report.waited = waited;
        report.liveness = self.heartbeat.liveness(now);
        Ok(report)
    }

    /// A non-success status, read as the thing an operator would do about it.
    fn classify(&mut self, response: &SourceResponse, at: Timestamp) -> FailureKind {
        let excerpt = response.body_excerpt();
        let name = &self.manifest.source_id;
        match response.status {
            401 | 403 => FailureKind::Permanent {
                detail: format!(
                    "{name} rejected this deployment's credential with HTTP {}. The credential \
                     is not quoted here and is written to no log by this connector",
                    response.status
                ),
            },
            404 => FailureKind::Permanent {
                detail: format!(
                    "{name} has no endpoint at the configured path (HTTP 404): {excerpt}"
                ),
            },
            status if response.is_rate_limited() => {
                // The limiter is told as well as the ladder: the ladder decides
                // when *this* request tries again, and the limiter is what
                // stops the next poll walking straight into the same wall.
                if let Some(wait) = response.retry_after {
                    self.limiter.pause_until(at.saturating_add(wait));
                }
                FailureKind::RateLimited {
                    retry_after: response.retry_after,
                    detail: format!(
                        "{name} is rate-limiting this deployment (HTTP {status}): {excerpt}"
                    ),
                }
            }
            status if (500..=599).contains(&status) => FailureKind::Transient {
                detail: format!("{name} failed to serve the request (HTTP {status}): {excerpt}"),
            },
            other => FailureKind::Permanent {
                detail: format!(
                    "{name} answered HTTP {other}, which this connector does not know how to \
                     read: {excerpt}"
                ),
            },
        }
    }

    /// The gates, in the order a failing one should stop the next.
    fn ingest(
        &mut self,
        connector: &mut dyn SourceConnector,
        response: &SourceResponse,
        target: &str,
        at: Timestamp,
    ) -> PollReport {
        let mut report = PollReport::empty(PollOutcome::Delivered, self.heartbeat.liveness(at));

        let payload: serde_json::Value = match serde_json::from_str(&response.body) {
            Ok(payload) => payload,
            Err(error) => {
                self.hold(
                    target,
                    QuarantineReason::DecodeFailure {
                        detail: format!(
                            "{} sent a body this connector cannot read: {error}",
                            self.manifest.source_id
                        ),
                    },
                    &response.body_excerpt(),
                    at,
                    &mut report,
                );
                return report;
            }
        };

        if let Err(error) = self
            .guard
            .admit_version(connector.declared_version(&payload))
        {
            self.hold(
                target,
                QuarantineReason::VersionMismatch {
                    detail: error.message().to_string(),
                },
                &response.body_excerpt(),
                at,
                &mut report,
            );
            return report;
        }

        let outcome = self.guard.check(&payload);
        if !outcome.conforms() {
            self.hold(
                target,
                QuarantineReason::SchemaViolation {
                    detail: outcome.describe(),
                },
                &response.body_excerpt(),
                at,
                &mut report,
            );
            return report;
        }

        let events = match connector.decode(&payload, &self.cursor) {
            Ok(events) => events,
            Err(error) => {
                self.hold(
                    target,
                    QuarantineReason::DecodeFailure {
                        detail: error.message().to_string(),
                    },
                    &response.body_excerpt(),
                    at,
                    &mut report,
                );
                return report;
            }
        };
        if events.len() > self.manifest.max_events_per_batch {
            self.hold(
                target,
                QuarantineReason::DecodeFailure {
                    detail: format!(
                        "{} sent {} events and the manifest's cap is {}: a response small enough \
                         to read is not automatically a response worth expanding",
                        self.manifest.source_id,
                        events.len(),
                        self.manifest.max_events_per_batch
                    ),
                },
                &response.body_excerpt(),
                at,
                &mut report,
            );
            return report;
        }

        // The heartbeat is fed from every event the source produced, including
        // the ones withheld and the ones already seen: the source *did* serve
        // them, and a feed judged only on what got past dedup would look stale
        // the moment a source starts re-serving its last page.
        let newest = events.iter().map(|event| event.event_time).max();
        self.heartbeat.answered(at, newest);

        for event in &events {
            self.admit(connector, event, at, &mut report);
        }

        let position = newest.map_or_else(
            || self.cursor.position.clone(),
            |at| CursorPosition::EventTime { at },
        );
        let accepted = report.admitted.len() as u64;
        self.cursor = connector.advance(&self.cursor, position, accepted);
        self.stats.admitted = self.stats.admitted.saturating_add(accepted);
        self.stats.duplicates = self.stats.duplicates.saturating_add(report.duplicates);
        self.stats.withheld = self.stats.withheld.saturating_add(report.withheld);
        report
    }

    /// One event through the per-event gates.
    ///
    /// The order is load-bearing. Knowability is checked *before* the dedup
    /// window records the fingerprint: recording it first would mark an event
    /// as seen while withholding it, and the next poll — the one that was
    /// supposed to deliver it — would drop it as a duplicate. That is a record
    /// lost with every counter reading zero.
    fn admit(
        &mut self,
        connector: &dyn SourceConnector,
        event: &RawEvent,
        at: Timestamp,
        report: &mut PollReport,
    ) {
        let knowable_at = event
            .event_time
            .saturating_add(self.manifest.publication_delay());
        if knowable_at > at {
            report.withheld = report.withheld.saturating_add(1);
            return;
        }

        let fingerprint = event.fingerprint(&self.manifest);
        if matches!(self.dedup.observe(&fingerprint), Novelty::Duplicate) {
            report.duplicates = report.duplicates.saturating_add(1);
            return;
        }

        let record = match connector.map(event, at) {
            Ok(record) => record,
            Err(error) => {
                self.hold(
                    &event.key,
                    QuarantineReason::MappingFailure {
                        detail: error.message().to_string(),
                    },
                    &event.body.to_string(),
                    at,
                    report,
                );
                return;
            }
        };

        let issues = record.validate();
        if !issues.is_empty() {
            self.hold(
                &event.key,
                QuarantineReason::ValidationFailure { issues },
                &event.body.to_string(),
                at,
                report,
            );
            return;
        }

        match MarketEventEnvelope::new(
            &self.manifest,
            event,
            record,
            at,
            connector.quality_of(event),
        ) {
            Ok(envelope) => report.admitted.push(envelope),
            Err(error) => self.hold(
                &event.key,
                QuarantineReason::MappingFailure {
                    detail: error.message().to_string(),
                },
                &event.body.to_string(),
                at,
                report,
            ),
        }
    }

    fn hold(
        &mut self,
        key: &str,
        reason: QuarantineReason,
        excerpt: &str,
        at: Timestamp,
        report: &mut PollReport,
    ) {
        self.quarantine.hold(key, None, reason, excerpt, at);
        report.quarantined = report.quarantined.saturating_add(1);
        self.stats.quarantined = self.stats.quarantined.saturating_add(1);
    }

    /// Lifecycle: **checkpoint**. The cursor, bound to the source and schema
    /// it means something under.
    pub fn checkpoint(&self, at: Timestamp) -> Checkpoint {
        Checkpoint::new(&self.manifest, self.cursor.clone(), at)
    }

    /// Lifecycle: **resume**. Restore a cursor, or refuse it.
    pub fn resume(
        &mut self,
        connector: &mut dyn SourceConnector,
        checkpoint: &Checkpoint,
    ) -> Result<()> {
        self.cursor = connector.resume(checkpoint)?;
        Ok(())
    }

    /// Lifecycle: **shutdown**.
    pub fn shutdown(&mut self, connector: &mut dyn SourceConnector, at: Timestamp) -> Result<()> {
        self.connected = false;
        connector.shutdown(at)
    }
}

/// The quality a connector reports when it has nothing to add.
///
/// Not `DataQuality::default`, which asserts a perfect measurement.
pub(crate) fn measured_quality() -> DataQuality {
    DataQuality::clean()
}
