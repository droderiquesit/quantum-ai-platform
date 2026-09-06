//! Who may have capital put to work, as an operator's decision on the record
//! (blueprint §43.3: identity verified, mandate registered, jurisdiction and
//! the `can_invest` flag).
//!
//! The failure this file prevents: `UserLedger::admit` evaluated a product's
//! eligibility in the user's jurisdiction, and this platform held no
//! eligibility registry, so the kernel could not call it without either
//! refusing every request or inventing an eligibility to pass the gate — and
//! it did neither, funding a user on the mandate alone. A mandate says whose
//! capital it is; it does not say that anybody checked who they are. Until
//! this registry existed nothing did.
//!
//! An [`Eligibility`] is a record that an operator verified a user — when,
//! where, whether they may invest, and until when. It is created and revoked
//! only by an [`EligibilityDecision`] carrying a [`DecidedBy`], the operator
//! who took it, and `DecidedBy` has no `Default` and refuses a blank subject:
//! there is no way to grant an eligibility from a bare `true` in a
//! configuration file, because a flag nobody signed is a decision nobody
//! took. The registry keeps the latest decision per user in a [`BTreeMap`]
//! and answers [`EligibilityRegistry::admit`] with an [`Ineligible`] reason
//! by name — unknown user, revoked, not yet verified, cannot invest,
//! expired, jurisdiction absent — so a refusal is a value a reviewer can
//! group on and a test can assert.
//!
//! # There is no `can_withdraw` here
//!
//! Blueprint §43.3 names `can_withdraw` beside `can_invest`. It is absent
//! from [`Eligibility`] on purpose, and not as a field that is always
//! `false`: ADR 0021 refuses the path by which capital leaves the platform,
//! and a field — however it is set — is a value a transfer path could one
//! day read. The refusal lives in
//! [`WithdrawalEntitlement`](super::entitlement::WithdrawalEntitlement), a
//! type with one variant, and nothing in this file gives it a second.

use super::identity::{Jurisdiction, UserId};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// The operator who took an eligibility decision, as an authenticated
/// identity names them.
///
/// The fields mirror what the risk engine's operator identity exposes — the
/// subject and method the authentication system reported, and a second
/// approver where one signed — without depending on that crate, because a
/// ledger record has to outlive the type that produced it. There is no
/// `Default` and no constructor from nothing: [`DecidedBy::operator`]
/// refuses a blank subject or method, and deserialising goes through the
/// same check, so a stored record that names nobody is refused on the way
/// back in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DecidedByRecord", into = "DecidedByRecord")]
pub struct DecidedBy {
    subject: String,
    method: String,
    second_approver: Option<String>,
}

/// The stored form of a [`DecidedBy`]: public fields so a record can be
/// written down, validated on the way back into a `DecidedBy`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedByRecord {
    pub subject: String,
    pub method: String,
    #[serde(default)]
    pub second_approver: Option<String>,
}

impl DecidedBy {
    /// Name the operator, refusing a subject or method that is blank.
    ///
    /// The refusal is what makes the identity load-bearing: a decision must
    /// name a person an audit can ask, and a blank subject is a decision
    /// attributed to the configuration file.
    pub fn operator(subject: impl Into<String>, method: impl Into<String>) -> Result<Self> {
        let subject = subject.into();
        let method = method.into();
        if subject.trim().is_empty() {
            return Err(Error::invalid(
                "an eligibility decision names no operator; it must carry the authenticated \
                 subject who took it — a flag in a configuration file is not a decision",
            ));
        }
        if method.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the eligibility decision by {subject} names no authentication method; an \
                 identity nobody authenticated is a name, not an operator"
            )));
        }
        Ok(Self {
            subject,
            method,
            second_approver: None,
        })
    }

    /// Record a second operator's approval. A second approver who is the
    /// same person is not a second approver, and is dropped rather than
    /// recorded as one.
    pub fn with_second_approver(mut self, approver: impl Into<String>) -> Self {
        let approver = approver.into();
        if !approver.trim().is_empty() && approver != self.subject {
            self.second_approver = Some(approver);
        }
        self
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn second_approver(&self) -> Option<&str> {
        self.second_approver.as_deref()
    }
}

