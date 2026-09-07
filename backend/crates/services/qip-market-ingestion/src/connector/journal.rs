//! What a source actually streamed, recorded where it outlives the process
//! that streamed it.
//!
//! # The measurement this exists to make possible
//!
//! `docs/plan/completion-plan.md` scores one live source *sustained* at seven
//! days of streaming, and scores it at the measured bar — real evidence, not a
//! repository test. Everything else in this module follows from what that
//! sentence demands and what the tree could supply on 2026-09-06:
//!
//! * Seven days is longer than a process. A revision rollout, an eviction, an
//!   out-of-memory kill and a redeploy all land inside the window, so the
//!   quantity being measured spans several processes and no in-memory counter
//!   can hold it. [`ConnectorRuntime::stats`] resets to zero at every start-up
//!   and is a per-process figure wearing a deployment's clothes.
//! * A number the platform asserts about itself and nobody can check afterwards
//!   is what the scorecard exists to catch. So this is a record on a durable
//!   store, with the instants in it, readable by whoever wants to disagree.
//!
//! What is recorded is what an operator would have to reconstruct otherwise and
//! could not: how many processes have carried this stream, how many polls each
//! kind of outcome, the first and last instant a record was *ingested*, the
//! oldest and newest instant a record was *true*, and how many redeliveries the
//! dedup window absorbed. The duplicate ratio is the one derived figure, and it
//! is derived here rather than by a reader because getting its denominator
//! wrong is how a stream that republished everything reads as a healthy one.
//!
//! # Bitemporal, over the span rather than at a point
//!
//! Four instants, not two, and the pairing is the point. `ingested` is the
//! platform's own knowledge axis — when the fact reached here — and `event` is
//! the world's — when it was true. A stream whose event span is a week and
//! whose ingest span is an hour backfilled; one whose event span is an hour and
//! whose ingest span is a week stalled and kept polling. Both read identically
//! on a record count alone, and both have happened to somebody.
//!
//! # What is bounded, and what that costs
//!
//! One key per source, holding one fixed-shape record: nine counters, four
//! optional instants and two sizes. Nothing here grows with the number of
//! records, the number of polls, or the number of days — a ledger after seven
//! days is byte-for-byte the same size as a ledger after one poll. That is a
//! deliberate refusal of the more useful thing: there is no per-day series
//! here, so this ledger cannot answer "was the stream healthy on Wednesday",
//! only "over the whole span, here is what happened". A per-day series is
//! bounded only by the length of the run, and an unbounded history is the thing
//! the data domain refuses first. The event log is where a per-day answer
//! belongs.
//!
//! # What this is not
//!
//! It is not evidence that anything streamed. It is the shape a week of
//! evidence would be recorded in, and it is written by whatever drives the poll
//! loop — which, on 2026-09-06, is no deployed process, because nothing is
//! deployed with an outbound path. A ledger with `sessions: 1` and a span of
//! minutes is exactly what an in-session proof produces and must not be quoted
//! as anything else.

use super::checkpoint::Checkpoint;
use super::envelope::MarketEventEnvelope;
use super::runtime::{PollOutcome, PollReport};
use qip_core::error::{Error, Result};
use qip_core::kv::{KeyValueStore, KeyValueStoreExt};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The two instants a span is measured between, when there is one.
///
/// A named pair rather than two `Option<Timestamp>` fields per axis, because
/// the invariant that matters — that the two are either both present or both
/// absent, and that the first never follows the last — is one a type can hold
/// and four loose fields cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstantSpan {
    pub first: Timestamp,
    pub last: Timestamp,
}

impl InstantSpan {
    /// A span of one instant.
    pub const fn at(instant: Timestamp) -> Self {
        Self {
            first: instant,
            last: instant,
        }
    }

    /// The span widened to include `instant`.
    ///
    /// Widens at both ends. A source that backfills delivers an event older
    /// than everything before it, and a span that only ever moved its `last`
    /// would report the backfill as having never happened.
    pub fn including(self, instant: Timestamp) -> Self {
        Self {
            first: if instant < self.first {
                instant
            } else {
                self.first
            },
            last: if instant > self.last {
                instant
            } else {
                self.last
            },
        }
    }

    pub fn extent(&self) -> Duration {
        self.last.since(self.first)
    }

