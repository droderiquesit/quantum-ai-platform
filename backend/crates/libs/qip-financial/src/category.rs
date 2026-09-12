//! The eight kinds of source the blueprint's §7.6.1 table names.
//!
//! Defined here, in a library both sides of the source registry depend on,
//! rather than in `qip-data-finder` where it was born. Two things have to name
//! a category and they sit on opposite sides of a dependency edge that runs
//! one way only: the finder's Classify stage assigns one to a discovered
//! candidate, and a shipped connector manifest in `qip-market-ingestion`
//! *declares* one for a source this platform's own authors wrote the adapter
//! for. `qip-data-finder` depends on `qip-market-ingestion`, not the reverse,
//! so an enum kept in the finder could never be written into a manifest — and
//! a second enum in the ingestion crate would be two definitions of one table
//! that will disagree the first time either is edited. The finder re-exports
//! this type under its old path, so nothing that classified before this move
//! reads any differently.
//!
//! What stays in the finder is everything that *decides*: `ContentSignal`,
//! the declared claim a discovered candidate carries, and the classifier that
//! refuses rather than guesses when the claim fits none of the eight. This
//! file is only the vocabulary.

use serde::{Deserialize, Serialize};

/// One of the blueprint's eight kinds of deep-web source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCategory {
    /// Full-text filing search, company registers, transparency registers,
    /// broker records, court dockets, patent and trademark offices.
    RegulatoryAndLegal,
    /// Customs and trade data, procurement portals, permit filings, drug and
    /// device approvals, energy and utility regulators, statistical agencies.
    GovernmentAndTrade,
    /// Investor relations, product and pricing pages, job postings, press
    /// rooms, developer changelogs, status pages.
    CorporateSelfDisclosure,
    /// Vessel tracking, port authorities, satellite imagery services,
    /// commodity inventory reports, weather services.
    PhysicalAndGeospatial,
    /// Developer forums, repository activity, governance forums for on-chain
    /// projects, specialist trade forums.
    CommunityAndTechnical,
    /// Pre-print servers, working paper series, conference proceedings.
    Academic,
    /// Product listings, price histories, inventory levels, auction results.
    Marketplace,
    /// The official pages that determine event outcomes.
    ResolutionSource,
}

impl SourceCategory {
    /// The eight categories, in the blueprint's own table order.
    pub const ALL: [Self; 8] = [
        Self::RegulatoryAndLegal,
        Self::GovernmentAndTrade,
        Self::CorporateSelfDisclosure,
        Self::PhysicalAndGeospatial,
        Self::CommunityAndTechnical,
        Self::Academic,
        Self::Marketplace,
        Self::ResolutionSource,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::RegulatoryAndLegal => "regulatory_and_legal",
            Self::GovernmentAndTrade => "government_and_trade",
            Self::CorporateSelfDisclosure => "corporate_self_disclosure",
            Self::PhysicalAndGeospatial => "physical_and_geospatial",
            Self::CommunityAndTechnical => "community_and_technical",
            Self::Academic => "academic",
            Self::Marketplace => "marketplace",
            Self::ResolutionSource => "resolution_source",
        }
    }

    /// The signal this category carries, per §7.6.1's own table — used only
    /// in decision records and banners, never as an input to a decision.
    pub const fn signal(&self) -> &'static str {
        match self {
            Self::RegulatoryAndLegal => {
                "corporate events before they are news, litigation exposure, ownership \
                 changes, innovation pipelines"
            }
            Self::GovernmentAndTrade => {
                "real activity before it reaches an income statement: supply chain \
                 movement, regulatory outcomes"
            }
            Self::CorporateSelfDisclosure => {
                "hiring velocity, price changes, product launches, outages — all public, \
                 all leading"
            }
            Self::PhysicalAndGeospatial => {
                "physical flow that precedes financial flow: commodity supply, freight \
                 congestion"
            }
            Self::CommunityAndTechnical => {
                "sentiment and intent that is public but unaggregated, protocol changes \
                 before they ship"
            }
            Self::Academic => "methods and findings before they are commercialised",
            Self::Marketplace => "direct input to product arbitrage and to consumer demand signals",
            Self::ResolutionSource => {
                "what prediction markets settle on; knowing the source is knowing the \
                 answer's timing"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every category is reachable, and `as_str` round-trips to something
    /// distinct per variant — the guard against two categories silently
    /// sharing one label, which matters more now that the label is also what
    /// a connector manifest writes.
    #[test]
    fn every_category_has_a_distinct_label_and_the_label_is_what_a_manifest_writes() {
        let labels: std::collections::BTreeSet<&str> = SourceCategory::ALL
            .iter()
            .map(SourceCategory::as_str)
            .collect();
        assert_eq!(
            labels.len(),
            SourceCategory::ALL.len(),
            "two categories share a label"
        );
        for category in SourceCategory::ALL {
            let written = serde_json::to_string(&category).expect("a unit variant serialises");
            assert_eq!(
                written,
                format!("\"{}\"", category.as_str()),
                "the serialised form and `as_str` disagree for {category:?}; a manifest \
                 declaring one would then name a category no banner can read back"
            );
        }
    }
}
