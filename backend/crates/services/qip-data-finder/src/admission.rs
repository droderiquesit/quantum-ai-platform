//! The licensing gate a connector source passes before a composition root
//! opens it.
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
//! the decision loop *is* the trading path, paper today and the recorded
//! destination tomorrow (ADR 0023): a source admitted here on a
//! research-only licence would be promoted onto live trading by nothing more
//! than the ceiling changing, which is precisely the quiet promotion the
//! rule's own example forbids.
//!
//! This module lives in the data finder rather than in a binary so that every
//! composition root that opens a connector — `qip-api` today — asks the same
//! catalogue the same questions. `qip-fastbrain` still carries its own copy of
//! these entries in `licensing.rs`; the two must say the same thing about
//! each source until that root is pointed here, and a disagreement between
//! them is a disagreement about one licence, which is the state the class
//! check below refuses on purpose.
//!
//! [`admit`] returns a [`LicensingDecision`] rather than `()` so the root can
//! state, in its banner, which licence admitted the source for which usages
//! at which instant — a gate whose only output is silence is one an operator
//! cannot tell from a gate that never ran.

use qip_contracts::governance::Usage;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::connector_feed::KNOWN_SOURCES;

use crate::legal::{LicensingPosture, SourceLicense};

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

/// The usages every source is asked about before it may feed the loop.
pub const REQUIRED_USAGES: [Usage; 2] = [Usage::Derive, Usage::Trade];

/// What the gate decided, for the banner and the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicensingDecision {
    pub source_id: String,
    /// The licence identifier the catalogue entry was written against.
    pub licence: String,
    /// The class the manifest declares and the catalogue agreed with.
    pub class: LicensingClass,
    /// The usages the licence was found to permit at `decided_at`.
    pub usages: Vec<Usage>,
    pub decided_at: Timestamp,
}

