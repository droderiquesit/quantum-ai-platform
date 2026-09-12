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
use qip_data_finder::reference::{DataPeriod, DataReference, SourceOrigin};
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_market_ingestion::connector::FetchDigest;
use qip_observability::metrics::{labels, names};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
///
/// It also carries the *revising* reference — the one that hashed
/// differently — and not only the finding about it. The reference's own
/// [`DataReferenceRecorded`] record sits in the Sense group and is evictable;
/// this one is permanent. A log that had evicted the reference record but
/// kept the revision restored a ledger that knew the extent had been revised
/// and held no latest reference to it, so the *next* revision of the same
/// extent found nothing to compare against and was missed. Schema version
/// two, because the field is required: a version-one record without it is
/// refused at resume rather than restored half-full, the same posture the
/// fabric journal takes for an older body schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRevisionDetected {
    pub revision: RevisionRecord,
    /// The reference whose hash contradicted the ledger's — what the source
    /// now serves — restored as the extent's latest reference on resume.
    pub reference: DataReference,
}

impl EventBody for SourceRevisionDetected {
    const TOPIC: Topic = Topic::SourceRevisionDetected;
    const SCHEMA_VERSION: u32 = 2;

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

/// Whether a closed-campaign frame's manifest names any of `symbols`, read
/// off the raw payload so a campaign that could not be contradicted is not
/// decoded in full. `true` for any payload not shaped as expected, so the
/// full decode — and its refusal — is what handles it.
fn manifest_may_name(
    payload: &serde_json::Value,
    symbols: &std::collections::BTreeSet<String>,
) -> bool {
    let Some(entries) = payload
        .get("manifest")
        .and_then(|manifest| manifest.get("entries"))
        .and_then(serde_json::Value::as_array)
    else {
        return true;
    };
    entries.iter().any(|entry| {
        entry
            .get("reference")
            .and_then(|reference| reference.get("symbols"))
            .and_then(serde_json::Value::as_array)
            .is_none_or(|named| {
                named
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|symbol| symbols.contains(symbol))
            })
    })
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