impl TryFrom<DecidedByRecord> for DecidedBy {
    type Error = Error;

    fn try_from(record: DecidedByRecord) -> Result<Self> {
        let by = Self::operator(record.subject, record.method)?;
        Ok(match record.second_approver {
            Some(approver) => by.with_second_approver(approver),
            None => by,
        })
    }
}

impl From<DecidedBy> for DecidedByRecord {
    fn from(by: DecidedBy) -> Self {
        Self {
            subject: by.subject,
            method: by.method,
            second_approver: by.second_approver,
        }
    }
}

/// The unvalidated terms of an eligibility, as a caller or a stored record
/// states them. An [`Eligibility`] is made from one only through
/// [`Eligibility::new`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EligibilityTerms {
    /// When the operator verified the user's identity.
    pub verified_at: Timestamp,
    /// Whether the user may have capital put to work. `false` is a verified
    /// user who may view their books and nothing more.
    pub can_invest: bool,
    /// The jurisdiction the user was verified in. Admission requires it to
    /// be the mandate's, because an eligibility verified in one place is not
    /// evidence about another.
    pub jurisdiction: Jurisdiction,
    /// When the verification lapses and must be taken again. Every
    /// eligibility expires: one without an end is a check nobody repeats.
    pub expires_at: Timestamp,
}

/// A validated eligibility: an operator's statement that a user was
/// verified, where, whether they may invest, and until when.
///
/// See the module comment for why there is no withdrawal field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EligibilityTerms", into = "EligibilityTerms")]
pub struct Eligibility {
    terms: EligibilityTerms,
}

impl Eligibility {
    /// Validate the terms, refusing an expiry that is not after the
    /// verification: such a record is expired the instant it is verified,
    /// and an eligibility that never admits anyone is a caller bug rather
    /// than a decision.
    pub fn new(terms: EligibilityTerms) -> Result<Self> {
        if terms.expires_at <= terms.verified_at {
            return Err(Error::invalid(format!(
                "an eligibility verified at {} and expiring at {} would never admit anyone; \
                 the expiry must be after the verification",
                terms.verified_at, terms.expires_at
            )));
        }
        Ok(Self { terms })
    }

    pub fn verified_at(&self) -> Timestamp {
        self.terms.verified_at
    }

    pub fn can_invest(&self) -> bool {
        self.terms.can_invest
    }

    pub fn jurisdiction(&self) -> Jurisdiction {
        self.terms.jurisdiction
    }

    pub fn expires_at(&self) -> Timestamp {
        self.terms.expires_at
    }

    pub fn terms(&self) -> &EligibilityTerms {
        &self.terms
    }
}

impl TryFrom<EligibilityTerms> for Eligibility {
    type Error = Error;

    fn try_from(terms: EligibilityTerms) -> Result<Self> {
        Self::new(terms)
    }
}

impl From<Eligibility> for EligibilityTerms {
    fn from(eligibility: Eligibility) -> Self {
        eligibility.terms
    }
}

/// What an operator decided about a user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum EligibilityDecision {
    /// The user is eligible on these terms, superseding any earlier record.
    Granted { eligibility: Eligibility },
    /// The user's eligibility is withdrawn. The record stays in the registry
    /// as revoked rather than being removed, so "never verified" and
    /// "verified and then revoked" are different answers.
    Revoked { reason: String },
}

/// One decision as the registry records it: for whom, what, by whom, when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EligibilityRecord {
    pub user: UserId,
    pub decision: EligibilityDecision,
    pub by: DecidedBy,
    pub decided_at: Timestamp,
}

