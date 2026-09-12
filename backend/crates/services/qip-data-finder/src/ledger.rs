//! The bounded ledger of what was fetched, and the revisions it caught.
//!
//! A [`DataReference`] on its own can say whether one re-fetch still hashes
//! the same; it cannot say that a poll last Tuesday used an extent the vendor
//! has since rewritten, because nothing held Tuesday's reference. This is
//! what holds it: every reference the platform builds, keyed on the extent
//! it describes — source, locator and period — so that the next reference to
//! the same extent is compared hash to hash against the last one. The bytes
//! are never kept; §22.1 files them as transient and §22.3 asks only for the
//! hash, and comparing two hashes is all a revision check ever was.
//!
//! # Bounded on both axes, by count
//!
//! [`ReferenceLedger::new`] takes a ceiling on references and a ceiling on
//! revision records, refuses zero for either, and evicts the oldest entry
//! rather than growing past the ceiling — `.claude/rules/domains/data-and-
//! streaming.md`'s rule, applied to the one structure in this crate that a
//! connector feeds on every poll. A ticker polled every two seconds for a
//! week would otherwise be three hundred thousand references. The defaults
//! ([`REFERENCE_LEDGER_BOUND`], [`REVISION_LEDGER_BOUND`]) are what a
//! composition root gets when it states nothing; the count is the bound
//! because count is what memory is a function of, and a time-based bound
//! would let a fast source hold more than a slow one.
//!
//! Eviction is by insertion order and not by last use: a reference the
//! platform keeps re-fetching stays at its original position and ages out
//! when the ledger fills, which is deliberate — the ledger records *what was
//! used when*, and refreshing an entry's age on every re-fetch would keep an
//! extent alive exactly as long as the source kept serving it, which is the
//! one case in which nothing needs remembering.
//!
//! # What a revision does here, and what it does one seam up
//!
//! Here a revision is a [`RevisionRecord`] in a second bounded queue, so a
//! consumer can ask whether a symbol and period it is about to train on has
//! been revised since it was used ([`ReferenceLedger::revision_covering`]).
//! The consequences that need an event log and a metrics registry — this
//! crate has neither — are the kernel's, in `qip_kernel::references`, which
//! owns one of these and acts on what [`ReferenceLedger::record`] returns.

use crate::reference::{DataPeriod, DataReference, RevisionCheck, SourceOrigin};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// References held by default: the newest 4,096 extents. At a few hundred
/// bytes each that is a low single-digit megabyte, and it is what a ticker
/// polled every two seconds holds for a little over two hours or a daily
/// table holds for eleven years.
pub const REFERENCE_LEDGER_BOUND: usize = 4_096;

/// Revision records held by default. Far fewer than references, because a
/// revision is rare by construction and a source producing a thousand of
/// them is a source somebody should already have quarantined.
pub const REVISION_LEDGER_BOUND: usize = 1_024;

/// The extent a reference describes: which source, which address, which
/// span of the world. Two references with the same key are two fetches of
/// one thing, and comparing their hashes is the revision check.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExtentKey {
    source_id: String,
    locator: String,
    period: DataPeriod,
}

impl ExtentKey {
    pub fn of(reference: &DataReference) -> Self {
        Self {
            source_id: reference.source_id().to_string(),
            locator: reference.locator().to_string(),
            period: reference.range(),
        }
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn period(&self) -> DataPeriod {
        self.period
    }
}

/// A source found to have revised an extent this platform had already used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevisionRecord {
    source_id: String,
    origin: SourceOrigin,
    locator: String,
    period: DataPeriod,
    symbols: BTreeSet<String>,
    /// The hash the platform used.
    was: String,
    /// The hash the source now serves.
    now: String,
    /// When the earlier reference was made — the instant after which
    /// anything built on this extent is built on a claim the source has
    /// since withdrawn.
    used_at: Timestamp,
    detected_at: Timestamp,
}

impl RevisionRecord {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn origin(&self) -> SourceOrigin {
        self.origin
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn period(&self) -> DataPeriod {
        self.period
    }

    pub fn symbols(&self) -> &BTreeSet<String> {
        &self.symbols
    }

    pub fn was(&self) -> &str {
        &self.was
    }

    pub fn now(&self) -> &str {
        &self.now
    }

