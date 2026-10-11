//! R12-GOV-6116 / 6118: legal permission as its own dimension, recorded on
//! every venue, product, strategy, entity and jurisdiction, and checked
//! before a strategy is deployed or a venue or instrument becomes usable.
//!
//! The failure prevented is assuming one legal entity may do everything it
//! can technically do. Before this module the only eligibility the platform
//! held was a *user's* (`qip-capital`'s ledger eligibility, checked when a
//! user is funded); a venue was usable because the platform could reach it,
//! a product because a strategy family listed it, and a strategy because its
//! gate evidence was good. None of those is a statement that anybody is
//! *permitted* to.
//!
//! [`EligibilityRegister`] holds one [`EligibilityRecord`] per subject — the
//! five kinds §25 names, [`SubjectKind`] — and every question it answers
//! fails closed:
//!
//! * a subject with no record is ineligible ([`Refusal::NoRecord`]), not
//!   presumed fine — "never assessed" and "assessed and permitted" must be
//!   different answers, and the permissive reading of silence is how a venue
//!   nobody reviewed gets traded;
//! * a record is good only in the jurisdictions it names, only between its
//!   grant and its expiry (every record expires; one that does not is a check
//!   nobody repeats), and not at all once revoked;
//! * a record names who granted it, refused blank.
//!
//! The two gates 6118 asks for are [`EligibilityRegister::admit_deployment`]
//! (a strategy, every product it trades and every venue it trades on, plus
//! the jurisdiction itself, all eligible there) and
//! [`EligibilityRegister::admit_enablement`] (a venue and an instrument's
//! product). Each returns every refusal, not the first, so an operator fixes
//! the whole list in one pass.
//!
//! No clock is read here: every question takes the instant it is asked at,
//! so a replay asks the same question and gets the same answer.

use qip_core::{Error, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The five kinds of subject §25 says carry eligibility metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Venue,
    Product,
    /// A user or legal entity.
    Entity,
    Jurisdiction,
    Strategy,
}

impl SubjectKind {
    pub const ALL: [Self; 5] = [
        Self::Venue,
        Self::Product,
        Self::Entity,
        Self::Jurisdiction,
        Self::Strategy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Venue => "venue",
            Self::Product => "product",
            Self::Entity => "entity",
            Self::Jurisdiction => "jurisdiction",
            Self::Strategy => "strategy",
        }
    }
}

/// What a record is about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub kind: SubjectKind,
    pub id: String,
}

impl Subject {
    pub fn new(kind: SubjectKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

impl std::fmt::Display for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} `{}`", self.kind.as_str(), self.id)
    }
}

/// One subject's legal permission, as granted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityRecord {
    pub subject: Subject,
    /// Where it is permitted. Empty is nowhere, never everywhere.
    pub jurisdictions: BTreeSet<String>,
    pub granted_by: String,
    pub granted_at: Timestamp,
    pub expires_at: Timestamp,
    /// Set once withdrawn; the record stays so "never permitted" and
    /// "permitted and then withdrawn" stay different answers.
    #[serde(default)]
    pub revoked: Option<String>,
}

/// Why a subject is not eligible, by name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "refusal")]
pub enum Refusal {
    NoRecord {
        subject: Subject,
    },
    Revoked {
        subject: Subject,
        reason: String,
    },
    NotYetGranted {
        subject: Subject,
    },
    Expired {
        subject: Subject,
    },
    JurisdictionAbsent {
        subject: Subject,
        jurisdiction: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRecord { subject } => {
                write!(
                    f,
                    "{subject} has no eligibility record; record one before using it"
                )
            }
            Self::Revoked { subject, reason } => write!(f, "{subject} was revoked: {reason}"),
            Self::NotYetGranted { subject } => {
                write!(f, "{subject} is granted from a later instant than this one")
            }
            Self::Expired { subject } => {
                write!(f, "{subject}'s eligibility has expired; renew it")
            }
            Self::JurisdictionAbsent {
                subject,
                jurisdiction,
            } => write!(f, "{subject} is not permitted in `{jurisdiction}`"),
        }
    }
}

/// Every subject's latest record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilityRegister {
    records: BTreeMap<Subject, EligibilityRecord>,
}

impl EligibilityRegister {
    pub fn new() -> Self {
        Self::default()
    }

    /// A register from a list of records, each validated as [`Self::record`]
    /// validates it; a subject recorded twice is refused, since which of two
    /// records in one document is meant is not a question to guess at.
    pub fn from_records(records: Vec<EligibilityRecord>) -> Result<Self, Error> {
        let mut register = Self::new();
        for record in records {
            if register.records.contains_key(&record.subject) {
                return Err(Error::invalid(format!(
                    "{} is recorded twice; keep the one that is meant",
                    record.subject
                )));
            }
            register.record(record)?;
        }
        Ok(register)
    }

