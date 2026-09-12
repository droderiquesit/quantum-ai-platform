//! Where a data reference gets its consequence.
//!
//! `qip-data-finder` can build a [`DataReference`] and keep it in a bounded
//! [`ReferenceLedger`], and the ledger can say that a source revised an
//! extent this platform already used. What that crate cannot do — it has no
//! event log and no metrics registry — is make the finding *cost* anything.
//! §22.3's table says what it must cost: "the backtest that used the original
//! is flagged, not silently invalidated." A revision the ledger detected and
//! nothing acted on would be exactly the control that cannot fire
//! `.claude/rules/10-product-direction.md` names as a defect.
//!
//! So this is the seam that sees both halves. A composition root hands the
//! kernel the [`FetchDigest`] its connector produced; the kernel looks the
//! source up among the [`AdmittedSource`]s the root admitted through the
//! licensing gate, refuses a digest from a source it holds no admission for,
//! builds the reference through the catalogue door, and records it. Every
//! reference is written to the hash-chained log as a [`DataReferenceRecorded`]
//! record *before* the in-memory ledger moves, so a restart rebuilds the
//! ledger from the log ([`Platform::resume_references`]) and the ledger is
//! never a fact the process knows and the record does not. A revision then
//! does four things, in this order and all four every time: it is written to
//! the log as a [`SourceRevisionDetected`] record, so it is reproducible from
//! the log alone; every campaign already closed on the log whose manifest
//! read the withdrawn bytes is named in a [`ResearchCampaignFlagged`] record
//! — the backtest that used the original, found by joining the revision
//! against the closed manifests rather than against whatever campaign happens
//! to be open; it moves `qip_data_revisions_detected_total`, so it can page
//! someone; and it stays in the ledger's revision queue, where
//! [`Platform::revision_covering`] answers the question a research run asks
//! before it trains on a subject and period.
//!
//! # Four topics, one fact each
//!
//! Each record here has its own [`Topic`]. `ResearchCampaignClosed` sat under
//! `LearningCompleted` until 2026-09-12, and `Platform::journal_entries`
//! decodes every frame on that topic as a cycle entry, so the first campaign
//! to close broke every later read of the journal. `SourceRevisionDetected`
//! sat under `DataQualityFailed`, which the ingestion suites already fill
//! with another body and which the log does not retain permanently, so
//! "a replay re-derives every flag" was a claim the retention policy could
//! falsify. Each body also carries an idempotency key, and
//! [`Platform::journal_once`] consults the log for it before appending, so a
//! fact journaled twice is one record.
//!
//! # Why the source has to be admitted *here* as well as at the feed
//!
//! The feed's `StandingAdmission` already refuses to poll a source the
//! catalogue has not admitted, so a digest can only ever come from an
//! admitted source. The kernel still refuses one it was not told about,
//! because the two admissions are one fact recorded twice by two different
//! parties — the feed's, at the socket, and the platform's, at the ledger —
//! and the ledger is what an audit reads. A reference for a source the
//! platform cannot name the licence of would be a reference with no
//! provenance, however the bytes arrived. The refusal names the missing step
//! rather than building a reference of unknown standing.

use crate::platform::Platform;
use qip_core::Decimal;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_data_finder::admission::AdmittedSource;
use qip_data_finder::ledger::{LedgerOutcome, ReferenceLedger, RevisionRecord};
use qip_data_finder::reference::{DataPeriod, DataReference};
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_market_ingestion::connector::FetchDigest;
use qip_observability::metrics::{labels, names};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The event-log record of the kernel referencing a fetch — every fetch,
/// whether the ledger found it first, unchanged or revised.
///
/// The ledger's own durable form. The in-memory [`ReferenceLedger`] is
/// process-lifetime; this record is what [`Platform::resume_references`]
/// rebuilds it from, in log order, so a restart does not begin with a ledger
/// that has never seen an extent and therefore cannot detect that the source
/// revised it. Filed under its own Sense-group topic, evictable like the
/// observations it describes and bounded by the log's capacity; the
/// revision a reference may reveal is the permanent record, below.
///
/// Idempotent on the extent and the hash: a re-fetch that hashes the same is
/// the same fact and is not written twice. After a restart, an extent that
/// was re-fetched unchanged is therefore held at the instant it was *first*
/// referenced rather than last, which is the earlier and so the more
/// conservative `used_at` for a later revision to flag against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataReferenceRecorded {
    pub reference: DataReference,
}

