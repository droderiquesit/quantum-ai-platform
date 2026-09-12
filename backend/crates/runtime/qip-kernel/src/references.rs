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
//! builds the reference through the catalogue door, and records it. A
//! revision then does three things, in this order and all three every time:
//! it is written to the hash-chained event log as a [`SourceRevisionDetected`]
//! record, so it is reproducible from the log alone; it moves
//! `qip_data_revisions_detected_total`, so it can page someone; and it stays
//! in the ledger's revision queue, where [`Platform::revision_covering`]
//! answers the question a research run asks before it trains on a symbol and
//! period — the flag the blueprint's row demands, consulted by
//! `qip-deepbrain`'s research campaign.
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
use qip_events::{EventBody, Topic};
use qip_market_ingestion::connector::FetchDigest;
use qip_observability::metrics::{labels, names};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The event-log record of a source revising an extent after use.
///
/// Filed under [`Topic::DataQualityFailed`]: a source that changes what it
/// serves for a period this platform already reasoned over is a data-quality
/// fact about the SENSE stage, and the existing consumers of that topic (the
/// API's stream and the ingestion suites) already read it as "something
/// arrived that cannot be trusted as it was". The record carries the whole
/// [`RevisionRecord`] — both hashes, the period, the symbols, when the
/// extent was used and when the revision was caught — so a replay can
/// re-derive every flag [`Platform::revision_covering`] would have raised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRevisionDetected {
    pub revision: RevisionRecord,
}

impl EventBody for SourceRevisionDetected {
    const TOPIC: Topic = Topic::DataQualityFailed;
    const SCHEMA_VERSION: u32 = 1;
}

/// What recording a reference on the platform produced.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedReference {
    pub reference: DataReference,
    pub outcome: LedgerOutcome,
}

impl RecordedReference {
    /// One line for a cycle summary.
    pub fn describe(&self) -> String {
        match &self.outcome {
            LedgerOutcome::Revised(revision) => revision.describe(),
            outcome => format!(
                "{} referenced {} covering {} to {}: {}",
                self.reference.source_id(),
                self.reference.locator(),
                self.reference.range().start().to_rfc3339(),
                self.reference.range().end().to_rfc3339(),
                outcome.as_str()
            ),
        }
    }
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

    /// Every source the platform holds an admission for, in id order.
    pub fn admitted_sources(&self) -> impl Iterator<Item = &AdmittedSource> {
        self.admitted_sources.values()
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
    /// A revision is journaled to the event log before the metric moves and
    /// before this returns: a revision the log does not hold is one a replay
    /// would not flag, and the metric would then be counting something the
    /// record cannot substantiate. The ledger keeps the newer reference in
    /// either case.
    pub fn record_reference(
        &mut self,
        reference: DataReference,
        now: Timestamp,
    ) -> Result<RecordedReference> {
        let outcome = self.references.record(reference.clone(), now);
        if let LedgerOutcome::Revised(revision) = &outcome {
            self.journal_record(
                SourceRevisionDetected {
                    revision: revision.clone(),
                },
                "kernel/references",
                now,
            )?;
            self.telemetry.metrics.count(
                names::DATA_REVISIONS_DETECTED,
                labels([("origin", revision.origin().as_str())]),
            );
        }
        self.telemetry.metrics.count(
            names::DATA_REFERENCES_RECORDED,
            labels([("outcome", outcome.as_str())]),
        );
        Ok(RecordedReference { reference, outcome })
    }

    /// The ledger itself, for health surfaces and tests.
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
}

impl Platform {
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
}

/// The event-log record of a research campaign closing: the manifest that
/// outlives the cache, and what the campaign found while it was open.
///
/// §22.4's arrow ends "cache expires and is deleted. What persists: the
/// manifest and the results", and its table's mitigation for a regulatory
/// demand is that "the manifest proves what was used at the time". A manifest
/// returned from `FetchCampaign::close` and dropped by its caller would prove
/// nothing to anyone; this record is where it goes. Filed under
/// [`Topic::LearningCompleted`], the LEARN group the event log retains
/// permanently, because a manifest an audit may demand in three years is not
/// one the log may evict to make room.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchCampaignClosed {
    pub campaign_id: String,
    /// The instrument the campaign assembled a research window for.
    pub subject: String,
    pub opened_at: Timestamp,
    pub closed_at: Timestamp,
    /// Every extent the campaign fetched, the statistics it sketched with
    /// their declared bounds, and which entries a revision flagged.
    pub manifest: qip_data_finder::campaign::CampaignManifest,
    /// Entries a revision flagged — on the ledger before the campaign opened
    /// or during it. Zero is "clean", not "unchecked": every entry was
    /// checked against the ledger.
    pub flagged: usize,
    /// Whether the subject's data class had enough independent backing to
    /// be promoted past validation (§22.3, rule 31).
    pub concentration: qip_data_finder::campaign::ConcentrationVerdict,
    /// Whether the window came from §22.1's fallback series because the
    /// subject's own stream no longer held enough history.
    pub fallback_used: bool,
}

impl EventBody for ResearchCampaignClosed {
    const TOPIC: Topic = Topic::LearningCompleted;
    const SCHEMA_VERSION: u32 = 1;
}

impl Platform {
    /// Journal a closed campaign's manifest and count the close.
    ///
    /// The record is appended before the metric moves, for the reason
    /// [`Platform::record_reference`] gives; a campaign the log does not hold
    /// is one whose manifest nobody can later be shown.
    pub fn journal_campaign(
        &mut self,
        closed: ResearchCampaignClosed,
        now: Timestamp,
    ) -> Result<()> {
        let outcome = if closed.flagged > 0 {
            "flagged"
        } else {
            "clean"
        };
        self.journal_record(closed, "kernel/campaign", now)?;
        self.telemetry.metrics.count(
            names::RESEARCH_CAMPAIGNS_CLOSED,
            labels([("outcome", outcome)]),
        );
        Ok(())
    }
}
