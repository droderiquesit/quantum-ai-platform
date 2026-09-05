//! Who registered with a venue, under which terms, and where the credential
//! lives — the honest counterpart to a scraper that "self-registers".
//!
//! The request this module answers was for a service that signs up for
//! exchange and venue APIs on its own, anonymously. That is refused, and the
//! refusal is structural rather than a comment: automated anonymous
//! registration circumvents the venue's terms and its identity checks, and
//! this platform's rules put the reading of a vendor's terms on the owner
//! (ADR 0034, ADR 0040) and the licensing posture *before* use
//! (`.claude/rules/domains/data-and-streaming.md`). A licence nobody read is
//! one nobody can be held to, and an account nobody owns is one nobody can
//! be asked about.
//!
//! What can be built instead is a record of a registration a person made:
//!
//! * [`RegistrationRequirement`] — what a source demands before it may be
//!   read, from nothing at all to an account that passed the venue's
//!   identity verification. Declared per source, never inferred from whether
//!   a request happened to succeed.
//! * [`RegistrationRecord`] — the fact that an operator, named, registered
//!   under the venue's terms, which they read at a stated instant, and put
//!   the resulting credential in Secret Manager under a stated name. The only
//!   constructor is [`RegistrationRecord::new`] and it refuses a blank
//!   operator; the `Deserialize` impl goes through the same constructor, so a
//!   record with no operator cannot arrive from a file either.
//! * [`RegistrationRegistry`] — the requirements and the records, keyed by
//!   source id in a `BTreeMap` so every refusal is reproducible, and the one
//!   question the admission gates ask: [`RegistrationRegistry::standing`].
//!
//! The credential never appears here. The record names the deployment
//! variable the connector manifest reads the credential from, whose `_FILE`
//! variant is where the Secret Manager projection lands, and that name is
//! screened by [`SecretRef`] — the ingestion manifest's own credential-shape
//! screen — so a pasted key is refused without being echoed.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_market_ingestion::connector::manifest::SecretRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a source demands of whoever reads it.
///
/// Declared per source from the venue's own documentation, and recorded even
/// when the answer is "nothing", because an absent requirement is a question
/// nobody asked rather than a source that needs no key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationRequirement {
    /// A public endpoint: no key, no account, no signup.
    Keyless,
    /// A key issued from a self-service developer page, under an account the
    /// venue can name — still a person's account, still the venue's terms.
    SelfServiceApiKey,
    /// An account with the venue, opened in the operator's own name.
    Account,
    /// An account that has passed the venue's identity verification.
    AccountWithIdentityVerification,
}

impl RegistrationRequirement {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Keyless => "keyless",
            Self::SelfServiceApiKey => "self_service_api_key",
            Self::Account => "account",
            Self::AccountWithIdentityVerification => "account_with_identity_verification",
        }
    }

    /// Whether a [`RegistrationRecord`] must exist before the source is read.
    pub const fn needs_registration(&self) -> bool {
        !matches!(self, Self::Keyless)
    }

    /// What the requirement asks for, in the words the refusal uses.
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Keyless => "no credential; the endpoint is public",
            Self::SelfServiceApiKey => {
                "an API key issued from the venue's self-service developer page, under an \
                 account the venue can name"
            }
            Self::Account => "an account with the venue, opened in the operator's own name",
            Self::AccountWithIdentityVerification => {
                "an account with the venue that has passed the venue's identity verification"
            }
        }
    }
}

/// The sentence every registration refusal carries, verbatim, so an operator
/// and a test can look for one delimited phrase rather than a paraphrase.
pub const NOT_OFFERED: &str =
    "anonymous or automated registration is not a path this platform offers";

/// A registration a named person made, under terms they read.
///
/// Private fields and one constructor. There is no way to build one without
/// an operator, and the operator is the point: it is the name the venue can
/// hold to its terms, and the name this platform's audit trail attributes
/// the credential to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RegistrationRecordWire", into = "RegistrationRecordWire")]
pub struct RegistrationRecord {
    source_id: String,
    operator: String,
    terms_read_at: Timestamp,
    terms: String,
    secret: SecretRef,
}

/// The on-disk shape. Deserialising goes through [`RegistrationRecord::new`],
/// so a file with an empty operator is refused at load and not discovered
/// when the refusal message has nobody to name.
#[derive(Serialize, Deserialize)]
struct RegistrationRecordWire {
    source_id: String,
    operator: String,
    terms_read_at: Timestamp,
    terms: String,
    secret: SecretRef,
}

impl TryFrom<RegistrationRecordWire> for RegistrationRecord {
    type Error = Error;

