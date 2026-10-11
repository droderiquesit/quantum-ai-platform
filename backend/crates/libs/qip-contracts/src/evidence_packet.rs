//! `EvidencePacket` (blueprint v12.0 §27, R12-CONTRACT-7062): one claim, as
//! one brain hands it to another, with everything needed to weigh it later.
//!
//! The failure this prevents is evidence that loses its provenance in
//! transit: a claim that arrives without its source, its licence, its content
//! hash or the claims it contradicts is a claim the receiver can only believe
//! or ignore, never audit. Every one of those is a required field here, and a
//! packet missing one is refused on construction and on decode alike.

use crate::governance::Usage;
use crate::validated::{
    Contract, Validated, basis_points, digest, each_named, named, names, refuse,
};
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

/// Where the claim is about. `Unlocated` is a statement the producer makes —
/// "this claim has no place" — not a missing value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Location {
    Unlocated,
    Place { name: String },
}

/// Where the claim came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    /// The Source Registry id.
    pub source_id: String,
    /// The locator the bytes were fetched from.
    pub locator: String,
    /// When the bytes were fetched — the instant the claim became knowable.
    pub retrieved_at: Timestamp,
}

/// The licence the claim is held under. `permitted` is the set of uses the
/// licence grants, so a receiver can refuse a use rather than assume one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenceTerms {
    pub licence_id: String,
    pub permitted: Vec<Usage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePacketFields {
    pub schema_version: u32,
    pub packet_id: String,
    /// The proposition the evidence supports, in one sentence.
    pub claim: String,
    /// The entities the claim is about; at least one.
    pub entities: Vec<String>,
    /// When the claim was true in the world (valid time).
    pub valid_at: Timestamp,
    pub location: Location,
    pub source: SourceRef,
    pub licence: LicenceTerms,
    /// The producer's confidence in the claim, in basis points.
    pub confidence_bp: u32,
    /// Ids of packets this one contradicts. Empty is a statement — "none
    /// known" — and is admitted; a blank or repeated id is not.
    pub contradicts: Vec<String>,
    /// SHA-256 of the source content the claim was extracted from.
    pub content_hash: String,
}

/// The §27 evidence packet. Built only through [`Validated::new`].
pub type EvidencePacket = Validated<EvidencePacketFields>;

impl Contract for EvidencePacketFields {
    const NAME: &'static str = "EvidencePacket";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id or claim, no entity, a blank place name, an
    /// unattributed source, a licence that names no id or grants no use, a
    /// confidence above [`crate::validated::MAX_BP`], a contradiction link
    /// that is blank, repeated or points at the packet itself, a malformed
    /// content hash. The order of `valid_at` and `retrieved_at` is not
    /// checked: a claim about a scheduled future instant is legitimately
    /// retrieved before it is valid.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "packet_id", &self.packet_id)?;
        named(c, "claim", &self.claim)?;
        names(c, "entities", &self.entities)?;
        if let Location::Place { name } = &self.location {
            named(c, "location.name", name)?;
        }
        named(c, "source.source_id", &self.source.source_id)?;
        named(c, "source.locator", &self.source.locator)?;
        named(c, "licence.licence_id", &self.licence.licence_id)?;
        if self.licence.permitted.is_empty() {
            return refuse(
                c,
                "carries a licence granting no use; evidence nobody may use is not evidence to \
                 send — evaluate the licence before the source is used",
            );
        }
        basis_points(c, "confidence_bp", self.confidence_bp)?;
        each_named(c, "contradicts", &self.contradicts)?;
        if self.contradicts.contains(&self.packet_id) {
            return refuse(
                c,
                "lists itself in `contradicts`; a claim cannot contradict itself",
            );
        }
        digest(c, "content_hash", &self.content_hash)?;
        Ok(())
    }
}