    pub fn used_at(&self) -> Timestamp {
        self.used_at
    }

    pub fn detected_at(&self) -> Timestamp {
        self.detected_at
    }

    /// Whether anything trained or backtested on `symbol` over `period`
    /// used this revised extent.
    pub fn covers(&self, symbol: &str, period: &DataPeriod) -> bool {
        self.symbols.contains(symbol) && self.period.overlaps(period)
    }

    /// Whether `reference` — some fetch, by anyone, of this source — used
    /// the bytes the source has since withdrawn, and is therefore the thing
    /// §22.3 says must be flagged.
    ///
    /// Two ways to have: the fetch was made no later than the instant the
    /// contradicted reference was (`used_at`), so it can only have seen the
    /// old bytes or older; or it hashes to exactly the hash that was
    /// withdrawn, whenever it was made. A fetch made after the revision, of
    /// the bytes the source now serves, used the current bytes and is not
    /// contradicted by them — and until 2026-09-12 `CampaignManifest::
    /// flag_revised` flagged exactly that campaign, the one that read the
    /// corrected extent, while the campaign that had read the original was
    /// closed on the log and flagged by nothing. This predicate is the one
    /// place the rule lives; the manifest and the kernel both ask it.
    pub fn contradicts(&self, reference: &DataReference) -> bool {
        reference.source_id() == self.source_id
            && reference
                .symbols()
                .iter()
                .any(|symbol| self.covers(symbol, &reference.range()))
            && (reference.retrieved_at() <= self.used_at || reference.content_hash() == self.was)
    }

    /// One line for a log or a cycle summary.
    pub fn describe(&self) -> String {
        format!(
            "{} revised {} covering {} to {} ({} symbol(s)): used as {} at {}, now serves {} \
             as of {}",
            self.source_id,
            self.locator,
            self.period.start().to_rfc3339(),
            self.period.end().to_rfc3339(),
            self.symbols.len(),
            &self.was[..12.min(self.was.len())],
            self.used_at.to_rfc3339(),
            &self.now[..12.min(self.now.len())],
            self.detected_at.to_rfc3339()
        )
    }
}

/// What recording a reference found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LedgerOutcome {
    /// The first reference to this extent.
    First,
    /// The extent was already referenced and hashes the same.
    Unchanged,
    /// The extent was already referenced and hashes differently: the source
    /// revised it after this platform used it.
    Revised(RevisionRecord),
}

impl LedgerOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Unchanged => "unchanged",
            Self::Revised(_) => "revised",
        }
    }

    pub const fn is_revised(&self) -> bool {
        matches!(self, Self::Revised(_))
    }
}

/// See the module doc.
///
/// Serialises and does not deserialise: [`ReferenceLedger::new`] refuses a
/// zero bound on either axis, and a `Deserialize` derive would have been a
/// second constructor that did not.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReferenceLedger {
    bound: usize,
    revision_bound: usize,
    /// Insertion order, oldest first — what eviction walks.
    order: VecDeque<ExtentKey>,
    /// The latest reference per extent.
    entries: BTreeMap<ExtentKey, DataReference>,
    /// Revisions caught, oldest first, bounded by `revision_bound`.
    revisions: VecDeque<RevisionRecord>,
    /// References evicted to stay within `bound`, over the ledger's life.
    /// A number, so "the ledger is full" is a fact a test or a cycle line can
    /// state rather than one inferred from a count that stopped rising.
    evicted: u64,
}

impl ReferenceLedger {
    /// A ledger holding at most `bound` references and `revision_bound`
    /// revision records. Zero is refused on either axis: a ledger that can
    /// hold nothing is a prohibition wearing a bound's name, and one that
    /// records no revisions is a revision check that cannot fire.
    pub fn new(bound: usize, revision_bound: usize) -> Result<Self> {
        if bound == 0 {
            return Err(Error::invalid(
                "a reference ledger must hold at least one reference; a bound of zero is a \
                 prohibition, not a bound",
            ));
        }
        if revision_bound == 0 {
            return Err(Error::invalid(
                "a reference ledger must keep at least one revision record, or a revision it \
                 detected would be a fact it immediately forgot",
            ));
        }
        Ok(Self {
            bound,
            revision_bound,
            order: VecDeque::new(),
            entries: BTreeMap::new(),
            revisions: VecDeque::new(),
            evicted: 0,
        })
    }

