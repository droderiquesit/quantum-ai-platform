//! What was actually fetched, from where, in what shape, and whether it still
//! matches — as distinct from a [`crate::decision::RegisteredSource`] or an
//! [`crate::admission::AdmittedSource`], which only say a source *may* be used.
//!
//! §22.3's pseudo-code:
//!
//! ```text
//! DataReference { source, endpoint, symbols, range, schema_version,
//!                 content_hash, retrieved_at, cost_estimate, availability }
//! ```
//!
//! and its own table names the content hash "the single most important
//! field, because it is what flags a backtest whose source revised its
//! history." Every constructor computes that hash with
//! `qip_core::sha256_hex` — the exact mechanism
//! [`qip_financial::manifest::SourceManifest`] already uses for §7.2's
//! content-hashed manifest, reused rather than reinvented, because a second
//! hashing scheme for the same claim ("we hold a reference to what these
//! bytes were") is exactly the kind of second source of truth
//! `.claude/rules/architecture/00-boundaries.md` forbids.
//!
//! # Three doors, one type
//!
//! A reference records which door its source came through, in
//! [`SourceOrigin`], because the three are vetted differently and a reader
//! who could not tell them apart would credit a generated stream with a
//! vendor's standing:
//!
//! * [`SourceOrigin::Discovered`] — a [`RegisteredSource`], produced only by
//!   `DataFinder::assess_one` after the discover → probe → classify → score →
//!   route pipeline. Carries the §7.6.1 category the Classify stage assigned,
//!   and [`DataReference::of`] refuses a source that has none.
//! * [`SourceOrigin::CatalogueAdmitted`] — an [`AdmittedSource`], produced
//!   only from a `LicensingDecision` the admission gate minted, carrying the
//!   category the shipped manifest declares (ADR 0057).
//! * [`SourceOrigin::Generated`] — a stream this platform generated itself:
//!   the synthetic exchange, a committed tape. Gated on the descriptor's
//!   licensing class being `Synthetic`, the one class barred from production
//!   decisions, and carrying no category at all: §7.6.1's table describes
//!   kinds of deep-web source, and "this process" is not one of them. The
//!   precedent is [`qip_financial::manifest::SourceManifest::generated`],
//!   which files generated text under a distinct source for the same reason.
//!
//! The category precondition ADR 0056 stated — a reference must say what kind
//! of source it came from — therefore still holds for every source a vendor
//! could be asked to serve again, and is answered structurally by the origin
//! for the one kind nobody could.

use crate::admission::AdmittedSource;
use crate::category::SourceCategory;
use crate::decision::RegisteredSource;
use crate::schema::SourceSchema;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::adapter::SourceDescriptor;
use qip_market_ingestion::connector::FetchDigest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The period of the underlying world a fetch covered — not when it was
/// fetched (that is [`DataReference::retrieved_at`]), but what instants its
/// records describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataPeriod {
    start: Timestamp,
    end: Timestamp,
}

impl DataPeriod {
    /// Build a period, refusing one that ends before it starts.
    ///
    /// A period is compared and intersected downstream on the assumption that
    /// `end >= start`; an inverted period would make every such comparison a
    /// guess about which end the caller meant.
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self> {
        if end < start {
            return Err(Error::invalid(format!(
                "a data period cannot end ({end}) before it starts ({start}); a fetch describes \
                 a span, and a span with its ends reversed names nothing"
            )));
        }
        Ok(Self { start, end })
    }

    /// A period covering a single instant — the common case for a reference
    /// point sample rather than a range of history.
    pub fn instant(at: Timestamp) -> Self {
        Self { start: at, end: at }
    }

    pub fn start(&self) -> Timestamp {
        self.start
    }

    pub fn end(&self) -> Timestamp {
        self.end
    }

    pub fn contains(&self, at: Timestamp) -> bool {
        at >= self.start && at <= self.end
    }

