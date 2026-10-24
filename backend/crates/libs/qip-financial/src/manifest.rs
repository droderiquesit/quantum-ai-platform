//! What the platform keeps instead of a document it ingested.
//!
//! Blueprint §7.2 divides an ingested document in two. Retained: the entities
//! it named, the facts it established, the causal edges its evidence supported,
//! and "a manifest pointing at the original, with a content hash". Discarded:
//! the article text itself. §56.4 states the same rule twice as a numbered
//! constraint — rule 34, "external history is referenced by manifest with a
//! content hash, never copied into permanent storage", and rule 36, "ingested
//! source text is not retained. Facts, entity links and a manifest with a hash
//! are."
//!
//! Until this type existed the platform had the first half of that discipline
//! for exactly one artefact — [`crate::catalogue::CatalogueManifest`] hashes
//! the instrument catalogue — and none of it for documents. A
//! [`crate::intelligence::NewsItem`] is an [`qip_events::EventBody`]: every
//! news item this platform ingested was copied verbatim into the hash-chained
//! event log, which is permanent by construction and sealed against edit. A
//! vendor's `Restricted` text — licensed, non-displayable, somebody else's
//! copyrighted expression — went in with it and could never come out.
//!
//! # Why the hash and not just the address
//!
//! An address alone makes the reference unfalsifiable. A vendor that edits a
//! filing in place, keeps its id and keeps its timestamp is indistinguishable
//! from one that never edited it — the failure
//! [`crate::intelligence::FundamentalUpdate::is_restatement`] exists to make
//! *stated* revisions visible, and which no stated field can catch when the
//! vendor says nothing. The hash is what turns "we saw this document" into a
//! claim a re-fetch can contradict. Without it a manifest asserts only that
//! something once lived at a URL.
//!
//! # What a manifest deliberately cannot do
//!
//! It does not fetch. `qip-financial` is a library and performs no I/O; a
//! manifest is a record, and re-reading the original is the caller's business
//! and the vendor's terms'. It also does not carry a byte offset: the extent
//! this platform references is always a whole document, because a partial
//! fetch of a filing is not a shorter filing, it is a different one — the same
//! argument the narrative adapter's per-document cap makes when it refuses an
//! oversized document rather than truncating it.

use qip_contracts::governance::Entitlement;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The source a manifest names when the text was generated in this process.
///
/// A distinct constant rather than the generator's own name, so that nothing
/// downstream can mistake a synthetic extent for one a vendor could be asked
/// to serve again. [`crate::quality::LicensingClass::Synthetic`] already keeps
/// these records out of production decisions; this keeps them out of the set
/// of things a re-fetch could check.
pub const GENERATED_SOURCE: &str = "generated";

/// A pointer to an original this platform read and did not keep.
///
/// Fields are private and there is one constructor, so a manifest cannot be
/// assembled in this process with a hash that does not belong to the bytes it
/// claims to describe. Deserialization is the exception and is deliberate: a
/// manifest read back out of the event log is a record of what was sealed, and
/// re-deriving it would be inventing a fact rather than replaying one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceManifest {
    /// The feed that served the original, spelled as
    /// [`crate::quality::Provenance::source`] spells it, so a record and its
    /// manifest cannot name two different origins.
    source: String,
    /// Where the original stays addressable. Opaque to this type: only the
    /// feed that wrote it knows how to resolve it.
    locator: String,
    /// Length of the referenced extent, in bytes.
    bytes: u64,
    /// SHA-256 of exactly those bytes, lowercase hex.
    sha256: String,
    /// When those bytes were read — not when the document was published. A
    /// re-fetch that disagrees disagrees as of this instant.
    retrieved_at: Timestamp,
    /// Which shape this record was written in. A sealed version-1 record has
    /// no such field, so it is read back as 1 and, being 1, is written back
    /// without it: replaying a v1 manifest reproduces the bytes that were
    /// sealed, which a field added unconditionally would not.
    #[serde(default = "version_one", skip_serializing_if = "is_version_one")]
    schema_version: u8,
    /// The six fields version 2 adds. `None` is *not recorded* — what every
    /// sealed v1 record reads as — and is a different statement from a field
    /// that was recorded and empty, which [`ManifestV2::new`] refuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v2: Option<ManifestV2>,
}

const fn version_one() -> u8 {
    1
}

const fn is_version_one(version: &u8) -> bool {
    *version == 1
}

