//! The licensing gate a connector source passes before it is opened.
//!
//! `.claude/rules/domains/data-and-streaming.md` is categorical: a source's
//! licensing posture is evaluated **before** the source is used, and a
//! research-only licence never reaches the trading path. The connector SDK
//! carries a licensing *class* in each manifest, but a class is a label, not
//! an evaluation — the evaluation is the reading of the actual terms, mapped
//! onto the usages this platform makes, and it lives here as a catalogue with
//! one entry per admitted source.
//!
//! The two questions asked of every source are [`Usage::Derive`] and
//! [`Usage::Trade`]. Derive because that is what the loop factually does with
//! a record — features, statistics, simulated decisions — and Trade because
//! this node's decision loop *is* the trading path, paper today and the
//! recorded destination tomorrow (ADR 0023): a source admitted here on a
//! research-only licence would be promoted onto live trading by nothing more
//! than the ceiling changing, which is precisely the quiet promotion the
//! rule's own example forbids.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_data_finder::{LicensingPosture, SourceLicense};
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::connector_feed::KNOWN_SOURCES;

/// One catalogued source: the evaluation of its actual terms.
#[derive(Debug)]
pub struct CatalogueEntry {
    /// Must match the manifest's `source_id` exactly.
    pub source_id: &'static str,
    /// Must match the manifest's licensing class. A mismatch means the
    /// manifest and this catalogue were edited independently, and the safe
    /// reading of a disagreement between two claims about one licence is
    /// that neither is current.
    pub expected_class: LicensingClass,
    /// The evaluation itself.
    pub posture: LicensingPosture,
}

/// The catalogue. One entry per source this build may open.
///
/// # coinbase-spot-ticker
///
/// Coinbase Exchange market data over the public, unauthenticated endpoint.
/// The terms read for this evaluation: Coinbase's Market Data terms permit
/// use of the public feed for internal purposes — consumption, analysis,
/// derivation, and acting on it — and do **not** grant redistribution or
/// display of the raw data to third parties. That is `Internal`, not
/// `Public`: the manifest's class says so, and the licence below grants
/// research, derivation and trading while withholding redistribution. No
/// expiry is stated in the terms; the entry carries none, and a change in
/// the vendor's terms is a change to this entry, reviewed like code.
pub fn catalogue() -> Result<Vec<CatalogueEntry>> {
    use qip_contracts::governance::Usage;
    Ok(vec![CatalogueEntry {
        source_id: "coinbase-spot-ticker",
        expected_class: LicensingClass::Internal,
        posture: LicensingPosture::declared(SourceLicense::new(
            "coinbase-exchange-market-data-terms",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
    }])
}

/// Admit a source for this node's use, or refuse it with the reason.
///
/// Called by the composition root before [`qip_market_ingestion::connector_feed::ConnectorFeed::open`],
/// which is what makes the ordering the rule demands — evaluation, then use —
/// a property of the code path rather than of anyone's memory.
pub fn admit(source_id: &str, manifest_class: LicensingClass, now: Timestamp) -> Result<()> {
    admit_from(&catalogue()?, source_id, manifest_class, now)
}

/// The same admission against a caller-supplied catalogue.
///
/// Split from [`admit`] so the refusal arms are testable with entries the
/// real catalogue must never contain — a research-only licence, a class
/// disagreement — without weakening the real catalogue to host them.
pub fn admit_from(
    entries: &[CatalogueEntry],
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<()> {
    use qip_contracts::governance::Usage;
    let entry = entries
        .iter()
        .find(|entry| entry.source_id == source_id)
        .ok_or_else(|| {
            Error::denied(format!(
                "{source_id:?} has no licensing evaluation in the catalogue, so its terms have \
                 not been read and it is refused. Evaluate the source's terms, write the entry, \
                 and have it reviewed — the catalogue is code on purpose"
            ))
        })?;
    if entry.expected_class != manifest_class {
        return Err(Error::denied(format!(
            "the manifest for {source_id} declares licensing class `{}` and the catalogue's \
             evaluation was written against `{}`. Two claims about one licence disagree, so \
             neither is treated as current; re-read the terms and update both together",
            format_args!("{manifest_class:?}"),
            format_args!("{:?}", entry.expected_class)
        )));
    }
    for usage in [Usage::Derive, Usage::Trade] {
        entry
            .posture
            .legality_for(usage, now)
            .require_permitted(&format!("{source_id} for {}", usage.as_str()))?;
    }
    // The premise of the whole module, kept honest mechanically: the source
    // being admitted is one the feed can actually open, or the catalogue has
    // an entry for something that cannot exist and the evaluation is
    // decoration.
    if !KNOWN_SOURCES.contains(&source_id) {
        return Err(Error::invalid(format!(
            "{source_id} is catalogued but no connector in this build carries it"
        )));
    }
    Ok(())
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_contracts::governance::Usage;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_catalogued_source_is_admitted_and_an_unknown_one_is_refused()
    -> qip_core::error::Result<()> {
        // The premise first: the shipped manifest's class is what the gate
        // will be handed in production, so admitting with anything else here
        // would test a path the composition root never takes.
        let class = qip_market_ingestion::connector_feed::shipped_class("coinbase-spot-ticker")?;
        admit("coinbase-spot-ticker", class, now())?;

        let refused = admit("some-unevaluated-endpoint", class, now());
        assert!(
            refused.is_err(),
            "a source with no licensing evaluation was admitted, so terms \
             nobody read were treated as read"
        );
        Ok(())
    }

    #[test]
    fn a_class_disagreement_between_manifest_and_catalogue_refuses_the_source() {
        // Two claims about one licence. If the manifest is edited to `public`
        // while the catalogue still says `internal` — or the reverse — the
        // safe reading is that neither is current, because whichever edit came
        // second was made without re-reading the terms alongside the other.
        let refused = admit("coinbase-spot-ticker", LicensingClass::Public, now());
        assert!(
            refused.is_err(),
            "a manifest claiming a different licensing class than the \
             catalogue's evaluation was admitted"
        );
    }

    #[test]
    fn a_research_only_licence_never_reaches_the_trading_path() -> qip_core::error::Result<()> {
        // The rule's own example, driven through the real gate against an
        // entry the real catalogue must never contain. The terms were read
        // and they grant research; asked about the trading path, the answer
        // is forbidden — not unknown — and the source does not open.
        let research_only = vec![CatalogueEntry {
            source_id: "research-feed",
            expected_class: LicensingClass::Internal,
            posture: LicensingPosture::declared(SourceLicense::new(
                "research-only-terms",
                // Research and derivation granted, trading withheld — so the
                // refusal below can only come from the Trade question, and a
                // gate that stopped asking it goes red here.
                [Usage::Research, Usage::Derive],
            )?),
        }];
        // Premise: everything short of trading is permitted, so the refusal
        // below can only be the Trade question being asked and answered.
        assert!(
            research_only[0]
                .posture
                .legality_for(Usage::Derive, now())
                .is_permitted()
        );
        let refused = admit_from(
            &research_only,
            "research-feed",
            LicensingClass::Internal,
            now(),
        )
        .expect_err("a research-only licence was admitted onto the trading path");
        // The refusal must be the Trade question's answer, by name. The first
        // version of this test accepted any error, and a mutation that
        // deleted the Trade question passed it anyway — the entry also fails
        // the known-sources integrity check, and that refusal was mistaken
        // for this one.
        assert!(
            refused.message().contains("for trade"),
            "the refusal was not about the trading usage: {}",
            refused.message()
        );
        Ok(())
    }
}