    /// Whether any instant lies in both periods. Closed on both ends, so two
    /// periods that share only a boundary instant overlap — a bar that closes
    /// at the instant a revised extent begins was priced from it.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// Which door a reference's source came through. See the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    /// Vetted by `DataFinder`'s discovery pipeline.
    Discovered,
    /// Admitted by the licensing catalogue for a shipped connector.
    CatalogueAdmitted,
    /// Generated by this platform; never a vendor, never production-grade.
    Generated,
}

impl SourceOrigin {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::CatalogueAdmitted => "catalogue_admitted",
            Self::Generated => "generated",
        }
    }

    /// Whether a source through this door is a vendor that could withdraw
    /// access — the thing §22.3's concentration rule counts. A stream this
    /// platform generated is not: it cannot withdraw, and two of them are
    /// not two independent sources of anything about the world. Until
    /// 2026-09-12 `assess_concentration` counted a synthetic tape beside a
    /// synthetic exchange as sufficient backing.
    pub const fn is_independent_vendor(&self) -> bool {
        matches!(self, Self::Discovered | Self::CatalogueAdmitted)
    }
}

/// Whether a re-fetch still says what a [`DataReference`] recorded.
///
/// A named type rather than a `bool`, so the record carries both hashes
/// rather than a caller having to have kept the old one somewhere to explain
/// what changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevisionCheck {
    /// The source still serves what this reference recorded.
    Unchanged,
    /// The bytes re-fetched hash differently. The source revised its history
    /// after this reference was made — §22.3's own reason the content hash
    /// is "the single most important field".
    Revised { was: String, now: String },
}

impl RevisionCheck {
    pub fn is_revised(&self) -> bool {
        matches!(self, Self::Revised { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Unchanged => "unchanged: the re-fetch hashes the same as the reference".into(),
            Self::Revised { was, now } => format!(
                "revised: the reference recorded {was} and a re-fetch now hashes {now}; the \
                 source changed what it serves for this extent after it was used"
            ),
        }
    }
}

/// A pointer to what was actually fetched: the source, the door it came
/// through and its §7.6.1 category where it has one, the address, the symbols
/// and period covered, the shape it was in, and what it cost — everything a
/// research scheduler or an audit needs to answer "what did we use, from
/// where, and does it still say that".
///
/// Fields are private and every constructor is gated on a value only a vetted
/// path can produce. No constructor fetches — this crate performs no I/O — so
/// the bytes hashed are always the caller's own read, exactly as
/// [`qip_financial::manifest::SourceManifest::of`] takes bytes the caller
/// already has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataReference {
    source_id: String,
    origin: SourceOrigin,
    /// `None` only for [`SourceOrigin::Generated`]; see the module doc.
    category: Option<SourceCategory>,
    /// Where this specific fetch was made — the source's endpoint plus
    /// whatever query or page identified this particular extent. Opaque here
    /// for the same reason [`qip_financial::manifest::SourceManifest`]'s
    /// locator is: only the source's own adapter knows how to resolve it.
    locator: String,
    symbols: BTreeSet<String>,
    range: DataPeriod,
    schema: SourceSchema,
    /// SHA-256 of exactly the bytes read, lowercase hex.
    content_hash: String,
    /// Length of the extent hashed, so a manifest reader can tell a one-line
    /// answer from a full table without re-fetching either.
    bytes: u64,
    retrieved_at: Timestamp,
    cost_estimate: Decimal,
    /// The source's known availability at the time of this fetch, in
    /// `[0, 1]` — carried so a research scheduler can weigh a reference from
    /// an unreliable source without re-deriving `SourceHealth` from scratch.
    availability: f64,
}