impl EventBody for DataReferenceRecorded {
    const TOPIC: Topic = Topic::DataReferenceRecorded;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        let range = self.reference.range();
        Some(format!(
            "reference:{}:{}:{}:{}:{}",
            self.reference.source_id(),
            self.reference.locator(),
            range.start().as_nanos(),
            range.end().as_nanos(),
            self.reference.content_hash()
        ))
    }
}

/// The event-log record of a source revising an extent after use.
///
/// Filed under [`Topic::SourceRevisionDetected`], in the LEARN group the log
/// retains permanently: a revision is what flags a backtest, and a flag the
/// log may evict is a replay that re-derives nothing. The record carries the
/// whole [`RevisionRecord`] — both hashes, the period, the subjects, when the
/// extent was used and when the revision was caught — so a replay can
/// re-derive every flag [`Platform::revision_covering`] would have raised,
/// and [`Platform::resume_references`] restores the revision queue from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRevisionDetected {
    pub revision: RevisionRecord,
}

impl EventBody for SourceRevisionDetected {
    const TOPIC: Topic = Topic::SourceRevisionDetected;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        let period = self.revision.period();
        Some(format!(
            "revision:{}:{}:{}:{}:{}:{}",
            self.revision.source_id(),
            self.revision.locator(),
            period.start().as_nanos(),
            period.end().as_nanos(),
            self.revision.was(),
            self.revision.now()
        ))
    }
}

/// What recording a reference on the platform produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedReference {
    pub reference: DataReference,
    pub outcome: LedgerOutcome,
}

/// The cost a reference from a shipped connector records.
///
/// Every source this build can open is a free tier — the public Coinbase
/// and Frankfurter endpoints, Alpaca's IEX feed on a paper account, Kalshi's
/// open market list — and no manifest declares a price, so the estimate is
/// zero and says so rather than inventing a tariff. A manifest field for a
/// metered source is where a non-zero figure would come from.
const CONNECTOR_FETCH_COST: Decimal = Decimal::ZERO;

/// The availability a delivered fetch records.
///
/// A digest exists only for a poll the source answered, so the source was
/// available for *this* fetch. Availability over a window is
/// `qip_data_finder::health::SourceHealth`'s measurement, and the ledger
/// does not pretend to have taken it.
const DELIVERED_FETCH_AVAILABILITY: f64 = 1.0;

/// The event-log record of a research campaign closing: the manifest that
/// outlives the cache, and what the campaign found while it was open.
///
/// §22.4's arrow ends "cache expires and is deleted. What persists: the
/// manifest and the results", and its table's mitigation for a regulatory
/// demand is that "the manifest proves what was used at the time". A manifest
/// returned from `FetchCampaign::close` and dropped by its caller would prove
/// nothing to anyone; this record is where it goes, and it is the only place
/// it goes — a second copy in a key-value store was two claims about one
/// fact, and the log is the record. Filed under
/// [`Topic::ResearchCampaignClosed`], in the LEARN group the event log
/// retains permanently, because a manifest an audit may demand in three
/// years is not one the log may evict to make room.
///
/// Whether the campaign was flagged and whether it drew on the fallback
/// series are *read off the manifest* ([`Self::flagged`],
/// [`Self::fallback_used`]) rather than carried beside it: two fields that
/// restated the manifest were two more claims about the same fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchCampaignClosed {
    pub campaign_id: String,
    /// The instrument the campaign assembled a research window for.
    pub subject: String,
    pub opened_at: Timestamp,
    pub closed_at: Timestamp,
    /// Every extent the campaign fetched, the statistics it sketched with
    /// their declared bounds, which entries a revision flagged, and which
    /// subjects came from the fallback series.
    pub manifest: qip_data_finder::campaign::CampaignManifest,
    /// Whether the subject's data class had enough independent backing to
    /// be promoted past validation (§22.3, rule 31).
    pub concentration: qip_data_finder::campaign::ConcentrationVerdict,
}

