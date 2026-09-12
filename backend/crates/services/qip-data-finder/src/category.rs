//! What kind of source a candidate actually is, and the refusal that follows
//! when it is not cleanly one of the eight the blueprint names.
//!
//! §7.6.1 tables eight categories of deep-web source — regulatory and legal,
//! government and trade, corporate self-disclosure, physical and geospatial,
//! community and technical, academic, marketplace, resolution sources — each
//! carrying a different kind of signal. §7.4's Classify stage asks the
//! question that places a candidate into one of them: "is this news, filings,
//! data, discussion, a marketplace, a leak forum?"
//!
//! The eight-variant enum itself lives in [`qip_financial::category`] and is
//! re-exported here under the path it was born with. It moved down one layer
//! because a shipped connector manifest in `qip-market-ingestion` now
//! *declares* a category too, and that crate sits below this one on the
//! dependency edge: an enum kept here could never be written into a manifest,
//! and a second enum there would be two copies of one table. What stays here
//! is the part that decides — [`ContentSignal`] and [`classify`].
//!
//! That question cannot be answered by inference in this crate: nothing here
//! reads a page (§7.4's own Sample stage, which does, is not built — see
//! `docs/DELIVERY-STATUS.md`'s row for it). What a directory listing, an
//! operator's note, or a prior human review *can* state is which of these
//! shapes a location is, and [`ContentSignal`] is that claim — the same kind
//! of pre-probe evidence [`crate::tier::TierEvidence::from_candidate`] already
//! builds from a candidate's own description, extended to a second question.
//!
//! [`classify`] is the refusal this module exists to hold: "news" and
//! "discussion" in the blueprint's own question are deliberately *not* among
//! the eight categories a source can land in, because neither names anything
//! about a source recurring, lawful and useful the way the table's eight do —
//! a general news wire is a surface-web feed the ordinary §7.3 pipeline
//! already ingests, and unspecialised discussion is exactly the shape a forum
//! has before someone has read enough of it to say what it is forums *about*.
//! A leak forum is excluded for the reason §7.5 gives one a hard line: no
//! category on this table describes a source whose defining trait is that it
//! deals in what should not have been shared. Force-fitting any of the three
//! into the nearest category would be exactly the "refuse rather than guess"
//! failure `.claude/rules/00-enterprise-governance.md` exists to prevent — a
//! category assigned because something has to be assigned reads downstream as
//! a finding, and it would not be one.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub use qip_financial::category::SourceCategory;

/// Classify from what is declared about a location, refusing rather than
/// guessing when the declaration does not cleanly name one of the eight.
///
/// `None` is refused outright: a candidate nobody has said anything about
/// beyond its endpoint has not been classified, and defaulting it to any one
/// category — even the most common — would be exactly the guess this
/// function exists not to make.
pub fn classify(signal: Option<&ContentSignal>) -> Result<SourceCategory> {
    let Some(signal) = signal else {
        return Err(Error::invalid(
            "no content signal is declared for this candidate; classification asks what a \
             location actually is — a filing search, customs data, a press room, a \
             specialist forum — and a candidate with nothing declared cannot be placed in a \
             category rather than guessed into one. Declare one with \
             `SourceCandidate::with_content_signal`",
        ));
    };
    match signal {
        ContentSignal::RegulatoryFiling | ContentSignal::CourtDocket => {
            Ok(SourceCategory::RegulatoryAndLegal)
        }
        ContentSignal::CustomsOrTradeData | ContentSignal::GovernmentProcurement => {
            Ok(SourceCategory::GovernmentAndTrade)
        }
        ContentSignal::InvestorRelations
        | ContentSignal::CorporatePressRoom
        | ContentSignal::JobPostings => Ok(SourceCategory::CorporateSelfDisclosure),
        ContentSignal::VesselOrPortTracking | ContentSignal::SatelliteOrWeatherImagery => {
            Ok(SourceCategory::PhysicalAndGeospatial)
        }
        ContentSignal::DeveloperOrRepositoryActivity | ContentSignal::SpecialistTradeForum => {
            Ok(SourceCategory::CommunityAndTechnical)
        }
        ContentSignal::PreprintOrWorkingPaper => Ok(SourceCategory::Academic),
        ContentSignal::ProductListingOrAuction => Ok(SourceCategory::Marketplace),
        ContentSignal::EventResolutionPage => Ok(SourceCategory::ResolutionSource),
        ContentSignal::GeneralNews
        | ContentSignal::UnspecialisedDiscussion
        | ContentSignal::LeakForum => Err(Error::invalid(format!(
            "`{}` fits none of the eight source categories cleanly. §7.4's own Classify \
             question is \"is this news, filings, data, discussion, a marketplace, a leak \
             forum?\" — general news, unspecialised discussion and a leak forum are exactly \
             the three shapes that question names and none of the eight admits, so the \
             candidate is refused rather than force-fit into whichever category is nearest",
            signal.as_str()
        ))),
    }
}

/// What a candidate is claimed to be, declared before any category is
/// assigned. The same kind of pre-probe claim
/// [`crate::source::SourceCandidate::declared_coverage`] already carries for
/// coverage and [`crate::source::SourceCandidate::declared_licensing`]
/// carries for licensing — a fact from whoever discovered the candidate,
/// verified nowhere in this crate because nothing here reads a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSignal {
    RegulatoryFiling,
    CourtDocket,
    CustomsOrTradeData,
    GovernmentProcurement,
    InvestorRelations,
    CorporatePressRoom,
    JobPostings,
    VesselOrPortTracking,
    SatelliteOrWeatherImagery,
    DeveloperOrRepositoryActivity,
    SpecialistTradeForum,
    PreprintOrWorkingPaper,
    ProductListingOrAuction,
    EventResolutionPage,
    /// A general news wire or similar. Deliberately unmapped — see the module
    /// doc comment.
    GeneralNews,
    /// A forum or board with no stated specialism. Deliberately unmapped.
    UnspecialisedDiscussion,
    /// A market for stolen data or leaked documents. Deliberately unmapped,
    /// and excluded again at §7.5's hard line regardless of tier.
    LeakForum,
}