impl DataReference {
    /// Describe what was fetched from a discovered `source`.
    ///
    /// Refuses:
    /// * a source with no recorded §7.6.1 category — see the module doc;
    /// * an empty locator, for the reason `SourceManifest::of` refuses one:
    ///   a reference that hashes bytes and names nowhere to re-fetch them is
    ///   not a reference;
    /// * no symbols, because a reference covering nothing identifiable
    ///   cannot be checked for concentration risk against anything;
    /// * empty bytes, for the reason `SourceManifest::of` refuses them: the
    ///   SHA-256 of nothing is the same for every extent that never arrived;
    /// * a negative cost estimate, which is a rebate no data source offers;
    /// * an availability outside `[0, 1]`.
    #[allow(clippy::too_many_arguments)]
    pub fn of(
        source: &RegisteredSource,
        locator: impl Into<String>,
        symbols: impl IntoIterator<Item = String>,
        range: DataPeriod,
        schema: SourceSchema,
        bytes: &[u8],
        retrieved_at: Timestamp,
        cost_estimate: Decimal,
        availability: f64,
    ) -> Result<Self> {
        let category = source.category().ok_or_else(|| {
            Error::invalid(format!(
                "`{}` has no §7.6.1 category recorded; classify it during assessment before \
                 referencing what was fetched from it — a data reference must say what kind of \
                 source it came from",
                source.id()
            ))
        })?;
        Self::build(
            source.id(),
            SourceOrigin::Discovered,
            Some(category),
            locator.into(),
            symbols,
            range,
            schema,
            bytes,
            retrieved_at,
            cost_estimate,
            availability,
        )
    }

    /// Describe what was fetched from a catalogue-admitted connector source.
    ///
    /// The category and schema come from the [`AdmittedSource`], which took
    /// them from the shipped manifest, so a reference from this door cannot
    /// claim a shape or a kind the manifest did not declare. The same
    /// refusals as [`Self::of`] apply to the locator, symbols, bytes, cost
    /// and availability.
    #[allow(clippy::too_many_arguments)]
    pub fn of_admitted(
        source: &AdmittedSource,
        locator: impl Into<String>,
        symbols: impl IntoIterator<Item = String>,
        range: DataPeriod,
        bytes: &[u8],
        retrieved_at: Timestamp,
        cost_estimate: Decimal,
        availability: f64,
    ) -> Result<Self> {
        Self::build(
            source.source_id(),
            SourceOrigin::CatalogueAdmitted,
            Some(source.category()),
            locator.into(),
            symbols,
            range,
            source.schema().clone(),
            bytes,
            retrieved_at,
            cost_estimate,
            availability,
        )
    }

    /// Describe a catalogue-admitted source's fetch from the digest the
    /// connector runtime took over its bytes.
    ///
    /// The hash is the digest's rather than computed here, and that is the
    /// point rather than a shortcut: the bytes were released to the loop and
    /// discarded, as §22.1 requires, and a [`FetchDigest`] has one
    /// constructor that hashes a body it was given — so a hash can only reach
    /// this constructor by having been taken over real bytes. Refuses a digest
    /// that names a different source than the admission does.
    ///
    /// The reference's symbols are the digest's *subjects* — this platform's
    /// ids for what the mapped records are about — and not its `symbols`,
    /// which are the vendor's own row keys. A campaign asks the ledger by
    /// `ObjectId`; a ledger keyed on `EUR/USD@2026-09-04` answered no
    /// campaign, and a connector's revision flagged nothing, until this
    /// distinction was made.
    pub fn from_digest(
        source: &AdmittedSource,
        digest: &FetchDigest,
        cost_estimate: Decimal,
        availability: f64,
    ) -> Result<Self> {
        if digest.source_id() != source.source_id() {
            return Err(Error::invalid(format!(
                "the digest names `{}` and the admission names `{}`; a reference binds one \
                 fetch to one admitted source, and these disagree",
                digest.source_id(),
                source.source_id()
            )));
        }
        let (start, end) = digest.period();
        Self::build_hashed(
            source.source_id(),
            SourceOrigin::CatalogueAdmitted,
            Some(source.category()),
            digest.locator().to_string(),
            digest.subjects().iter().cloned(),
            DataPeriod::new(start, end)?,
            source.schema().clone(),
            digest.sha256().to_string(),
            digest.bytes(),
            digest.retrieved_at(),
            cost_estimate,
            availability,
        )
    }