impl ResearchCampaignClosed {
    /// Entries a revision flagged — on the ledger before the campaign opened
    /// or during it. Zero is "clean", not "unchecked": every entry was
    /// checked against the ledger.
    pub fn flagged(&self) -> usize {
        self.manifest.flagged().count()
    }

    /// Whether the window came from §22.1's fallback series because the
    /// subject's own stream no longer held enough history.
    pub fn fallback_used(&self) -> bool {
        self.manifest.fallbacks().contains(&self.subject)
    }

    /// Whether this campaign read the bytes `revision` says the source has
    /// since withdrawn — [`RevisionRecord::contradicts`] over every entry of
    /// the manifest.
    pub fn used_revised_extent(&self, revision: &RevisionRecord) -> bool {
        self.manifest
            .entries()
            .iter()
            .any(|entry| revision.contradicts(entry.reference()))
    }
}

impl EventBody for ResearchCampaignClosed {
    const TOPIC: Topic = Topic::ResearchCampaignClosed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("campaign:{}", self.campaign_id))
    }
}

/// The event-log record that a campaign already closed on the log used an
/// extent the source has since revised — §22.3's "the backtest that used
/// the original is flagged", as a record naming the backtest.
///
/// Written by [`Platform::record_reference`] when a revision is detected,
/// for every [`ResearchCampaignClosed`] record on the log whose manifest the
/// revision contradicts. The campaign that is *open* when the revision is
/// detected — the one that fetched the corrected bytes — is not the one
/// flagged, and until 2026-09-12 it was the only one that was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchCampaignFlagged {
    pub campaign_id: String,
    pub subject: String,
    pub revision: RevisionRecord,
}

impl EventBody for ResearchCampaignFlagged {
    const TOPIC: Topic = Topic::ResearchCampaignFlagged;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "campaign-flag:{}:{}:{}",
            self.campaign_id,
            self.revision.was(),
            self.revision.now()
        ))
    }
}

impl Platform {
    /// Tell the platform that a composition root's licensing gate admitted
    /// `source`, so digests from it can be referenced.
    ///
    /// Returns the admission this one replaces, if the source was already
    /// admitted — a re-admission after an operator approval is the same
    /// source under a fresh decision, and the ledger's references to it are
    /// still its references.
    pub fn admit_source(&mut self, source: AdmittedSource) -> Option<AdmittedSource> {
        self.admitted_sources
            .insert(source.source_id().to_string(), source)
    }

    /// The admission the platform holds for `source_id`, if any.
    pub fn admitted_source(&self, source_id: &str) -> Option<&AdmittedSource> {
        self.admitted_sources.get(source_id)
    }

    /// Reference what a connector poll fetched, and act on a revision.
    ///
    /// Refuses a digest from a source the platform holds no admission for —
    /// see the module doc for why this is asked again here — and refuses,
    /// through `DataReference::from_digest`, a digest that does not describe
    /// the admitted source it names.
    pub fn reference_fetch(
        &mut self,
        digest: &FetchDigest,
        now: Timestamp,
    ) -> Result<RecordedReference> {
        let admitted = self
            .admitted_sources
            .get(digest.source_id())
            .ok_or_else(|| {
                Error::denied(format!(
                    "`{}` served {} byte(s) from {} but this platform holds no admission for it; \
                     nothing from it is referenced. The composition root admits a source through \
                     the licensing gate with `Platform::admit_source` before its first poll, and \
                     a digest arriving without one is a root that skipped that step",
                    digest.source_id(),
                    digest.bytes(),
                    digest.locator()
                ))
            })?;
        let reference = DataReference::from_digest(
            admitted,
            digest,
            CONNECTOR_FETCH_COST,
            DELIVERED_FETCH_AVAILABILITY,
        )?;
        self.record_reference(reference, now)
    }

