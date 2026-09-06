//! Whether anybody read the terms of the provider a model resolves to.
//!
//! ADR 0037 accepts the hosted adapter for the platform lane and applies none
//! of it, and the sequence it names is a person's: *read the terms of the
//! providers the chosen model resolves to*, create the secret, then set the
//! variables. Until this module the first of those acts left no trace a
//! process could read. The variables were the entire gate, so a deployment
//! that set three of them and mounted a token would have begun sending the
//! REASON stage's evidence to a vendor with nothing anywhere in the process
//! able to say whether the terms of the provider actually serving it had ever
//! been read — the failure ADR 0037 names under "what would make this wrong",
//! arriving quietly rather than as a decision.
//!
//! The shape is the one `qip_data_finder::registration::RegistrationRecord`
//! already uses for a venue, and deliberately so: a named operator read a
//! named document at a stated instant. What differs is the subject — a
//! provider on the router's list rather than a data source — and that no
//! credential is named here at all, because the deep brain reads its
//! credential from one variable fixed in code.
//!
//! The record is evidence, not permission. It cannot make a provider
//! acceptable; it records that a person read what they were agreeing to and
//! can be asked about it afterwards. A blank operator is refused by the only
//! constructor, and the `Deserialize` impl goes through that constructor, so
//! an unattributed attestation cannot arrive from a file either — an
//! attestation nobody signed is the anonymous acceptance the venue module
//! refuses in its own domain.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The path of the committed attestation the deployment mounts.
///
/// Read by the deep brain alone, for the same reason the provider variables
/// are: this is the one root that may construct a hosted adapter (ADR 0037,
/// decision 4). Unset means no provider is constructed, whatever else is
/// configured, and the banner says so naming this variable.
pub const ATTESTATION_PATH_VARIABLE: &str = "QIP_MODEL_PROVIDER_ATTESTATION_PATH";

/// One provider's terms, read by a named person at a stated instant.
///
/// Private fields and one constructor. The operator is the point: it is the
/// name that can be asked what the terms said about training on inputs, and
/// the name this platform's audit trail attributes the decision to use the
/// provider to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProviderTermsWire", into = "ProviderTermsWire")]
pub struct ProviderTerms {
    provider: String,
    operator: String,
    terms_read_at: Timestamp,
    terms: String,
}

/// The on-disk shape. Deserialising goes through [`ProviderTerms::new`], so a
/// file with a blank operator is refused where the refusal can name the file
/// rather than at the point where there is nobody left to name.
#[derive(Serialize, Deserialize)]
struct ProviderTermsWire {
    provider: String,
    operator: String,
    terms_read_at: Timestamp,
    terms: String,
}

impl TryFrom<ProviderTermsWire> for ProviderTerms {
    type Error = Error;

    fn try_from(wire: ProviderTermsWire) -> Result<Self> {
        Self::new(wire.provider, wire.operator, wire.terms_read_at, wire.terms)
    }
}

impl From<ProviderTerms> for ProviderTermsWire {
    fn from(record: ProviderTerms) -> Self {
        Self {
            provider: record.provider,
            operator: record.operator,
            terms_read_at: record.terms_read_at,
            terms: record.terms,
        }
    }
}

