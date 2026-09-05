//! The mandates the ledger holds, under the desk's ceiling.
//!
//! The failure this file prevents: a user's mandate is a promise about
//! capital the desk has, and a registry that admitted mandates without
//! reference to the desk would sooner or later promise more than there is —
//! two users each with the whole book, or one with a risk tolerance the desk
//! itself does not carry. Every user mandate is therefore admitted against
//! the desk's own mandate as a ceiling, term by term, and in aggregate: the
//! capital under user mandates may not exceed the desk's capital, because
//! the desk's capital is the only capital there is.
//!
//! A mandate is recorded under a [`MandateId`] of its own, distinct from the
//! user, because a mandate is never replaced in place (the terms a fill was
//! booked under are part of the attribution) and a superseding mandate needs
//! a name the old one did not have. An id seen twice is refused with the
//! holder named: the second registration is a retry or a reused id, and the
//! registry cannot tell which.
//!
//! Nothing here moves capital. A registration is a record that a mandate may
//! exist; the book it opens is empty until funded, and funding is checked
//! against the mandate again at the one place capital enters a book.

use super::identity::{MandateId, UserId};
use super::mandate::{Mandate, PermittedFamilies};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The id the desk's own mandate is registered under. A literal, because
/// the desk is the one holder that exists before any registration.
pub const DESK_MANDATE_ID: &str = "desk";

/// A mandate as the registry recorded it: which id, when, and the terms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisteredMandate {
    pub id: MandateId,
    pub mandate: Mandate,
    pub registered_at: Timestamp,
}

/// The registry as a stored record: the desk, then every user registration
/// in the order the registry replays them. Deserialising a
/// [`MandateRegistry`] replays each through [`MandateRegistry::register`],
/// so a stored record that has gone bad — an id twice, a capital above the
/// desk's — is refused on the way back in rather than trusted because it
/// was once ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegistryRecord {
    pub desk: UserId,
    pub desk_mandate: RegisteredMandate,
    /// Keyed by user, which is also the replay order.
    pub users: BTreeMap<UserId, RegisteredMandate>,
}

/// The mandates, keyed by the user who holds each, under the desk's ceiling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RegistryRecord", into = "RegistryRecord")]
pub struct MandateRegistry {
    desk: UserId,
    desk_id: MandateId,
    desk_registered_at: Timestamp,
    /// Every holder including the desk, so a reader who asks "who holds a
    /// mandate" is answered with one map in one order.
    mandates: BTreeMap<UserId, Mandate>,
    /// The id and instant of each registration, beside `mandates` rather
    /// than inside it so the map of terms stays the map the ledger reads.
    registrations: BTreeMap<UserId, (MandateId, Timestamp)>,
    /// Every id ever registered, to the user holding it.
    ids: BTreeMap<MandateId, UserId>,
}

impl MandateRegistry {
    /// A registry whose ceiling is the desk's mandate. The desk is registered
    /// under [`DESK_MANDATE_ID`] and is not itself checked against the
    /// ceiling, since it is the ceiling.
    pub fn new(desk: UserId, mandate: Mandate, at: Timestamp) -> Result<Self> {
        let desk_id = MandateId::new(DESK_MANDATE_ID)?;
        let mut mandates = BTreeMap::new();
        mandates.insert(desk.clone(), mandate);
        let mut registrations = BTreeMap::new();
        registrations.insert(desk.clone(), (desk_id.clone(), at));
        let mut ids = BTreeMap::new();
        ids.insert(desk_id.clone(), desk.clone());
        Ok(Self {
            desk,
            desk_id,
            desk_registered_at: at,
            mandates,
            registrations,
            ids,
        })
    }

    pub fn desk(&self) -> &UserId {
        &self.desk
    }

    /// The ceiling every user mandate is admitted under.
    pub fn desk_mandate(&self) -> &Mandate {
        self.mandates
            .get(&self.desk)
            .unwrap_or_else(|| unreachable!("the desk is registered at construction"))
    }

    pub fn mandate(&self, user: &UserId) -> Option<&Mandate> {
        self.mandates.get(user)
    }

    pub fn mandates(&self) -> &BTreeMap<UserId, Mandate> {
        &self.mandates
    }

    /// The registration behind a user's mandate, or `None` for a user who
    /// holds none.
    pub fn registration(&self, user: &UserId) -> Option<RegisteredMandate> {
        let mandate = self.mandates.get(user)?;
        let (id, registered_at) = self.registrations.get(user)?;
        Some(RegisteredMandate {
            id: id.clone(),
            mandate: mandate.clone(),
            registered_at: *registered_at,
        })
    }

    /// Who holds a mandate id, if anyone does.
    pub fn holder(&self, id: &MandateId) -> Option<&UserId> {
        self.ids.get(id)
    }

    /// The capital under user mandates — everyone but the desk — which may
    /// never exceed the desk's.
    pub fn capital_under_users(&self) -> Decimal {
        self.mandates
            .iter()
            .filter(|(user, _)| **user != self.desk)
            .map(|(_, mandate)| mandate.capital())
            .sum()
    }

