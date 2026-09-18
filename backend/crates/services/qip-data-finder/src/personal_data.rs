//! §7.6.6's last unbuilt rule: *personal data on private individuals is not
//! registered*.
//!
//! The blueprint gives the reason in one line — "It is not a trading signal,
//! and holding it is a liability" — and until now that rule was a sentence in
//! a document with nothing in the tree that could enforce it. A stated
//! exclusion nothing checks is the `MaxExpectedShortfall` shape: it reads as
//! protection in a governance table and cannot refuse anything.
//!
//! # What this screens, and what it refuses to guess
//!
//! It screens the **field names the probe actually observed**, taken from
//! [`crate::schema::SourceSchema`], which is derived from the sampled body
//! rather than from anything the candidate claims about itself. That is this
//! crate's standing distinction between a claim and evidence — a
//! [`crate::source::SourceCandidate`] says what it is and a
//! [`crate::source::Source`] carries what the probe found — and a screen that
//! trusted the candidate's own description would be asking the source whether
//! it holds personal data.
//!
//! It does **not** try to decide whether a named human is a private
//! individual or a public one. That distinction cannot be drawn from a field
//! name, and a screen that guessed it would be wrong in the expensive
//! direction. Instead the rule set is confined to identifiers that are never
//! a trading signal *whoever* they belong to: a passport number, a date of
//! birth, a home address, a payment card, a device identifier. A filing that
//! names a company's directors is not caught here and should not be — that is
//! the corporate source class §7.1 wants next, and a gate that refused it
//! would be a gate that refuses everything.
//!
//! So this is a **floor, not a ceiling**. It catches the unambiguous direct
//! identifier and claims nothing about the rest. Saying so here matters more
//! than the rule list does: an operator who reads this as a complete personal
//! data classifier will register a feed it never claimed to judge.
//!
//! # Why the verdict is three-valued
//!
//! [`PersonalDataScreen::NotScreened`] exists because the alternative is a
//! control that passes vacuously. A payload this phase cannot parse yields an
//! empty [`crate::schema::SourceSchema`] — [`crate::probe::ProbeEvidence`]
//! records the absence in the fingerprint rather than aborting — and a screen
//! that read "no fields carry an identifier" off zero fields would clear every
//! non-JSON source in the catalogue while reporting the same verdict as a feed
//! that was genuinely examined. That is the same failure this crate already
//! refuses for robots.txt: "the absence of a robots.txt is not a permission to
//! crawl it". Nothing screened is not nothing found, and only
//! [`PersonalDataScreen::Clear`] permits registration.
//!
//! The arm has a second trigger that is easier to miss than the first, and
//! missing it would have left the control unable to fire on the commonest
//! shape. `SourceSchema` files a payload with no members of its own — `{}`,
//! `[]`, or a bare scalar — under a single reserved path rather than under
//! nothing, so `fields()` is *not* empty for a body in which nothing was
//! observed. A screen keyed on `fields().is_empty()` therefore answers
//! `Clear` for `{}` and reports one screened field where zero were examined.
//! [`screen`] counts named fields, not entries.
//!
//! # Why there is no `Deserialize`
//!
//! These types derive `Serialize` and deliberately not `Deserialize`. A screen
//! result is a finding computed from evidence at a point in time; if it could
//! be read back from a document, a `Clear` verdict nobody ever computed could
//! be handed to the platform alongside the source it was supposed to judge.
//! The way to obtain one is [`screen`], and there is no other.

use crate::schema::SourceSchema;
use serde::Serialize;

/// The class of identifier a field was found to carry.
///
/// Each arm names a category that is a liability to hold and worthless as a
/// signal. The class travels with the finding so that a refusal says which
/// rule fired rather than only that one did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalIdentifier {
    /// A state-issued identifier for a natural person.
    NationalIdentifier,
    /// A way to contact a natural person directly.
    ContactDetail,
    /// Where a natural person lives.
    ResidentialAddress,
    /// When or where a natural person was born.
    BirthDetail,
    /// A measurement of a natural person's body.
    Biometric,
    /// An instrument that draws on a natural person's money.
    PaymentInstrument,
    /// An identifier that follows a natural person between sessions.
    OnlineIdentifier,
}

impl PersonalIdentifier {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NationalIdentifier => "national identifier",
            Self::ContactDetail => "contact detail",
            Self::ResidentialAddress => "residential address",
            Self::BirthDetail => "birth detail",
            Self::Biometric => "biometric",
            Self::PaymentInstrument => "payment instrument",
            Self::OnlineIdentifier => "online identifier",
        }
    }
}

/// One field the screen found to carry a personal identifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PersonalDataFinding {
    field: String,
    identifier: PersonalIdentifier,
    matched: String,
}