impl ProviderTerms {
    /// Record that an operator read one provider's terms.
    ///
    /// * `provider` — the provider as the router's catalogue names it, which
    ///   is the string a model identifier pins with `:<provider>` and the
    ///   string `--probe` prints. Blank is refused: an attestation that names
    ///   no provider matches every provider or none, and either reading is a
    ///   guess.
    /// * `operator` — who read them. Blank is refused: an attestation with
    ///   nobody's name on it asserts that somebody, once, read something.
    /// * `terms_read_at` — when. Carried so a later change in the terms has a
    ///   date to be compared against.
    /// * `terms` — the URL or document name of what was read. Blank is
    ///   refused: "the terms" without a citation is a claim nobody can
    ///   re-read.
    pub fn new(
        provider: impl Into<String>,
        operator: impl Into<String>,
        terms_read_at: Timestamp,
        terms: impl Into<String>,
    ) -> Result<Self> {
        let provider = provider.into();
        if provider.trim().is_empty() {
            return Err(Error::invalid(
                "a provider-terms attestation must name the provider whose terms were read, as \
                 the router's catalogue names it",
            ));
        }
        let operator = operator.into();
        if operator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the attestation for provider `{provider}` names no operator. Somebody read \
                 those terms and can be asked what they said about training on the evidence \
                 this platform sends; a record with nobody's name on it cannot be asked"
            )));
        }
        let terms = terms.into();
        if terms.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the attestation for provider `{provider}` cites no terms — give the URL or the \
                 document name that was read, or nobody can re-read it when it changes"
            )));
        }
        Ok(Self {
            provider: provider.trim().to_string(),
            operator: operator.trim().to_string(),
            terms_read_at,
            terms: terms.trim().to_string(),
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn operator(&self) -> &str {
        &self.operator
    }

    pub fn terms_read_at(&self) -> Timestamp {
        self.terms_read_at
    }

    pub fn terms(&self) -> &str {
        &self.terms
    }

    /// One clause for the start-up banner.
    ///
    /// The provider is not repeated here: the banner names it from the model
    /// identifier that pinned it, and printing it twice from two places is
    /// two claims about one fact that can drift apart.
    pub fn describe(&self) -> String {
        format!(
            "attested by {} under {} read at {}",
            self.operator,
            self.terms,
            self.terms_read_at.to_rfc3339()
        )
    }
}

/// Every provider whose terms an operator has read, keyed by provider.
///
/// A `BTreeMap` because the order reaches the banner and a refusal message: a
/// start-up line that lists the attested providers in a different order on
/// each run is one nobody can diff between two deployments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTermsAttestation {
    records: BTreeMap<String, ProviderTerms>,
}

impl ProviderTermsAttestation {
    /// Read the committed file's bytes.
    ///
    /// The document is a JSON array of records. Three refusals, each of which
    /// has a deployment behind it that would otherwise read as attested:
    ///
    /// * Bytes that are not the document — a mount that landed on the wrong
    ///   file, or a hand-edited record missing a field.
    /// * An empty array. A file attesting nothing that admits a provider is
    ///   the control that cannot fire; whoever mounted it believes terms were
    ///   read.
    /// * The same provider twice. Two records for one provider can disagree
    ///   about when the terms were read, and the one that would win is
    ///   whichever the parser saw last — an implementation detail standing in
    ///   for a person's decision.
    pub fn load(text: &str) -> Result<Self> {
        let parsed: Vec<ProviderTerms> = serde_json::from_str(text).map_err(|error| {
            Error::invalid(format!(
                "the provider-terms attestation cannot be read: {error}. It is a JSON array of \
                 records, each naming the provider, the operator who read its terms, the terms \
                 and the instant they were read"
            ))
        })?;
        if parsed.is_empty() {
            return Err(Error::invalid(
                "the provider-terms attestation names no provider. An empty attestation admits \
                 nothing while reading as the record that terms were read, which is worse than \
                 the absent file it replaced",
            ));
        }
        let mut records = BTreeMap::new();
        for record in parsed {
            if let Some(existing) = records.insert(record.provider().to_string(), record) {
                return Err(Error::invalid(format!(
                    "the provider-terms attestation names `{}` twice. Two records for one \
                     provider can disagree about when its terms were read, and which one wins \
                     would be a detail of the parser rather than a decision anybody made",
                    existing.provider()
                )));
            }
        }
        Ok(Self { records })
    }

    /// The record attesting one provider, if a person read its terms.
    pub fn attesting(&self, provider: &str) -> Option<&ProviderTerms> {
        self.records.get(provider)
    }

    /// Every provider named, in the order the banner prints them.
    pub fn providers(&self) -> Vec<&str> {
        self.records.keys().map(String::as_str).collect()
    }

    /// One line for the start-up banner and for a refusal that has to say what
    /// *was* attested, so an operator can see the spelling they got wrong.
    pub fn describe(&self) -> String {
        format!("terms read for {}", self.providers().join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instant in every fixture. A fixed value, so a record's `read at`
    /// clause can be asserted verbatim rather than round-tripped.
    fn read_at() -> Timestamp {
        Timestamp::from_civil(2026, 9, 4)
    }

    fn document(records: &str) -> String {
        format!("[{records}]")
    }

    fn one(provider: &str, operator: &str) -> String {
        format!(
            r#"{{"provider":"{provider}","operator":"{operator}",
                "terms_read_at":"2026-09-04T00:00:00Z","terms":"https://example.test/terms"}}"#
        )
    }

