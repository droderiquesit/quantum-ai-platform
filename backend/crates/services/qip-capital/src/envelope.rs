//! Issuing the grants an edge cell trades against.
//!
//! A cell does not ask the central plane for permission before each order; it
//! checks the order against a [`CapitalEnvelope`] it already holds. That is
//! what makes the edge fast, and it means the envelope is the only thing
//! standing between a misbehaving or disconnected cell and the book. Two
//! properties carry that weight.
//!
//! **It is signed**, over [`CapitalEnvelope::signing_payload`], which covers
//! every field that bounds what the cell may do. A cell that cannot verify a
//! grant must refuse it.
//!
//! **It expires**, and the expiry is short. This is the backstop for the
//! failure that has no other answer: a cell the central plane cannot reach.
//! There is no message that stops a cell which is not listening, so the only
//! reliable bound on what an unreachable cell can lose is the clock — it stops
//! by itself when the grant runs out. Everything else about recall
//! ([`crate::recall`]) is an optimisation on top of that.
//!
//! # What this signing scheme is not
//!
//! [`EnvelopeIssuer`] signs with HMAC-SHA-256 ([`qip_core::hmac_sha256`]),
//! which is **symmetric**. Verification and signing use the same key, so every
//! cell that can check a grant can also mint one. That is adequate for a
//! single-operator deployment and for tests, and it is not adequate for
//! production. A production deployment needs, and this module deliberately
//! does not provide:
//!
//! * **Asymmetric signatures.** Ed25519 or ECDSA, so a cell holds only a
//!   public key and a compromised cell cannot issue itself capital.
//! * **A private key that never reaches application memory** — an HSM or a
//!   platform key store, with signing as a service call.
//! * **Key identity and rotation.** The `key_id` here is carried and recorded
//!   but nothing rotates it, and there is no mechanism for a cell to learn a
//!   new key or to reject one that has been retired.
//! * **Revocation.** Envelopes carry no serial number, so a leaked envelope
//!   cannot be individually revoked before its expiry — which is exactly why
//!   the expiry ceiling here is hours rather than weeks.
//! * **Replay scoping.** Nothing binds an envelope to a session or a nonce, so
//!   an envelope replayed to a restarted cell is accepted until it expires.
//!
//! Naming these here rather than in an issue tracker is deliberate: the gap
//! travels with the code that has it.
//!
//! # Since ADR 0110
//!
//! [`EnvelopeIssuer::asymmetric`] closes the first two items: it signs with
//! an [`ArtefactSigner`] under the `envelope` key — Cloud KMS in a
//! deployment, its private key never in this process — and verifies with an
//! [`ArtefactVerifier`] over the public keys a cell also holds, so a cell can
//! check a grant and cannot mint one. Key identity is the KMS key-version
//! name carried in every signature, and rotation is a new version shipped in
//! the trust bundle before it signs. Revocation of an individual envelope
//! and replay scoping are still not provided; expiry remains the backstop.
//! The shared-key issuer stays for the release ADR 0110 keeps HMAC for.

use crate::allocation::Allocation;
use qip_contracts::CapitalEnvelope;
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, ArtefactVerifier, sign_artefact};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::{constant_time_eq, from_hex, to_hex};
use qip_core::{Decimal, Duration, Timestamp, hmac_sha256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The longest an envelope may live.
///
/// Twelve hours, which is under one trading session. The number is a bet about
/// how long the central plane can be unreachable before an unattended cell
/// becomes the larger risk, and it is short because nothing here can revoke an
/// individual envelope: expiry is the only revocation mechanism there is.
pub const MAXIMUM_ENVELOPE_VALIDITY: Duration = Duration::from_hours(12);

/// What fraction of an envelope any single order may commit, by default.
const DEFAULT_ORDER_FRACTION: Decimal = Decimal::from_raw(100_000_000);

/// What fraction of an envelope may be lost before the cell stops itself.
const DEFAULT_LOSS_FRACTION: Decimal = Decimal::from_raw(200_000_000);

/// The terms of one grant, separate from the signing of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvelopeTerms {
    pub strategy: StrategyId,
    pub cell: String,
    pub gross_limit: Decimal,
    /// The most any single order may commit, as a fraction of the gross limit.
    pub order_fraction: Decimal,
    /// The loss at which the cell stops itself, as a fraction of the gross
    /// limit.
    pub loss_fraction: Decimal,
    /// Venues the grant is good at. An empty list grants no venues — see
    /// [`CapitalEnvelope::permits_venue`].
    pub venues: Vec<VenueId>,
    pub validity: Duration,
}