    /// Withdraw the admission the platform holds for `source_id`, returning
    /// it, so a licence that has stopped granting stops backing anything.
    ///
    /// The composition root's standing gate re-asks the licence question on
    /// every use, and a refusal used to leave the previous `AdmittedSource`
    /// in this table: the round was refused, and the ledger went on counting
    /// the source as a live vendor behind every subject it had ever named.
    /// `None` when no admission was held, which is not an error — a gate
    /// that refused at its first check never admitted anything to withdraw.
    pub fn withdraw_source(&mut self, source_id: &str) -> Option<AdmittedSource> {
        self.admitted_sources.remove(source_id)
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
                    reference: reference.clone(),
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
    ///
    /// # Fails closed on a frame it cannot read
    ///
    /// A closed-campaign frame that does not decode — an older body schema,
    /// a payload edited on disk — is an error from [`Platform::record_reference`],
    /// not a frame skipped. The alternative is a revision recorded as
    /// contradicting nobody because the one campaign that used the original
    /// was the one whose manifest could not be read, and §22.3's "flagged,
    /// not silently invalidated" would then be silently un-flagged.
    ///
    /// # Cost
    ///
    /// Every closed-campaign frame the log retains is read on every revision.
    /// The topic is permanently retained, so the set grows for the life of
    /// the log — one frame per learning round, a few hundred kilobytes each
    /// with its manifest. A revision is rare by construction, so the join is
    /// bounded by the log's capacity rather than by a second index here; a
    /// second index would be a second claim about which campaigns the log
    /// holds. What is bounded cheaply is the decode: the symbols a frame's
    /// manifest entries name are read off the raw payload first, and a
    /// campaign none of whose entries names a symbol the revision covers is
    /// not decoded further, because `contradicts` could never hold of it. A
    /// frame whose payload does not have that shape is decoded in full and
    /// refused there, not skipped here.
    fn flag_closed_campaigns(
        &mut self,
        revision: &RevisionRecord,
        now: Timestamp,
    ) -> Result<Vec<String>> {
        let closed: Vec<ResearchCampaignClosed> = self
            .event_log()
            .by_topic(Topic::ResearchCampaignClosed)
            .into_iter()
            .filter(|frame| manifest_may_name(&frame.payload, revision.symbols()))
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
    ///
    /// # The chain is checked before a frame is believed
    ///
    /// `EventLog::open` parses the file and refuses a reused event id; it
    /// does not recompute a single hash. Until 2026-09-12 this function
    /// restored every reference and revision frame verbatim from a log it
    /// had never verified, so a frame edited on disk — a `catalogue_admitted`
    /// origin written over a `generated` one, a hash swapped — became the
    /// ledger's latest reference to its extent with the chain still
    /// reporting whatever it reported. Now, where the log holds any frame
    /// this function would restore, the chain over the retained span is
    /// verified first ([`EventLog::verify_retained_chain`]) and a broken
    /// link refuses the resume by sequence, the posture the fabric journal's
    /// resume takes. A log holding no such frame is not checked here: there
    /// is nothing to restore from it, and the check belongs to whoever reads
    /// it.
    pub(crate) fn resume_references(log: &EventLog) -> Result<ReferenceLedger> {
        let mut ledger = ReferenceLedger::bounded();
        let restorable = log.records().iter().any(|record| {
            matches!(
                record.event.topic,
                Topic::DataReferenceRecorded | Topic::SourceRevisionDetected
            )
        });
        if !restorable {
            return Ok(ledger);
        }
        if let Err(sequence) = log.verify_retained_chain() {
            return Err(Error::invalid(format!(
                "the event log holds data reference records but its hash chain breaks at \
                 sequence {sequence}, so the reference ledger cannot be rebuilt from it; a \
                 ledger restored from frames nobody can verify would attribute a licence to \
                 bytes nobody fetched. Archive the log and start a new one, or restore the \
                 file the chain was written over"
            )));
        }
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
                    // The revising reference too: its own Sense-group record
                    // may have been evicted, and without it the next revision
                    // of the same extent has nothing to be compared against.
                    ledger.restore_reference(detected.reference);
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

    /// The distinct sources whose held references name `symbol`, each with
    /// the door it came through — what `assess_concentration` counts, and
    /// it counts only the vendor doors.
    ///
    /// A catalogue-admitted reference counts only while this process holds
    /// a live [`AdmittedSource`] for it. The ledger is rebuilt from the log
    /// on every restart, so it holds references from sources the previous
    /// process admitted and this one has not — a connector the deployment no
    /// longer configures, a licence the gate has since refused — and a
    /// reference restored from the log is a fact about what was fetched
    /// then, not a vendor standing behind the subject now. Such a reference
    /// is left out rather than re-labelled: the origin is what happened, and
    /// the count is what is true of this process.
    pub fn sources_backing(&self, symbol: &str) -> BTreeMap<String, SourceOrigin> {
        self.references
            .sources_backing(symbol)
            .into_iter()
            .filter(|(source_id, origin)| {
                *origin != SourceOrigin::CatalogueAdmitted
                    || self.admitted_sources.contains_key(source_id)
            })
            .collect()
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
    ///
    /// Returns whether a record was written: `false` means the log already
    /// held a campaign with this id. That is the caller's fact to act on,
    /// not this method's to swallow — a caller that has just minted the id
    /// and is told the log already holds it has minted an id that collides
    /// with a previous process's, and its manifest is not on the log. Until
    /// 2026-09-12 this returned `()`, every restarted deep brain restarted
    /// its cycle count at one, and every manifest after the first restart
    /// was suppressed as a duplicate of the previous run's while the round
    /// line said "manifest journaled".
    pub fn journal_campaign(
        &mut self,
        closed: ResearchCampaignClosed,
        now: Timestamp,
    ) -> Result<bool> {
        let outcome = if closed.flagged() > 0 {
            "flagged"
        } else {
            "clean"
        };
        let written = self.journal_once(closed, "kernel/campaign", now)?;
        if written {
            self.telemetry.metrics.count(
                names::RESEARCH_CAMPAIGNS_CLOSED,
                labels([("outcome", outcome)]),
            );
        }
        Ok(written)
    }
}

// In-crate, because `resume_references` is `pub(crate)` and the property
// under test is what it restores from a log that has evicted part of what
// it once held — a shape an integration test cannot build, since the
// platform's log capacity is not a configuration knob.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use qip_core::Duration;
    use qip_data_finder::schema::{FieldType, SourceSchema};
    use qip_financial::quality::LicensingClass;
    use qip_financial::universe::Universe;
    use qip_market_ingestion::adapter::SourceDescriptor;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) = qip_core::Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    fn generated(bytes: &[u8], at: Timestamp) -> Result<DataReference> {
        DataReference::of_generated(
            &SourceDescriptor {
                name: "synthetic-exchange".to_string(),
                provider: "this process".to_string(),
                licensing: LicensingClass::Synthetic,
                topics: vec![Topic::MarketBar],
                expected_latency: Duration::ZERO,
                production_requirement: None,
            },
            "bars://synthetic-exchange/AAA?interval=1m",
            ["AAA".to_string()],
            DataPeriod::instant(start()),
            SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
            bytes,
            at,
        )
    }

    /// A revision record restores the extent's latest reference as well as
    /// the revision, so a log that has evicted the reference's own
    /// Sense-group record still rebuilds a ledger that can catch the *next*
    /// revision of that extent. The eviction is simulated by re-chaining
    /// every frame but the `DataReferenceRecorded` ones into a fresh log —
    /// the platform's capacity is not a knob, and this is exactly what the
    /// log's own eviction leaves behind.
    ///
    /// Mutated by deleting `ledger.restore_reference(detected.reference)`
    /// in `resume_references` — confirmed the restored ledger then holds no
    /// reference to the extent and this fails, then restored.
    #[test]
    fn a_revision_restores_the_revising_reference_after_its_own_record_was_evicted() -> Result<()> {
        let mut platform = platform()?;
        let later = start().saturating_add(Duration::from_hours(1));
        platform.record_reference(generated(b"version one", start())?, start())?;
        let revised = platform.record_reference(generated(b"version two", later)?, later)?;
        assert!(
            revised.outcome.is_revised(),
            "premise: the extent was revised"
        );

        let mut evicted = EventLog::in_memory();
        let mut dropped = 0usize;
        for record in platform.event_log().records() {
            if record.event.topic == Topic::DataReferenceRecorded {
                dropped += 1;
                continue;
            }
            evicted.append(&record.event)?;
        }
        assert_eq!(dropped, 2, "premise: both reference records were evicted");
        assert_eq!(
            evicted.by_topic(Topic::SourceRevisionDetected).len(),
            1,
            "premise: the permanent revision record survived"
        );

        let ledger = Platform::resume_references(&evicted)?;
        assert_eq!(ledger.revisions().count(), 1);
        let held = ledger
            .get(
                "synthetic-exchange",
                "bars://synthetic-exchange/AAA?interval=1m",
                DataPeriod::instant(start()),
            )
            .ok_or_else(|| Error::not_found("the revising reference was not restored"))?;
        assert_eq!(
            held.content_hash(),
            revised.reference.content_hash(),
            "the ledger must hold what the source now serves, or the next revision of this \
             extent is compared against nothing"
        );
        Ok(())
    }
}