impl PersonalDataFinding {
    /// The dotted field path, exactly as the observed schema names it.
    pub fn field(&self) -> &str {
        &self.field
    }

    pub const fn identifier(&self) -> PersonalIdentifier {
        self.identifier
    }

    /// The token or phrase that fired, so a reviewer can see *why* the field
    /// matched instead of re-deriving it from the rule table.
    pub fn matched(&self) -> &str {
        &self.matched
    }

    pub fn describe(&self) -> String {
        format!(
            "`{}` carries a {} (matched on `{}`)",
            self.field,
            self.identifier.as_str(),
            self.matched
        )
    }
}

/// What the screen found in the fields the probe observed.
///
/// Ordered by permissiveness in the same direction [`crate::legal::Legality`]
/// is, and for the same reason: only the affirmative arm permits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
pub enum PersonalDataScreen {
    /// Fields were examined and none carried a personal identifier. The count
    /// is carried so the record shows what was examined rather than only the
    /// answer — a verdict with no denominator cannot be audited.
    Clear { fields_screened: usize },
    /// At least one field carries a personal identifier. Every finding is
    /// listed, not just the first, because a publisher asked to remove one
    /// field should be told about all of them at once.
    Carries { findings: Vec<PersonalDataFinding> },
    /// The probe observed no *named* field, so nothing was examined. Carries
    /// the question that has to be answered to move it, the way
    /// [`crate::legal::Legality::Unknown`] does.
    NotScreened { question: String },
}

impl PersonalDataScreen {
    /// Whether this verdict permits registration. Only `Clear` does.
    ///
    /// `NotScreened` answering `false` here is the whole point of the arm
    /// existing; see the module doc.
    pub fn permits_registration(&self) -> bool {
        matches!(self, Self::Clear { .. })
    }

    /// A sentence for the lifecycle record, in the register's own voice.
    pub fn describe(&self) -> String {
        match self {
            Self::Clear { fields_screened } => format!(
                "no personal identifier among the {fields_screened} field(s) the probe observed"
            ),
            Self::Carries { findings } => {
                let listed: Vec<String> =
                    findings.iter().map(PersonalDataFinding::describe).collect();
                format!(
                    "personal data on natural persons: {}; \u{a7}7.6.6 does not register it, and a \
                     feed without those field(s) is what to ask the publisher for",
                    listed.join("; ")
                )
            }
            Self::NotScreened { question } => question.clone(),
        }
    }

    /// The findings, empty for every arm but `Carries`.
    pub fn findings(&self) -> &[PersonalDataFinding] {
        match self {
            Self::Carries { findings } => findings,
            _ => &[],
        }
    }
}

/// One rule: a phrase of lowercase tokens, and what it identifies.
///
/// Phrases rather than substrings, because a substring rule here is the trap
/// this repository has already been bitten by. `phone` as a substring matches
/// `microphone`, and a product page is a source class §7.6.4 names by name —
/// the marketplace observation. `dob` as a substring matches `dobra`, which is
/// a currency. Tokens are compared whole.
type Rule = (&'static [&'static str], PersonalIdentifier);

/// The path [`crate::schema::SourceSchema`] records a payload under when it
/// has no members of its own. Kept here as a named constant so that the
/// exclusion in [`screen`] is legible as a deliberate rule rather than a
/// magic string, and so a reader can find both ends of it.
const RESERVED_SCALAR_PATH: &str = "$";