    #[test]
    fn an_attestation_without_an_operator_cannot_be_built_by_the_constructor_or_from_a_file() {
        // The failure this prevents: an attestation asserting that somebody,
        // once, read something. The operator is the whole value of the record
        // — it is the name that can be asked what the terms said about
        // training on the evidence this platform sends — so a record without
        // one must be unconstructable rather than merely discouraged.
        let refusal = ProviderTerms::new("together", "  ", read_at(), "https://example.test/terms")
            .expect_err("a blank operator was accepted");
        assert!(
            refusal.message().contains("names no operator")
                && refusal.message().contains("together"),
            "the refusal does not say what is missing or for which provider: {}",
            refusal.message()
        );

        // And not through the file either: the `Deserialize` impl goes through
        // the same constructor, so the refusal cannot be walked around by
        // writing the record down instead of building it.
        let refusal = ProviderTermsAttestation::load(&document(&one("together", "")))
            .expect_err("a file with a blank operator was loaded");
        assert!(
            refusal.message().contains("names no operator"),
            "the file path does not reach the constructor's refusal: {}",
            refusal.message()
        );

        // The premise: the same document with an operator does load, or this
        // test would pass on any refusal at all.
        let loaded = ProviderTermsAttestation::load(&document(&one("together", "A. Operator")))
            .expect("an attested record is a valid document");
        assert_eq!(
            loaded
                .attesting("together")
                .expect("the provider is attested")
                .operator(),
            "A. Operator"
        );
    }

    #[test]
    fn a_record_missing_its_provider_or_its_terms_is_refused_by_name() {
        let refusal =
            ProviderTerms::new("", "A. Operator", read_at(), "https://example.test/terms")
                .expect_err("a blank provider was accepted");
        assert!(
            refusal.message().contains("must name the provider"),
            "{}",
            refusal.message()
        );

        let refusal = ProviderTerms::new("together", "A. Operator", read_at(), " ")
            .expect_err("a blank terms citation was accepted");
        assert!(
            refusal.message().contains("cites no terms"),
            "a citation nobody can re-read was accepted: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_attestation_that_names_nothing_or_names_a_provider_twice_is_refused() {
        // An empty array is the file that reads as a control and admits
        // nothing; a repeated provider is two claims about one fact, where the
        // winner would be whichever the parser saw last.
        let refusal =
            ProviderTermsAttestation::load("[]").expect_err("an empty attestation was loaded");
        assert!(
            refusal.message().contains("names no provider"),
            "{}",
            refusal.message()
        );

        let twice = document(&format!(
            "{},{}",
            one("together", "A. Operator"),
            one("together", "B. Operator")
        ));
        let refusal =
            ProviderTermsAttestation::load(&twice).expect_err("a repeated provider was loaded");
        assert!(
            refusal.message().contains("`together` twice"),
            "{}",
            refusal.message()
        );

        let refusal = ProviderTermsAttestation::load("not a document")
            .expect_err("arbitrary bytes were read as an attestation");
        assert!(
            refusal.message().contains("JSON array of"),
            "the refusal does not say what the file should have been: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_loaded_attestation_answers_only_for_the_providers_it_names() {
        let loaded = ProviderTermsAttestation::load(&document(&format!(
            "{},{}",
            one("together", "A. Operator"),
            one("fireworks-ai", "A. Operator")
        )))
        .expect("two attested providers are a valid document");
        assert_eq!(
            loaded.providers(),
            vec!["fireworks-ai", "together"],
            "the providers are not in the sorted order the banner and refusals print"
        );
        assert!(
            loaded.attesting("together").is_some(),
            "the premise: an attested provider is found"
        );
        assert!(
            loaded.attesting("togethe").is_none() && loaded.attesting("together-x").is_none(),
            "a provider whose name merely resembles an attested one was treated as attested"
        );
        assert!(
            loaded.describe().contains("together") && loaded.describe().contains("fireworks-ai"),
            "the banner line does not name what was attested: {}",
            loaded.describe()
        );
        assert_eq!(
            loaded.attesting("together").expect("attested").describe(),
            "attested by A. Operator under https://example.test/terms read at \
             2026-09-04T00:00:00.000Z"
        );
    }
}