    /// The default bounds. See the constants for what they hold.
    pub fn bounded() -> Self {
        Self {
            bound: REFERENCE_LEDGER_BOUND,
            revision_bound: REVISION_LEDGER_BOUND,
            order: VecDeque::new(),
            entries: BTreeMap::new(),
            revisions: VecDeque::new(),
            evicted: 0,
        }
    }

    pub const fn bound(&self) -> usize {
        self.bound
    }

    pub const fn revision_bound(&self) -> usize {
        self.revision_bound
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub const fn evicted(&self) -> u64 {
        self.evicted
    }

    /// What recording `reference` would find, without recording it.
    ///
    /// Split from [`Self::record`] so a caller with a log can write the
    /// finding down *before* the ledger changes: a revision journaled after
    /// the in-memory ledger had already moved on was, for the instant between
    /// the two, a fact the process knew and the record did not, and a crash
    /// in that instant would have left a ledger that flagged nothing on the
    /// next start. Deterministic over the same ledger and reference, so
    /// `record` reaches the same outcome an instant later.
    pub fn assess(&self, reference: &DataReference, now: Timestamp) -> LedgerOutcome {
        let key = ExtentKey::of(reference);
        let Some(previous) = self.entries.get(&key) else {
            return LedgerOutcome::First;
        };
        match previous.verify_against(reference) {
            RevisionCheck::Unchanged => LedgerOutcome::Unchanged,
            RevisionCheck::Revised { was, now: latest } => LedgerOutcome::Revised(RevisionRecord {
                source_id: key.source_id,
                origin: previous.origin(),
                locator: key.locator,
                period: key.period,
                symbols: previous.symbols().clone(),
                was,
                now: latest,
                used_at: previous.retrieved_at(),
                detected_at: now,
            }),
        }
    }

    /// Record `reference`, comparing it against the last reference to the
    /// same extent if there was one, and keep the newer of the two.
    ///
    /// Never grows past the bound: when the ledger is full and the extent is
    /// new, the oldest extent is evicted first. A revision is recorded in the
    /// revision queue — bounded the same way — and returned, so the caller
    /// can act on it; the reference is still kept, because the ledger's job
    /// is to know what the source *now* serves as well as what it served.
    pub fn record(&mut self, reference: DataReference, now: Timestamp) -> LedgerOutcome {
        let outcome = self.assess(&reference, now);
        if let LedgerOutcome::Revised(revision) = &outcome {
            self.restore_revision(revision.clone());
        }
        self.restore_reference(reference);
        outcome
    }

    /// Put `reference` in the ledger as the latest reference to its extent,
    /// detecting nothing.
    ///
    /// The replay half of [`Self::record`]: a kernel rebuilding this ledger
    /// from its event log restores each recorded reference through here and
    /// each recorded revision through [`Self::restore_revision`], rather than
    /// re-running `record` and re-detecting — and re-counting — revisions the
    /// log already holds. Bounded exactly as `record` is.
    pub fn restore_reference(&mut self, reference: DataReference) {
        let key = ExtentKey::of(&reference);
        if let Some(held) = self.entries.get_mut(&key) {
            *held = reference;
            return;
        }
        while self.entries.len() >= self.bound {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.entries.remove(&oldest);
                    self.evicted = self.evicted.saturating_add(1);
                }
                // The order and the entries cannot disagree — every insert
                // below writes both — so this arm is unreachable in practice
                // and, if it ever were reached, growing past the bound is
                // the one thing this method must not do.
                None => break,
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, reference);
    }

    /// Put a revision the log already holds back in the bounded queue.
    pub fn restore_revision(&mut self, revision: RevisionRecord) {
        if self.revisions.len() >= self.revision_bound {
            self.revisions.pop_front();
        }
        self.revisions.push_back(revision);
    }

    /// The latest reference to an extent, if the ledger still holds one.
    pub fn get(
        &self,
        source_id: &str,
        locator: &str,
        period: DataPeriod,
    ) -> Option<&DataReference> {
        self.entries.get(&ExtentKey {
            source_id: source_id.to_string(),
            locator: locator.to_string(),
            period,
        })
    }

    /// Every revision caught, oldest first.
    pub fn revisions(&self) -> impl Iterator<Item = &RevisionRecord> {
        self.revisions.iter()
    }