    fn try_from(wire: RegistrationRecordWire) -> Result<Self> {
        Self::new(
            wire.source_id,
            wire.operator,
            wire.terms_read_at,
            wire.terms,
            wire.secret,
        )
    }
}

impl From<RegistrationRecord> for RegistrationRecordWire {
    fn from(record: RegistrationRecord) -> Self {
        Self {
            source_id: record.source_id,
            operator: record.operator,
            terms_read_at: record.terms_read_at,
            terms: record.terms,
            secret: record.secret,
        }
    }
}

impl RegistrationRecord {
    /// Record a registration.
    ///
    /// * `operator` — who registered, in a form the venue and the audit trail
    ///   both recognise. Blank is refused: an unattributed registration is
    ///   the anonymous one this module exists to refuse.
    /// * `terms_read_at` — the instant the operator read the venue's terms.
    ///   Carried so that a later change in the terms has a date to be
    ///   compared against.
    /// * `terms` — the URL or document name of what was read. Blank is
    ///   refused: "the terms" without a citation is a claim nobody can
    ///   re-read.
    /// * `secret` — the deployment variable the connector manifest reads the
    ///   credential from; the Secret Manager secret is projected to the file
    ///   `<VARIABLE>_FILE` names, and `qip_core::secret` resolves it. Passed
    ///   as a [`SecretRef`] so the value cannot be passed at all: the type
    ///   refuses anything that is not a `SCREAMING_SNAKE_CASE` variable name,
    ///   which a key is not.
    pub fn new(
        source_id: impl Into<String>,
        operator: impl Into<String>,
        terms_read_at: Timestamp,
        terms: impl Into<String>,
        secret: SecretRef,
    ) -> Result<Self> {
        let source_id = source_id.into();
        if source_id.trim().is_empty() {
            return Err(Error::invalid(
                "a registration record must name the source it registers",
            ));
        }
        let operator = operator.into();
        if operator.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a registration record for `{source_id}` must name the operator who registered \
                 with the venue; {NOT_OFFERED}, and a record with nobody's name on it is exactly \
                 that"
            )));
        }
        let terms = terms.into();
        if terms.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a registration record for `{source_id}` must cite the terms the operator read \
                 — a URL or a document name — or nobody can re-read them when they change"
            )));
        }
        // The shape screen is the manifest's, so the record and the manifest
        // agree on what a credential name looks like; validated again here
        // because a `SecretRef` that arrived by deserialisation was not
        // screened on the way in.
        secret.validate()?;
        Ok(Self {
            source_id,
            operator,
            terms_read_at,
            terms,
            secret,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
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

    /// The name the credential is read under. Never the value.
    pub fn secret(&self) -> &SecretRef {
        &self.secret
    }

    /// One line for a banner or a decision record.
    pub fn describe(&self) -> String {
        format!(
            "registered by {} under {} read at {}, credential named `{}`",
            self.operator,
            self.terms,
            self.terms_read_at.to_rfc3339(),
            self.secret.variable()
        )
    }
}

/// What the registry found for a source that passed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "standing", rename_all = "snake_case")]
pub enum RegistrationStanding {
    /// The source needs no registration.
    Keyless,
    /// The source needs one and a named person made it.
    Registered { record: RegistrationRecord },
}

impl RegistrationStanding {
    pub fn describe(&self) -> String {
        match self {
            Self::Keyless => "keyless; no registration needed".to_string(),
            Self::Registered { record } => record.describe(),
        }
    }
}

/// The requirements and the records, by source id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRegistry {
    requirements: BTreeMap<String, RegistrationRequirement>,
    records: BTreeMap<String, RegistrationRecord>,
}