    /// Describe an extent this platform generated itself.
    ///
    /// Refuses any descriptor whose licensing class is not `Synthetic`: that
    /// class is the one `qip_financial::quality::LicensingClass` bars from
    /// production decisions, and it is what marks a stream as this process's
    /// own rather than a vendor's. A licensed feed filed through this door
    /// would lose its category and its standing, and a generated stream filed
    /// through either other door would gain a standing it never earned — so
    /// each door refuses the other's sources by construction.
    ///
    /// Costs nothing and is always available, and both are stated rather than
    /// taken as parameters: this process generated the bytes, so there was no
    /// vendor to charge or to be unreachable.
    pub fn of_generated(
        source: &SourceDescriptor,
        locator: impl Into<String>,
        symbols: impl IntoIterator<Item = String>,
        range: DataPeriod,
        schema: SourceSchema,
        bytes: &[u8],
        retrieved_at: Timestamp,
    ) -> Result<Self> {
        if source.licensing != LicensingClass::Synthetic {
            return Err(Error::denied(format!(
                "`{}` declares licensing class `{:?}`, not `Synthetic`; only a stream this \
                 platform generated may be referenced as generated. A vendor source is \
                 referenced through the catalogue door, after the licensing gate has admitted \
                 it, or not at all",
                source.name, source.licensing
            )));
        }
        Self::build(
            &source.name,
            SourceOrigin::Generated,
            None,
            locator.into(),
            symbols,
            range,
            schema,
            bytes,
            retrieved_at,
            Decimal::ZERO,
            1.0,
        )
    }

