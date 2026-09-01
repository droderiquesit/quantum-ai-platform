//! Policy payloads a cell has actually verified.
//!
//! The same argument as [`crate::envelope`], for the other thing the centre
//! ships. [`qip_contracts::PolicyPayload`] is a public type, so a well-typed
//! payload nobody signed can be constructed anywhere in the workspace.
//! [`VerifiedPolicy`] is the evidence one was: its inner value is private and
//! its only constructor recomputes the signature against the cell's own key,
//! so the cell's policy state cannot be fed an unsigned payload however it
//! arrived.
//!
//! Verification here is about *authenticity and address*: the MAC and the
//! cell name. It is deliberately not about *freshness* — an old payload
//! verifies fine, and every slot in it then reads as stale, which is §6.2's
//! narrowing doing its work. The replay that matters, re-applying an old
//! sequence to un-halt or re-widen a cell, is refused at the application
//! seam, where the last applied sequence lives.

use qip_contracts::policy::PolicyPayload;
use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::{Timestamp, hmac_sha256};

/// A payload whose signature this cell has checked against its own key.
///
/// There is no other way to obtain one, and [`crate::Cell::apply_policy`]
/// takes this rather than a bare payload.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedPolicy {
    inner: PolicyPayload,
    verified_at: Timestamp,
}

impl VerifiedPolicy {
    /// Verify a payload against the cell's shared key.
    ///
    /// Refuses a bad signature and a payload addressed to another cell — a
    /// correctly signed payload for a *different* cell is exactly the replay
    /// a signature alone does not stop.
    pub fn verify(
        payload: PolicyPayload,
        key: &[u8],
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a cell with no policy key cannot verify a payload and must not apply one",
            ));
        }
        let signing = payload.signing_payload()?;
        let expected = to_hex(&hmac_sha256(key, signing.as_bytes()));
        if !crate::envelope::constant_time_eq(expected.as_bytes(), payload.signature.as_bytes()) {
            return Err(Error::denied(format!(
                "policy payload {} does not verify against this cell's key",
                payload.sequence
            )));
        }
        if payload.cell != expected_cell {
            return Err(Error::denied(format!(
                "a policy payload for cell {} was presented to cell {expected_cell}",
                payload.cell
            )));
        }
        Ok(Self {
            inner: payload,
            verified_at: now,
        })
    }

    pub fn payload(&self) -> &PolicyPayload {
        &self.inner
    }

    pub const fn verified_at(&self) -> Timestamp {
        self.verified_at
    }

    pub fn sequence(&self) -> u64 {
        self.inner.sequence
    }

    pub fn halted(&self) -> bool {
        self.inner.halted
    }
}