impl RegistrationRegistry {
    /// Nothing declared and nothing recorded.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The requirements of the four sources this build carries connectors
    /// for, and no records: a record is the owner's to make.
    ///
    /// # coinbase-spot-ticker and frankfurter-ecb-reference-rates — keyless
    ///
    /// Both manifests declare `auth: { scheme: none }` and describe the
    /// endpoint as free, unauthenticated, no signup. That is the definition
    /// of [`RegistrationRequirement::Keyless`].
    ///
    /// # alpaca-daily-bars — an account, and possibly more
    ///
    /// The manifest reads two credentials (`QIP_ALPACA_API_KEY_ID` and
    /// `QIP_ALPACA_API_SECRET_KEY`) and says "an account is required"; ADR
    /// 0034 adds that the market-data terms and the paper brokerage are the
    /// same account. Nothing in this repository states whether opening that
    /// account involves the venue's identity verification, and this table
    /// does not guess: it records [`RegistrationRequirement::Account`] and
    /// notes that the requirement may be raised to
    /// `AccountWithIdentityVerification` when the owner reads the terms — a
    /// one-line change here, reviewed like code.
    ///
    /// # kalshi-markets — an account, fail closed
    ///
    /// The manifest reads an unauthenticated endpoint (`auth: none`), which
    /// would read as keyless. But Kalshi is a CFTC-regulated designated
    /// contract market whose API terms have not been read (ADR 0034), and
    /// whether an anonymous reader of the public list is bound by or
    /// permitted under those terms is exactly what the unread terms would
    /// say. The restrictive default is the safe one: `Account` until the
    /// owner has read them, with the same note as Alpaca about raising it.
    pub fn shipped() -> Self {
        Self::empty()
            .with_requirement("coinbase-spot-ticker", RegistrationRequirement::Keyless)
            .with_requirement(
                "frankfurter-ecb-reference-rates",
                RegistrationRequirement::Keyless,
            )
            .with_requirement("alpaca-daily-bars", RegistrationRequirement::Account)
            .with_requirement("kalshi-markets", RegistrationRequirement::Account)
    }

    /// Declare what a source demands.
    pub fn with_requirement(
        mut self,
        source_id: impl Into<String>,
        requirement: RegistrationRequirement,
    ) -> Self {
        self.requirements.insert(source_id.into(), requirement);
        self
    }

    /// Record a registration an operator made.
    ///
    /// A record for a source with no declared requirement is refused: the
    /// record says a person registered, but not for what, and a registry
    /// that accepted it would let a later `Keyless` declaration erase the
    /// fact that anyone had to.
    pub fn with_record(mut self, record: RegistrationRecord) -> Result<Self> {
        if !self.requirements.contains_key(record.source_id()) {
            return Err(Error::invalid(format!(
                "a registration is recorded for `{}` but no registration requirement is declared \
                 for it; declare the requirement first so the record has something to satisfy",
                record.source_id()
            )));
        }
        self.records.insert(record.source_id().to_string(), record);
        Ok(self)
    }

    pub fn requirement(&self, source_id: &str) -> Option<RegistrationRequirement> {
        self.requirements.get(source_id).copied()
    }

    pub fn record(&self, source_id: &str) -> Option<&RegistrationRecord> {
        self.records.get(source_id)
    }

    /// Where a source stands: keyless, registered by a named person, or
    /// refused.
    ///
    /// Refused when no requirement is declared — an unasked question is not
    /// a keyless source — and when the requirement needs a record and none
    /// exists. The second refusal names who must register and says, in so
    /// many words, that the platform will not do it for them.
    pub fn standing(&self, source_id: &str) -> Result<RegistrationStanding> {
        let requirement = self.requirement(source_id).ok_or_else(|| {
            Error::denied(format!(
                "`{source_id}` has no registration requirement declared, so whether it needs an \
                 account is unknown and it is refused; declare `keyless` explicitly if the \
                 endpoint is public, or the requirement the venue's documentation states"
            ))
        })?;
        self.standing_under(source_id, requirement)
    }

    /// The same question where the endpoint's own declaration stands in for
    /// an undeclared requirement.
    ///
    /// The finder assesses candidates nobody has written a requirement for,
    /// and one fact about them is not a guess: whether the endpoint declared
    /// that it needs a credential. An undeclared source that needs none is
    /// keyless by its own description; an undeclared source that needs one
    /// is refused until somebody says what kind of registration issues it.
    pub fn standing_for_endpoint(
        &self,
        source_id: &str,
        endpoint_needs_credential: bool,
    ) -> Result<RegistrationStanding> {
        match self.requirement(source_id) {
            Some(requirement) => self.standing_under(source_id, requirement),
            None if endpoint_needs_credential => Err(Error::denied(format!(
                "`{source_id}` needs a credential and no registration requirement is declared \
                 for it, so who must register and under what terms is unknown; declare the \
                 requirement with `FinderConfig::with_registration_requirement` and record the \
                 registration the operator made. {NOT_OFFERED}"
            ))),
            None => Ok(RegistrationStanding::Keyless),
        }
    }

