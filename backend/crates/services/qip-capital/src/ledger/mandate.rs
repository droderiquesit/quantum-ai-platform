//! The terms a user's capital is managed under (blueprint §43.3).
//!
//! A mandate is the object the attribution chain terminates in before the
//! user — `Strategy → StrategyFamily → Mandate → User` — and the object an
//! entitlement is evaluated against. Every field is validated at
//! construction and none is corrected: a share of 1.2 is a caller that
//! confused percent with fraction, and a mandate that silently clamped it to
//! 1 would explore with all of that user's capital while the caller believed
//! it was exploring with a fifth.

use super::identity::Jurisdiction;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Which strategy families a mandate lets capital reach.
///
/// `Only` is never empty — a mandate that permits no family is a mandate
/// nothing can invest under, and the caller almost certainly meant something
/// else. `Any` is for the platform's own desk, whose family gate is the
/// lifecycle's promotion rather than a user's choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermittedFamilies {
    Any,
    Only(BTreeSet<String>),
}

impl PermittedFamilies {
    pub fn permits(&self, family: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Only(families) => families.contains(family),
        }
    }
}

/// The unvalidated terms, as a caller or a stored record states them.
///
/// Public fields so a record can be written down; a [`Mandate`] is made from
/// one only through [`Mandate::new`], and deserialising a `Mandate` goes
/// through the same validation, so a stored record that has gone bad is
/// refused on the way back in rather than trusted because it was once ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MandateTerms {
    /// The capital the user has placed under management. Non-negative;
    /// zero is a mandate that can view and not invest.
    pub capital: Decimal,
    pub currency: Currency,
    /// The share of capital the user will tolerate losing, in `[0, 1]`.
    /// A fraction rather than a label, because the allocator sizes against
    /// a number and a label is a number somebody else picks later.
    pub risk_tolerance: Decimal,
    pub permitted_families: PermittedFamilies,
    /// Capital that stays liquid however the strategies are sized.
    /// Non-negative and at most `capital`, because a floor above the
    /// capital is a floor that can never be met.
    pub liquidity_floor: Decimal,
    /// The share of capital that may be spent on information gain rather
    /// than expected return, in `[0, 1]` (blueprint §43.3,
    /// `ExplorationProbe`).
    pub exploration_share: Decimal,
    pub jurisdiction: Jurisdiction,
}

/// A validated mandate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "MandateTerms", into = "MandateTerms")]
pub struct Mandate {
    terms: MandateTerms,
}

impl Mandate {
    /// Validate the terms, refusing by field name.
    pub fn new(terms: MandateTerms) -> Result<Self> {
        if terms.capital.is_negative() {
            return Err(Error::invalid(format!(
                "a mandate's capital cannot be negative ({}); a user who owes the platform \
                 is a reconciliation finding, not a mandate",
                terms.capital
            )));
        }
        if !is_unit_fraction(terms.risk_tolerance) {
            return Err(Error::invalid(format!(
                "risk_tolerance must be a fraction in [0, 1], not {}; if this was a \
                 percentage, divide by one hundred",
                terms.risk_tolerance
            )));
        }
        if let PermittedFamilies::Only(families) = &terms.permitted_families {
            if families.is_empty() {
                return Err(Error::invalid(
                    "permitted_families names no family; a mandate nothing can invest under \
                     is refused rather than enrolled — name at least one family, or use \
                     PermittedFamilies::Any for the desk",
                ));
            }
            if let Some(blank) = families.iter().find(|f| f.trim().is_empty()) {
                return Err(Error::invalid(format!(
                    "permitted_families contains the blank family {blank:?}; every family \
                     must be named"
                )));
            }
        }
        if terms.liquidity_floor.is_negative() {
            return Err(Error::invalid(format!(
                "liquidity_floor cannot be negative ({})",
                terms.liquidity_floor
            )));
        }
        if terms.liquidity_floor > terms.capital {
            return Err(Error::invalid(format!(
                "liquidity_floor {} exceeds the mandate's capital {}; a floor above the \
                 capital can never be met, so every sizing under it would be refused",
                terms.liquidity_floor, terms.capital
            )));
        }
        if !is_unit_fraction(terms.exploration_share) {
            return Err(Error::invalid(format!(
                "exploration_share must be a fraction in [0, 1], not {}",
                terms.exploration_share
            )));
        }
        Ok(Self { terms })
    }

    /// The platform's own desk: every family permitted, no exploration set
    /// aside, no liquidity floor, and a jurisdiction of `ZZ` — the ISO 3166
    /// user-assigned range, so it can never collide with a real one and an
    /// eligibility table that lists real jurisdictions never admits the desk
    /// by accident.
    ///
    /// This is the mandate a kernel books under until users exist, so that
    /// wiring the ledger changed nothing about what the platform already
    /// did. Capital is the book's opening equity; a negative one is refused
    /// like any other.
    pub fn desk(capital: Decimal, currency: Currency) -> Result<Self> {
        Self::new(MandateTerms {
            capital,
            currency,
            risk_tolerance: Decimal::ONE,
            permitted_families: PermittedFamilies::Any,
            liquidity_floor: Decimal::ZERO,
            exploration_share: Decimal::ZERO,
            jurisdiction: Jurisdiction::new("ZZ")?,
        })
    }

    pub fn capital(&self) -> Decimal {
        self.terms.capital
    }

    pub fn currency(&self) -> Currency {
        self.terms.currency
    }

    pub fn risk_tolerance(&self) -> Decimal {
        self.terms.risk_tolerance
    }

    pub fn permitted_families(&self) -> &PermittedFamilies {
        &self.terms.permitted_families
    }

    pub fn liquidity_floor(&self) -> Decimal {
        self.terms.liquidity_floor
    }

    pub fn exploration_share(&self) -> Decimal {
        self.terms.exploration_share
    }

    pub fn jurisdiction(&self) -> Jurisdiction {
        self.terms.jurisdiction
    }

    /// The capital a mandate can put to work: what is under management
    /// less the floor that stays liquid.
    pub fn investable(&self) -> Decimal {
        self.terms.capital - self.terms.liquidity_floor
    }

    pub fn terms(&self) -> &MandateTerms {
        &self.terms
    }
}

impl TryFrom<MandateTerms> for Mandate {
    type Error = Error;

    fn try_from(terms: MandateTerms) -> Result<Self> {
        Self::new(terms)
    }
}

impl From<Mandate> for MandateTerms {
    fn from(mandate: Mandate) -> Self {
        mandate.terms
    }
}

fn is_unit_fraction(value: Decimal) -> bool {
    !value.is_negative() && value <= Decimal::ONE
}
