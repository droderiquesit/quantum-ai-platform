//! §22.4's fetch-on-demand campaign, bounded and TTL-scoped, and the
//! concentration-risk mitigation its own table names.
//!
//! The blueprint's arrow:
//!
//! ```text
//! campaign starts -> resolve references -> fetch into TTL cache -> verify
//! hashes -> backtest and purged CV over the cache -> emit results,
//! statistics, manifest -> cache expires and is deleted. What persists: the
//! manifest and the results.
//! ```
//!
//! This crate performs no I/O and holds no backtesting or purged
//! cross-validation logic — those live in `qip-portfolio-engine` and
//! `qip-learning-engine`, out of scope for this change and out of this
//! crate's dependency direction regardless (`backend/crates/services` may
//! not depend on `backend/crates/runtime`). What is built here is everything
//! upstream of that boundary and the one downstream fact §22.3 exists to
//! support: the bounded cache a campaign fetches into, the manifest that
//! outlives it, and the concentration check that can hold a universe back
//! before any of the rest runs.
//!
//! # Which of the section's five mitigations this closes
//!
//! Read the row in `docs/DELIVERY-STATUS.md` before assuming more than this
//! list states; it is written to be checked against the code, not paraphrased
//! from it.
//!
//! * **Built.** *"Source revises history after use."* [`ResearchCache`]
//!   verifies every re-fetch against the reference already cached for the
//!   same locator ([`DataReference::verify`]) and [`CampaignManifest`] flags
//!   every entry for that locator once a mismatch is found
//!   ([`FetchCampaign::fetch`]).
//! * **Built.** *"Research is slower than a local copy."* [`CacheBound`]'s
//!   TTL and entry ceiling, scoped to one [`FetchCampaign`], are exactly the
//!   "campaign-scoped caching and batching" the row names — batching itself
//!   is the caller's business (nothing here schedules a run), the bound is
//!   what makes the cache safe to hold at all.
//! * **Built.** *"Regulatory demand for data not retained."* [`CampaignManifest`]
//!   is the named, deliberately retained extract list — it survives
//!   [`FetchCampaign::close`], which is exactly the arrow's own "the cache
//!   expires and is deleted; what persists: the manifest and the results".
//! * **Partially built.** *"Vendor withdraws historical access."* The
//!   two-registered-sources half is [`assess_concentration`]. The row's
//!   second sentence — "bar-level fallback retained for three years on
//!   traded instruments" — is §22.1's retention taxonomy
//!   (`RetentionClass`, a fallback OHLCV series), which
//!   `docs/DELIVERY-STATUS.md`'s §22.1 row records as not existing yet; nothing
//!   here builds it, because doing so would be re-scoping this change onto a
//!   different section's absent foundation rather than closing the one this
//!   change owns.
//! * **Not built.** *"Sketch or reservoir error affects a model."* §22.2's
//!   row records that no sketch — no t-digest, no reservoir, no
//!   count-min — exists anywhere in this codebase. A mitigation that bounds a
//!   sketch's error has nothing to bound until one exists.
//!
//! Three of five, one of the three partial. `docs/DELIVERY-STATUS.md` records
//! the same count under §22.4 and the row is `PARTIAL`, not `REACHED`, for
//! exactly that reason.

use crate::ledger::RevisionRecord;
use crate::reference::{DataReference, RevisionCheck};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_numerics::sketch::ErrorBound;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A statistic a campaign estimated with a sketch rather than counted, and
/// the error the sketch declares for it — §22.4's "bounds are declared and
/// monitored", made a field on the manifest so the number never travels
/// without the bound that qualifies it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SketchedStatisticWire")]
pub struct SketchedStatistic {
    /// What was estimated, e.g. `bars_per_subject`.
    name: String,
    /// What it was estimated for, e.g. the subject.
    key: String,
    estimate: u64,
    /// Everything the sketch counted, which the bound is a fraction of.
    total: u64,
    bound: ErrorBound,
}

/// The wire shape, validated through [`SketchedStatistic::new`] on the way
/// in, so a manifest read back off the log cannot carry a statistic the
/// constructor would have refused.
#[derive(Deserialize)]
struct SketchedStatisticWire {
    name: String,
    key: String,
    estimate: u64,
    total: u64,
    bound: ErrorBound,
}