    /// Record a reference the caller built — through any of the three doors
    /// — in the bounded ledger, and act on a revision.
    ///
    /// The log is written before the ledger moves, in every case: the
    /// reference itself first, then, on a revision, the revision and the
    /// closed campaigns it contradicts, and only then does the in-memory
    /// ledger take the reference. A ledger that had moved before the log
    /// held the fact was, for that instant, a fact a crash would lose; and
    /// a revision the log does not hold is one a replay would not flag, so
    /// the metric would count something the record cannot substantiate. The
    /// ledger keeps the newer reference in either case.
    pub fn record_reference(
        &mut self,
        reference: DataReference,
        now: Timestamp,
    ) -> Result<RecordedReference> {
        let outcome = self.references.assess(&reference, now);
        self.journal_once(
            DataReferenceRecorded {
                reference: reference.clone(),
            },
            "kernel/references",
            now,
        )?;
        if let LedgerOutcome::Revised(revision) = &outcome {
            self.journal_once(
                SourceRevisionDetected {
                    revision: revision.clone(),
                },
                "kernel/references",
                now,
            )?;
            self.flag_closed_campaigns(revision, now)?;
            self.telemetry.metrics.count(
                names::DATA_REVISIONS_DETECTED,
                labels([("origin", revision.origin().as_str())]),
            );
        }
        let applied = self.references.record(reference.clone(), now);
        self.telemetry.metrics.count(
            names::DATA_REFERENCES_RECORDED,
            labels([("outcome", applied.as_str())]),
        );
        Ok(RecordedReference {
            reference,
            outcome: applied,
        })
    }

    /// Name, on the log, every campaign already closed there whose manifest
    /// read the bytes `revision` says were withdrawn — §22.3's requirement,
    /// found by joining the revision against the closed manifests rather
    /// than against whatever campaign happens to be open. Returns the
    /// campaign ids flagged, in log order.
    fn flag_closed_campaigns(
        &mut self,
        revision: &RevisionRecord,
        now: Timestamp,
    ) -> Result<Vec<String>> {
        let closed: Vec<ResearchCampaignClosed> = self
            .event_log()
            .by_topic(Topic::ResearchCampaignClosed)
            .into_iter()
            .map(|frame| {
                StreamEnvelope::from_frame(frame)?
                    .decode::<ResearchCampaignClosed>()
                    .map(|envelope| envelope.body)
            })
            .collect::<Result<Vec<_>>>()?;
        let contradicted: Vec<(String, String)> = closed
            .into_iter()
            .filter(|closed| closed.used_revised_extent(revision))
            .map(|closed| (closed.campaign_id, closed.subject))
            .collect();
        let mut flagged = Vec::with_capacity(contradicted.len());
        for (campaign_id, subject) in contradicted {
            let written = self.journal_once(
                ResearchCampaignFlagged {
                    campaign_id: campaign_id.clone(),
                    subject,
                    revision: revision.clone(),
                },
                "kernel/references",
                now,
            )?;
            if written {
                self.telemetry.metrics.count(
                    names::RESEARCH_CAMPAIGNS_FLAGGED,
                    labels([("origin", revision.origin().as_str())]),
                );
                flagged.push(campaign_id);
            }
        }
        Ok(flagged)
    }

    /// Append `body` to the log and publish it, unless the log already
    /// holds a record with its idempotency key — in which case nothing is
    /// written and `false` comes back.
    ///
    /// The idempotency key was a fact every envelope carried and nothing
    /// consulted at the log, so two journal calls for one fact wrote two
    /// records each claiming to be the one. A body with no key is always
    /// written; the bodies in this module all carry one.
    pub(crate) fn journal_once<B: EventBody>(
        &mut self,
        body: B,
        origin: &str,
        now: Timestamp,
    ) -> Result<bool> {
        if let Some(key) = body.idempotency_key()
            && self
                .event_log()
                .holds_idempotent(&format!("{}:{key}", B::TOPIC.name()))
        {
            return Ok(false);
        }
        self.journal_record(body, origin, now)?;
        Ok(true)
    }