    /// Admit a user's mandate under the desk's ceiling.
    ///
    /// Refuses, in this order and by name: a mandate id already registered
    /// (naming its holder), a user who already holds a mandate, a currency
    /// other than the desk's, a capital above the desk's, a risk tolerance
    /// above the desk's, an exploration share above the desk's, a family the
    /// desk does not permit, and a capital that would take the total under
    /// user mandates past the desk's. A refused registration records
    /// nothing. The liquidity floor has no ceiling: a higher floor is a
    /// tighter mandate, not a looser one.
    pub fn register(
        &mut self,
        user: UserId,
        id: MandateId,
        mandate: Mandate,
        at: Timestamp,
    ) -> Result<()> {
        if let Some(holder) = self.ids.get(&id) {
            return Err(Error::invalid(format!(
                "the mandate id {id} is already registered to {holder}; a second registration \
                 under one id is a retry or a reused id, and the registry cannot tell which — \
                 record the new terms under a new id"
            )));
        }
        if self.mandates.contains_key(&user) {
            return Err(Error::invalid(format!(
                "{user} already holds a mandate; a mandate is not replaced in place — \
                 record a new one under the change that supersedes it"
            )));
        }
        let desk = self.desk_mandate();
        if mandate.currency() != desk.currency() {
            return Err(Error::invalid(format!(
                "the mandate {id} for {user} is in {} and the desk's is in {}; a user mandate \
                 is a share of the desk's capital and is denominated as the desk is",
                mandate.currency(),
                desk.currency()
            )));
        }
        if mandate.capital() > desk.capital() {
            return Err(Error::denied(format!(
                "the mandate {id} for {user} places {} under management against a desk \
                 capital of {}; no user mandate exceeds the desk's capital",
                mandate.capital(),
                desk.capital()
            )));
        }
        if mandate.risk_tolerance() > desk.risk_tolerance() {
            return Err(Error::denied(format!(
                "the mandate {id} for {user} tolerates losing {} of its capital against the \
                 desk's tolerance of {}; no user mandate tolerates more than the desk does",
                mandate.risk_tolerance(),
                desk.risk_tolerance()
            )));
        }
        if mandate.exploration_share() > desk.exploration_share() {
            return Err(Error::denied(format!(
                "the mandate {id} for {user} sets aside {} for exploration against the desk's \
                 {}; a user cannot explore with a share the desk has not set aside",
                mandate.exploration_share(),
                desk.exploration_share()
            )));
        }
        if let Some(family) =
            family_outside(mandate.permitted_families(), desk.permitted_families())
        {
            return Err(Error::denied(format!(
                "the mandate {id} for {user} permits the family {family}, which the desk's \
                 mandate does not; a user mandate reaches only the families the desk reaches"
            )));
        }
        let under_users = self.capital_under_users();
        if under_users + mandate.capital() > desk.capital() {
            return Err(Error::denied(format!(
                "the mandate {id} for {user} would take the capital under user mandates from \
                 {under_users} to {} against a desk capital of {}; the desk's capital is the \
                 only capital there is, and it cannot be promised twice",
                under_users + mandate.capital(),
                desk.capital()
            )));
        }
        self.ids.insert(id.clone(), user.clone());
        self.registrations.insert(user.clone(), (id, at));
        self.mandates.insert(user, mandate);
        Ok(())
    }
}

/// The first family `user` permits that `desk` does not, or `None` when the
/// user's families are within the desk's. A user permitting `Any` under a
/// desk permitting only some is outside it, and the offending family is
/// named as `any`.
fn family_outside(user: &PermittedFamilies, desk: &PermittedFamilies) -> Option<String> {
    match (user, desk) {
        (_, PermittedFamilies::Any) => None,
        (PermittedFamilies::Any, PermittedFamilies::Only(_)) => Some("any".to_string()),
        (PermittedFamilies::Only(families), PermittedFamilies::Only(permitted)) => families
            .iter()
            .find(|family| !permitted.contains(*family))
            .cloned(),
    }
}

impl TryFrom<RegistryRecord> for MandateRegistry {
    type Error = Error;

    fn try_from(record: RegistryRecord) -> Result<Self> {
        if record.desk_mandate.id.as_str() != DESK_MANDATE_ID {
            return Err(Error::invalid(format!(
                "the stored registry's desk mandate is under the id {}, not {DESK_MANDATE_ID}; \
                 the record is not one this registry wrote",
                record.desk_mandate.id
            )));
        }
        let mut registry = Self::new(
            record.desk,
            record.desk_mandate.mandate,
            record.desk_mandate.registered_at,
        )?;
        for (user, registered) in record.users {
            registry.register(
                user,
                registered.id,
                registered.mandate,
                registered.registered_at,
            )?;
        }
        Ok(registry)
    }
}

impl From<MandateRegistry> for RegistryRecord {
    fn from(registry: MandateRegistry) -> Self {
        let desk_mandate = RegisteredMandate {
            id: registry.desk_id.clone(),
            mandate: registry.desk_mandate().clone(),
            registered_at: registry.desk_registered_at,
        };
        let users = registry
            .mandates
            .keys()
            .filter(|user| **user != registry.desk)
            .filter_map(|user| {
                registry
                    .registration(user)
                    .map(|registered| (user.clone(), registered))
            })
            .collect();
        Self {
            desk: registry.desk,
            desk_mandate,
            users,
        }
    }
}
