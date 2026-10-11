//! `OpportunityToken` (blueprint v12.0 §27, R12-CONTRACT-7068): one
//! multi-leg opportunity as it passes between cells over the mesh.
//!
//! The mesh already has an `OpportunityToken` *function* (qip-mesh); what it
//! lacked was the record. An arbitrage that crosses cells is only safe if
//! every cell holding a leg agrees on the same legs, the same size, the same
//! expiry and the same unwind plan — so a token missing any of them is
//! refused, and a token that supersedes another names the version it
//! replaces so two cells cannot each act on a different revision.

use crate::signal::OpportunityId;
use crate::validated::{Contract, Validated, after, each_named, listed, named, positive, refuse};
use crate::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenLeg {
    pub venue: VenueId,
    pub instrument: String,
    pub side: LegSide,
    /// Quantity, strictly positive; the side carries the direction.
    pub quantity: Decimal,
}

/// How the legs may be worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPolicy {
    /// All legs or none; a cell that cannot place its leg releases the token.
    AllOrNone,
    /// Legs in the listed order, each only after the previous filled.
    Sequential,
}

/// What a cell does with a leg that filled when another did not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum UnwindPolicy {
    /// Flatten the filled legs at market, accepting up to this loss.
    Flatten { max_loss: Decimal },
    /// Hedge the residual on a named instrument and venue.
    Hedge { venue: VenueId, instrument: String },
}

/// Which revision of an opportunity this is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TokenLineage {
    Original,
    Supersedes { version: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpportunityTokenFields {
    pub schema_version: u32,
    pub opportunity: OpportunityId,
    /// The cycle, or market, the opportunity was found in.
    pub cycle_id: String,
    pub version: u64,
    pub lineage: TokenLineage,
    /// At least two: a single leg is an order, not an opportunity.
    pub legs: Vec<TokenLeg>,
    /// The notional the whole opportunity is sized at.
    pub size: Decimal,
    /// Expected edge net of costs, in basis points of `size`. Signed so a
    /// token whose edge has gone negative can still be described — and
    /// refused below.
    pub edge_bp: i32,
    pub issued_at: Timestamp,
    /// The token's time to live, as an instant.
    pub expires_at: Timestamp,
    /// Capacity reservations held for this token (venue reservation ids);
    /// may be empty when nothing was reserved.
    pub reservations: Vec<String>,
    pub execution: ExecutionPolicy,
    pub unwind: UnwindPolicy,
}

/// The §27 opportunity token. Built only through [`Validated::new`].
pub type OpportunityToken = Validated<OpportunityTokenFields>;

impl Contract for OpportunityTokenFields {
    const NAME: &'static str = "OpportunityToken";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses fewer than two legs, a non-positive leg quantity or size, a
    /// non-positive edge (an opportunity that loses money before it starts is
    /// not one to circulate), an expiry not after issue, a negative unwind
    /// loss bound, a blank name anywhere, and a lineage that does not move
    /// forward: an `Original` must be version 1 and a supersession must
    /// replace an earlier version.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "opportunity", self.opportunity.as_str())?;
        named(c, "cycle_id", &self.cycle_id)?;
        listed(c, "legs", &self.legs)?;
        if self.legs.len() < 2 {
            return refuse(c, "needs at least two legs; a single leg is an order");
        }
        for leg in &self.legs {
            named(c, "legs.venue", leg.venue.as_str())?;
            named(c, "legs.instrument", &leg.instrument)?;
            positive(c, "legs.quantity", leg.quantity)?;
        }
        positive(c, "size", self.size)?;
        if self.edge_bp <= 0 {
            return refuse(
                c,
                format!(
                    "states an edge of {} bp; a non-positive edge is not an opportunity",
                    self.edge_bp
                ),
            );
        }
        after(
            c,
            "expires_at",
            self.expires_at,
            "issued_at",
            self.issued_at,
        )?;
        each_named(c, "reservations", &self.reservations)?;
        match &self.unwind {
            UnwindPolicy::Flatten { max_loss } if max_loss.is_negative() => {
                return refuse(c, "bounds its unwind loss with a negative amount");
            }
            UnwindPolicy::Hedge { venue, instrument } => {
                named(c, "unwind.venue", venue.as_str())?;
                named(c, "unwind.instrument", instrument)?;
            }
            UnwindPolicy::Flatten { .. } => {}
        }
        match self.lineage {
            TokenLineage::Original if self.version != 1 => refuse(
                c,
                format!(
                    "claims to be original at version {}; an original is version 1",
                    self.version
                ),
            ),
            TokenLineage::Supersedes { version } if version >= self.version => refuse(
                c,
                format!(
                    "at version {} claims to supersede version {version}; a revision replaces \
                     an earlier one",
                    self.version
                ),
            ),
            _ => Ok(()),
        }
    }
}