    /// Rebuild the reference ledger from what the log retains, in log
    /// order: every [`DataReferenceRecorded`] restored as the latest
    /// reference to its extent, every [`SourceRevisionDetected`] restored to
    /// the revision queue. Called from `Platform::new`, before anything is
    /// journaled, so a restarted process resumes knowing what it referenced
    /// and what it found revised rather than beginning with a ledger that
    /// has seen nothing and can therefore detect nothing.
    ///
    /// Bounded by the ledger's own bounds, so a log longer than the ledger
    /// restores the newest extents. Records from a log written by an older
    /// body schema are refused rather than skipped, as the fabric journal's
    /// resume refuses them: a ledger rebuilt from half a log would flag
    /// half of what it should and say nothing about the other half.
    pub(crate) fn resume_references(log: &EventLog) -> Result<ReferenceLedger> {
        let mut ledger = ReferenceLedger::bounded();
        for record in log.records() {
            match record.event.topic {
                Topic::DataReferenceRecorded => {
                    let recorded = StreamEnvelope::from_frame(&record.event)?
                        .decode::<DataReferenceRecorded>()?
                        .body;
                    ledger.restore_reference(recorded.reference);
                }
                Topic::SourceRevisionDetected => {
                    let detected = StreamEnvelope::from_frame(&record.event)?
                        .decode::<SourceRevisionDetected>()?
                        .body;
                    ledger.restore_revision(detected.revision);
                }
                _ => {}
            }
        }
        Ok(ledger)
    }

    /// The ledger itself, for the research campaign and the tests that
    /// drive it. No health surface reads it yet, and this line used to say
    /// one did.
    pub fn reference_ledger(&self) -> &ReferenceLedger {
        &self.references
    }

    /// Whether an extent from `source_id` naming `symbol` and overlapping
    /// `period` has been revised since this platform used it — the flag a
    /// research run reads before training on the pair.
    pub fn revision_covering(
        &self,
        source_id: &str,
        symbol: &str,
        period: &DataPeriod,
    ) -> Option<&RevisionRecord> {
        self.references.revision_covering(source_id, symbol, period)
    }

    /// The distinct sources whose held references name `symbol`.
    pub fn sources_backing(&self, symbol: &str) -> BTreeSet<String> {
        self.references.sources_backing(symbol)
    }

    /// §22.1's fallback series, as the platform holds it.
    pub fn fallback_series(&self) -> &qip_data_finder::retention::FallbackSeries {
        &self.fallback
    }

    /// The daily bars retained for `subject`, oldest first — what a research
    /// campaign assembles from when the subject's own stream has withdrawn
    /// its history. Empty for a subject no daily bar has ever been observed
    /// for, which is every subject on a minute-bar feed.
    pub fn fallback_bars(&self, subject: &str) -> &[qip_market::bar::Bar] {
        self.fallback.bars(subject)
    }

    /// Journal a closed campaign's manifest and count the close.
    ///
    /// The record is appended before the metric moves, for the reason
    /// [`Platform::record_reference`] gives; a campaign the log does not hold
    /// is one whose manifest nobody can later be shown. The outcome label is
    /// read off the manifest, which is the one place the fact lives. A
    /// campaign journaled twice is one record and one count.
    pub fn journal_campaign(
        &mut self,
        closed: ResearchCampaignClosed,
        now: Timestamp,
    ) -> Result<()> {
        let outcome = if closed.flagged() > 0 {
            "flagged"
        } else {
            "clean"
        };
        if self.journal_once(closed, "kernel/campaign", now)? {
            self.telemetry.metrics.count(
                names::RESEARCH_CAMPAIGNS_CLOSED,
                labels([("outcome", outcome)]),
            );
        }
        Ok(())
    }
}