impl EnvelopeTerms {
    /// Terms from an allocation, with the house defaults for the sub-limits.
    pub fn from_allocation(allocation: &Allocation, validity: Duration) -> Self {
        Self {
            strategy: allocation.strategy.clone(),
            cell: allocation.cell.clone(),
            gross_limit: allocation.notional,
            order_fraction: DEFAULT_ORDER_FRACTION,
            loss_fraction: DEFAULT_LOSS_FRACTION,
            venues: vec![allocation.venue.clone()],
            validity,
        }
    }

    pub fn with_order_fraction(mut self, fraction: Decimal) -> Self {
        self.order_fraction = fraction;
        self
    }

    pub fn with_loss_fraction(mut self, fraction: Decimal) -> Self {
        self.loss_fraction = fraction;
        self
    }

    pub fn with_venues(mut self, venues: Vec<VenueId>) -> Self {
        self.venues = venues;
        self
    }
}

/// Signs and verifies capital envelopes.
///
/// Holds the key material, so it is constructed once at the central plane and
/// never crosses a process boundary. See the module documentation for what
/// this scheme does not provide.
#[derive(Clone)]
pub struct EnvelopeIssuer {
    key: IssuerKey,
    key_id: String,
}

/// What an issuer signs with.
#[derive(Clone)]
enum IssuerKey {
    /// The pre-ADR-0110 shared HMAC key.
    Shared(Vec<u8>),
    /// ADR 0110: a signer under the `envelope` key and a verifier over the
    /// public keys the cells hold.
    Asymmetric {
        signer: Arc<dyn ArtefactSigner>,
        verifier: Arc<dyn ArtefactVerifier>,
    },
}

// The key is deliberately not in the `Debug` output: a struct that prints its
// own secret gets it into a log the first time anything derives `Debug` on a
// type that contains it.
impl std::fmt::Debug for EnvelopeIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = match &self.key {
            IssuerKey::Shared(_) => "hmac-sha256",
            IssuerKey::Asymmetric { .. } => qip_contracts::trust::SIGNATURE_SCHEME,
        };
        f.debug_struct("EnvelopeIssuer")
            .field("key_id", &self.key_id)
            .field("scheme", &scheme)
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

impl EnvelopeIssuer {
    /// Build an issuer.
    ///
    /// The key must be at least 32 bytes. A short HMAC key is not a weaker
    /// signature, it is a guessable one, and this is the boundary that decides
    /// whether a cell may commit capital.
    pub fn new(signing_key: impl Into<Vec<u8>>, key_id: impl Into<String>) -> Result<Self> {
        let signing_key = signing_key.into();
        if signing_key.len() < 32 {
            return Err(Error::denied(
                "a capital signing key must be at least 32 bytes",
            ));
        }
        let key_id = key_id.into();
        if key_id.trim().is_empty() {
            return Err(Error::invalid("a signing key must be identifiable"));
        }
        Ok(Self {
            key: IssuerKey::Shared(signing_key),
            key_id,
        })
    }