impl TryFrom<SketchedStatisticWire> for SketchedStatistic {
    type Error = Error;

    fn try_from(wire: SketchedStatisticWire) -> Result<Self> {
        Self::new(wire.name, wire.key, wire.estimate, wire.total, wire.bound)
    }
}

impl SketchedStatistic {
    /// Refuses an estimate above the total: a count-min estimate is a
    /// minimum over counters that each saw at most every increment, so an
    /// estimate larger than everything counted is not a loose estimate but
    /// a number no sketch produced.
    pub fn new(
        name: impl Into<String>,
        key: impl Into<String>,
        estimate: u64,
        total: u64,
        bound: ErrorBound,
    ) -> Result<Self> {
        if estimate > total {
            return Err(Error::invalid(format!(
                "a sketched estimate of {estimate} exceeds the {total} the sketch counted in \
                 all; no count-min sketch produces that number, so it is refused rather than \
                 recorded beside a bound it cannot have come from"
            )));
        }
        Ok(Self {
            name: name.into(),
            key: key.into(),
            estimate,
            total,
            bound,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub const fn estimate(&self) -> u64 {
        self.estimate
    }

    pub const fn total(&self) -> u64 {
        self.total
    }

    pub const fn bound(&self) -> ErrorBound {
        self.bound
    }

    /// The most the estimate may exceed the truth by, at the volume actually
    /// counted.
    pub fn absolute_error(&self) -> f64 {
        self.bound.absolute_error(self.total)
    }

    pub fn describe(&self) -> String {
        format!(
            "{} for {}: {} (never below the truth, above it by at most {:.1} with probability \
             {})",
            self.name,
            self.key,
            self.estimate,
            self.absolute_error(),
            1.0 - self.bound.delta()
        )
    }
}

/// How long a fetched extract may live in a campaign's cache, and how many
/// extracts the cache may hold at once.
///
/// Both bounds are mandatory and both are refused at zero: a cache with no
/// TTL is the unbounded retention `.claude/rules/domains/data-and-streaming.md`
/// forbids, and a cache with no entry ceiling grows with every reference a
/// caller resolves, which is the same failure with a different axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheBound {
    ttl: Duration,
    max_entries: usize,
}

impl CacheBound {
    pub fn new(ttl: Duration, max_entries: usize) -> Result<Self> {
        if ttl.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a research cache must have a positive time-to-live; an unbounded cache is the \
                 unbounded retention this platform's data domain forbids",
            ));
        }
        if max_entries == 0 {
            return Err(Error::invalid(
                "a research cache must admit at least one extract; a bound of zero is a \
                 prohibition, not a bound",
            ));
        }
        Ok(Self { ttl, max_entries })
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}

/// One extract a campaign is holding, and when it stops being usable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CachedExtract {
    reference: DataReference,
    bytes: Vec<u8>,
    expires_at: Timestamp,
}

/// A bounded, TTL-scoped cache of fetched extracts, keyed on each reference's
/// locator.
///
/// Never grows past [`CacheBound::max_entries`]: [`Self::insert`] evicts
/// whatever has expired first and only then checks the ceiling, so a cache
/// that has genuinely emptied out makes room for new work, and one that has
/// not is refused rather than allowed to exceed the bound it was given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResearchCache {
    bound: CacheBound,
    entries: BTreeMap<String, CachedExtract>,
}

impl ResearchCache {
    pub fn new(bound: CacheBound) -> Self {
        Self {
            bound,
            entries: BTreeMap::new(),
        }
    }

    pub fn bound(&self) -> CacheBound {
        self.bound
    }

