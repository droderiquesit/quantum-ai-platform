//! Capital granted to a strategy at an edge cell, and what it has used.
//!
//! An edge cell trades without asking the central plane, which is what makes
//! it fast. The envelope is how that stays safe: capital is granted in
//! advance, signed, bounded and expiring, so the worst a disconnected cell can
//! do is spend an amount somebody already approved.

use crate::signal::StrategyId;
use crate::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// What a strategy at a cell is permitted to commit.
///
/// Fields are private and the constructor is the only way in, because an
/// envelope that can be widened in place is not a limit. Widening means asking
/// the central plane for a new one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalEnvelope {
    strategy: StrategyId,
    cell: String,
    /// The most that may be at risk at once.
    gross_limit: Decimal,
    /// The most any single order may commit.
    order_limit: Decimal,
    /// The loss at which the cell stops this strategy on its own authority.
    loss_limit: Decimal,
    /// Venues this grant is good at. An empty list is no venues, not all of
    /// them — the permissive reading of an empty list is how grants leak.
    venues: Vec<VenueId>,
    granted_at: Timestamp,
    expires_at: Timestamp,
    /// Who approved the grant, and the signature over its contents.
    approver: String,
    signature: String,
}

impl CapitalEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strategy: StrategyId,
        cell: impl Into<String>,
        gross_limit: Decimal,
        order_limit: Decimal,
        loss_limit: Decimal,
        venues: Vec<VenueId>,
        granted_at: Timestamp,
        expires_at: Timestamp,
        approver: impl Into<String>,
        signature: impl Into<String>,
    ) -> Result<Self> {
        if gross_limit <= Decimal::ZERO {
            return Err(Error::invalid("a capital envelope needs a positive limit"));
        }
        if order_limit > gross_limit {
            return Err(Error::invalid(
                "a single order may not commit more than the whole envelope",
            ));
        }
        if expires_at <= granted_at {
            return Err(Error::invalid(
                "a capital envelope must expire after it is granted",
            ));
        }
        let approver = approver.into();
        if approver.trim().is_empty() {
            return Err(Error::denied("a capital envelope needs a named approver"));
        }
        Ok(Self {
            strategy,
            cell: cell.into(),
            gross_limit,
            order_limit,
            loss_limit,
            venues,
            granted_at,
            expires_at,
            approver,
            signature: signature.into(),
        })
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn gross_limit(&self) -> Decimal {
        self.gross_limit
    }

    pub fn order_limit(&self) -> Decimal {
        self.order_limit
    }

    pub fn loss_limit(&self) -> Decimal {
        self.loss_limit
    }

    pub fn approver(&self) -> &str {
        &self.approver
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    pub fn is_live(&self, now: Timestamp) -> bool {
        now >= self.granted_at && now < self.expires_at
    }

    pub fn permits_venue(&self, venue: &VenueId) -> bool {
        self.venues.iter().any(|v| v == venue)
    }

    /// The bytes a signature is taken over.
    ///
    /// Every field that bounds what the cell may do. A signature that does not
    /// cover a limit is a signature over the wrong thing.
    pub fn signing_payload(&self) -> String {
        let venues: Vec<&str> = self.venues.iter().map(VenueId::as_str).collect();
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.strategy.as_str(),
            self.cell,
            self.gross_limit,
            self.order_limit,
            self.loss_limit,
            venues.join(","),
            self.granted_at.as_secs(),
            self.expires_at.as_secs(),
            self.approver
        )
    }
}

/// What a strategy has actually committed against its envelope.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilisation {
    pub gross_committed: Decimal,
    pub realised_loss: Decimal,
    pub orders_sent: u64,
}

/// The answer to "may this order go".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CapitalGrant {
    /// Send it at the requested size.
    Full,
    /// Send it, but smaller. Carries the size, so a caller cannot mistake a
    /// reduction for an approval of what it asked for.
    Reduced(Decimal),
    /// Do not send it.
    Refused(String),
}

impl CapitalGrant {
    pub fn permitted_quantity(&self, requested: Decimal) -> Decimal {
        match self {
            Self::Full => requested,
            Self::Reduced(q) => *q,
            Self::Refused(_) => Decimal::ZERO,
        }
    }

    pub const fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

impl CapitalEnvelope {
    /// Decide an order against this envelope and what has already been used.
    ///
    /// Every refusal names which bound was hit, because "refused" without a
    /// reason is the least actionable message an execution system can produce.
    pub fn admit(
        &self,
        venue: &VenueId,
        notional: Decimal,
        used: &Utilisation,
        now: Timestamp,
    ) -> CapitalGrant {
        if !self.is_live(now) {
            return CapitalGrant::Refused("the capital envelope has expired".to_string());
        }
        if !self.permits_venue(venue) {
            return CapitalGrant::Refused(format!(
                "the envelope does not cover venue {}",
                venue.as_str()
            ));
        }
        if used.realised_loss >= self.loss_limit {
            return CapitalGrant::Refused(format!(
                "realised loss {} reached the {} limit",
                used.realised_loss, self.loss_limit
            ));
        }
        let headroom = self.gross_limit - used.gross_committed;
        if headroom <= Decimal::ZERO {
            return CapitalGrant::Refused(format!(
                "the {} gross limit is fully committed",
                self.gross_limit
            ));
        }
        let cap = if self.order_limit < headroom {
            self.order_limit
        } else {
            headroom
        };
        if notional <= cap {
            CapitalGrant::Full
        } else {
            CapitalGrant::Reduced(cap)
        }
    }
}