    pub fn describe(&self) -> String {
        format!(
            "{} .. {} ({:.2} day(s))",
            self.first.to_rfc3339(),
            self.last.to_rfc3339(),
            self.extent().as_days_f64()
        )
    }
}

/// Everything one source's stream has done, across every process that carried
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLedger {
    pub source_id: String,
    /// Processes that have opened this stream. One per start-up, and the figure
    /// that turns "seven days of streaming" into a claim about a deployment
    /// rather than about an uptime nobody measured: seven days across two
    /// hundred sessions is a crash loop, and a record with only a span in it
    /// would call that a success.
    pub sessions: u64,
    pub polls: u64,
    /// Polls the source answered.
    pub delivered: u64,
    /// Polls the rate limiter held back. No request was made and nothing lost.
    pub deferred: u64,
    /// Polls in which every permitted attempt failed.
    pub refused: u64,
    /// Records that passed every gate.
    pub admitted: u64,
    /// Redeliveries the dedup window recognised. The number that says whether
    /// the window is doing anything.
    pub duplicates: u64,
    /// Records real but not yet knowable at the poll's horizon. Not a loss.
    pub withheld: u64,
    pub quarantined: u64,
    /// When records reached this platform: the knowledge axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingested: Option<InstantSpan>,
    /// When the facts in them were true: the world's axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<InstantSpan>,
    /// Fingerprints the last checkpoint carried forward, so a reader can tell a
    /// restart that resumed its dedup window from one that started blind.
    pub carried_fingerprints: usize,
}

