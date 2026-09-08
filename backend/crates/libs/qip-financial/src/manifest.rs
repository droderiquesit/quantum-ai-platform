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
        }
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
}