    /// The most recently detected revision, if any, of an extent from
    /// `source_id` that named `symbol` and overlaps `period` — the question
    /// a research run asks before it trains on the pair.
    pub fn revision_covering(
        &self,
        source_id: &str,
        symbol: &str,
        period: &DataPeriod,
    ) -> Option<&RevisionRecord> {
        self.revisions
            .iter()
            .rev()
            .find(|revision| revision.source_id == source_id && revision.covers(symbol, period))
    }

    /// The distinct sources whose held references name `symbol` — the set
    /// `crate::campaign::assess_concentration` is asked about.
    pub fn sources_backing(&self, symbol: &str) -> BTreeSet<String> {
        self.entries
            .values()
            .filter(|reference| reference.symbols().contains(symbol))
            .map(|reference| reference.source_id().to_string())
            .collect()
    }
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use crate::schema::{FieldType, SourceSchema};
    use qip_core::Duration;
    use qip_events::Topic;
    use qip_financial::quality::LicensingClass;
    use qip_market_ingestion::adapter::SourceDescriptor;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn descriptor(name: &str) -> SourceDescriptor {
        SourceDescriptor {
            name: name.to_string(),
            provider: "this process".to_string(),
            licensing: LicensingClass::Synthetic,
            topics: vec![Topic::MarketBar],
            expected_latency: Duration::ZERO,
            production_requirement: None,
        }
    }

    /// A generated reference — the door with the fewest preconditions, so
    /// these tests are about the ledger and not about admission.
    fn reference(
        source: &str,
        locator: &str,
        at: Timestamp,
        bytes: &[u8],
    ) -> Result<DataReference> {
        DataReference::of_generated(
            &descriptor(source),
            locator,
            ["AAA".to_string()],
            DataPeriod::instant(at),
            SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
            bytes,
            at,
        )
    }

    /// The ledger never exceeds its bound, evicts the oldest extent first,
    /// and counts what it evicted.
    ///
    /// Mutated by deleting the `while self.entries.len() >= self.bound` loop
    /// in `record` — confirmed the ledger then holds four against a bound of
    /// three and this test fails, then restored.
    #[test]
    fn the_reference_ledger_never_grows_past_its_bound_and_evicts_the_oldest() -> Result<()> {
        let mut ledger = ReferenceLedger::new(3, 8)?;
        for index in 0..4 {
            let at = now().saturating_add(Duration::from_secs(index));
            let outcome = ledger.record(
                reference("synthetic-exchange", "bars://AAA", at, b"one")?,
                at,
            );
            assert_eq!(
                outcome,
                LedgerOutcome::First,
                "each instant is a new extent"
            );
        }
        assert_eq!(ledger.len(), 3, "the ledger grew past its bound");
        assert_eq!(ledger.evicted(), 1);
        assert!(
            ledger
                .get(
                    "synthetic-exchange",
                    "bars://AAA",
                    DataPeriod::instant(now())
                )
                .is_none(),
            "the oldest extent must be the one evicted"
        );
        assert!(
            ledger
                .get(
                    "synthetic-exchange",
                    "bars://AAA",
                    DataPeriod::instant(now().saturating_add(Duration::from_secs(3)))
                )
                .is_some(),
            "the newest extent must survive"
        );

        // Zero is refused on both axes.
        assert!(ReferenceLedger::new(0, 1).is_err());
        assert!(ReferenceLedger::new(1, 0).is_err());
        Ok(())
    }

