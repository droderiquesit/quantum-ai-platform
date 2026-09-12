//! What was actually fetched, from where, in what shape, and whether it still
//! matches — as distinct from a [`crate::decision::RegisteredSource`], which
//! only says a source *may* be used.
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
//! history." [`DataReference::of`] computes that hash with
//! `qip_core::sha256_hex` — the exact mechanism
//! [`qip_financial::manifest::SourceManifest`] already uses for §7.2's
//! content-hashed manifest, reused rather than reinvented, because a second
//! hashing scheme for the same claim ("we hold a reference to what these
//! bytes were") is exactly the kind of second source of truth
//! `.claude/rules/architecture/00-boundaries.md` forbids.
//!
//! A `DataReference` cannot be built without a `SourceCategory` already
//! recorded on the `RegisteredSource` it names: [`DataReference::of`] takes a
//! `&RegisteredSource` and refuses one with no category, so a reference can
//! always say what *kind* of source it came from — §7.6.1 landing before this
//! type is why the category is a precondition rather than an optional field.

use crate::category::SourceCategory;
use crate::decision::RegisteredSource;
use crate::schema::SourceSchema;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The period of the underlying world a fetch covered — not when it was
/// fetched (that is [`DataReference::retrieved_at`]), but what instants its
/// records describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// A pointer to what was actually fetched: the source and its §7.6.1
/// category, the address, the symbols and period covered, the shape it was
/// in, and what it cost — everything a research scheduler or an audit needs
/// to answer "what did we use, from where, and does it still say that".
///
/// Fields are private with one constructor. `DataReference::of` never
/// fetches — this crate performs no I/O — so the bytes it hashes are always
/// the caller's own read, exactly as
/// [`qip_financial::manifest::SourceManifest::of`] takes bytes the caller
/// already has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataReference {
    source_id: String,
    category: SourceCategory,
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
    retrieved_at: Timestamp,
    cost_estimate: Decimal,
    /// The source's known availability at the time of this fetch, in
    /// `[0, 1]` — carried so a research scheduler can weigh a reference from
    /// an unreliable source without re-deriving `SourceHealth` from scratch.
    availability: f64,
}

impl DataReference {
    /// Describe what was fetched from `source`.
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
        let locator = locator.into();
        if locator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{}` has no locator, so the specific extent fetched is \
                 not addressable and cannot be re-fetched to verify its hash",
                source.id()
            )));
        }
        let symbols: BTreeSet<String> = symbols.into_iter().collect();
        if symbols.is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{}` names no symbols; a reference covering nothing \
                 identifiable cannot be checked for concentration risk against anything",
                source.id()
            )));
        }
        if bytes.is_empty() {
            return Err(Error::invalid(format!(
                "the data reference for `{}` covers no bytes; the SHA-256 of nothing is the \
                 same for every extent that never arrived, so an empty extent is refused rather \
                 than hashed",
                source.id()
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
            source_id: source.id().to_string(),
            category,
            locator,
            symbols,
            range,
            schema,
            content_hash: qip_core::sha256_hex(bytes),
            retrieved_at,
            cost_estimate,
            availability,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn category(&self) -> SourceCategory {
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
        let now = qip_core::sha256_hex(bytes);
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
    use crate::category::ContentSignal;
    use crate::coverage::{SourceCoverage, SourceRegion, UpdateFrequency};
    use crate::endpoint::{AccessMechanism, AuthRequirement, SourceEndpoint};
    use crate::finder::{DataFinder, FinderConfig};
    use crate::legal::{LicensingPosture, SourceLicense};
    use crate::probe::{HeadResponse, InMemoryProbe, PayloadSample, RobotsFetch};
    use crate::quality::SourceCost;
    use crate::source::{SourceCandidate, SourceIdentity};
    use qip_contracts::governance::Usage;
    use qip_core::{Currency, Duration};
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
        assert_eq!(built.category(), SourceCategory::RegulatoryAndLegal);
        Ok(())
    }

    /// A re-fetch producing the same bytes is unchanged; one producing
    /// different bytes is flagged as revised, naming both hashes.
    ///
    /// Mutated by replacing `RevisionCheck::Revised` with
    /// `RevisionCheck::Unchanged` unconditionally in `DataReference::verify`
    /// — confirmed the revised-bytes half of this test then fails, then
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
        Ok(())
    }

    /// `DataPeriod` refuses an inverted span, and admits an instant.
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
    }
}
