//! What a user may do, decided fresh on every request (blueprint §43.3).
//!
//! An entitlement is derived, never stored as authority: it is computed from
//! the jurisdiction, the product's eligibility, the role and the mandate at
//! the instant of the request, so a mandate that changed, a product that was
//! withdrawn from a jurisdiction, or a role that lapsed is reflected on the
//! next request rather than on the next login. A cached grant is a grant
//! that outlives its reasons.
//!
//! The withdrawal arm is the reason this module has a doc comment worth
//! reading. ADR 0021 permits the deterministic half of the blueprint's
//! treasury and refuses the path by which capital leaves the platform; ADR
//! 0023 keeps that refusal in force and puts capital movement last and
//! separate. [`WithdrawalEntitlement`] therefore has exactly one variant.
//! There is no `Granted` arm to construct, no field to set, no
//! `Deserialize` impl through which a stored record could smuggle one in,
//! and no function in this crate returns anything else. A later change that
//! wanted a granted withdrawal would have to add the variant here, in the
//! file whose top says why it is absent — which is the reviewed act ADR 0021
//! requires, and not something a flag can do.

use super::identity::{Jurisdiction, UserId};
use super::mandate::Mandate;
use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The role a request arrives under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    /// May see their own books and nothing else.
    Viewer,
    /// May raise an investment request inside their mandate.
    Investor,
    /// The desk. May invest and view; may not withdraw, like everyone.
    Operator,
}

/// Where a product — one strategy family offered to users — may be sold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductEligibility {
    pub family: String,
    /// Empty means eligible nowhere, which is the honest default for a
    /// family nobody has cleared: a product with no eligibility record is
    /// refused everywhere rather than admitted everywhere.
    pub eligible_in: BTreeSet<Jurisdiction>,
}

impl ProductEligibility {
    pub fn new(family: impl Into<String>) -> Self {
        Self {
            family: family.into(),
            eligible_in: BTreeSet::new(),
        }
    }

    pub fn eligible_in(mut self, jurisdiction: Jurisdiction) -> Self {
        self.eligible_in.insert(jurisdiction);
        self
    }
}

/// A capability that can go either way, with the reason it went that way.
///
/// The basis of a grant is recorded beside the grant so a reviewer reading
/// the log sees *why* a request was admitted, not only that it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    Granted { basis: String },
    Refused { reason: String },
}

impl Capability {
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }
}

/// The withdrawal capability, which in this platform is refused.
///
/// One variant, on purpose, per ADR 0021: capital does not leave the
/// platform through anything this repository builds. This type serialises
/// so an evaluation can be journalled, and deliberately does **not**
/// deserialise: a record is evidence of what was decided, never an input
/// that decides. Nothing outside this module constructs it — see
/// [`Entitlement::evaluate`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum WithdrawalEntitlement {
    Refused { reason: String },
}

impl WithdrawalEntitlement {
    /// The only constructor, and it is private to the module: the reason is
    /// fixed at the ADR rather than supplied by a caller who might one day
    /// supply something else.
    fn refused() -> Self {
        Self::Refused {
            reason: "capital does not leave the platform: ADR 0021 refuses the signing and \
                     withdrawal half of the treasury and ADR 0023 keeps that in force; a \
                     withdrawal is a separate, later, separately approved decision"
                .to_string(),
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Refused { reason } => reason,
        }
    }
}

/// What one user may do with one product, at one instant.
///
/// Fields are private so the only way to hold an `Entitlement` is to have
/// evaluated one; there is no builder and no `Deserialize`, for the reason
/// [`WithdrawalEntitlement`] gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entitlement {
    user: UserId,
    role: Role,
    family: String,
    evaluated_at: Timestamp,
    can_view: Capability,
    can_invest: Capability,
    can_withdraw: WithdrawalEntitlement,
}

impl Entitlement {
    /// Decide, from the four inputs §43.3 names, what the user may do.
    ///
    /// Viewing is granted to anyone holding a mandate — the mandate is the
    /// proof of a relationship. Investing needs all of: a role that invests,
    /// a product eligible in the user's jurisdiction, a family the mandate
    /// permits, and investable capital above zero. Each refusal names the
    /// input that refused it, so the answer to "why can't I" is on the
    /// record rather than in a support queue.
    pub fn evaluate(
        user: &UserId,
        mandate: &Mandate,
        role: Role,
        product: &ProductEligibility,
        now: Timestamp,
    ) -> Self {
        let can_view = Capability::Granted {
            basis: format!("{user} holds a mandate in {}", mandate.jurisdiction()),
        };
        let can_invest = Self::investment_capability(user, mandate, role, product);
        Self {
            user: user.clone(),
            role,
            family: product.family.clone(),
            evaluated_at: now,
            can_view,
            can_invest,
            can_withdraw: WithdrawalEntitlement::refused(),
        }
    }

    fn investment_capability(
        user: &UserId,
        mandate: &Mandate,
        role: Role,
        product: &ProductEligibility,
    ) -> Capability {
        if role == Role::Viewer {
            return Capability::Refused {
                reason: format!("{user} holds the viewer role, which does not invest"),
            };
        }
        let jurisdiction = mandate.jurisdiction();
        if !product.eligible_in.contains(&jurisdiction) {
            return Capability::Refused {
                reason: format!(
                    "the family {} is not eligible in {jurisdiction}; it is eligible in {}",
                    product.family,
                    if product.eligible_in.is_empty() {
                        "no jurisdiction".to_string()
                    } else {
                        product
                            .eligible_in
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            };
        }
        if !mandate.permitted_families().permits(&product.family) {
            return Capability::Refused {
                reason: format!(
                    "the mandate of {user} does not permit the family {}",
                    product.family
                ),
            };
        }
        if !mandate.investable().is_positive() {
            return Capability::Refused {
                reason: format!(
                    "the mandate of {user} has no investable capital: {} under management \
                     less a liquidity floor of {}",
                    mandate.capital(),
                    mandate.liquidity_floor()
                ),
            };
        }
        Capability::Granted {
            basis: format!(
                "{role:?} role, {} eligible in {jurisdiction}, family permitted, {} investable",
                product.family,
                mandate.investable()
            ),
        }
    }

    pub fn user(&self) -> &UserId {
        &self.user
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn evaluated_at(&self) -> Timestamp {
        self.evaluated_at
    }

    pub fn can_view(&self) -> &Capability {
        &self.can_view
    }

    pub fn can_invest(&self) -> &Capability {
        &self.can_invest
    }

    /// Always refused. The return type is the proof: it has no other arm.
    pub fn can_withdraw(&self) -> &WithdrawalEntitlement {
        &self.can_withdraw
    }
}
