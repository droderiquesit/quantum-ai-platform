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

use crate::trust::TrustRoot;
use qip_contracts::policy::PolicyPayload;
use qip_contracts::trust::ArtefactClass;
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

    /// Verify a payload against the cell's [`TrustRoot`] — the trust bundle
    /// under ADR 0110, or the shared key for the release that keeps it.
    ///
    /// The same refusals as [`Self::verify`], in the same order: the
    /// signature first, then the address. Under a bundle the signature must
    /// be an `ecdsa-p256-sha256` signature under the `policy` key; an HMAC is
    /// refused because it never parses as one.
    pub fn verify_with(
        payload: PolicyPayload,
        root: &TrustRoot,
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        let signing = payload.signing_payload()?;
        root.check(
            ArtefactClass::Policy,
            signing.as_bytes(),
            &payload.signature,
            now,
        )
        .map_err(|error| {
            Error::denied(format!(
                "policy payload {} is refused: {}",
                payload.sequence,
                error.message()
            ))
        })?;
        if payload.cell != expected_cell {
            return Err(Error::denied(format!(
                "a policy payload for cell {} was presented to cell {expected_cell}",
                payload.cell
            )));
        }
        // ADR 0110, Amendment 1: the payload is one process's per-cycle
        // issuance, so its signature cannot be what admits a model to the
        // order path. Under a bundle every model the manifest names must
        // carry its own two-person release under the `model-pack` key. The
        // whole payload is refused rather than the model quietly dropped:
        // a centre naming an unreleased model is compromised or broken, and
        // a cell that edited the list would be running a third one nobody
        // signed. A shared-key cell cannot verify a pack and keeps the
        // pre-amendment reading until the shared key's removal date.
        if let (TrustRoot::Bundle(verifier), Some(manifest)) =
            (root, payload.trained_models.value())
        {
            manifest
                .check_releases(verifier.as_ref(), now)
                .map_err(|error| {
                    Error::denied(format!(
                        "policy payload {} is refused: {}",
                        payload.sequence,
                        error.message()
                    ))
                })?;
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

/// A halt command whose signature this cell has checked against its own key.
///
/// The same discipline as [`VerifiedPolicy`], and the same reason there is no
/// other way to obtain one: [`crate::Cell::apply_halt`] takes this, so a cell
/// cannot be stopped — or, through the release barrier, kept stopped — by an
/// unsigned frame.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedHalt {
    inner: qip_contracts::policy::HaltCommand,
    verified_at: Timestamp,
}

impl VerifiedHalt {
    pub fn verify(
        command: qip_contracts::policy::HaltCommand,
        key: &[u8],
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a cell with no policy key cannot verify a halt and must not apply one",
            ));
        }
        let expected = to_hex(&hmac_sha256(key, command.signing_payload().as_bytes()));
        if !crate::envelope::constant_time_eq(expected.as_bytes(), command.signature.as_bytes()) {
            return Err(Error::denied(
                "the halt command does not verify against this cell's key".to_string(),
            ));
        }
        if command.cell != expected_cell {
            return Err(Error::denied(format!(
                "a halt for cell {} was presented to cell {expected_cell}",
                command.cell
            )));
        }
        Ok(Self {
            inner: command,
            verified_at: now,
        })
    }

    /// Verify a halt against the cell's [`TrustRoot`], under the `halt` key.
    ///
    /// One signature and nothing else: a halt only restricts, and ADR 0110
    /// keeps it one-person so stopping never waits on a second approver.
    pub fn verify_with(
        command: qip_contracts::policy::HaltCommand,
        root: &TrustRoot,
        expected_cell: &str,
        now: Timestamp,
    ) -> Result<Self> {
        root.check(
            ArtefactClass::Halt,
            command.signing_payload().as_bytes(),
            &command.signature,
            now,
        )
        .map_err(|error| {
            Error::denied(format!("the halt command is refused: {}", error.message()))
        })?;
        if command.cell != expected_cell {
            return Err(Error::denied(format!(
                "a halt for cell {} was presented to cell {expected_cell}",
                command.cell
            )));
        }
        Ok(Self {
            inner: command,
            verified_at: now,
        })
    }

    pub fn issued_at(&self) -> Timestamp {
        self.inner.issued_at
    }

    pub fn reason(&self) -> &str {
        &self.inner.reason
    }

    pub const fn verified_at(&self) -> Timestamp {
        self.verified_at
    }
}