    fn standing_under(
        &self,
        source_id: &str,
        requirement: RegistrationRequirement,
    ) -> Result<RegistrationStanding> {
        if !requirement.needs_registration() {
            return Ok(RegistrationStanding::Keyless);
        }
        match self.record(source_id) {
            Some(record) => Ok(RegistrationStanding::Registered {
                record: record.clone(),
            }),
            None => Err(Error::denied(format!(
                "`{source_id}` requires {} (requirement `{}`) and no registration record exists \
                 for it, so it is refused. The platform's owner must register with the venue \
                 under their own identity, read its terms, create the credential in the venue's \
                 dashboard, place it in Secret Manager as a `_FILE`-projected secret, and record \
                 the registration — see docs/operations/registering-a-venue.md. {NOT_OFFERED}: \
                 it circumvents the venue's terms and identity checks, and a licence nobody read \
                 is one nobody can be held to",
                requirement.describe(),
                requirement.as_str()
            ))),
        }
    }
}

// The workspace denies `panic_in_result_fn` for production code; a test that
// returns `Result` so it can use `?` on the constructor under test still has
// to assert, and the abort is its reporting mechanism.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn read_at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn secret() -> Result<SecretRef> {
        SecretRef::new("QIP_VENUE_API_KEY")
    }

    #[test]
    fn a_record_without_an_operator_cannot_be_built_by_constructor_or_by_deserialisation()
    -> Result<()> {
        // Premise: the same inputs with an operator build a record, so the
        // refusals below are about the operator and nothing else.
        let built = RegistrationRecord::new(
            "venue-feed",
            "d.roderiques",
            read_at(),
            "https://venue.example/terms",
            secret()?,
        )?;
        assert_eq!(built.operator(), "d.roderiques");

        for blank in ["", "   ", "\t"] {
            let refused = RegistrationRecord::new(
                "venue-feed",
                blank,
                read_at(),
                "https://venue.example/terms",
                secret()?,
            )
            .expect_err("a record with no operator was built, which is the anonymous registration");
            assert!(
                refused.message().contains(NOT_OFFERED),
                "the refusal does not say why an operator is required: {}",
                refused.message()
            );
        }

        // The deserialiser is the other door, and it goes through the same
        // constructor: a file that omits the operator's name is refused at
        // load, not discovered when the refusal has nobody to name.
        let file = serde_json::json!({
            "source_id": "venue-feed",
            "operator": "",
            "terms_read_at": built.terms_read_at(),
            "terms": "https://venue.example/terms",
            "secret": { "variable": "QIP_VENUE_API_KEY" },
        });
        let refused = serde_json::from_value::<RegistrationRecord>(file)
            .expect_err("a record with a blank operator was deserialised");
        assert!(
            refused.to_string().contains(NOT_OFFERED),
            "the load-time refusal is not the constructor's: {refused}"
        );

        // And the round trip of a good one is byte-identical, so the
        // `try_from` path costs nothing a config file would notice.
        let text = serde_json::to_string(&built)?;
        let back: RegistrationRecord = serde_json::from_str(&text)?;
        assert_eq!(back, built);
        Ok(())
    }

    #[test]
    fn a_record_carrying_a_key_shaped_secret_reference_is_refused_without_echoing_it() -> Result<()>
    {
        // Premise: the record accepts a variable name, so the refusals below
        // are about the shape of the reference.
        RegistrationRecord::new(
            "venue-feed",
            "d.roderiques",
            read_at(),
            "https://venue.example/terms",
            SecretRef::new("QIP_VENUE_API_KEY")?,
        )?;

        // Three shapes a pasted credential takes. Each is refused at
        // `SecretRef::new` — the manifest's screen — and the refusal must
        // not repeat the value: it is a key, and the error goes to stderr
        // and the health detail.
        let pasted = [
            "sk-live-9f2a7c1e4b8d",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "QIP_VENUE_API_KEY=abc123",
        ];
        for value in pasted {
            let refused = SecretRef::new(value)
                .expect_err("a key-shaped value was accepted as a secret reference");
            assert!(
                !refused.message().contains(value),
                "the refusal echoed the value it refused: {}",
                refused.message()
            );
        }

        // A reference that was not screened on the way in — the
        // deserialisation path builds a `SecretRef` from any string — is
        // screened by the record's constructor, so the wire form cannot
        // smuggle a key past the door the constructor guards.
        let file = serde_json::json!({
            "source_id": "venue-feed",
            "operator": "d.roderiques",
            "terms_read_at": read_at(),
            "terms": "https://venue.example/terms",
            "secret": { "variable": "sk-live-9f2a7c1e4b8d" },
        });
        let refused = serde_json::from_value::<RegistrationRecord>(file)
            .expect_err("a key-shaped secret reference was deserialised into a record");
        assert!(
            refused
                .to_string()
                .contains("not a deployment variable name"),
            "the refusal is not the shape screen's: {refused}"
        );
        assert!(!refused.to_string().contains("sk-live"));
        Ok(())
    }

    #[test]
    fn a_source_needing_an_account_is_refused_without_a_record_naming_the_requirement() -> Result<()>
    {
        let registry = RegistrationRegistry::empty()
            .with_requirement("venue-feed", RegistrationRequirement::Account);
        // Premise: the requirement is one that needs a record.
        assert!(RegistrationRequirement::Account.needs_registration());

        let refused = registry
            .standing("venue-feed")
            .expect_err("a source needing an account was admitted with nobody registered");
        let message = refused.message();
        assert!(
            message.contains("requirement `account`"),
            "the refusal does not name the requirement: {message}"
        );
        assert!(
            message.contains("owner must register with the venue under their own identity"),
            "the refusal does not say who must register: {message}"
        );
        assert!(
            message.contains(NOT_OFFERED),
            "the refusal does not say that anonymous registration is not offered: {message}"
        );

        // With a record, the standing carries the record — the operator's
        // name reaches the banner, not just a boolean.
        let registered = registry.with_record(RegistrationRecord::new(
            "venue-feed",
            "d.roderiques",
            read_at(),
            "https://venue.example/terms",
            secret()?,
        )?)?;
        match registered.standing("venue-feed")? {
            RegistrationStanding::Registered { record } => {
                assert_eq!(record.operator(), "d.roderiques");
            }
            RegistrationStanding::Keyless => panic!("an account source stood as keyless"),
        }
        Ok(())
    }

    #[test]
    fn a_keyless_source_stands_as_keyless_and_an_undeclared_one_is_refused() -> Result<()> {
        let registry = RegistrationRegistry::empty()
            .with_requirement("public-feed", RegistrationRequirement::Keyless);
        assert_eq!(
            registry.standing("public-feed")?,
            RegistrationStanding::Keyless
        );

        // Unknown is not keyless: a source nobody declared a requirement for
        // is refused, and the refusal says what to declare.
        let refused = registry
            .standing("unlisted-feed")
            .expect_err("a source with no declared requirement was treated as keyless");
        assert!(
            refused
                .message()
                .contains("no registration requirement declared"),
            "{}",
            refused.message()
        );

        // The endpoint-informed question: an undeclared source whose endpoint
        // needs nothing is keyless by its own description; one whose endpoint
        // needs a credential is refused until someone says what registration
        // issues it.
        assert_eq!(
            registry.standing_for_endpoint("unlisted-feed", false)?,
            RegistrationStanding::Keyless
        );
        let refused = registry
            .standing_for_endpoint("unlisted-feed", true)
            .expect_err("an undeclared credentialed source was treated as keyless");
        assert!(refused.message().contains(NOT_OFFERED));
        Ok(())
    }

    #[test]
    fn a_record_for_a_source_with_no_declared_requirement_is_refused() -> Result<()> {
        let refused = RegistrationRegistry::empty()
            .with_record(RegistrationRecord::new(
                "venue-feed",
                "d.roderiques",
                read_at(),
                "https://venue.example/terms",
                secret()?,
            )?)
            .expect_err("a record was accepted for a source whose requirement nobody declared");
        assert!(
            refused
                .message()
                .contains("no registration requirement is declared"),
            "{}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn the_shipped_registry_declares_every_known_source_and_records_nobody() {
        // Premise: the registry has every source the build carries a
        // connector for, so a source the gate is asked about is never
        // refused as undeclared by accident.
        let shipped = RegistrationRegistry::shipped();
        for source_id in qip_market_ingestion::connector_feed::KNOWN_SOURCES {
            assert!(
                shipped.requirement(source_id).is_some(),
                "{source_id} has no registration requirement declared"
            );
            assert!(
                shipped.record(source_id).is_none(),
                "{source_id} ships with a registration record, which only the owner can make"
            );
        }
        assert_eq!(
            shipped.requirement("coinbase-spot-ticker"),
            Some(RegistrationRequirement::Keyless)
        );
        assert_eq!(
            shipped.requirement("frankfurter-ecb-reference-rates"),
            Some(RegistrationRequirement::Keyless)
        );
        // The two ADR 0034 candidates need a person's account, and the table
        // says so even though one of them reads an unauthenticated endpoint.
        for source_id in ["alpaca-daily-bars", "kalshi-markets"] {
            assert!(
                shipped
                    .requirement(source_id)
                    .is_some_and(|requirement| requirement.needs_registration()),
                "{source_id} is declared keyless, and its venue requires an account"
            );
        }
    }
}