/// Why a user is not eligible, by name.
///
/// One variant per gate, in the order the registry runs them, so a refusal
/// is a value a reviewer can group on. [`Ineligible::name`] is the stable
/// token a journal carries and [`Ineligible::describe`] the sentence a
/// person reads; the sentence embeds the token in parentheses so a reader
/// of either finds the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Ineligible {
    /// The user holds no mandate; eligibility is a statement about a
    /// mandate holder.
    NoMandate,
    /// No operator has decided anything about this user.
    UnknownUser,
    /// The latest decision revoked the eligibility.
    Revoked,
    /// The verification is dated after the instant asked about.
    NotYetVerified,
    /// Verified, and cleared to view only.
    CannotInvest,
    /// The verification has lapsed.
    Expired,
    /// The mandate's jurisdiction is absent from the eligibility, which
    /// was verified somewhere else.
    JurisdictionAbsent {
        verified: Jurisdiction,
        mandate: Jurisdiction,
    },
}

impl Ineligible {
    /// The stable token for a journal or a metric label.
    pub fn name(self) -> &'static str {
        match self {
            Self::NoMandate => "no_mandate",
            Self::UnknownUser => "unknown_user",
            Self::Revoked => "revoked",
            Self::NotYetVerified => "not_yet_verified",
            Self::CannotInvest => "cannot_invest",
            Self::Expired => "expired",
            Self::JurisdictionAbsent { .. } => "jurisdiction_absent",
        }
    }

    /// The refusal as a person reads it, naming what to do instead.
    pub fn describe(self, user: &UserId) -> String {
        let name = self.name();
        match self {
            Self::NoMandate => format!(
                "{user} is not eligible ({name}): no mandate is registered, and eligibility is \
                 a statement about a mandate holder — register the mandate first"
            ),
            Self::UnknownUser => format!(
                "{user} is not eligible ({name}): no operator has verified this user; an \
                 eligibility decision must be taken and recorded before capital is put to work"
            ),
            Self::Revoked => format!(
                "{user} is not eligible ({name}): the eligibility was revoked; a new decision \
                 by an operator is required"
            ),
            Self::NotYetVerified => format!(
                "{user} is not eligible ({name}): the verification is dated after the instant \
                 asked about, so at that instant nobody had verified this user"
            ),
            Self::CannotInvest => format!(
                "{user} is not eligible ({name}): the operator cleared this user to view and \
                 not to invest"
            ),
            Self::Expired => format!(
                "{user} is not eligible ({name}): the verification has lapsed and must be \
                 taken again"
            ),
            Self::JurisdictionAbsent { verified, mandate } => format!(
                "{user} is not eligible ({name}): the eligibility was verified in {verified} \
                 and the mandate is in {mandate}; a verification in one jurisdiction is not \
                 evidence about another"
            ),
        }
    }
}

impl fmt::Display for Ineligible {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The latest eligibility decision per user, keyed by [`UserId`].
///
/// The event log holds the history; the registry holds what currently
/// stands. Two ways in, with different rules because they are different
/// things: [`EligibilityRegistry::replay`] takes *decisions* in the order
/// they were taken and refuses one out of order or a revocation of nobody,
/// while a stored registry — the serialised form — is the set of *standing*
/// records and is refused on the way back in only where it names a user
/// twice, since a standing revocation is a legitimate state the decision
/// rule would wrongly refuse. Each record's own validation (a named
/// operator, an expiry after the verification) applies on both paths.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<EligibilityRecord>", into = "Vec<EligibilityRecord>")]
pub struct EligibilityRegistry {
    records: BTreeMap<UserId, EligibilityRecord>,
}

impl EligibilityRegistry {
    /// A registry in which nobody is eligible — the honest starting state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a registry from its decisions, in the order they were taken.
    /// Refuses whatever [`Self::decide`] refuses.
    pub fn replay(records: impl IntoIterator<Item = EligibilityRecord>) -> Result<Self> {
        let mut registry = Self::new();
        for record in records {
            registry.decide(record)?;
        }
        Ok(registry)
    }

