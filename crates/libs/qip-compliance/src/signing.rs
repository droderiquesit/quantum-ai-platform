//! The signing key shared by the capital-approval and artifact controls.
//!
//! # What this is, and what it is not
//!
//! [`SigningKey`] is HMAC-SHA256 over an injected secret, using
//! `qip_core::hmac_sha256`. It is a genuine integrity control: bytes that
//! change do not verify, and a signature cannot be produced without the
//! secret. It is **not** a signature in the sense a regulator or a
//! counterparty means, and a deployment that treats it as one has a control
//! that looks stronger than it is.
//!
//! HMAC is symmetric. Whoever can verify can also sign, so a signature proves
//! only that *someone holding the key* produced the artifact — never which
//! person or service did. Concretely, a production deployment needs all of:
//!
//! * **Asymmetric signing** — Ed25519 or ECDSA P-256, private key held in a
//!   KMS or HSM and never in process memory, so verification does not confer
//!   the ability to sign.
//! * **Identity binding** — a certificate or key-attestation chain tying the
//!   public key to a named signer, so `Provenance::signer` is a claim the
//!   verifier can check rather than a self-declared string.
//! * **Key rotation and revocation** — key ids in the signature, an overlap
//!   window, and a revocation list, so a leaked key invalidates its artifacts
//!   without invalidating everything.
//! * **Countersigned timestamping** — an independent time source over the
//!   signature, so `built_at` cannot be backdated by the signer.
//!
//! None of those are in this build; there is no KMS and no key material here.
//! The key id travels with every signature so that when asymmetric signing
//! arrives, existing records say which key they were made under.

use qip_core::error::{Error, Result};
use qip_core::hash::{constant_time_eq, from_hex, hmac_sha256, to_hex};

/// A symmetric key used to sign and verify platform artifacts.
///
/// The secret is private and there is no accessor for it — a key that can be
/// read back out of the type ends up in a log line eventually.
#[derive(Clone)]
pub struct SigningKey {
    key_id: String,
    secret: Vec<u8>,
}

/// Redacts the secret. The key id is safe to print and is what an audit needs.
impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Below this, a secret is guessable and the signature is decoration.
const MINIMUM_SECRET_BYTES: usize = 32;

impl SigningKey {
    /// Take a key from a deployment's secret material.
    ///
    /// Refuses a short secret. A 16-byte HMAC key is within reach of an
    /// offline search, and a signature that can be forged is worse than no
    /// signature because the store reports it as verified.
    pub fn from_secret(key_id: impl Into<String>, secret: &[u8]) -> Result<Self> {
        let key_id = key_id.into();
        if key_id.trim().is_empty() {
            return Err(Error::invalid(
                "a signing key must have an id; signatures made under an unnamed key cannot be \
                 rotated or revoked",
            ));
        }
        if secret.len() < MINIMUM_SECRET_BYTES {
            return Err(Error::invalid(format!(
                "a signing secret must be at least {MINIMUM_SECRET_BYTES} bytes; {} is short \
                 enough to search offline",
                secret.len()
            )));
        }
        Ok(Self {
            key_id,
            secret: secret.to_vec(),
        })
    }

    /// Which key a signature was made under, carried alongside every record.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Sign a payload, returning lowercase hex.
    ///
    /// The key id is mixed into the message as well as travelling beside it,
    /// so a signature made under one key cannot be replayed as though it were
    /// made under another after a rotation.
    pub fn sign(&self, payload: &str) -> String {
        let message = format!("{}|{payload}", self.key_id);
        to_hex(&hmac_sha256(&self.secret, message.as_bytes()))
    }

    /// Whether a signature covers a payload.
    ///
    /// Compared in constant time: a verifier that returns early on the first
    /// wrong byte tells an attacker how much of a forgery was right.
    pub fn verifies(&self, payload: &str, signature: &str) -> bool {
        let Some(provided) = from_hex(signature) else {
            return false;
        };
        let message = format!("{}|{payload}", self.key_id);
        constant_time_eq(&provided, &hmac_sha256(&self.secret, message.as_bytes()))
    }

    /// Verify or explain the failure, for call sites that must not continue.
    pub fn require(&self, what: &str, payload: &str, signature: &str) -> Result<()> {
        if signature.trim().is_empty() {
            return Err(Error::denied(format!(
                "{what} carries no signature; an unsigned artifact is not admissible"
            )));
        }
        if self.verifies(payload, signature) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "the signature on {what} does not verify under key {}; either the content changed \
             after signing or it was signed under a different key",
            self.key_id
        )))
    }
}