    /// The doors that hold the bytes hash them here; the invariants are
    /// checked in [`Self::build_hashed`].
    #[allow(clippy::too_many_arguments)]
    fn build(
        source_id: &str,
        origin: SourceOrigin,
        category: Option<SourceCategory>,
        locator: String,
        symbols: impl IntoIterator<Item = String>,
        range: DataPeriod,
        schema: SourceSchema,
        bytes: &[u8],
        retrieved_at: Timestamp,
        cost_estimate: Decimal,
        availability: f64,
    ) -> Result<Self> {
        if bytes.is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{source_id}` covers no bytes; the SHA-256 of nothing is \
                 the same for every extent that never arrived, so an empty extent is refused \
                 rather than hashed"
            )));
        }
        Self::build_hashed(
            source_id,
            origin,
            category,
            locator,
            symbols,
            range,
            schema,
            qip_core::sha256_hex(bytes),
            bytes.len() as u64,
            retrieved_at,
            cost_estimate,
            availability,
        )
    }

    /// The one place the invariants every door shares are checked.
    #[allow(clippy::too_many_arguments)]
    fn build_hashed(
        source_id: &str,
        origin: SourceOrigin,
        category: Option<SourceCategory>,
        locator: String,
        symbols: impl IntoIterator<Item = String>,
        range: DataPeriod,
        schema: SourceSchema,
        content_hash: String,
        bytes: u64,
        retrieved_at: Timestamp,
        cost_estimate: Decimal,
        availability: f64,
    ) -> Result<Self> {
        if locator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{source_id}` has no locator, so the specific extent \
                 fetched is not addressable and cannot be re-fetched to verify its hash"
            )));
        }
        let symbols: BTreeSet<String> = symbols.into_iter().collect();
        if symbols.is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{source_id}` names no symbols; a reference covering \
                 nothing identifiable cannot be checked for concentration risk against anything"
            )));
        }
        if bytes == 0 {
            return Err(Error::invalid(format!(
                "the data reference for `{source_id}` covers no bytes; a hash over nothing \
                 describes every extent that never arrived, so an empty extent is refused"
            )));
        }
        if cost_estimate.is_negative() {
            return Err(Error::invalid(
                "a negative cost estimate is a rebate, and no data source offers one",
            ));
        }
        if !availability.is_finite() || !(0.0..=1.0).contains(&availability) {
            return Err(Error::invalid(format!(
                "availability must be a fraction in [0, 1], not {availability}"
            )));
        }
        Ok(Self {
            source_id: source_id.to_string(),
            origin,
            category,
            locator,
            symbols,
            range,
            schema,
            content_hash,
            bytes,
            retrieved_at,
            cost_estimate,
            availability,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn origin(&self) -> SourceOrigin {
        self.origin
    }

    /// The §7.6.1 category, present for every source a vendor could be asked
    /// to serve again and absent only for [`SourceOrigin::Generated`].
    pub fn category(&self) -> Option<SourceCategory> {
        self.category
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn symbols(&self) -> &BTreeSet<String> {
        &self.symbols
    }

    pub fn range(&self) -> DataPeriod {
        self.range
    }

    /// The shape the data was in when it was used. §22.3 calls this
    /// `schema_version`; this crate already has a versioned shape in
    /// [`SourceSchema`] (its own fingerprint over field names and types), so
    /// the reference carries that rather than a second, looser notion of
    /// version.
    pub fn schema(&self) -> &SourceSchema {
        &self.schema
    }

    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn retrieved_at(&self) -> Timestamp {
        self.retrieved_at
    }

    pub fn cost_estimate(&self) -> Decimal {
        self.cost_estimate
    }

    pub fn availability(&self) -> f64 {
        self.availability
    }

    /// Whether a re-fetch producing `bytes` still matches what this reference
    /// recorded.
    pub fn verify(&self, bytes: &[u8]) -> RevisionCheck {
        self.compare_hash(qip_core::sha256_hex(bytes))
    }

    /// Whether a later reference to the same extent still hashes the same.
    ///
    /// For a ledger that keeps references and not bytes: the bytes a
    /// connector fetched are released to the loop and not retained, so the
    /// only thing left to compare a re-fetch against is the hash the earlier
    /// reference recorded — which is all §22.3 ever asked for.
    pub fn verify_against(&self, later: &Self) -> RevisionCheck {
        self.compare_hash(later.content_hash.clone())
    }

    fn compare_hash(&self, now: String) -> RevisionCheck {
        if now == self.content_hash {
            RevisionCheck::Unchanged
        } else {
            RevisionCheck::Revised {
                was: self.content_hash.clone(),
                now,
            }
        }
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
    use crate::admission::{self, CatalogueEntry};
    use crate::category::ContentSignal;
    use crate::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
    use crate::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
    use crate::finder::{DataFinder, FinderConfig};
    use crate::legal::{LicensingPosture, SourceLicense};
    use crate::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
    use crate::quality::SourceCost;
    use crate::registration::RegistrationRegistry;
    use crate::source::{SourceCandidate, SourceIdentity};
    use qip_contracts::governance::Usage;
    use qip_core::{Currency, Duration};
    use qip_events::Topic;
    use qip_financial::asset_class::AssetClass;
    use qip_market_ingestion::connectors::FrankfurterRatesConnector;

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

    /// A candidate that will probe cleanly and register on the surface web,
    /// with a declared content signal so §7.6.1 classifies it.
    fn registered_source(
        id: &str,
        url: &str,
        host: &str,
    ) -> Result<(RegisteredSource, DataFinder)> {
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
        assert!(
            decisions[0].is_registered(),
            "premise: the fixture candidate must register for this test to exercise a real \
             RegisteredSource, got {:?}",
            decisions[0].outcome()
        );
        let registered = finder.registered(id).expect("registered above").clone();
        Ok((registered, finder))
    }

    /// The shipped Frankfurter source, through the real catalogue and the
    /// real gate — the door a composition root uses.
    fn admitted_frankfurter() -> Result<AdmittedSource> {
        let manifest = FrankfurterRatesConnector::shipped_manifest()?;
        let decision = admission::admit(&manifest.source_id, manifest.licensing, now())?;
        AdmittedSource::from_decision(&decision, &manifest)
    }

    /// A `DataReference` cannot be built for a source with no recorded
    /// category — the precondition the module doc names.
    ///
    /// Mutated by deleting the `ok_or_else` refusal in `DataReference::of`
    /// (replacing `source.category()` handling with a default category) —
    /// confirmed this test then fails because the reference is built instead
    /// of refused, then restored.
    #[test]
    fn a_data_reference_cannot_be_built_for_a_source_with_no_recorded_category() -> Result<()> {
        let (registered, _finder) = registered_source(
            "filing-search-one",
            "https://example.gov/filings?entity=1",
            "example.gov",
        )?;

        // The refusal's other half: a candidate assessed with no declared
        // content signal registers, but is never categorised.
        let url = "https://uncategorised.example/data";
        let host = "uncategorised.example";
        let candidate = SourceCandidate::new(
            SourceIdentity::new("uncategorised", "Uncategorised Source", "Nobody")?,
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
        )?;
        // premise: no content signal was declared
        assert!(candidate.content_signal().is_none());

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
        assert!(decisions[0].is_registered(), "premise: candidate registers");
        let uncategorised = finder
            .registered("uncategorised")
            .expect("registered above");
        assert!(
            uncategorised.category().is_none(),
            "premise: an undeclared signal leaves the source uncategorised"
        );

        let error = DataReference::of(
            uncategorised,
            "https://uncategorised.example/data",
            ["ACME".to_string()],
            DataPeriod::instant(now()),
            uncategorised.source().schema().clone(),
            b"payload",
            now(),
            Decimal::ZERO,
            0.9,
        )
        .expect_err("a data reference was built for a source with no recorded category");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        assert!(
            error.message().contains("has no §7.6.1 category recorded"),
            "the refusal does not name the missing category: {error}"
        );

        // And the categorised fixture from the top of the file does build,
        // proving the refusal above is about the missing category and not
        // some other input.
        let built = DataReference::of(
            &registered,
            "https://example.gov/filings?entity=1",
            ["ACME".to_string()],
            DataPeriod::instant(now()),
            registered.source().schema().clone(),
            b"payload",
            now(),
            Decimal::ZERO,
            0.9,
        )?;
        assert_eq!(built.category(), Some(SourceCategory::RegulatoryAndLegal));
        assert_eq!(built.origin(), SourceOrigin::Discovered);
        Ok(())
    }

    /// A re-fetch producing the same bytes is unchanged; one producing
    /// different bytes is flagged as revised, naming both hashes.
    ///
    /// Mutated by replacing `RevisionCheck::Revised` with
    /// `RevisionCheck::Unchanged` unconditionally in `compare_hash` —
    /// confirmed the revised-bytes half of this test then fails, then
    /// restored.
    #[test]
    fn a_content_hash_mismatch_on_re_fetch_is_detected() -> Result<()> {
        let (registered, _finder) = registered_source(
            "filing-search-two",
            "https://example.gov/filings?entity=1",
            "example.gov",
        )?;
        let reference = DataReference::of(
            &registered,
            "https://example.gov/filings?entity=1",
            ["ACME".to_string()],
            DataPeriod::instant(now()),
            registered.source().schema().clone(),
            b"filing version one",
            now(),
            Decimal::ZERO,
            0.9,
        )?;

        assert_eq!(
            reference.verify(b"filing version one"),
            RevisionCheck::Unchanged
        );

        let revised = reference.verify(b"filing version two, revised");
        assert!(
            revised.is_revised(),
            "a changed extent was not detected as revised"
        );
        match &revised {
            RevisionCheck::Revised { was, now } => {
                assert_eq!(was, reference.content_hash());
                assert_ne!(
                    now, was,
                    "the revised hash must differ from the recorded one"
                );
            }
            RevisionCheck::Unchanged => panic!("a changed extent reported unchanged"),
        }

        // The ledger's form of the same question: a later reference to the
        // same extent, compared hash to hash with no bytes retained.
        let later = DataReference::of(
            &registered,
            "https://example.gov/filings?entity=1",
            ["ACME".to_string()],
            DataPeriod::instant(now()),
            registered.source().schema().clone(),
            b"filing version two, revised",
            now(),
            Decimal::ZERO,
            0.9,
        )?;
        assert_eq!(reference.verify_against(&later), revised);
        Ok(())
    }

    /// `DataPeriod` refuses an inverted span, admits an instant, and treats a
    /// shared boundary instant as an overlap.
    #[test]
    fn a_data_period_refuses_an_end_before_its_start() {
        let start = now();
        let end = start.saturating_sub(Duration::from_secs(1));
        let error =
            DataPeriod::new(start, end).expect_err("a period ending before it starts was accepted");
        assert_eq!(error.code(), "invalid", "got {error:?}");

        let instant = DataPeriod::instant(start);
        assert!(instant.contains(start));
        assert!(!instant.contains(start.saturating_add(Duration::from_secs(1))));

        let later = start.saturating_add(Duration::from_secs(10));
        let first = DataPeriod::new(start, later).expect("a forward span");
        let touching = DataPeriod::instant(later);
        let beyond = DataPeriod::instant(later.saturating_add(Duration::from_secs(1)));
        assert!(first.overlaps(&touching), "a shared boundary is an overlap");
        assert!(!first.overlaps(&beyond));
    }

    /// The catalogue door: a source the real gate admitted, with the category
    /// its shipped manifest declares, builds a reference whose origin says
    /// which door it came through.
    ///
    /// Mutated by changing `SourceOrigin::CatalogueAdmitted` to
    /// `SourceOrigin::Discovered` in `of_admitted` — confirmed this test then
    /// fails on the origin, then restored.
    #[test]
    fn a_catalogue_admitted_source_builds_a_reference_that_names_its_door() -> Result<()> {
        let admitted = admitted_frankfurter()?;
        assert_eq!(
            admitted.category(),
            SourceCategory::GovernmentAndTrade,
            "premise: the shipped manifest declares the ECB rates as government and trade data"
        );
        let reference = DataReference::of_admitted(
            &admitted,
            admitted.endpoint(),
            ["USD".to_string(), "GBP".to_string()],
            DataPeriod::instant(now()),
            br#"{"base":"EUR","date":"2026-09-04","rates":{"GBP":0.85898,"USD":1.1622}}"#,
            now(),
            Decimal::ZERO,
            1.0,
        )?;
        assert_eq!(reference.origin(), SourceOrigin::CatalogueAdmitted);
        assert_eq!(
            reference.category(),
            Some(SourceCategory::GovernmentAndTrade)
        );
        assert_eq!(reference.source_id(), "frankfurter-ecb-reference-rates");
        assert_eq!(
            reference.schema().fingerprint(),
            admitted.schema().fingerprint(),
            "the reference must carry the shape the manifest declared, not a second one"
        );
        assert!(reference.bytes() > 0);
        Ok(())
    }

    /// The licensing gate is the only way through the catalogue door: a
    /// source whose terms are unread never yields a `LicensingDecision`, so
    /// no `AdmittedSource` and no reference can exist for it. Proven against
    /// the real catalogue's own refusal of Kalshi, whose posture is
    /// `Ambiguous`, and against a research-only licence the real catalogue
    /// must never contain.
    ///
    /// Mutated by making `admit_from_registered` skip the usage loop
    /// (`for usage in [] as [Usage; 0] {}`) — confirmed the research-only
    /// half then fails because a decision is minted for a licence that never
    /// granted `Trade`, then restored. The Kalshi half stays refused under
    /// that mutation, and deliberately so: an `Ambiguous` posture has no
    /// licence to name, and the gate's last check refuses a decision it
    /// cannot attribute to one. Two refusals for two different reasons is
    /// the point of asserting both.
    #[test]
    fn a_source_the_licensing_gate_refuses_cannot_reach_the_catalogue_door() -> Result<()> {
        let kalshi = qip_market_ingestion::connectors::KalshiMarketsConnector::shipped_manifest()?;
        assert!(
            kalshi.category.is_some(),
            "premise: the refusal below must be about the licence, not a missing category"
        );
        let refused = admission::admit(&kalshi.source_id, kalshi.licensing, now())
            .expect_err("a source whose terms are unread was admitted");
        assert_eq!(refused.code(), "denied", "got {refused:?}");

        // A research-only licence for a real connector: every usage question
        // but `Research` answers `forbidden`, so the gate refuses and nothing
        // downstream can be built.
        let manifest = FrankfurterRatesConnector::shipped_manifest()?;
        let research_only = vec![CatalogueEntry {
            source_id: "frankfurter-ecb-reference-rates",
            expected_class: manifest.licensing,
            posture: LicensingPosture::declared(SourceLicense::new(
                "research-only-for-this-test",
                [Usage::Research],
            )?),
        }];
        let refused = admission::admit_from_registered(
            &research_only,
            &RegistrationRegistry::shipped(),
            &manifest.source_id,
            manifest.licensing,
            now(),
        )
        .expect_err("a research-only licence admitted a source onto the trading path");
        assert_eq!(refused.code(), "denied", "got {refused:?}");
        Ok(())
    }

    /// The other half of the door's honesty: a manifest that declares no
    /// category, or whose class disagrees with the decision, or that is
    /// synthetic, is refused even with a genuine decision in hand.
    ///
    /// Mutated by replacing the `manifest.category.ok_or_else(...)` refusal
    /// with `.unwrap_or(SourceCategory::Marketplace)` — confirmed the
    /// undeclared-category half then fails, then restored.
    #[test]
    fn an_admitted_source_refuses_a_manifest_it_cannot_honestly_describe() -> Result<()> {
        let manifest = FrankfurterRatesConnector::shipped_manifest()?;
        let decision = admission::admit(&manifest.source_id, manifest.licensing, now())?;

        let mut undeclared = manifest.clone();
        undeclared.category = None;
        let error = AdmittedSource::from_decision(&decision, &undeclared)
            .expect_err("a manifest with no declared category was admitted as a reference source");
        assert!(
            error.message().contains("declares no §7.6.1 category"),
            "the refusal does not name the missing declaration: {error}"
        );

        let mut disagreeing = manifest.clone();
        disagreeing.licensing = LicensingClass::Internal;
        let error = AdmittedSource::from_decision(&decision, &disagreeing)
            .expect_err("a class disagreement was admitted");
        assert_eq!(error.code(), "denied", "got {error:?}");

        let mut synthetic = manifest.clone();
        synthetic.licensing = LicensingClass::Synthetic;
        assert!(
            AdmittedSource::from_decision(&decision, &synthetic).is_err(),
            "a synthetic manifest was admitted through the catalogue door"
        );

        let mut other = manifest;
        other.source_id = "coinbase-spot-ticker".to_string();
        assert!(
            AdmittedSource::from_decision(&decision, &other).is_err(),
            "a decision about one source admitted a manifest for another"
        );
        Ok(())
    }

    /// The generated door admits only a synthetic stream and refuses a
    /// licensed one, and a reference through it carries no category.
    ///
    /// Mutated by inverting the `licensing != Synthetic` check — confirmed
    /// both halves then fail, then restored.
    #[test]
    fn the_generated_door_admits_only_a_synthetic_stream() -> Result<()> {
        let synthetic = SourceDescriptor {
            name: "synthetic-exchange".to_string(),
            provider: "this process".to_string(),
            licensing: LicensingClass::Synthetic,
            topics: vec![Topic::MarketBar],
            expected_latency: Duration::ZERO,
            production_requirement: None,
        };
        let reference = DataReference::of_generated(
            &synthetic,
            "bars://synthetic-exchange/AAA?interval=1m",
            ["AAA".to_string()],
            DataPeriod::instant(now()),
            SourceSchema::from_fields([("close".to_string(), crate::schema::FieldType::Number)]),
            b"[{\"close\":\"100\"}]",
            now(),
        )?;
        assert_eq!(reference.origin(), SourceOrigin::Generated);
        assert_eq!(reference.category(), None);
        assert_eq!(reference.cost_estimate(), Decimal::ZERO);

        let licensed = SourceDescriptor {
            licensing: LicensingClass::Licensed,
            ..synthetic
        };
        let error = DataReference::of_generated(
            &licensed,
            "bars://vendor/AAA",
            ["AAA".to_string()],
            DataPeriod::instant(now()),
            SourceSchema::from_fields([]),
            b"[]",
            now(),
        )
        .expect_err("a licensed feed was referenced as generated");
        assert_eq!(error.code(), "denied", "got {error:?}");
        Ok(())
    }
}
