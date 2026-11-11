//! Hedge narrowings a cell has verified, and the one rule it applies them by
//! (ADR 0126 decision 4, R12-CAPITAL-10098).
//!
//! A hedge plan is the centre's. What reaches a cell is a
//! [`qip_contracts::policy::HedgeNarrowing`]: per instrument and venue, a
//! target net between what the cell holds and zero, and a deadline. The
//! cell's whole use of it is a refusal — an order that would leave the
//! instrument's net further from the target, or carry it past the target,
//! is not sent. Nothing here sizes an order, raises a limit, names a venue
//! or admits anything: an order the narrowing lets through still meets
//! every gate it met before, so a cell that never hears a narrowing is
//! exactly as bounded as its envelope leaves it, and one that hears a wrong
//! one loses trades and not capital.

use crate::trust::TrustRoot;
use qip_contracts::policy::{HedgeNarrowing, HedgeTarget};
use qip_contracts::trust::ArtefactClass;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::{Decimal, ObjectId, Timestamp, hmac_sha256};

/// The gate an order moving away from a live hedge target is refused at.
///
/// A constant, so the refusal series' `gate` label stays bounded by source
/// literals however many seams record under it.
pub const GATE_HEDGE_TARGET: &str = "hedge_target";

/// The gate a narrowing the cell will not apply is journaled under: one for
/// another cell's plan would be caught at verification, so what lands here
/// is a frame past its deadline, a replay, a venue no live envelope grants,
/// or a target that is not between the cell's holding and zero.
pub const GATE_HEDGE_FRAME: &str = "hedge_frame";

/// A hedge narrowing whose signature this cell has checked.
///
/// The same discipline as [`crate::VerifiedEnvelope`]: the inner value is
/// private and the only constructors recompute the signature, so
/// [`crate::Cell::apply_hedge`] cannot be handed a narrowing nobody signed.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedHedge {
    inner: HedgeNarrowing,
    verified_at: Timestamp,
}

impl VerifiedHedge {
    /// Verify against the cell's shared key: signature, shape, then address.
    pub fn verify(
        narrowing: HedgeNarrowing,
        key: &[u8],
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a cell with no envelope key cannot verify a hedge narrowing and must not apply \
                 one",
            ));
        }
        let expected = to_hex(&hmac_sha256(key, narrowing.signing_payload().as_bytes()));
        if !crate::envelope::constant_time_eq(expected.as_bytes(), narrowing.signature.as_bytes()) {
            return Err(Error::denied(format!(
                "hedge narrowing {} does not verify against this cell's key",
                narrowing.plan_id
            )));
        }
        Self::addressed(narrowing, expected_cell, now)
    }

    /// Verify against the cell's [`TrustRoot`], under the `envelope` key: a
    /// narrowing is capital authority and is signed as capital is.
    pub fn verify_with(
        narrowing: HedgeNarrowing,
        root: &TrustRoot,
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        root.check(
            ArtefactClass::Envelope,
            narrowing.signing_payload().as_bytes(),
            &narrowing.signature,
            now,
        )
        .map_err(|error| {
            Error::denied(format!(
                "hedge narrowing {} is refused: {}",
                narrowing.plan_id,
                error.message()
            ))
        })?;
        Self::addressed(narrowing, expected_cell, now)
    }

    /// The checks a correct signature does not make: a well-signed
    /// narrowing for a different cell is exactly the replay a signature
    /// alone does not stop, and a well-signed malformed one is still not
    /// something a cell can apply.
    fn addressed(narrowing: HedgeNarrowing, expected_cell: &str, now: Timestamp) -> Result<Self> {
        narrowing.check().map_err(|error| {
            Error::denied(format!(
                "hedge narrowing {} is refused: {}",
                narrowing.plan_id,
                error.message()
            ))
        })?;
        if narrowing.cell != expected_cell {
            return Err(Error::denied(format!(
                "a hedge narrowing for cell {} was presented to cell {expected_cell}",
                narrowing.cell
            )));
        }
        Ok(Self {
            inner: narrowing,
            verified_at: now,
        })
    }

    pub fn plan_id(&self) -> &str {
        &self.inner.plan_id
    }

    pub fn issued_at(&self) -> Timestamp {
        self.inner.issued_at
    }

    pub fn deadline(&self) -> Timestamp {
        self.inner.deadline
    }

    pub fn targets(&self) -> &[HedgeTarget] {
        &self.inner.targets
    }

    pub fn signature(&self) -> &str {
        &self.inner.signature
    }

    pub const fn verified_at(&self) -> Timestamp {
        self.verified_at
    }

    /// Whether the targets still bind. Read at every use, like an
    /// envelope's expiry: a deadline consulted only on arrival would bind a
    /// cut-off cell to a plan the centre meant to have lapsed.
    pub fn is_live(&self, now: Timestamp) -> bool {
        now < self.inner.deadline
    }

    /// The target for one instrument at one venue, if this names it.
    pub fn target(&self, venue: &VenueId, object_id: &ObjectId) -> Option<Decimal> {
        self.inner
            .targets
            .iter()
            .find(|target| &target.venue == venue && &target.object_id == object_id)
            .map(|target| target.target_net)
    }
}