/// The rule table.
///
/// Every entry is an identifier that is worthless as a trading signal whoever
/// it belongs to. Nothing here matches a bare `name`, `address` or `id`: those
/// appear in almost every feed and in a corporate registry they describe a
/// company, so a rule on them would refuse the source class §7.1 is missing.
/// `address` therefore appears only qualified by a person.
const RULES: &[Rule] = &[
    // State-issued identifiers for a natural person.
    (&["ssn"], PersonalIdentifier::NationalIdentifier),
    (
        &["social", "security", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    (
        &["social", "insurance", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    (
        &["national", "insurance", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    (
        &["passport", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    (
        &["driver", "licence", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    (
        &["driver", "license", "number"],
        PersonalIdentifier::NationalIdentifier,
    ),
    // Ways to reach a natural person.
    (&["email"], PersonalIdentifier::ContactDetail),
    (&["e", "mail"], PersonalIdentifier::ContactDetail),
    (&["phone", "number"], PersonalIdentifier::ContactDetail),
    (&["mobile", "number"], PersonalIdentifier::ContactDetail),
    (&["telephone", "number"], PersonalIdentifier::ContactDetail),
    (&["home", "phone"], PersonalIdentifier::ContactDetail),
    // Where a natural person lives. Qualified, always: a registered office is
    // a company's public address and is not caught here.
    (&["home", "address"], PersonalIdentifier::ResidentialAddress),
    (
        &["residential", "address"],
        PersonalIdentifier::ResidentialAddress,
    ),
    (
        &["home", "postcode"],
        PersonalIdentifier::ResidentialAddress,
    ),
    // Birth details.
    (&["date", "of", "birth"], PersonalIdentifier::BirthDetail),
    (&["birth", "date"], PersonalIdentifier::BirthDetail),
    (&["birthdate"], PersonalIdentifier::BirthDetail),
    (&["dob"], PersonalIdentifier::BirthDetail),
    (&["place", "of", "birth"], PersonalIdentifier::BirthDetail),
    // Biometrics. `fingerprint` alone is deliberately absent: this crate
    // fingerprints every schema and every envelope, and a rule on it would
    // refuse the platform's own vocabulary.
    (&["biometric"], PersonalIdentifier::Biometric),
    (&["biometrics"], PersonalIdentifier::Biometric),
    // Payment instruments belonging to a natural person.
    (&["card", "number"], PersonalIdentifier::PaymentInstrument),
    (&["cardholder"], PersonalIdentifier::PaymentInstrument),
    (&["cvv"], PersonalIdentifier::PaymentInstrument),
    (
        &["card", "verification", "value"],
        PersonalIdentifier::PaymentInstrument,
    ),
    // Identifiers that follow a person between sessions.
    (&["ip", "address"], PersonalIdentifier::OnlineIdentifier),
    (&["mac", "address"], PersonalIdentifier::OnlineIdentifier),
    (&["device", "id"], PersonalIdentifier::OnlineIdentifier),
    (
        &["device", "identifier"],
        PersonalIdentifier::OnlineIdentifier,
    ),
    (&["cookie", "id"], PersonalIdentifier::OnlineIdentifier),
    (&["advertising", "id"], PersonalIdentifier::OnlineIdentifier),
    (&["imei"], PersonalIdentifier::OnlineIdentifier),
];

/// Split a field path into lowercase word tokens.
///
/// Three boundaries, because feeds use all three in one payload: the
/// separators (`.`, `_`, `-`, and anything else non-alphanumeric), the
/// camelCase hump, and the letter/digit edge. `holder.dateOfBirth` and
/// `holder_date_of_birth` and `HolderDOB2` must all reduce to comparable
/// tokens or the rule table would need an entry per house style.
fn tokens(field: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for character in field.chars() {
        if !character.is_ascii_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }
        if let Some(last) = previous {
            let hump = last.is_ascii_lowercase() && character.is_ascii_uppercase();
            let edge = last.is_ascii_digit() != character.is_ascii_digit();
            if (hump || edge) && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
        current.push(character.to_ascii_lowercase());
        previous = Some(character);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether `phrase` appears as a contiguous run of whole tokens in `tokens`.
fn contains_phrase(tokens: &[String], phrase: &[&str]) -> bool {
    if phrase.is_empty() || phrase.len() > tokens.len() {
        return false;
    }
    tokens
        .windows(phrase.len())
        .any(|window| window.iter().zip(phrase).all(|(token, want)| token == want))
}

/// Screen one observed schema for personal identifiers.
///
/// Takes the schema rather than the source so that the only thing it can
/// consult is what the probe actually saw. Handing it a `Source` would let a
/// later change reach for the candidate's own declaration, which is the
/// distinction the module doc says this screen exists to keep.
pub fn screen(schema: &SourceSchema) -> PersonalDataScreen {
    // Named fields only, and the exclusion of the reserved path is the
    // difference between a control that can fire and one that cannot.
    // `SourceSchema` records a payload with no members — `{}`, `[]`, or a
    // bare scalar — under the reserved path `$`, so `fields()` is non-empty
    // for payloads in which nothing was actually observed. Counting `$` as a
    // screened field would have reported `Clear { fields_screened: 1 }` for a
    // body that named nothing at all, which is a verdict with a denominator
    // of zero wearing a denominator of one. This screen matches field
    // *names*; a payload that has none was not screened, whatever its shape.
    let named: Vec<&String> = schema
        .fields()
        .keys()
        .filter(|name| name.as_str() != RESERVED_SCALAR_PATH)
        .collect();
    if named.is_empty() {
        return PersonalDataScreen::NotScreened {
            question: "no named field was observed in the sampled payload, so nothing was \
                       screened for personal data; supply a probe sample this phase can parse \
                       into named fields before registering this source"
                .to_string(),
        };
    }
    let mut findings = Vec::new();
    for name in named.iter().copied() {
        let observed = tokens(name);
        for (phrase, identifier) in RULES {
            if contains_phrase(&observed, phrase) {
                findings.push(PersonalDataFinding {
                    field: name.clone(),
                    identifier: *identifier,
                    matched: phrase.join(" "),
                });
            }
        }
    }
    if findings.is_empty() {
        PersonalDataScreen::Clear {
            fields_screened: named.len(),
        }
    } else {
        PersonalDataScreen::Carries { findings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldType;

    fn schema_of(names: &[&str]) -> SourceSchema {
        SourceSchema::from_fields(
            names
                .iter()
                .map(|name| ((*name).to_string(), FieldType::Text)),
        )
    }

    #[test]
    fn a_field_path_splits_on_separators_humps_and_the_digit_edge() {
        assert_eq!(
            tokens("holder.dateOfBirth"),
            ["holder", "date", "of", "birth"]
        );
        assert_eq!(tokens("home_address"), ["home", "address"]);
        assert_eq!(tokens("address2"), ["address", "2"]);
        assert_eq!(tokens("SSN"), ["ssn"]);
    }

    #[test]
    fn a_phrase_matches_only_whole_tokens_and_never_a_substring() {
        // The substring trap, with the two cases that would actually occur:
        // a product page selling a microphone, and a price in Sao Tome dobra.
        assert!(!contains_phrase(&tokens("microphone"), &["phone"]));
        assert!(!contains_phrase(&tokens("price_dobra"), &["dob"]));
        assert!(contains_phrase(
            &tokens("contact_phone_number"),
            &["phone", "number"]
        ));
    }

    #[test]
    fn an_empty_schema_is_not_screened_rather_than_reported_clear() {
        // Two routes to an empty field map, and both must refuse: a payload
        // this phase cannot parse, which `ProbeEvidence` files as no fields
        // at all, and an empty JSON array, which parses cleanly to the same.
        let unparsed = SourceSchema::from_fields([]);
        let empty_array = SourceSchema::parse("[]").expect("parses as JSON");
        assert!(
            empty_array.fields().is_empty(),
            "premise: `[]` must reach the schema as an empty field map"
        );
        for schema in [unparsed, empty_array] {
            let verdict = screen(&schema);
            assert!(
                matches!(verdict, PersonalDataScreen::NotScreened { .. }),
                "a payload nothing was read from must not read as examined, got {verdict:?}"
            );
            assert!(!verdict.permits_registration());
        }
    }

    #[test]
    fn a_payload_with_no_members_is_not_screened_however_the_schema_files_it() {
        // `{}` and a bare scalar reach `SourceSchema` as the single reserved
        // path, so `fields()` is non-empty while nothing was named. (`[]`
        // yields a genuinely empty map and is the other test's case; the two
        // shapes must both refuse, by different routes.)
        // A screen keyed on `fields().is_empty()` answers `Clear` here, with
        // a denominator of one and nothing behind it, and that is the shape
        // of a control that cannot fire.
        for body in ["{}", "41"] {
            let schema = SourceSchema::parse(body).expect("parses as JSON");
            assert!(
                !schema.fields().is_empty(),
                "premise: `{body}` must reach the schema as a non-empty field map,                  or this test proves nothing beyond the empty case"
            );
            let verdict = screen(&schema);
            assert!(
                matches!(verdict, PersonalDataScreen::NotScreened { .. }),
                "`{body}` named no field and must not read as screened, got {verdict:?}"
            );
        }
    }

    #[test]
    fn a_quote_payload_is_clear_and_carries_what_it_examined() {
        let verdict = screen(&schema_of(&["symbol", "bid", "ask", "volume"]));
        assert_eq!(verdict, PersonalDataScreen::Clear { fields_screened: 4 });
        assert!(verdict.permits_registration());
    }

    #[test]
    fn a_companys_registered_office_and_directors_are_not_personal_data() {
        // The false positive that would matter: a corporate registry is the
        // source class §7.1 names as missing, and a rule on a bare `address`
        // or `name` would refuse every one of them.
        let verdict = screen(&schema_of(&[
            "company.name",
            "company.registered_address",
            "company.street_address",
            "directors.0.name",
            "filing.id",
        ]));
        assert!(
            verdict.permits_registration(),
            "a corporate registry must still register, got {verdict:?}"
        );
    }

    #[test]
    fn every_finding_is_listed_rather_than_only_the_first() {
        let verdict = screen(&schema_of(&["subscriber.email", "subscriber.home_address"]));
        let findings = verdict.findings();
        assert_eq!(findings.len(), 2, "got {findings:?}");
        assert_eq!(findings[0].identifier(), PersonalIdentifier::ContactDetail);
        assert_eq!(
            findings[1].identifier(),
            PersonalIdentifier::ResidentialAddress
        );
        assert!(!verdict.permits_registration());
    }
}