    /// An issuer that signs under the `envelope` key of ADR 0110 and
    /// verifies with the public keys a cell holds.
    ///
    /// Refuses a signer published for any other class: an envelope signed
    /// under the `policy` key would be refused by every cell, and refusing
    /// it here names the misconfiguration instead of a fleet that will not
    /// trade. The key id is the signer's key version, so the records that
    /// cite a grant's key name the KMS version that made it.
    pub fn asymmetric(
        signer: Arc<dyn ArtefactSigner>,
        verifier: Arc<dyn ArtefactVerifier>,
    ) -> Result<Self> {
        if signer.class() != ArtefactClass::Envelope {
            return Err(Error::denied(format!(
                "capital envelopes are signed under the envelope key, and the signer offered is                  published for {}",
                signer.class()
            )));
        }
        let key_id = signer.key_version().to_string();
        Ok(Self {
            key: IssuerKey::Asymmetric { signer, verifier },
            key_id,
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Whether grants from this issuer carry an asymmetric signature.
    pub const fn is_asymmetric(&self) -> bool {
        matches!(self.key, IssuerKey::Asymmetric { .. })
    }

    /// The signature this issuer would produce over a payload.
    fn sign(&self, payload: &str) -> Result<String> {
        match &self.key {
            IssuerKey::Shared(key) => Ok(to_hex(&hmac_sha256(key, payload.as_bytes()))),
            IssuerKey::Asymmetric { signer, .. } => {
                sign_artefact(signer.as_ref(), ArtefactClass::Envelope, payload.as_bytes())
            }
        }
    }

    /// Issue a signed, expiring envelope.
    ///
    /// The [`Approval`] is required and must be dual. This is the same rule
    /// the lifecycle gates apply, restated at the point capital actually
    /// moves, because the two are separate systems and a grant issued outside
    /// a promotion would otherwise carry no human decision at all.
    pub fn issue(
        &self,
        terms: &EnvelopeTerms,
        approval: &Approval,
        now: Timestamp,
    ) -> Result<CapitalEnvelope> {
        if !approval.is_dual() {
            return Err(Error::denied(format!(
                "issuing capital to {} needs two approvers; {} approved alone",
                terms.cell, approval.approver
            )));
        }
        if terms.validity <= Duration::ZERO {
            return Err(Error::invalid(
                "an envelope with no validity period grants nothing",
            ));
        }
        if terms.validity > MAXIMUM_ENVELOPE_VALIDITY {
            return Err(Error::denied(format!(
                "an envelope may not live longer than {:.1} hour(s); expiry is the only \
                 backstop against a cell the central plane cannot reach",
                MAXIMUM_ENVELOPE_VALIDITY.as_secs_f64() / 3600.0
            )));
        }
        for (label, fraction) in [
            ("order", terms.order_fraction),
            ("loss", terms.loss_fraction),
        ] {
            if !fraction.is_positive() || fraction > Decimal::ONE {
                return Err(Error::invalid(format!(
                    "the {label} fraction must lie in (0, 1]"
                )));
            }
        }

        let order_limit = terms
            .gross_limit
            .checked_mul(terms.order_fraction)
            .ok_or_else(|| Error::numeric("the order limit overflowed"))?;
        let loss_limit = terms
            .gross_limit
            .checked_mul(terms.loss_fraction)
            .ok_or_else(|| Error::numeric("the loss limit overflowed"))?;
        let expires_at = now.saturating_add(terms.validity);

        // Built once unsigned to obtain the payload, then rebuilt with the
        // signature over it. The payload is a function of the fields, so the
        // two constructions agree by construction.
        let unsigned = CapitalEnvelope::new(
            terms.strategy.clone(),
            terms.cell.clone(),
            terms.gross_limit,
            order_limit,
            loss_limit,
            terms.venues.clone(),
            now,
            expires_at,
            approval.approver.clone(),
            String::new(),
        )?;
        let signature = self.sign(&unsigned.signing_payload())?;

        CapitalEnvelope::new(
            terms.strategy.clone(),
            terms.cell.clone(),
            terms.gross_limit,
            order_limit,
            loss_limit,
            terms.venues.clone(),
            now,
            expires_at,
            approval.approver.clone(),
            signature,
        )
    }

    /// Verify a grant, refusing an unsigned, tampered or expired one.
    ///
    /// Signatures are compared in constant time. A comparison that returns
    /// early on the first differing byte tells an attacker how much of a
    /// forged signature is correct, which turns forging one from infeasible
    /// into a few thousand attempts per byte.
    pub fn verify(&self, envelope: &CapitalEnvelope, now: Timestamp) -> Result<()> {
        let refusal = || {
            Error::denied(format!(
                "the envelope for {} at {} does not verify under key {}; its terms have been \
                 altered since it was signed, or it was not signed here",
                envelope.strategy(),
                envelope.cell(),
                self.key_id
            ))
        };
        match &self.key {
            IssuerKey::Shared(_) => {
                let presented = from_hex(envelope.signature()).ok_or_else(|| {
                    Error::denied("the envelope carries no readable signature and is refused")
                })?;
                let expected =
                    from_hex(&self.sign(&envelope.signing_payload())?).ok_or_else(|| {
                        Error::numeric("the expected signature was not representable")
                    })?;
                if !constant_time_eq(&presented, &expected) {
                    return Err(refusal());
                }
            }
            IssuerKey::Asymmetric { verifier, .. } => {
                verifier
                    .verify(
                        ArtefactClass::Envelope,
                        envelope.signing_payload().as_bytes(),
                        envelope.signature(),
                        now,
                    )
                    .map_err(|error| {
                        Error::denied(format!("{}: {}", refusal().message(), error.message()))
                    })?;
            }
        }
        if !envelope.is_live(now) {
            return Err(Error::denied(format!(
                "the envelope for {} at {} expired at {}",
                envelope.strategy(),
                envelope.cell(),
                envelope.expires_at().to_rfc3339()
            )));
        }
        Ok(())
    }

    /// Whether a grant verifies, without the reason.
    pub fn is_valid(&self, envelope: &CapitalEnvelope, now: Timestamp) -> bool {
        self.verify(envelope, now).is_ok()
    }
}