    /// The decision that currently stands for a user, if any.
    pub fn record(&self, user: &UserId) -> Option<&EligibilityRecord> {
        self.records.get(user)
    }

    /// Every standing decision, in user order.
    pub fn records(&self) -> &BTreeMap<UserId, EligibilityRecord> {
        &self.records
    }

    /// Apply a decision.
    ///
    /// Refuses a revocation of a user with no decision on record — there is
    /// nothing to revoke, and recording it would make "never verified"
    /// read as "verified and revoked" — and a decision dated before the one
    /// that stands, because decisions are applied in the order they were
    /// taken and a replay that accepted them out of order would rebuild a
    /// different registry. The operator is carried in the record; nothing
    /// here can be decided without one.
    pub fn decide(&mut self, record: EligibilityRecord) -> Result<()> {
        if let Some(standing) = self.records.get(&record.user) {
            if record.decided_at < standing.decided_at {
                return Err(Error::invalid(format!(
                    "the eligibility decision for {} at {} is earlier than the one standing \
                     at {}; decisions are applied in the order they were taken",
                    record.user, record.decided_at, standing.decided_at
                )));
            }
        } else if let EligibilityDecision::Revoked { .. } = record.decision {
            return Err(Error::invalid(format!(
                "nothing to revoke: no eligibility decision is on record for {}",
                record.user
            )));
        }
        self.records.insert(record.user.clone(), record);
        Ok(())
    }

    /// Whether a user with a mandate in `mandate_jurisdiction` may have
    /// capital put to work at `now`, and if not, why by name.
    ///
    /// The gates run in a fixed order and the first to refuse is the answer:
    /// a decision on record, not revoked, verified by `now`, cleared to
    /// invest, not yet expired, and verified in the mandate's jurisdiction.
    /// The expiry is exclusive — an eligibility is refused on the instant it
    /// expires — because "until" means until.
    pub fn admit(
        &self,
        user: &UserId,
        mandate_jurisdiction: Jurisdiction,
        now: Timestamp,
    ) -> std::result::Result<&Eligibility, Ineligible> {
        let Some(record) = self.records.get(user) else {
            return Err(Ineligible::UnknownUser);
        };
        let eligibility = match &record.decision {
            EligibilityDecision::Revoked { .. } => return Err(Ineligible::Revoked),
            EligibilityDecision::Granted { eligibility } => eligibility,
        };
        if now < eligibility.verified_at() {
            return Err(Ineligible::NotYetVerified);
        }
        if !eligibility.can_invest() {
            return Err(Ineligible::CannotInvest);
        }
        if now >= eligibility.expires_at() {
            return Err(Ineligible::Expired);
        }
        if eligibility.jurisdiction() != mandate_jurisdiction {
            return Err(Ineligible::JurisdictionAbsent {
                verified: eligibility.jurisdiction(),
                mandate: mandate_jurisdiction,
            });
        }
        Ok(eligibility)
    }
}

impl TryFrom<Vec<EligibilityRecord>> for EligibilityRegistry {
    type Error = Error;

    fn try_from(records: Vec<EligibilityRecord>) -> Result<Self> {
        let mut registry = Self::new();
        for record in records {
            if registry.records.contains_key(&record.user) {
                return Err(Error::invalid(format!(
                    "the stored eligibility registry names {} twice; a registry holds one \
                     standing decision per user, and the record is not one this registry wrote",
                    record.user
                )));
            }
            registry.records.insert(record.user.clone(), record);
        }
        Ok(registry)
    }
}

impl From<EligibilityRegistry> for Vec<EligibilityRecord> {
    fn from(registry: EligibilityRegistry) -> Self {
        registry.records.into_values().collect()
    }
}