impl ContentSignal {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::RegulatoryFiling => "regulatory_filing",
            Self::CourtDocket => "court_docket",
            Self::CustomsOrTradeData => "customs_or_trade_data",
            Self::GovernmentProcurement => "government_procurement",
            Self::InvestorRelations => "investor_relations",
            Self::CorporatePressRoom => "corporate_press_room",
            Self::JobPostings => "job_postings",
            Self::VesselOrPortTracking => "vessel_or_port_tracking",
            Self::SatelliteOrWeatherImagery => "satellite_or_weather_imagery",
            Self::DeveloperOrRepositoryActivity => "developer_or_repository_activity",
            Self::SpecialistTradeForum => "specialist_trade_forum",
            Self::PreprintOrWorkingPaper => "preprint_or_working_paper",
            Self::ProductListingOrAuction => "product_listing_or_auction",
            Self::EventResolutionPage => "event_resolution_page",
            Self::GeneralNews => "general_news",
            Self::UnspecialisedDiscussion => "unspecialised_discussion",
            Self::LeakForum => "leak_forum",
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

    /// Every mapped signal lands in exactly the category the blueprint's
    /// table names it under. Broken out per category rather than asserted as
    /// one big table so a failure names which pairing broke.
    #[test]
    fn every_mapped_signal_classifies_into_its_own_table_row() -> Result<()> {
        let cases = [
            (
                ContentSignal::RegulatoryFiling,
                SourceCategory::RegulatoryAndLegal,
            ),
            (
                ContentSignal::CourtDocket,
                SourceCategory::RegulatoryAndLegal,
            ),
            (
                ContentSignal::CustomsOrTradeData,
                SourceCategory::GovernmentAndTrade,
            ),
            (
                ContentSignal::GovernmentProcurement,
                SourceCategory::GovernmentAndTrade,
            ),
            (
                ContentSignal::InvestorRelations,
                SourceCategory::CorporateSelfDisclosure,
            ),
            (
                ContentSignal::CorporatePressRoom,
                SourceCategory::CorporateSelfDisclosure,
            ),
            (
                ContentSignal::JobPostings,
                SourceCategory::CorporateSelfDisclosure,
            ),
            (
                ContentSignal::VesselOrPortTracking,
                SourceCategory::PhysicalAndGeospatial,
            ),
            (
                ContentSignal::SatelliteOrWeatherImagery,
                SourceCategory::PhysicalAndGeospatial,
            ),
            (
                ContentSignal::DeveloperOrRepositoryActivity,
                SourceCategory::CommunityAndTechnical,
            ),
            (
                ContentSignal::SpecialistTradeForum,
                SourceCategory::CommunityAndTechnical,
            ),
            (
                ContentSignal::PreprintOrWorkingPaper,
                SourceCategory::Academic,
            ),
            (
                ContentSignal::ProductListingOrAuction,
                SourceCategory::Marketplace,
            ),
            (
                ContentSignal::EventResolutionPage,
                SourceCategory::ResolutionSource,
            ),
        ];
        for (signal, expected) in cases {
            assert_eq!(
                classify(Some(&signal))?,
                expected,
                "{signal:?} did not classify into {expected:?}"
            );
        }
        Ok(())
    }

    /// The refusal this module exists for: a candidate whose declared signal
    /// is one of the three the blueprint's own question names but that no
    /// category admits is refused, not force-fit into the nearest row.
    ///
    /// Mutated by replacing the `Err` arm with, e.g.,
    /// `Ok(SourceCategory::CorporateSelfDisclosure)` for `GeneralNews` —
    /// confirmed to fail this assertion, then restored.
    #[test]
    fn a_signal_that_fits_no_category_is_refused_rather_than_force_fit() {
        for signal in [
            ContentSignal::GeneralNews,
            ContentSignal::UnspecialisedDiscussion,
            ContentSignal::LeakForum,
        ] {
            let error = classify(Some(&signal))
                .expect_err(&format!("{signal:?} was classified into a category"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
            assert!(
                error
                    .message()
                    .contains("fits none of the eight source categories cleanly"),
                "the refusal does not name why {signal:?} was refused: {error}"
            );
        }
    }

    /// No declared signal at all is refused, not defaulted to any category —
    /// the same "unknown is not permitted" discipline `legal::Legality`
    /// holds, applied to a second question.
    #[test]
    fn no_declared_signal_is_refused_rather_than_defaulted() {
        let error = classify(None).expect_err("an unclassified candidate was classified");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        assert!(
            error.message().contains("no content signal is declared"),
            "the refusal does not say a signal is missing: {error}"
        );
    }

    /// Every category is reachable, and `as_str` round-trips to something
    /// distinct per variant — the guard against two categories silently
    /// sharing one label.
    #[test]
    fn every_category_has_a_distinct_label() {
        let labels: std::collections::BTreeSet<&str> = SourceCategory::ALL
            .iter()
            .map(SourceCategory::as_str)
            .collect();
        assert_eq!(
            labels.len(),
            SourceCategory::ALL.len(),
            "two categories share a label"
        );
    }
}
