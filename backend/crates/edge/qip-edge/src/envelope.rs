//! Capital envelopes a cell has actually verified.
//!
//! [`qip_contracts::CapitalEnvelope::new`] is public, so a well-typed envelope
//! nobody approved can be constructed anywhere in the workspace. That is
//! deliberate — the allocator has to build one somehow — but it means the type
//! alone is not evidence of anything.
//!
//! [`VerifiedEnvelope`] is that evidence. Its inner value is private and its
//! only constructor recomputes the signature, so a cell that types its capital
//! checks against this cannot be handed an unapproved grant. Construction is
//! not the control; verification is, and this is where it happens.

use qip_contracts::capital::{CapitalEnvelope, CapitalGrant, Utilisation};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::{Decimal, Timestamp, hmac_sha256};

/// An envelope whose signature this cell has checked against its own key.
///
/// There is no other way to obtain one, and every capital decision in the cell
/// takes this rather than a bare [`CapitalEnvelope`].
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedEnvelope {
    inner: CapitalEnvelope,
    verified_at: Timestamp,
}

impl VerifiedEnvelope {
    /// Verify an envelope against the cell's shared key.
    ///
    /// Refuses a bad signature, an envelope outside its validity window, and
    /// one whose strategy or cell does not match what the caller expected —
    /// a correctly signed grant for a *different* cell is exactly the replay a
    /// signature alone does not stop.
    pub fn verify(
        envelope: CapitalEnvelope,
        key: &[u8],
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a cell with no envelope key cannot verify capital and must not trade",
            ));
        }
        let expected = to_hex(&hmac_sha256(key, envelope.signing_payload().as_bytes()));
        if !constant_time_eq(expected.as_bytes(), envelope.signature().as_bytes()) {
            return Err(Error::denied(format!(
                "the capital envelope for {} does not verify against this cell's key",
                envelope.strategy().as_str()
            )));
        }
        if envelope.cell() != expected_cell {
            return Err(Error::denied(format!(
                "an envelope for cell {} was presented to cell {expected_cell}",
                envelope.cell()
            )));
        }
        if !envelope.is_live(now) {
            return Err(Error::denied(format!(
                "the capital envelope for {} is outside its validity window",
                envelope.strategy().as_str()
            )));
        }
        Ok(Self {
            inner: envelope,
            verified_at: now,
        })
    }

    pub fn strategy(&self) -> &StrategyId {
        self.inner.strategy()
    }

    pub fn cell(&self) -> &str {
        self.inner.cell()
    }

    pub fn expires_at(&self) -> Timestamp {
        self.inner.expires_at()
    }

    pub fn approver(&self) -> &str {
        self.inner.approver()
    }

    pub const fn verified_at(&self) -> Timestamp {
        self.verified_at
    }

    /// Whether the grant is still inside its window.
    ///
    /// Checked again at every use rather than once at verification: expiry is
    /// the backstop that bounds a cell which has lost contact with the centre,
    /// and a backstop consulted only on arrival is not one.
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.inner.is_live(now)
    }

    /// Decide an order against this grant and what has already been used.
    pub fn admit(
        &self,
        venue: &VenueId,
        notional: Decimal,
        used: &Utilisation,
        now: Timestamp,
    ) -> CapitalGrant {
        self.inner.admit(venue, notional, used, now)
    }
}

/// Compare two byte strings without leaking where they first differ.
///
/// A signature check that returns early on the first mismatch tells an
/// attacker how much of a forgery was correct, which turns forging one into a
/// linear search rather than an exhaustive one.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

/// Sign an envelope's payload the way a cell will verify it.
///
/// Present so a test and the central allocator agree on the construction. It
/// is not a production signing path: HMAC proves possession of a shared
/// secret, not the identity of a signer. See
/// `docs/operations/external-dependencies.md` for what asymmetric signing
/// would need.
pub fn sign_payload(key: &[u8], payload: &str) -> String {
    to_hex(&hmac_sha256(key, payload.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_is_length_aware_and_content_correct() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abcde"));
        assert!(constant_time_eq(b"", b""));
    }
}