/// How a re-fetch of the original is to be made, as a typed instruction
/// rather than a URL template a caller would have to interpret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refetch {
    /// Read the manifest's own locator again and compare hashes.
    Locator,
    /// Resume the feed from [`ManifestV2::cursor`]. Only constructible with a
    /// cursor present, because a resume with nowhere to resume from is a full
    /// re-read wearing a cheaper name.
    FromCursor,
    /// The original cannot be served again, and the record says why instead
    /// of leaving a later reader to assume it can.
    NotRefetchable { reason: String },
}

/// The fields ADR 0132 decision 3 adds in manifest version 2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestV2 {
    /// The source's own version tag. Absent where the source has none; never
    /// derived from the hash, which would pass this platform's fact off as the
    /// source's.
    etag: Option<String>,
    /// What the licence says this platform may do with the bytes.
    entitlement: Entitlement,
    /// The opaque position the feed resumes from.
    cursor: Option<String>,
    /// The instant after which the bytes must not be served without a re-fetch.
    fresh_until: Timestamp,
    /// Name and version of the code that parsed the bytes.
    parser_name: String,
    parser_version: String,
    /// How to read the original again.
    refetch: Refetch,
}

impl ManifestV2 {
    /// Assemble the six fields, refusing each that would read as recorded and
    /// say nothing: a blank etag or cursor (absence is `None`), a parser with
    /// no name or version (lineage nobody can replay), a freshness bound that
    /// is not after `retrieved_at` (bytes born stale), a resume instruction
    /// with no cursor, and a `NotRefetchable` with no reason.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        etag: Option<String>,
        entitlement: Entitlement,
        cursor: Option<String>,
        fresh_until: Timestamp,
        parser_name: impl Into<String>,
        parser_version: impl Into<String>,
        refetch: Refetch,
    ) -> Result<Self> {
        let parser_name = parser_name.into();
        let parser_version = parser_version.into();
        if etag.as_deref().is_some_and(|e| e.trim().is_empty()) {
            return Err(Error::invalid(
                "a blank etag is a version tag nobody issued; pass None when the source has none",
            ));
        }
        if cursor.as_deref().is_some_and(|c| c.trim().is_empty()) {
            return Err(Error::invalid(
                "a blank cursor resumes from nowhere; pass None when the feed has no position",
            ));
        }
        if parser_name.trim().is_empty() || parser_version.trim().is_empty() {
            return Err(Error::invalid(
                "parser lineage needs both the parser's name and its version, or a replay cannot                  tell which code read these bytes",
            ));
        }
        match &refetch {
            Refetch::FromCursor if cursor.is_none() => {
                return Err(Error::invalid(
                    "a FromCursor re-fetch needs the cursor it resumes from; use Locator to read                      the whole original again",
                ));
            }
            Refetch::NotRefetchable { reason } if reason.trim().is_empty() => {
                return Err(Error::invalid(
                    "a NotRefetchable manifest must say why, so no one assumes it can be served                      again",
                ));
            }
            _ => {}
        }
        Ok(Self {
            etag,
            entitlement,
            cursor,
            fresh_until,
            parser_name,
            parser_version,
            refetch,
        })
    }

    /// The source's version tag, when it has one.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// The licence the bytes were read under.
    pub fn entitlement(&self) -> &Entitlement {
        &self.entitlement
    }

    /// The feed's resume position, when it has one.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// The instant after which the bytes must be re-fetched before use.
    pub fn fresh_until(&self) -> Timestamp {
        self.fresh_until
    }

    /// The parser's name and version.
    pub fn parser(&self) -> (&str, &str) {
        (&self.parser_name, &self.parser_version)
    }

    /// How to read the original again.
    pub fn refetch(&self) -> &Refetch {
        &self.refetch
    }
}