    /// Record (or supersede) one subject's eligibility.
    pub fn record(&mut self, record: EligibilityRecord) -> Result<(), Error> {
        if record.subject.id.trim().is_empty() {
            return Err(Error::invalid(
                "an eligibility record names a blank subject",
            ));
        }
        if record.granted_by.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the eligibility of {} names no grantor; an unattributed permission cannot be \
                 reviewed",
                record.subject
            )));
        }
        if record.expires_at <= record.granted_at {
            return Err(Error::invalid(format!(
                "the eligibility of {} expires no later than it is granted; it would never \
                 admit anything",
                record.subject
            )));
        }
        if record.jurisdictions.is_empty()
            || record.jurisdictions.iter().any(|j| j.trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "the eligibility of {} names no jurisdiction, or a blank one; an empty list is \
                 nowhere, so record where it is permitted",
                record.subject
            )));
        }
        if record
            .revoked
            .as_deref()
            .is_some_and(|r| r.trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "the revocation of {} gives no reason",
                record.subject
            )));
        }
        self.records.insert(record.subject.clone(), record);
        Ok(())
    }

    pub fn get(&self, subject: &Subject) -> Option<&EligibilityRecord> {
        self.records.get(subject)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Whether `subject` may be used in `jurisdiction` at `at`.
    pub fn check(
        &self,
        subject: &Subject,
        jurisdiction: &str,
        at: Timestamp,
    ) -> Result<(), Refusal> {
        let Some(record) = self.records.get(subject) else {
            return Err(Refusal::NoRecord {
                subject: subject.clone(),
            });
        };
        if let Some(reason) = &record.revoked {
            return Err(Refusal::Revoked {
                subject: subject.clone(),
                reason: reason.clone(),
            });
        }
        if at < record.granted_at {
            return Err(Refusal::NotYetGranted {
                subject: subject.clone(),
            });
        }
        if at >= record.expires_at {
            return Err(Refusal::Expired {
                subject: subject.clone(),
            });
        }
        if !record.jurisdictions.contains(jurisdiction) {
            return Err(Refusal::JurisdictionAbsent {
                subject: subject.clone(),
                jurisdiction: jurisdiction.to_string(),
            });
        }
        Ok(())
    }

    /// Every refusal among `subjects` in `jurisdiction` — the jurisdiction
    /// itself always among them, since a jurisdiction the platform has not
    /// recorded as one it may operate in admits nothing inside it.
    pub fn refusals<'a>(
        &self,
        subjects: impl IntoIterator<Item = &'a Subject>,
        jurisdiction: &str,
        at: Timestamp,
    ) -> Vec<Refusal> {
        let place = Subject::new(SubjectKind::Jurisdiction, jurisdiction);
        let mut all: BTreeSet<&Subject> = subjects.into_iter().collect();
        all.insert(&place);
        all.into_iter()
            .filter_map(|s| self.check(s, jurisdiction, at).err())
            .collect()
    }

    /// R12-GOV-6118: may `strategy`, trading `products` on `venues`, be
    /// deployed in `jurisdiction` at `at`? `Err` lists every refusal.
    pub fn admit_deployment(
        &self,
        strategy: &str,
        products: &[String],
        venues: &[String],
        jurisdiction: &str,
        at: Timestamp,
    ) -> Result<(), Error> {
        if products.is_empty() || venues.is_empty() {
            return Err(Error::invalid(
                "a deployment names the products it trades and the venues it trades on; an \
                 empty list would check nothing",
            ));
        }
        let mut subjects = vec![Subject::new(SubjectKind::Strategy, strategy)];
        subjects.extend(
            products
                .iter()
                .map(|p| Subject::new(SubjectKind::Product, p.as_str())),
        );
        subjects.extend(
            venues
                .iter()
                .map(|v| Subject::new(SubjectKind::Venue, v.as_str())),
        );
        refused("deploy", &self.refusals(&subjects, jurisdiction, at))
    }

    /// R12-GOV-6118: may `venue` be enabled to trade `product` in
    /// `jurisdiction` at `at`? `Err` lists every refusal.
    pub fn admit_enablement(
        &self,
        venue: &str,
        product: &str,
        jurisdiction: &str,
        at: Timestamp,
    ) -> Result<(), Error> {
        let subjects = [
            Subject::new(SubjectKind::Venue, venue),
            Subject::new(SubjectKind::Product, product),
        ];
        refused("enable", &self.refusals(&subjects, jurisdiction, at))
    }
}

fn refused(act: &str, refusals: &[Refusal]) -> Result<(), Error> {
    if refusals.is_empty() {
        return Ok(());
    }
    let reasons: Vec<String> = refusals.iter().map(ToString::to_string).collect();
    Err(Error::denied(format!(
        "not eligible to {act}: {}",
        reasons.join("; ")
    )))
}