impl LicensingDecision {
    /// One line an operator can read at start-up.
    pub fn describe(&self) -> String {
        format!(
            "admitted under licence `{}` (class {:?}) for {} at {}",
            self.licence,
            self.class,
            self.usages
                .iter()
                .map(|usage| usage.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            self.decided_at.to_rfc3339()
        )
    }
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
///
/// # frankfurter-ecb-reference-rates
///
/// The euro foreign-exchange reference rates the ECB publishes each working
/// day, served unauthenticated by Frankfurter. The terms read for this
/// evaluation: the ECB permits reuse of its published reference rates,
/// including for commercial purposes, provided the source is acknowledged;
/// Frankfurter is a free relay of that same series and adds no term of its
/// own restricting it. That is `Public` — the one class in this catalogue
/// whose grant includes `Redistribute`, and the reason a number derived from
/// it may be shown to a client where a Coinbase-derived one may not.
///
/// The acknowledgement obligation is the thing to notice and the thing this
/// entry cannot enforce: `LicensingClass::Public` is a statement about what
/// the platform may do, not a mechanism that attributes anything. Displaying
/// these rates in the console without naming the ECB would satisfy every
/// check in this file and still breach the terms it cites. Nothing displays
/// them today; whoever first does owns that.
///
/// No expiry is stated, so the entry carries none. This posture was written
/// from the published terms and not from a negotiated agreement — there is
/// no contract to read — so a change in either party's terms is a change to
/// this entry, reviewed like code.
///
/// # kalshi-markets and alpaca-daily-bars — refused until their terms are read
///
/// The two remaining ADR 0034 candidates. Both connectors exist and both are
/// on `KNOWN_SOURCES`, so this catalogue must say something about them or
/// `admit` refuses them as uncatalogued — which is the right outcome for the
/// wrong reason, and an operator reading the refusal would go looking for a
/// missing entry rather than for the terms. Each carries
/// [`LicensingPosture::Ambiguous`]: the terms exist and nobody has mapped
/// them onto this platform's usages, so every usage question answers
/// `unknown` and `admit` refuses. ADR 0034 is explicit that its description
/// of each vendor's terms is not an evaluation and not legal advice; the
/// evidence names the document to read. Neither is `Declared`, and neither
/// becomes so by an edit to the connector or the manifest — only by
/// replacing the posture here, with the terms cited, under review.
///
/// Both manifests declare `Restricted`, the most restrictive class short of
/// `Synthetic`, and `expected_class` agrees so that the refusal an operator
/// sees is the licensing one and not a class disagreement. When the terms
/// are read the class may relax; it does so in the manifest and here in one
/// commit, or the disagreement check refuses the source again.
pub fn catalogue() -> Result<Vec<CatalogueEntry>> {
    Ok(vec![
        CatalogueEntry {
            source_id: "kalshi-markets",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::ambiguous(
                "Kalshi's terms of service and API terms at https://kalshi.com/terms have not \
                 been read against this platform's usages; ADR 0034 names the source as a \
                 candidate only",
            ),
        },
        CatalogueEntry {
            source_id: "alpaca-daily-bars",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::ambiguous(
                "Alpaca's market-data terms and account agreement at \
                 https://alpaca.markets/terms-and-conditions have not been read against this \
                 platform's usages; ADR 0034 names the source as a candidate only, and its \
                 paper brokerage is the account the data terms come with",
            ),
        },
        CatalogueEntry {
            source_id: "coinbase-spot-ticker",
            expected_class: LicensingClass::Internal,
            posture: LicensingPosture::declared(SourceLicense::new(
                "coinbase-exchange-market-data-terms",
                [Usage::Research, Usage::Derive, Usage::Trade],
            )?),
        },
        CatalogueEntry {
            source_id: "frankfurter-ecb-reference-rates",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::declared(SourceLicense::new(
                "ecb-reference-rates-via-frankfurter",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        },
    ])
}

/// Admit a source for the loop's use, or refuse it with the reason.
///
/// Called by a composition root before
/// [`qip_market_ingestion::connector_feed::ConnectorFeed::open`], which is
/// what makes the ordering the rule demands — evaluation, then use — a
/// property of the code path rather than of anyone's memory.
pub fn admit(
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<LicensingDecision> {
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
) -> Result<LicensingDecision> {
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
    for usage in REQUIRED_USAGES {
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
    let licence = entry
        .posture
        .license()
        .map(|license| license.identifier().to_string())
        .ok_or_else(|| {
            Error::denied(format!(
                "{source_id} passed every usage question without a declared licence, which \
                 cannot happen: a posture with no licence answers every usage question \
                 `unknown`"
            ))
        })?;
    Ok(LicensingDecision {
        source_id: source_id.to_string(),
        licence,
        class: manifest_class,
        usages: REQUIRED_USAGES.to_vec(),
        decided_at: now,
    })
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_catalogued_source_is_admitted_with_its_licence_named_and_an_unknown_one_is_refused()
    -> Result<()> {
        // The premise first: the shipped manifest's class is what the gate
        // will be handed in production, so admitting with anything else here
        // would test a path the composition root never takes.
        let class =
            qip_market_ingestion::connector_feed::shipped_class("frankfurter-ecb-reference-rates")?;
        let decision = admit("frankfurter-ecb-reference-rates", class, now())?;
        assert_eq!(decision.licence, "ecb-reference-rates-via-frankfurter");
        assert_eq!(decision.class, LicensingClass::Public);
        assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
        assert_eq!(decision.decided_at, now());
        // The banner line names the licence and both usages; a decision an
        // operator cannot read is a gate they cannot tell ran.
        let line = decision.describe();
        assert!(
            line.contains("ecb-reference-rates-via-frankfurter")
                && line.contains("derive and trade"),
            "the decision does not say what admitted the source: {line}"
        );

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
    fn a_research_only_licence_never_reaches_the_trading_path() -> Result<()> {
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
        // The refusal must be the Trade question's answer, by name. A version
        // of this test in another root once accepted any error, and a
        // mutation that deleted the Trade question passed it anyway — the
        // entry also fails the known-sources integrity check, and that
        // refusal was mistaken for this one.
        assert!(
            refused.message().contains("for trade"),
            "the refusal was not about the trading usage: {}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn the_adr_0034_candidates_whose_terms_are_unread_are_refused_by_the_real_catalogue()
    -> Result<()> {
        // Not a placeholder catalogue: the shipped one, with the class the
        // shipped manifest declares — the exact call the composition root
        // makes. The refusal must be the Derive question's own answer and
        // must name the terms to read, so an operator paged on it goes to
        // the document and not to this file.
        for source_id in ["kalshi-markets", "alpaca-daily-bars"] {
            // Premise: the source is one the feed can open by name, so the
            // refusal below cannot be the known-sources integrity check.
            assert!(
                qip_market_ingestion::connector_feed::KNOWN_SOURCES.contains(&source_id),
                "{source_id} is not a source the build carries"
            );
            let class = qip_market_ingestion::connector_feed::shipped_class(source_id)?;
            let entry = catalogue()?
                .into_iter()
                .find(|entry| entry.source_id == source_id)
                .unwrap_or_else(|| panic!("{source_id} has no catalogue entry"));
            assert!(
                entry.posture.license().is_none(),
                "{source_id} carries a declared licence, and ADR 0034 says its terms are unread"
            );
            assert_eq!(entry.expected_class, class);

            let refused = admit(source_id, class, now()).expect_err(&format!(
                "{source_id} was admitted, so terms nobody read were treated as read"
            ));
            let message = refused.message();
            assert!(
                message.contains(&format!(
                    "{source_id} for derive may not be collected because its legality is \
                     undetermined"
                )),
                "the refusal is not the Derive question's answer: {message}"
            );
            assert!(
                message.contains("https://"),
                "the refusal does not name the terms to read: {message}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_unevaluated_posture_is_refused_as_undetermined_not_admitted_by_default() -> Result<()> {
        // The gate's third arm: terms nobody found. `Undetermined` answers
        // every usage `unknown`, and unknown is not permission — a source
        // entered in the catalogue as a placeholder must still not open.
        let placeholder = vec![CatalogueEntry {
            source_id: "frankfurter-ecb-reference-rates",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::Undetermined,
        }];
        let refused = admit_from(
            &placeholder,
            "frankfurter-ecb-reference-rates",
            LicensingClass::Public,
            now(),
        )
        .expect_err("a source with no terms located was admitted");
        // The phrase is the usage question's own answer, from
        // `Legality::require_permitted`, and not merely the word: a first
        // version of this test matched `undetermined` and was satisfied by
        // the backstop refusal below the usage loop when a mutation deleted
        // the loop — the word was a substring of a different refusal.
        assert!(
            refused
                .message()
                .contains("for derive may not be collected because its legality is undetermined"),
            "the refusal is not the Derive question's answer: {}",
            refused.message()
        );
        Ok(())
    }
}
