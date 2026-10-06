//! Who may create, trade or wager on an event venue, decided before an order
//! object exists.
//!
//! A price is not a permission. A contract the platform can value may still be
//! unlawful for this entity, in this jurisdiction, on this venue, for this
//! product, so every question here answers "refused" until a record says
//! otherwise: an empty [`AccessBook`] admits nothing. That is the same
//! fail-closed rule `RegulatoryConstraints::permits_venue` now applies, where
//! an empty `approved_venues` set refuses every venue; it used to read "empty
//! means unrestricted", and an empty book here must not repeat that failure.
//!
//! The permits ([`OrderPermit`], [`CreationPermit`]) have private fields, so a
//! caller cannot hold one without passing the gate that issues it. Regulated
//! wagering is a different [`VenueKind`] with its own entry point; an order
//! sent down the event-contract path to a wagering venue (or the reverse) is
//! refused rather than re-routed, because the two carry different law.
//!
//! This module prices nothing and [`crate::belief::fair_values`] reads nothing
//! from it: a priced event is never assumed tradable, and a tradable one is
//! never priced differently for being so.

use qip_contracts::VenueId;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Which body of rules a venue operates under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VenueKind {
    /// An event-contract venue.
    Event,
    /// A regulated wagering venue, isolated from the event path.
    Wagering,
}

/// A jurisdiction code, normalised so `" us-ny "` and `US-NY` are one value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Jurisdiction(String);

impl Jurisdiction {
    pub fn new(code: &str) -> Result<Self> {
        let code = code.trim().to_ascii_uppercase();
        if code.is_empty() {
            return Err(Error::invalid(
                "a jurisdiction code is empty; name the jurisdiction",
            ));
        }
        Ok(Self(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one venue permits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenuePolicy {
    pub venue: VenueId,
    pub kind: VenueKind,
    /// Jurisdictions in which a market may be created here.
    pub creation: BTreeSet<Jurisdiction>,
    /// Jurisdictions in which an order may be placed here.
    pub trading: BTreeSet<Jurisdiction>,
    /// Products the venue permits.
    pub products: BTreeSet<String>,
    /// Youngest permitted age in years; an entity with no recorded age is
    /// refused rather than assumed to be old enough.
    pub minimum_age: Option<u8>,
}

/// What is recorded about one entity or user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRecord {
    pub jurisdiction: Jurisdiction,
    pub eligible: bool,
    pub age_years: Option<u8>,
    /// Venues this entity may use. Availability is per entity and per venue.
    pub venues: BTreeSet<VenueId>,
}

/// Proof an order passed the gate. Not constructible outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderPermit {
    venue: VenueId,
    entity: String,
    product: String,
}

impl OrderPermit {
    pub fn venue(&self) -> &VenueId {
        &self.venue
    }
    pub fn entity(&self) -> &str {
        &self.entity
    }
    pub fn product(&self) -> &str {
        &self.product
    }
}

/// Proof market creation was permitted. Not constructible outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationPermit {
    venue: VenueId,
    jurisdiction: Jurisdiction,
}

impl CreationPermit {
    pub fn venue(&self) -> &VenueId {
        &self.venue
    }
    pub fn jurisdiction(&self) -> &Jurisdiction {
        &self.jurisdiction
    }
}

/// Every policy and entity record; empty by default, so it refuses everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessBook {
    policies: BTreeMap<VenueId, VenuePolicy>,
    entities: BTreeMap<String, EntityRecord>,
}

impl AccessBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_policy(&mut self, policy: VenuePolicy) {
        self.policies.insert(policy.venue.clone(), policy);
    }

    pub fn set_entity(&mut self, id: &str, record: EntityRecord) {
        self.entities.insert(id.to_string(), record);
    }

    fn any_policy(&self, venue: &VenueId) -> Result<&VenuePolicy> {
        self.policies.get(venue).ok_or_else(|| {
            Error::denied(format!(
                "venue {venue} has no access policy; configure one, because none means refused"
            ))
        })
    }

    /// Whether a market may be created at `venue` from `jurisdiction`.
    pub fn admit_creation(
        &self,
        venue: &VenueId,
        jurisdiction: &Jurisdiction,
    ) -> Result<CreationPermit> {
        let policy = self.any_policy(venue)?;
        if !policy.creation.contains(jurisdiction) {
            return Err(Error::denied(format!(
                "creating a market at {venue} is not permitted from {jurisdiction}"
            )));
        }
        Ok(CreationPermit {
            venue: venue.clone(),
            jurisdiction: jurisdiction.clone(),
        })
    }

    /// An order on an event-contract venue.
    pub fn admit_event_order(
        &self,
        entity: &str,
        venue: &VenueId,
        product: &str,
    ) -> Result<OrderPermit> {
        self.admit(entity, venue, product, VenueKind::Event)
    }

    /// An order on a regulated wagering venue.
    pub fn admit_wagering_order(
        &self,
        entity: &str,
        venue: &VenueId,
        product: &str,
    ) -> Result<OrderPermit> {
        self.admit(entity, venue, product, VenueKind::Wagering)
    }

    fn admit(
        &self,
        entity: &str,
        venue: &VenueId,
        product: &str,
        path: VenueKind,
    ) -> Result<OrderPermit> {
        let policy = self.any_policy(venue)?;
        if policy.kind != path {
            return Err(Error::denied(format!(
                "venue {venue} is a {:?} venue and this is the {path:?} path; \
                 use the entry point that matches the venue",
                policy.kind
            )));
        }
        let record = self.entities.get(entity).ok_or_else(|| {
            Error::denied(format!(
                "entity {entity} has no eligibility record; record one, because none means refused"
            ))
        })?;
        if !record.eligible {
            return Err(Error::denied(format!("entity {entity} is not eligible")));
        }
        if let Some(minimum) = policy.minimum_age {
            match record.age_years {
                Some(age) if age >= minimum => {}
                Some(age) => {
                    return Err(Error::denied(format!(
                        "entity {entity} is {age}, under the {minimum} that {venue} requires"
                    )));
                }
                None => {
                    return Err(Error::denied(format!(
                        "entity {entity} has no recorded age and {venue} requires {minimum}"
                    )));
                }
            }
        }
        if !policy.trading.contains(&record.jurisdiction) {
            return Err(Error::denied(format!(
                "{venue} does not permit orders from {}",
                record.jurisdiction
            )));
        }
        if !record.venues.contains(venue) {
            return Err(Error::denied(format!(
                "{venue} is not available to entity {entity}; a priced event is not a tradable one"
            )));
        }
        if !policy.products.contains(product) {
            return Err(Error::denied(format!(
                "product {product} is not permitted at {venue}"
            )));
        }
        Ok(OrderPermit {
            venue: venue.clone(),
            entity: entity.to_string(),
            product: product.to_string(),
        })
    }
}