    /// Drop every extract whose TTL has elapsed as of `now`.
    pub fn evict_expired(&mut self, now: Timestamp) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }

    /// Insert a freshly fetched extract, evicting expired entries first.
    ///
    /// Refuses when the cache is already at its bound and the locator is not
    /// already present — growing past the stated ceiling to avoid an awkward
    /// refusal is exactly the unbounded buffer this cache exists not to be.
    /// Replacing an existing locator's entry is always allowed regardless of
    /// the count, because it does not grow the cache.
    pub fn insert(
        &mut self,
        reference: DataReference,
        bytes: Vec<u8>,
        now: Timestamp,
    ) -> Result<()> {
        self.evict_expired(now);
        let locator = reference.locator().to_string();
        if !self.entries.contains_key(&locator) && self.entries.len() >= self.bound.max_entries {
            return Err(Error::denied(format!(
                "the research cache already holds its bound of {} extract(s); nothing has \
                 expired to make room for `{locator}`. Close the campaign, wait for an entry to \
                 expire, or open one with a larger bound",
                self.bound.max_entries
            )));
        }
        let expires_at = now.saturating_add(self.bound.ttl);
        self.entries.insert(
            locator,
            CachedExtract {
                reference,
                bytes,
                expires_at,
            },
        );
        Ok(())
    }

    /// The reference and bytes cached for `locator`, if present and not
    /// expired as of `now`.
    pub fn get(&self, locator: &str, now: Timestamp) -> Option<(&DataReference, &[u8])> {
        self.entries
            .get(locator)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| (&entry.reference, entry.bytes.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One fetch a campaign made, when, and whether a later verification during
/// the same campaign found the source had revised what it serves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestEntry {
    reference: DataReference,
    fetched_at: Timestamp,
    /// `None` until a later fetch of the same locator disagrees with this
    /// one. Absence says "not re-checked", not "confirmed unchanged" — the
    /// distinction `legal::Legality` already holds between unknown and
    /// permitted, applied to a manifest entry instead of a licence.
    flagged: Option<RevisionCheck>,
}

impl ManifestEntry {
    pub fn reference(&self) -> &DataReference {
        &self.reference
    }

    pub fn fetched_at(&self) -> Timestamp {
        self.fetched_at
    }

    pub fn flagged(&self) -> Option<&RevisionCheck> {
        self.flagged.as_ref()
    }
}

/// What a campaign fetched, in order, and which entries a revision has
/// flagged. Survives the campaign that produced it — per the blueprint's own
/// arrow, the cache is deleted and the manifest persists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CampaignManifest {
    entries: Vec<ManifestEntry>,
    /// Statistics the campaign sketched rather than counted, each with its
    /// declared bound. Empty for a campaign that sketched nothing.
    #[serde(default)]
    statistics: Vec<SketchedStatistic>,
    /// Subjects whose window came from §22.1's fallback series because the
    /// subject's own stream no longer held enough history — the "vendor
    /// withdraws historical access" mitigation, recorded where an audit can
    /// see that the insurance was drawn on.
    #[serde(default)]
    fallbacks: BTreeSet<String>,
}

impl CampaignManifest {
    fn record(&mut self, reference: DataReference, fetched_at: Timestamp) {
        self.entries.push(ManifestEntry {
            reference,
            fetched_at,
            flagged: None,
        });
    }

    /// Flag every entry a ledger-detected revision contradicts: same
    /// source, a symbol in common, overlapping periods. The ledger sees
    /// across campaigns where [`Self::flag_revision`] sees within one, and a
    /// campaign that read a window the ledger already knew to be revised
    /// must say so on its own manifest rather than leave the reader to join
    /// two records.
    fn flag_revised(&mut self, revision: &RevisionRecord) {
        let check = RevisionCheck::Revised {
            was: revision.was().to_string(),
            now: revision.now().to_string(),
        };
        for entry in self.entries.iter_mut().filter(|entry| {
            entry.reference.source_id() == revision.source_id()
                && entry
                    .reference
                    .symbols()
                    .iter()
                    .any(|symbol| revision.covers(symbol, &entry.reference.range()))
        }) {
            entry.flagged = Some(check.clone());
        }
    }

    pub fn statistics(&self) -> &[SketchedStatistic] {
        &self.statistics
    }

    pub fn fallbacks(&self) -> &BTreeSet<String> {
        &self.fallbacks
    }