impl SourceManifest {
    /// Describe `bytes` as read from `source` at `locator`.
    ///
    /// Refuses rather than defaults on all three inputs, because each empty
    /// value produces a manifest that reads as a reference and is not one: no
    /// source means nothing can be asked to serve it again, no locator means
    /// there is no original to point at, and no bytes means the hash is
    /// `e3b0c442...` — the SHA-256 of the empty input, which is the same for
    /// every document nobody read and would make "we hold a manifest for it"
    /// true of a document that never arrived.
    pub fn of(
        source: impl Into<String>,
        locator: impl Into<String>,
        retrieved_at: Timestamp,
        bytes: &[u8],
    ) -> Result<Self> {
        let source = source.into();
        let locator = locator.into();
        if source.trim().is_empty() {
            return Err(Error::invalid(
                "a source manifest with no source names nothing that could serve the original \
                 again; give it the feed's provenance source",
            ));
        }
        if locator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the manifest for a document from {source} has no locator, so the original is not \
                 addressable and referencing it instead of copying it would lose it; state where \
                 the document stays retrievable"
            )));
        }
        if bytes.is_empty() {
            return Err(Error::invalid(format!(
                "the manifest for {locator} covers no bytes; the SHA-256 of nothing is the same \
                 for every document that never arrived, so an empty extent is refused rather \
                 than hashed"
            )));
        }
        Ok(Self {
            source,
            locator,
            bytes: bytes.len() as u64,
            sha256: qip_core::sha256_hex(bytes),
            retrieved_at,
            schema_version: 1,
            v2: None,
        })
    }

    /// The manifest for text this platform generated rather than fetched.
    ///
    /// The synthetic streams exist to exercise the pipeline end to end, and a
    /// generated story written into the event log is exactly as permanent as a
    /// vendor's. So generated text is referenced on the same terms, with the
    /// one real difference stated in the record instead of left for a reader
    /// to infer: the source is [`GENERATED_SOURCE`] and not a feed, because
    /// there is no vendor to ask for it again. What reproduces it is the
    /// generator and its seed, which is what the locator names.
    ///
    /// Infallible where [`SourceManifest::of`] refuses, and structurally so
    /// rather than by assertion: the source is a constant, the locator carries
    /// a constant prefix, and the hashed extent carries two separators, so
    /// none of the three empty inputs `of` refuses is reachable from any
    /// argument. That matters because the synthetic generator's `step` is
    /// infallible by design — a `Result` here would be unwrapped, or would
    /// make a generated story silently droppable, and both are worse than a
    /// constructor that cannot fail.
    pub fn generated(generator: &str, item_id: &str, at: Timestamp, text: &str) -> Self {
        let extent = format!("{generator}\n{item_id}\n{text}");
        Self {
            source: GENERATED_SOURCE.to_string(),
            locator: format!("{GENERATED_SOURCE}:{generator}#{item_id}"),
            bytes: extent.len() as u64,
            sha256: qip_core::sha256_hex(extent.as_bytes()),
            retrieved_at: at,
            schema_version: 1,
            v2: None,
        }
    }

    /// Upgrade to version 2 by attaching the six fields ADR 0132 adds.
    ///
    /// Refuses a freshness bound that is not after `retrieved_at`, which needs
    /// both halves and so cannot live in [`ManifestV2::new`]. Consumes `self`
    /// so a v1 record sealed in the log is never mutated, only superseded by a
    /// new value.
    pub fn with_v2(mut self, v2: ManifestV2) -> Result<Self> {
        if v2.fresh_until <= self.retrieved_at {
            return Err(Error::invalid(
                "fresh_until must be after retrieved_at; bytes that are stale when read are not \
                 a fresh extent",
            ));
        }
        self.schema_version = 2;
        self.v2 = Some(v2);
        Ok(self)
    }

    /// 1 for a record written before the six fields existed, 2 after.
    pub fn schema_version(&self) -> u8 {
        self.schema_version
    }

    /// The six version-2 fields; `None` means *not recorded*, as every v1
    /// record reads, and never means empty.
    pub fn v2(&self) -> Option<&ManifestV2> {
        self.v2.as_ref()
    }

    /// The feed that served the original.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where the original stays addressable.
    pub fn locator(&self) -> &str {
        &self.locator
    }

    /// Length of the referenced extent, in bytes.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// SHA-256 of the referenced extent, lowercase hex.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// When the extent was read.
    pub fn retrieved_at(&self) -> Timestamp {
        self.retrieved_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    #[test]
    fn the_hash_is_of_the_bytes_that_were_read() {
        let manifest = SourceManifest::of("wire", "https://vendor/doc/1", at(), b"abc")
            .expect("three bytes from a named feed is a manifest");
        assert_eq!(
            manifest.sha256(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "FIPS 180-4's own \"abc\" vector; a hash of anything else is a hash of the wrong bytes"
        );
        assert_eq!(manifest.bytes(), 3);
        assert_ne!(
            manifest.sha256(),
            qip_core::sha256_hex(b"abd"),
            "a document edited in place must not hash to what the manifest recorded, or the \
             manifest would make an edit unfalsifiable"
        );
    }

    /// An empty extent is refused rather than hashed.
    ///
    /// The SHA-256 of nothing is `e3b0c442…` for every input, so a manifest
    /// over zero bytes is the same manifest for a document that arrived and
    /// one that never did — and it reads downstream as a reference either way.
    /// This is the arm most likely to be "fixed" by someone who meets a feed
    /// that serves empty documents; it must stay a refusal.
    #[test]
    fn a_manifest_over_no_bytes_is_refused_because_the_hash_of_nothing_says_nothing() {
        let error = SourceManifest::of("wire", "http://vendor/doc/1", at(), b"")
            .expect_err("a manifest was built over an empty extent");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        assert!(
            error.message().contains("never arrived"),
            "the refusal must say what an empty hash would claim: {error}"
        );
    }

    /// A manifest with no locator is a reference to nothing.
    ///
    /// Discarding the text is only safe because the original stays
    /// addressable. A manifest that hashes bytes and names nowhere to find
    /// them again is not a reference; it is a deletion with a receipt.
    #[test]
    fn a_manifest_with_no_locator_is_refused_because_referencing_would_then_be_deleting() {
        let error = SourceManifest::of("wire", "   ", at(), b"abc")
            .expect_err("a manifest was built with a blank locator");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        assert!(
            error.message().contains("addressable"),
            "the refusal must name what is missing: {error}"
        );

        let no_source = SourceManifest::of(" ", "http://vendor/doc/1", at(), b"abc")
            .expect_err("a manifest was built with a blank source");
        assert_eq!(no_source.code(), "invalid", "got {no_source:?}");
    }

    /// Generated text is referenced on the same terms, and says so.
    ///
    /// The synthetic streams write `NewsItem`s into the same event log, so a
    /// generated story is as permanent as a vendor's. What must differ is the
    /// claim: `GENERATED_SOURCE` and not a feed name, because nothing can be
    /// asked to serve it again, and a re-fetch check against it would be
    /// checking a fetch that cannot happen.
    #[test]
    fn a_generated_manifest_names_the_generator_and_never_a_feed_that_could_be_re_fetched() {
        let manifest = SourceManifest::generated("synthetic-news", "routine-1", at(), "text");
        assert_eq!(manifest.source(), GENERATED_SOURCE);
        assert_ne!(
            manifest.source(),
            "synthetic-news",
            "the generator's name belongs in the locator; putting it in `source` would make a \
             generated extent look like a vendor's"
        );
        assert_eq!(manifest.locator(), "generated:synthetic-news#routine-1");
        assert!(
            manifest.bytes() > 0 && manifest.sha256().len() == 64,
            "the three empty cases `of` refuses must be unreachable here, whatever the arguments"
        );

        // The arm that keeps it infallible: even with everything blank the
        // separators make the extent non-empty and the constant makes the
        // locator non-empty, so there is no argument for which this
        // constructor would need to refuse.
        let degenerate = SourceManifest::generated("", "", at(), "");
        assert_eq!(degenerate.bytes(), 2, "two separators and nothing else");
        assert_eq!(degenerate.source(), GENERATED_SOURCE);
    }

    fn v2_fields() -> ManifestV2 {
        ManifestV2::new(
            Some("W/\"v7\"".to_string()),
            Entitlement::Granted {
                dataset: "wire".to_string(),
                usage: qip_contracts::governance::Usage::Research,
                expires_at: Timestamp::from_millis(1_800_000_000_000),
            },
            Some("page-42".to_string()),
            Timestamp::from_millis(1_700_000_060_000),
            "wire-parser",
            "3.1.0",
            Refetch::FromCursor,
        )
        .expect("six well-formed fields are a version-2 record")
    }

    fn v2_manifest() -> SourceManifest {
        SourceManifest::of("wire", "https://vendor/doc/1", at(), b"abc")
            .and_then(|m| m.with_v2(v2_fields()))
            .expect("a manifest with fresh bytes upgrades")
    }

    /// A round trip that drops any of the six fields must fail here.
    ///
    /// Each field is compared individually after the trip, so a serde
    /// attribute that skips one (the way `skip_serializing_if` skips the
    /// whole block on v1) reads as a named field gone and not as a vague
    /// inequality.
    #[test]
    fn a_version_two_manifest_round_trips_with_every_one_of_its_six_fields() {
        let manifest = v2_manifest();
        assert_eq!(manifest.schema_version(), 2, "premise: this is a v2 record");
        let json = serde_json::to_string(&manifest).expect("a manifest serializes");
        let back: SourceManifest = serde_json::from_str(&json).expect("and reads back");
        let (before, after) = (
            manifest.v2().expect("premise: v2 fields present"),
            back.v2().expect("the v2 block survived the trip"),
        );
        assert_eq!(after.etag(), before.etag(), "etag dropped");
        assert_eq!(
            after.entitlement(),
            before.entitlement(),
            "entitlement dropped"
        );
        assert_eq!(after.cursor(), before.cursor(), "cursor dropped");
        assert_eq!(
            after.fresh_until(),
            before.fresh_until(),
            "fresh_until dropped"
        );
        assert_eq!(
            after.parser(),
            ("wire-parser", "3.1.0"),
            "parser lineage dropped"
        );
        assert_eq!(after.refetch(), &Refetch::FromCursor, "refetch dropped");
        assert_eq!(back, manifest);
    }

    /// A manifest sealed before version 2 existed stays readable and says
    /// *not recorded* — and writes back as the bytes that were sealed.
    #[test]
    fn a_sealed_version_one_manifest_still_decodes_reads_the_six_as_not_recorded_and_re_encodes_identically()
     {
        let sealed = serde_json::to_string(
            &SourceManifest::of("wire", "https://vendor/doc/1", at(), b"abc").expect("v1"),
        )
        .expect("serializes");
        assert!(
            !sealed.contains("schema_version") && !sealed.contains("v2"),
            "premise: a v1 record carries neither new key, as one sealed before them would: {sealed}"
        );
        let decoded: SourceManifest = serde_json::from_str(&sealed).expect("v1 stays readable");
        assert_eq!(decoded.schema_version(), 1);
        assert!(
            decoded.v2().is_none(),
            "not recorded is None, never an empty field set"
        );
        assert_eq!(
            serde_json::to_string(&decoded).expect("re-encodes"),
            sealed,
            "replaying a sealed record must not change its bytes"
        );
    }

    #[test]
    fn a_version_two_field_that_would_read_as_recorded_and_say_nothing_is_refused() {
        let granted = || Entitlement::Granted {
            dataset: "wire".to_string(),
            usage: qip_contracts::governance::Usage::Research,
            expires_at: Timestamp::from_millis(1_800_000_000_000),
        };
        let fresh = Timestamp::from_millis(1_700_000_060_000);
        let new = |etag: Option<&str>, cursor: Option<&str>, name: &str, version: &str, refetch| {
            ManifestV2::new(
                etag.map(str::to_string),
                granted(),
                cursor.map(str::to_string),
                fresh,
                name,
                version,
                refetch,
            )
        };
        assert!(
            new(Some(" "), None, "p", "1", Refetch::Locator).is_err(),
            "blank etag"
        );
        assert!(
            new(None, Some(""), "p", "1", Refetch::Locator).is_err(),
            "blank cursor"
        );
        assert!(
            new(None, None, "", "1", Refetch::Locator).is_err(),
            "no parser name"
        );
        assert!(
            new(None, None, "p", " ", Refetch::Locator).is_err(),
            "no parser version"
        );
        assert!(
            new(None, None, "p", "1", Refetch::FromCursor).is_err(),
            "resume with no cursor"
        );
        assert!(
            new(
                None,
                None,
                "p",
                "1",
                Refetch::NotRefetchable { reason: " ".into() }
            )
            .is_err(),
            "unexplained NotRefetchable"
        );
        assert!(
            new(None, None, "p", "1", Refetch::Locator).is_ok(),
            "premise: the same call with honest inputs is admitted, so the refusals above are about the inputs"
        );

        let stale = ManifestV2::new(None, granted(), None, at(), "p", "1", Refetch::Locator)
            .expect("fields alone are well-formed");
        let error = SourceManifest::of("wire", "https://vendor/doc/1", at(), b"abc")
            .and_then(|m| m.with_v2(stale))
            .expect_err("bytes fresh only until the instant they were read are stale");
        assert_eq!(error.code(), "invalid", "got {error:?}");
    }
}