    /// A second reference to the same extent is compared hash to hash: the
    /// same bytes are `Unchanged`, different bytes are a `Revised` record
    /// carrying both hashes and the instant the earlier reference was used —
    /// and the ledger keeps the newer reference so it knows what the source
    /// now serves.
    ///
    /// Mutated by replacing the `RevisionCheck::Revised` arm in `record` with
    /// `LedgerOutcome::Unchanged` — confirmed this test then fails on the
    /// outcome, then restored.
    #[test]
    fn a_re_recorded_extent_with_a_different_hash_is_a_revision() -> Result<()> {
        let mut ledger = ReferenceLedger::bounded();
        let first = reference("synthetic-exchange", "bars://AAA", now(), b"version one")?;
        assert_eq!(ledger.record(first.clone(), now()), LedgerOutcome::First);

        let later = now().saturating_add(Duration::from_secs(60));
        let same = DataReference::of_generated(
            &descriptor("synthetic-exchange"),
            "bars://AAA",
            ["AAA".to_string()],
            DataPeriod::instant(now()),
            SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
            b"version one",
            later,
        )?;
        assert_eq!(ledger.record(same, later), LedgerOutcome::Unchanged);
        assert!(
            ledger.revisions().next().is_none(),
            "premise: nothing revised yet"
        );

        let latest = later.saturating_add(Duration::from_secs(60));
        let revised = DataReference::of_generated(
            &descriptor("synthetic-exchange"),
            "bars://AAA",
            ["AAA".to_string()],
            DataPeriod::instant(now()),
            SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
            b"version two",
            latest,
        )?;
        let outcome = ledger.record(revised.clone(), latest);
        let LedgerOutcome::Revised(record) = outcome else {
            panic!("a changed extent was recorded as {outcome:?}");
        };
        assert_eq!(record.was(), first.content_hash());
        assert_eq!(record.now(), revised.content_hash());
        assert_eq!(record.detected_at(), latest);
        assert_eq!(
            record.used_at(),
            later,
            "used_at is the instant of the reference being contradicted, i.e. the latest \
             one the platform had used"
        );
        assert_eq!(record.origin(), SourceOrigin::Generated);
        assert_eq!(
            ledger.len(),
            1,
            "a re-recorded extent does not grow the ledger"
        );
        assert_eq!(
            ledger
                .get(
                    "synthetic-exchange",
                    "bars://AAA",
                    DataPeriod::instant(now())
                )
                .map(DataReference::content_hash),
            Some(revised.content_hash()),
            "the ledger must keep what the source now serves"
        );
        assert_eq!(ledger.revisions().count(), 1);
        Ok(())
    }

    /// A revision is found by the symbol and period a consumer is about to
    /// use, and not by a symbol or period it did not cover; and the sources
    /// backing a symbol are counted distinctly.
    ///
    /// Mutated by replacing `self.period.overlaps(period)` in
    /// `RevisionRecord::covers` with `true` — confirmed the disjoint-period
    /// half then fails, then restored.
    #[test]
    fn a_revision_is_found_by_the_symbol_and_period_it_covers() -> Result<()> {
        let mut ledger = ReferenceLedger::bounded();
        let window = DataPeriod::new(now(), now().saturating_add(Duration::from_hours(1)))?;
        let build = |source: &str, bytes: &[u8], at: Timestamp| {
            DataReference::of_generated(
                &descriptor(source),
                "bars://AAA",
                ["AAA".to_string()],
                window,
                SourceSchema::from_fields([("close".to_string(), FieldType::Number)]),
                bytes,
                at,
            )
        };
        ledger.record(build("synthetic-exchange", b"one", now())?, now());
        let later = now().saturating_add(Duration::from_hours(2));
        assert!(
            ledger
                .record(build("synthetic-exchange", b"two", later)?, later)
                .is_revised(),
            "premise: the extent was revised"
        );

        let inside = DataPeriod::instant(now().saturating_add(Duration::from_mins(30)));
        assert!(
            ledger
                .revision_covering("synthetic-exchange", "AAA", &inside)
                .is_some(),
            "a period inside the revised extent must be found"
        );
        let disjoint = DataPeriod::instant(now().saturating_add(Duration::from_hours(3)));
        assert!(
            ledger
                .revision_covering("synthetic-exchange", "AAA", &disjoint)
                .is_none(),
            "a period the revised extent never covered must not be flagged"
        );
        assert!(
            ledger
                .revision_covering("synthetic-exchange", "BBB", &inside)
                .is_none(),
            "a symbol the extent never named must not be flagged"
        );
        assert!(
            ledger
                .revision_covering("another-source", "AAA", &inside)
                .is_none(),
            "another source's extents are another source's"
        );

        // Two sources back AAA once a second one is recorded; the same source
        // twice is still one.
        assert_eq!(ledger.sources_backing("AAA").len(), 1);
        ledger.record(build("committed-tape", b"three", later)?, later);
        assert_eq!(ledger.sources_backing("AAA").len(), 2);
        assert!(ledger.sources_backing("BBB").is_empty());
        Ok(())
    }
}