impl StreamLedger {
    fn new(source_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            ..Self::default()
        }
    }

    /// Take one poll's report into the record.
    fn absorb(&mut self, report: &PollReport, at: Timestamp) {
        self.polls = self.polls.saturating_add(1);
        match report.outcome {
            PollOutcome::Delivered => self.delivered = self.delivered.saturating_add(1),
            PollOutcome::Deferred { .. } => self.deferred = self.deferred.saturating_add(1),
            PollOutcome::Refused => self.refused = self.refused.saturating_add(1),
        }
        self.duplicates = self.duplicates.saturating_add(report.duplicates);
        self.withheld = self.withheld.saturating_add(report.withheld);
        self.quarantined = self.quarantined.saturating_add(report.quarantined);
        for envelope in &report.admitted {
            self.admit(envelope, at);
        }
    }

    /// The two axes moved by one admitted record.
    ///
    /// `at` rather than the envelope's own `ingest_time` for the knowledge
    /// axis: they are the same instant by construction — the runtime stamps
    /// the caller's horizon — and taking the caller's keeps this record
    /// honest if a future connector ever stamps something else. The event axis
    /// can only come from the envelope, because it is the source's claim and
    /// not this platform's.
    fn admit(&mut self, envelope: &MarketEventEnvelope, at: Timestamp) {
        self.admitted = self.admitted.saturating_add(1);
        self.ingested = Some(match self.ingested {
            None => InstantSpan::at(at),
            Some(span) => span.including(at),
        });
        let event_time = envelope.event_time();
        self.event = Some(match self.event {
            None => InstantSpan::at(event_time),
            Some(span) => span.including(event_time),
        });
    }

    /// Records the dedup window judged: everything that reached
    /// [`super::dedup::DedupWindow::observe`] and was found new or repeated.
    ///
    /// The denominator of [`Self::duplicate_ratio`], given a name of its own so
    /// that the ratio has one definition rather than one in arithmetic and
    /// another in prose. A quarantined record was fingerprinted before it was
    /// mapped, so it belongs here; a withheld one never was, so it does not.
    pub fn fingerprinted(&self) -> u64 {
        self.admitted
            .saturating_add(self.duplicates)
            .saturating_add(self.quarantined)
    }

    /// Redeliveries as a share of the records the dedup window judged, which is
    /// **not** a share of everything the source delivered.
    ///
    /// This doc said "everything the source delivered" while the arithmetic
    /// divided by `admitted + duplicates`, and its own warning applies to it:
    /// getting the denominator wrong is how a stream that republished
    /// everything reads as a healthy one. The denominator is now
    /// [`Self::fingerprinted`], and the two records it deliberately treats
    /// differently are the reason a sentence had to be picked and kept:
    ///
    /// * A **withheld** record is real but not yet knowable at the poll's
    ///   horizon, and `ConnectorRuntime::admit` returns before it is ever
    ///   fingerprinted. It cannot be a duplicate and cannot be found one, so
    ///   counting it below the line would depress the ratio in proportion to
    ///   the manifest's publication delay — a number about dissemination,
    ///   reported as a number about republication.
    /// * A **quarantined** record *was* fingerprinted — the window observes
    ///   before the connector maps or validates — so it is counted. Leaving it
    ///   out understated the denominator by exactly the records a broken feed
    ///   produces most of, which is the wrong direction to be wrong in.
    ///
    /// The two are therefore not a partition of `delivered`:
    /// `admitted + duplicates + withheld + quarantined` is what the source
    /// served, and this ratio is over that sum less `withheld`. Whoever quotes
    /// the figure has to say so, which is why [`Self::describe`] prints the
    /// denominator beside it rather than a bare number a reader must guess at.
    ///
    /// `None` when nothing was fingerprinted, rather than zero: a stream that
    /// has seen no records has not got a duplicate ratio of nought, it has not
    /// got one, and reporting the two the same way is how a dead feed reads as
    /// a perfectly deduplicated one.
    ///
    /// `f64` and not `Decimal`: this is a statistic about the feed, not money.
    pub fn duplicate_ratio(&self) -> Option<f64> {
        let fingerprinted = self.fingerprinted();
        if fingerprinted == 0 {
            return None;
        }
        // The crossing point from exact counts to statistics. Both counts are
        // u64 and the ratio is descriptive; nothing sizes a position from it.
        Some(self.duplicates as f64 / fingerprinted as f64)
    }

    /// How long this stream has been ingesting, across every session.
    pub fn ingest_span(&self) -> Option<Duration> {
        self.ingested.map(|span| span.extent())
    }

    /// Whether the stream has ingested across at least `days` days.
    ///
    /// The arithmetic behind the completion plan's seven-day bar, kept here so
    /// that whoever claims the bar and whoever checks the claim are reading the
    /// same subtraction. It answers about the *span*, which is necessary and
    /// not sufficient: a span of seven days with two polls in it is not seven
    /// days of streaming, and the caller has `polls`, `sessions` and
    /// `duplicate_ratio` beside this to say so.
    pub fn spans_at_least_days(&self, days: i64) -> bool {
        self.ingest_span()
            .is_some_and(|span| span.as_nanos() >= Duration::from_days(days).as_nanos())
    }

    /// One block an operator can read, and check against the vendor.
    ///
    /// Deliberately verbose and deliberately not a metric: a Prometheus gauge
    /// is a number at a scrape and this is the whole run, including the two
    /// spans that make a stalled stream distinguishable from a backfilling one.
    pub fn describe(&self) -> String {
        let ingested = self
            .ingested
            .map_or_else(|| "none".to_string(), |span| span.describe());
        let event = self
            .event
            .map_or_else(|| "none".to_string(), |span| span.describe());
        // The denominator is printed with the figure, never the figure alone.
        // A bare ratio is one a reader completes from the counters beside it,
        // and the obvious completion — everything the source delivered — is
        // the wrong one: withheld records are not fingerprinted and are not
        // below the line. See `duplicate_ratio`.
        let ratio = self.duplicate_ratio().map_or_else(
            || "no records fingerprinted".to_string(),
            |ratio| {
                format!(
                    "{:.4} of {} fingerprinted (withheld excluded)",
                    ratio,
                    self.fingerprinted()
                )
            },
        );
        format!(
            "stream `{}`: {} session(s), {} poll(s) ({} delivered, {} deferred, {} refused); \
             {} admitted, {} duplicate, {} withheld, {} quarantined; duplicate ratio {}; \
             ingested {}; event {}; {} fingerprint(s) carried across the last restart",
            self.source_id,
            self.sessions,
            self.polls,
            self.delivered,
            self.deferred,
            self.refused,
            self.admitted,
            self.duplicates,
            self.withheld,
            self.quarantined,
            ratio,
            ingested,
            event,
            self.carried_fingerprints,
        )
    }
}

