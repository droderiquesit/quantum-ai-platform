//! Which strategy family may be sold where, as a compliance determination on
//! the record (blueprint §43.3).
//!
//! The failure this file prevents: funding consulted the eligibility
//! registry — an operator's decision that *this user* was verified, where,
//! and until when — and stopped there. Nothing asked whether the *product*
//! the capital was going into may be offered to that user at all.
//! [`Entitlement::evaluate`](super::entitlement::Entitlement::evaluate) has
//! always taken a [`ProductEligibility`] and the kernel had none to give, so
//! rather than invent one it evaluated no entitlement at all, and a user
//! verified in one jurisdiction could be funded into any family whatever the
//! family was cleared for.
//!
//! Nothing is cleared here by default, and the blueprint says why: which
//! capabilities may be offered to which accounts in which jurisdictions is a
//! compliance determination, not a design one. So an empty catalogue means
//! nobody has taken that determination, and a family absent from it is
//! refused everywhere — the same honest default
//! [`ProductEligibility`]'s own empty `eligible_in` carries.
//!
//! A record cleared in *no* jurisdiction is refused at
//! [`ProductCatalogue::offer`] rather than stored. Otherwise "nobody decided"
//! and "somebody decided, and cleared it nowhere" would be two states that
//! refuse identically and read identically, and the first would be
//! indistinguishable from a determination that had been taken and lost.
//!
//! # There is no `can_withdraw` here
//!
//! For the reason [`super::eligibility`] gives and ADR 0021 fixes. This
//! catalogue clears a family for *investment* in a jurisdiction; there is no
//! field, arm or argument through which anything here could clear a
//! withdrawal, and adding one would mean adding a second variant to
//! [`WithdrawalEntitlement`](super::entitlement::WithdrawalEntitlement),
//! which is the reviewed act ADR 0021 requires.

use super::entitlement::ProductEligibility;
use super::identity::Jurisdiction;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The families cleared for sale and where, keyed by family.
///
/// A [`BTreeMap`] because the catalogue reaches a rendered report and a
/// replay that reordered would not be a replay. The serialised form is the
/// list of offerings; it is refused on the way back in where it names a
/// family twice or carries a record the live path would have refused, so a
/// stored catalogue that has gone bad is not trusted because it was once
/// ours.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<ProductEligibility>", into = "Vec<ProductEligibility>")]
pub struct ProductCatalogue {
    offerings: BTreeMap<String, ProductEligibility>,
}

impl ProductCatalogue {
    /// A catalogue in which no family is cleared anywhere — the honest
    /// starting state, and the one a platform holds until compliance has
    /// taken a determination it can point at.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear a family in the jurisdictions the record names, superseding any
    /// earlier offering of the same family.
    ///
    /// Refuses a blank family — a product nobody can name is a product
    /// nothing can look up — and an offering cleared in no jurisdiction, for
    /// the reason the module comment gives: absence already means refused
    /// everywhere, and a stored empty record would be a second way to say
    /// the same thing that a reader would mistake for a decision.
    ///
    /// Superseding rather than refusing a second offering is deliberate: a
    /// jurisdiction is added to or removed from a product's clearance by
    /// compliance more than once, and the history belongs in the caller's
    /// event log rather than in this map, which holds what stands.
    pub fn offer(&mut self, product: ProductEligibility) -> Result<()> {
        if product.family.trim().is_empty() {
            return Err(Error::invalid(
                "a product offering names no family; name the strategy family it clears, \
                 because a family nothing can look up is a clearance nothing can read",
            ));
        }
        if product.eligible_in.is_empty() {
            return Err(Error::invalid(format!(
                "the offering of {} clears it in no jurisdiction, which refuses everywhere — \
                 the same answer as leaving it out of the catalogue. Name the jurisdictions it \
                 is cleared in, or record no offering at all",
                product.family
            )));
        }
        self.offerings.insert(product.family.clone(), product);
        Ok(())
    }

    /// The product to evaluate an entitlement against, for a family.
    ///
    /// A family nobody has cleared answers with an offering eligible in no
    /// jurisdiction rather than `None`, so the caller cannot forget the case:
    /// the entitlement is evaluated either way and refuses, naming the family
    /// and the jurisdiction it was refused in.
    pub fn offering(&self, family: &str) -> ProductEligibility {
        self.offerings
            .get(family)
            .cloned()
            .unwrap_or_else(|| ProductEligibility::new(family))
    }

    /// The standing offering for a family, or `None` where none was taken.
    /// Use [`Self::offering`] to evaluate; this is for a caller that needs to
    /// tell "cleared nowhere" from "never decided".
    pub fn cleared(&self, family: &str) -> Option<&ProductEligibility> {
        self.offerings.get(family)
    }

    /// Whether a family is cleared in one jurisdiction, as the gate asks it.
    pub fn clears(&self, family: &str, jurisdiction: Jurisdiction) -> bool {
        self.offerings
            .get(family)
            .is_some_and(|product| product.eligible_in.contains(&jurisdiction))
    }

    /// Every standing offering, in family order.
    pub fn offerings(&self) -> &BTreeMap<String, ProductEligibility> {
        &self.offerings
    }

    pub fn is_empty(&self) -> bool {
        self.offerings.is_empty()
    }

    pub fn len(&self) -> usize {
        self.offerings.len()
    }

    /// Rebuild a catalogue from the offerings that were taken, in order.
    /// Refuses whatever [`Self::offer`] refuses.
    pub fn replay(products: impl IntoIterator<Item = ProductEligibility>) -> Result<Self> {
        let mut catalogue = Self::new();
        for product in products {
            catalogue.offer(product)?;
        }
        Ok(catalogue)
    }
}

impl TryFrom<Vec<ProductEligibility>> for ProductCatalogue {
    type Error = Error;

    fn try_from(products: Vec<ProductEligibility>) -> Result<Self> {
        let mut catalogue = Self::new();
        for product in products {
            if catalogue.offerings.contains_key(&product.family) {
                return Err(Error::invalid(format!(
                    "the stored product catalogue names the family {} twice; a catalogue holds \
                     one standing offering per family, and the record is not one this catalogue \
                     wrote",
                    product.family
                )));
            }
            catalogue.offer(product)?;
        }
        Ok(catalogue)
    }
}

impl From<ProductCatalogue> for Vec<ProductEligibility> {
    fn from(catalogue: ProductCatalogue) -> Self {
        catalogue.offerings.into_values().collect()
    }
}