/// Whether `target` lies between `held` and zero, both ends included.
///
/// The only targets a narrowing may state. One beyond the holding, or on the
/// other side of flat, would make "toward the target" mean adding exposure,
/// which is a new envelope's job and never this frame's.
pub fn is_toward_zero(held: Decimal, target: Decimal) -> bool {
    between(target, Decimal::ZERO, held)
}

/// Whether an order of `signed_size` on an instrument netting `held` moves
/// away from `target`: the resulting net is not between the present net and
/// the target. An order that overshoots the target is "away" as much as one
/// in the wrong direction, because past the target it is opening the
/// position the plan was reducing. An unrepresentable sum is "away": the
/// rule only refuses, so it refuses what it cannot compute.
pub fn moves_away(held: Decimal, target: Decimal, signed_size: Decimal) -> bool {
    match held.checked_add(signed_size) {
        Some(after) => !between(after, held, target),
        None => true,
    }
}

fn between(value: Decimal, one: Decimal, other: Decimal) -> bool {
    let (low, high) = if one <= other {
        (one, other)
    } else {
        (other, one)
    };
    low <= value && value <= high
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    #[test]
    fn an_order_is_away_when_it_adds_to_the_holding_or_carries_it_past_the_target() {
        // Long 100, target 40.
        assert!(moves_away(dec!("100"), dec!("40"), dec!("1")));
        assert!(!moves_away(dec!("100"), dec!("40"), dec!("-60")));
        assert!(!moves_away(dec!("100"), dec!("40"), dec!("-10")));
        assert!(moves_away(dec!("100"), dec!("40"), dec!("-61")));
        // Short 100, target flat.
        assert!(moves_away(dec!("-100"), dec!("0"), dec!("-1")));
        assert!(!moves_away(dec!("-100"), dec!("0"), dec!("100")));
        assert!(moves_away(dec!("-100"), dec!("0"), dec!("101")));
        // At the target every order is away from it.
        assert!(moves_away(dec!("40"), dec!("40"), dec!("1")));
        assert!(moves_away(dec!("40"), dec!("40"), dec!("-1")));
    }

    #[test]
    fn a_target_is_toward_zero_only_between_the_holding_and_flat() {
        assert!(is_toward_zero(dec!("100"), dec!("0")));
        assert!(is_toward_zero(dec!("100"), dec!("100")));
        assert!(is_toward_zero(dec!("-100"), dec!("-30")));
        assert!(!is_toward_zero(dec!("100"), dec!("101")));
        assert!(!is_toward_zero(dec!("100"), dec!("-1")));
        assert!(!is_toward_zero(dec!("0"), dec!("1")));
        assert!(is_toward_zero(dec!("0"), dec!("0")));
    }
}