/// A stream's durable record and its resume position, on a key-value store.
///
/// Through the [`KeyValueStore`] port rather than the filesystem, for the
/// reason every other durable thing here goes through it: a test runs it in
/// memory and a deployment chooses the adapter, and neither has a branch in it
/// deciding which.
#[derive(Debug)]
pub struct StreamJournal {
    store: Arc<dyn KeyValueStore>,
    source_id: String,
    ledger: StreamLedger,
}

impl StreamJournal {
    /// The key prefix every entry lives under.
    pub const NAMESPACE: &'static str = "ingestion-stream";

    fn ledger_key(source_id: &str) -> String {
        format!("{}/{source_id}/ledger", Self::NAMESPACE)
    }

    fn checkpoint_key(source_id: &str) -> String {
        format!("{}/{source_id}/checkpoint", Self::NAMESPACE)
    }

    /// Open the journal for one source, counting this process as a session.
    ///
    /// Returns the checkpoint the last session left, for the caller to hand to
    /// [`super::runtime::ConnectorRuntime::resume`] before its first poll.
    /// Returning it rather than applying it is not fastidiousness: a resume
    /// needs the connector, the connector is the caller's, and a journal that
    /// reached for one would be a second place that decides what a source is.
    ///
    /// The session count is written **at open**, before any poll. A process
    /// that starts, restores a window and then dies has still been a session,
    /// and a count written at shutdown would miss exactly the sessions an
    /// operator most needs to see.
    pub fn open(
        store: Arc<dyn KeyValueStore>,
        source_id: &str,
    ) -> Result<(Self, Option<Checkpoint>)> {
        if source_id.is_empty() {
            return Err(Error::invalid(
                "a stream journal needs the source's id; an empty one would put every source's \
                 record under one key and make the ledger the sum of feeds nobody could separate",
            ));
        }
        let mut ledger: StreamLedger = store
            .get_as(&Self::ledger_key(source_id))?
            .unwrap_or_else(|| StreamLedger::new(source_id));
        if ledger.source_id != source_id {
            return Err(Error::invalid(format!(
                "the ledger under `{}` records source `{}` and this journal was opened for `{}`. \
                 Two streams sharing a key would sum into one record neither could be read out \
                 of; the store namespace is wrong, not the ledger",
                Self::ledger_key(source_id),
                ledger.source_id,
                source_id
            )));
        }
        ledger.sessions = ledger.sessions.saturating_add(1);
        let checkpoint: Option<Checkpoint> = store.get_as(&Self::checkpoint_key(source_id))?;
        let journal = Self {
            store,
            source_id: source_id.to_string(),
            ledger,
        };
        journal.write_ledger()?;
        Ok((journal, checkpoint))
    }

    pub const fn ledger(&self) -> &StreamLedger {
        &self.ledger
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Record one poll and persist the ledger.
    ///
    /// Persisted on every poll rather than on a timer or at shutdown. The
    /// process this is measuring is one whose failure modes include being
    /// killed without notice, and a ledger flushed at shutdown records nothing
    /// about the runs worth recording.
    pub fn record(&mut self, report: &PollReport, at: Timestamp) -> Result<()> {
        self.ledger.absorb(report, at);
        self.write_ledger()
    }

    /// Persist the resume position, and note how much of the window it carries.
    pub fn commit(&mut self, checkpoint: &Checkpoint) -> Result<()> {
        if checkpoint.source_id != self.source_id {
            return Err(Error::invalid(format!(
                "this journal is for `{}` and the checkpoint belongs to `{}`. Storing it would \
                 give one source another's resume position: a gap on one side and a replay on \
                 the other, both silent",
                self.source_id, checkpoint.source_id
            )));
        }
        // Refuse a carry past the bound here too, so nothing unbounded is
        // written rather than only refused on the way back in. A store that
        // already holds an oversized carry is a restart that has to be repaired
        // by hand.
        let carried = checkpoint.carried()?;
        self.store
            .put_as(&Self::checkpoint_key(&self.source_id), checkpoint)?;
        self.ledger.carried_fingerprints = carried.len();
        self.write_ledger()
    }

    fn write_ledger(&self) -> Result<()> {
        self.store
            .put_as(&Self::ledger_key(&self.source_id), &self.ledger)
    }
}