    /// Flag every entry fetched for `locator` as revised — a later re-fetch
    /// inside the same campaign found the source no longer serves what an
    /// earlier entry recorded, so every entry naming that locator is now a
    /// claim the campaign itself has contradicted.
    fn flag_revision(&mut self, locator: &str, check: RevisionCheck) {
        for entry in self
            .entries
            .iter_mut()
            .filter(|entry| entry.reference.locator() == locator)
        {
            entry.flagged = Some(check.clone());
        }
    }

    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    /// Entries a re-fetch inside this campaign found revised.
    pub fn flagged(&self) -> impl Iterator<Item = &ManifestEntry> {
        self.entries.iter().filter(|entry| {
            entry
                .flagged
                .as_ref()
                .is_some_and(RevisionCheck::is_revised)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A bounded, named research campaign: resolve references, fetch into a
/// TTL-scoped cache, verify hashes against whatever was cached before, record
/// a manifest — the campaign starts to "verify hashes" step of the blueprint's
/// arrow. Backtesting and purged cross-validation over the cache, and
/// emitting statistics, are the caller's business in a crate this one may not
/// depend on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FetchCampaign {
    id: String,
    cache: ResearchCache,
    manifest: CampaignManifest,
    opened_at: Timestamp,
}

impl FetchCampaign {
    /// Open a campaign, refusing an unnamed one.
    ///
    /// A campaign's manifest is the thing that outlives it, per the
    /// blueprint's own arrow; a manifest nobody can attribute to a run is not
    /// the audit trail that arrow describes.
    pub fn open(id: impl Into<String>, bound: CacheBound, opened_at: Timestamp) -> Result<Self> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(Error::invalid(
                "a fetch campaign must be named, so the manifest it produces can be attributed \
                 to the run that made it",
            ));
        }
        Ok(Self {
            id,
            cache: ResearchCache::new(bound),
            manifest: CampaignManifest::default(),
            opened_at,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    pub fn cache(&self) -> &ResearchCache {
        &self.cache
    }

    pub fn manifest(&self) -> &CampaignManifest {
        &self.manifest
    }

    /// Resolve `reference` into the cache.
    ///
    /// `bytes` is the caller's own read — this crate opens no socket — and is
    /// verified against whatever this campaign already cached for the same
    /// locator, if anything. A mismatch flags every manifest entry recorded
    /// for that locator so far, and is still recorded and cached: a revised
    /// source is a fact the manifest must carry, not an error that stops the
    /// campaign.
    pub fn fetch(
        &mut self,
        reference: DataReference,
        bytes: &[u8],
        now: Timestamp,
    ) -> Result<RevisionCheck> {
        let check = match self.cache.get(reference.locator(), now) {
            Some((cached, _)) => cached.verify(bytes),
            None => RevisionCheck::Unchanged,
        };
        // Recorded before flagging, not after: `flag_revision` marks every
        // manifest entry that already names this locator, and this fetch's
        // own entry has to be one of them or the newest fetch — the one a
        // re-run would most need to distrust — would be the one entry left
        // unflagged.
        self.manifest.record(reference.clone(), now);
        if check.is_revised() {
            self.manifest
                .flag_revision(reference.locator(), check.clone());
        }
        self.cache.insert(reference, bytes.to_vec(), now)?;
        Ok(check)
    }

    /// Flag every manifest entry a revision the ledger detected contradicts.
    /// See [`CampaignManifest::flag_revised`].
    pub fn flag_revised(&mut self, revision: &RevisionRecord) {
        self.manifest.flag_revised(revision);
    }

    /// Record a statistic this campaign estimated with a sketch, with the
    /// bound the sketch declares.
    pub fn attach_statistic(&mut self, statistic: SketchedStatistic) {
        self.manifest.statistics.push(statistic);
    }

    /// Record that `subject`'s window was drawn from the fallback series.
    pub fn record_fallback(&mut self, subject: impl Into<String>) {
        self.manifest.fallbacks.insert(subject.into());
    }

    /// Close the campaign. The cache is dropped here — deleted, per the
    /// blueprint's arrow — and only the manifest is returned.
    pub fn close(self) -> CampaignManifest {
        self.manifest
    }
}

/// Whether a data class has enough independently viable backing to be
/// promoted past validation.
///
/// §22.3's own table: "a data class with only one viable source is a
/// concentration risk, and a universe with fewer than two cannot be promoted
/// past validation." [`MINIMUM_VIABLE_SOURCES`] is that number, named rather
/// than inlined so the rule is one constant a reviewer can find, not a `2`
/// buried in a comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcentrationVerdict {
    /// At least [`ConcentrationVerdict::MINIMUM_VIABLE_SOURCES`] independent,
    /// non-quarantined sources back this class.
    Sufficient { viable_sources: usize },
    /// A concentration risk: fewer than the minimum back this class, and it
    /// must be held back from promotion past validation however good the
    /// one source it has is.
    HeldBack { viable_sources: usize },
}

impl ConcentrationVerdict {
    /// §22.3's own number: "a universe with fewer than two cannot be
    /// promoted past validation".
    pub const MINIMUM_VIABLE_SOURCES: usize = 2;

    pub fn is_sufficient(&self) -> bool {
        matches!(self, Self::Sufficient { .. })
    }

    pub fn viable_sources(&self) -> usize {
        match self {
            Self::Sufficient { viable_sources } | Self::HeldBack { viable_sources } => {
                *viable_sources
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Sufficient { viable_sources } => format!(
                "sufficient: {viable_sources} independently viable source(s) back this class"
            ),
            Self::HeldBack { viable_sources } => format!(
                "held back: only {viable_sources} independently viable source(s) back this \
                 class, and §22.3 requires at least {} before promotion past validation — a \
                 concentration risk",
                Self::MINIMUM_VIABLE_SOURCES
            ),
        }
    }
}

/// §22.4's concentration-risk mitigation: assess whether a data class has
/// enough independent backing to be promoted.
///
/// Takes the distinct source identifiers backing the class directly, already
/// filtered to whichever the caller considers viable (typically: registered
/// and not quarantined — see
/// `crate::decision::RegisteredSource::is_quarantined`). Kept as a plain set
/// rather than a method on `DataFinder` because "viable" is the caller's own
/// judgement about a class (asset class, instrument, or however a universe is
/// carved) that this crate has no concept of; what this function owns is only
/// the arithmetic §22.3's row states, applied to whatever set the caller
/// hands it.
pub fn assess_concentration<'a>(
    viable_source_ids: impl IntoIterator<Item = &'a str>,
) -> ConcentrationVerdict {
    let distinct: std::collections::BTreeSet<&str> = viable_source_ids.into_iter().collect();
    let viable_sources = distinct.len();
    if viable_sources >= ConcentrationVerdict::MINIMUM_VIABLE_SOURCES {
        ConcentrationVerdict::Sufficient { viable_sources }
    } else {
        ConcentrationVerdict::HeldBack { viable_sources }
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
    use crate::category::ContentSignal;
    use crate::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
    use crate::decision::RegisteredSource;
    use crate::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
    use crate::finder::{DataFinder, FinderConfig};
    use crate::legal::{LicensingPosture, SourceLicense};
    use crate::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
    use crate::quality::SourceCost;
    use crate::reference::DataPeriod;
    use crate::source::{SourceCandidate, SourceIdentity};
    use qip_contracts::governance::Usage;
    use qip_core::{Currency, Decimal};
    use qip_events::Topic;
    use qip_financial::asset_class::AssetClass;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn rest() -> AccessMechanism {
        AccessMechanism::Rest {
            auth: AuthRequirement::None,
            incremental_parameter: None,
            page_size: 500,
        }
    }

    fn registered(id: &str, url: &str, host: &str) -> Result<RegisteredSource> {
        let candidate = SourceCandidate::new(
            SourceIdentity::new(id, "Example Filing Search", "Example Registry")?,
            SourceEndpoint::parse(url, rest())?,
            SourceCoverage::new(
                [AssetClass::Equity],
                [SourceRegion::Global],
                ["ACME".to_string()],
                UpdateFrequency::Daily,
            )?,
            LicensingPosture::declared(SourceLicense::new(
                "example-open-data",
                [Usage::Research, Usage::Derive, Usage::Trade],
            )?),
            SourceCost::free(Currency::USD),
            SourceRegion::Global,
            [Topic::ReferenceDataUpdated],
            "operator declared",
            now(),
        )?
        .with_content_signal(ContentSignal::RegulatoryFiling);

        let mut probe = InMemoryProbe::new()
            .with_robots(
                host,
                RobotsFetch::Served {
                    body: "User-agent: *\nAllow: /\n".to_string(),
                    latency: Duration::from_millis(5),
                },
            )
            .with_head(
                url,
                HeadResponse {
                    status: 200,
                    content_type: Some("application/json".to_string()),
                    content_length: Some(2),
                    last_modified: None,
                    latency: Duration::from_millis(5),
                },
            )
            .with_sample(
                url,
                PayloadSample {
                    body: "{}".to_string(),
                    media_type: "application/json".to_string(),
                    payload_at: Some(now()),
                    latency: Duration::from_millis(5),
                },
            );

        let mut finder = DataFinder::new(FinderConfig::new(
            "qip-test-crawler/1.0",
            Usage::Derive,
            "desk-owner",
            7,
        )?);
        let decisions = finder.assess(vec![candidate], &mut probe, now())?;
        assert!(decisions[0].is_registered(), "premise: fixture registers");
        Ok(finder.registered(id).expect("registered above").clone())
    }

    fn reference(source: &RegisteredSource, url: &str, bytes: &[u8]) -> Result<DataReference> {
        DataReference::of(
            source,
            url,
            ["ACME".to_string()],
            DataPeriod::instant(now()),
            source.source().schema().clone(),
            bytes,
            now(),
            Decimal::ZERO,
            0.9,
        )
    }

    /// A universe backed by exactly one viable source is held back — the
    /// concentration-risk mitigation §22.3's own table names.
    ///
    /// Mutated by changing `MINIMUM_VIABLE_SOURCES` to `1` — confirmed this
    /// test then fails because one source reads as sufficient, then restored.
    #[test]
    fn a_universe_backed_by_only_one_viable_source_is_held_back() {
        let verdict = assess_concentration(["source-a"]);
        assert!(
            !verdict.is_sufficient(),
            "one viable source was treated as sufficient backing"
        );
        assert_eq!(verdict.viable_sources(), 1);
        assert!(
            verdict.describe().contains("concentration risk"),
            "the verdict does not name the risk: {}",
            verdict.describe()
        );

        let two = assess_concentration(["source-a", "source-b"]);
        assert!(
            two.is_sufficient(),
            "two independent viable sources were not treated as sufficient"
        );
        assert_eq!(two.viable_sources(), 2);

        // Duplicates of the same source id do not count twice — two reports
        // of the same one source is still one source.
        let duplicated = assess_concentration(["source-a", "source-a"]);
        assert!(
            !duplicated.is_sufficient(),
            "the same source counted twice was treated as two independent sources"
        );
    }

    /// The cache never exceeds its stated bound, and evicting an expired
    /// entry makes room for a new one.
    ///
    /// Mutated by removing the `evict_expired` call at the top of `insert` —
    /// confirmed the cache then refuses the third insert even after the TTL
    /// elapsed, failing this test, then restored.
    #[test]
    fn the_research_cache_never_grows_past_its_bound_and_eviction_makes_room() -> Result<()> {
        let bound = CacheBound::new(Duration::from_secs(10), 1)?;
        let mut cache = ResearchCache::new(bound);
        let source = registered(
            "cache-source",
            "https://example.gov/filings?entity=1",
            "example.gov",
        )?;
        let first = reference(&source, "https://example.gov/filings?entity=1", b"one")?;
        cache.insert(first, b"one".to_vec(), now())?;
        assert_eq!(cache.len(), 1);

        let second = reference(&source, "https://example.gov/filings?entity=2", b"two")?;
        let refused = cache
            .insert(second.clone(), b"two".to_vec(), now())
            .expect_err("the cache admitted a second entry past its bound of one");
        assert!(
            refused.message().contains("bound of 1"),
            "the refusal does not name the bound: {refused}"
        );

        // Past the TTL, the first entry is expired and evicting it makes
        // room — the bound is on live entries, not a permanent ceiling.
        let later = now().saturating_add(Duration::from_secs(11));
        cache.insert(second, b"two".to_vec(), later)?;
        assert_eq!(
            cache.len(),
            1,
            "eviction did not make room for the new entry"
        );
        Ok(())
    }

    /// A campaign detects a source revising the same locator between two
    /// fetches, flags every manifest entry for it, and the manifest survives
    /// the campaign's close while the cache does not.
    ///
    /// Mutated by deleting the `flag_revision` call in `FetchCampaign::fetch`
    /// — confirmed the manifest then reports no flagged entries even after a
    /// hash mismatch, failing this test, then restored.
    #[test]
    fn a_campaign_flags_manifest_entries_when_a_source_revises_between_fetches() -> Result<()> {
        let source = registered(
            "campaign-source",
            "https://example.gov/filings?entity=1",
            "example.gov",
        )?;
        let bound = CacheBound::new(Duration::from_secs(3_600), 8)?;
        let mut campaign = FetchCampaign::open("research-run-1", bound, now())?;

        let url = "https://example.gov/filings?entity=1";
        let first = reference(&source, url, b"filing version one")?;
        let outcome = campaign.fetch(first, b"filing version one", now())?;
        assert_eq!(outcome, RevisionCheck::Unchanged);
        assert!(
            campaign.manifest().flagged().next().is_none(),
            "the first fetch of a locator must not be flagged: nothing came before it"
        );

        let later = now().saturating_add(Duration::from_secs(60));
        let second = reference(&source, url, b"filing version two")?;
        let outcome = campaign.fetch(second, b"filing version two", later)?;
        assert!(outcome.is_revised(), "a revised re-fetch was not detected");

        let flagged: Vec<_> = campaign.manifest().flagged().collect();
        assert_eq!(
            flagged.len(),
            2,
            "both manifest entries for the revised locator must be flagged, not only the newer one"
        );

        let manifest = campaign.close();
        assert_eq!(manifest.entries().len(), 2);
        Ok(())
    }

    /// A sketched statistic whose estimate exceeds its total is a number no
    /// sketch produced, and it is refused in code and on the wire alike.
    ///
    /// Mutated by deleting the `estimate > total` refusal in
    /// `SketchedStatistic::new` — confirmed both halves then accept, then
    /// restored.
    #[test]
    fn a_sketched_estimate_above_its_total_is_refused_in_code_and_on_the_wire() -> Result<()> {
        let bound = ErrorBound::new(0.01, 0.01)?;
        // Premise: a possible statistic builds and round-trips.
        let honest = SketchedStatistic::new("bars_per_subject", "AAA", 300, 300, bound)?;
        let text = serde_json::to_string(&honest)?;
        let back: SketchedStatistic = serde_json::from_str(&text)?;
        assert_eq!(back, honest);

        let refused = SketchedStatistic::new("bars_per_subject", "AAA", 301, 300, bound)
            .expect_err("an estimate above everything counted was recorded");
        assert_eq!(refused.code(), "invalid", "got {refused:?}");

        // The same statistic hand-written onto the wire is refused at the
        // read, because the read goes through `new`.
        let forged = text.replace("\"estimate\":300", "\"estimate\":301");
        assert_ne!(forged, text, "premise: the forgery changed the text");
        assert!(
            serde_json::from_str::<SketchedStatistic>(&forged).is_err(),
            "a statistic the constructor refuses was accepted off the wire"
        );
        Ok(())
    }

    /// Opening a campaign with a blank name is refused.
    #[test]
    fn an_unnamed_campaign_is_refused() -> Result<()> {
        let bound = CacheBound::new(Duration::from_secs(10), 1)?;
        let error = FetchCampaign::open("   ", bound, now())
            .expect_err("a campaign with a blank name was opened");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        Ok(())
    }

    /// A zero TTL or a zero entry bound is refused, not clamped to one.
    #[test]
    fn a_cache_bound_of_zero_is_refused_on_either_axis() {
        assert!(CacheBound::new(Duration::ZERO, 4).is_err());
        assert!(CacheBound::new(Duration::from_secs(1), 0).is_err());
    }
}
