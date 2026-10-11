//! What a cell verifies the centre's artefacts against (ADR 0110).
//!
//! Two roots, and a cell holds exactly one of them:
//!
//! * [`TrustRoot::Bundle`] — the public keys of a committed trust bundle,
//!   one per active key version per class. Policy payloads, halts and
//!   capital envelopes must carry an `ecdsa-p256-sha256` signature under the
//!   key published for their class, verified with `ring` through
//!   [`qip_transport::trust::BundleVerifier`]. The cell holds no secret: a
//!   node's disk read in full no longer yields the ability to mint policy or
//!   capital for every region, which is the property ADR 0110 exists for.
//! * [`TrustRoot::SharedKey`] — the HMAC key the platform used before ADR
//!   0110. Kept for one release so a fleet can move without a flag day, and
//!   refused at start-up after [`SHARED_KEY_REMOVED_AFTER`] — see
//!   [`refuse_shared_key_after_removal`].
//!
//! **Never both.** A cell that accepted either scheme would be exactly as
//! strong as the weaker one, which ADR 0110's order of work names. A bundle
//! cell refuses an HMAC by construction — an HMAC hex string never parses as
//! an asymmetric signature — and a shared-key cell refuses an asymmetric
//! signature by name rather than trying it as a MAC.
//!
//! **No network call on this path.** Verification is a pure function of the
//! root, the bytes and the signature; a cell that cannot reach Google must
//! still be able to accept a halt (ADR 0008).

use qip_contracts::trust::{ArtefactClass, ArtefactSignature, ArtefactVerifier, SIGNATURE_SCHEME};
use qip_core::error::{Error, Result};
use qip_core::hash::to_hex;
use qip_core::{Timestamp, hmac_sha256};
use qip_transport::trust::BundleVerifier;
use std::sync::Arc;

/// The removal date and its start-up refusal live in `qip-transport` so the
/// API — which signs, and does not link this crate — and the edge node —
/// which verifies — read one date and cannot drift apart.
pub use qip_transport::trust::{SHARED_KEY_REMOVED_AFTER, refuse_shared_key_after_removal};

/// The keys a cell checks the centre's signatures against.
#[derive(Clone)]
pub enum TrustRoot {
    /// Public keys under ADR 0110. Asymmetric signatures only.
    Bundle(Arc<BundleVerifier>),
    /// The pre-ADR-0110 shared HMAC key. Never empty.
    SharedKey(Vec<u8>),
}

// The shared key is deliberately absent from `Debug`; the bundle holds only
// public keys and may be shown.
impl std::fmt::Debug for TrustRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundle(verifier) => f
                .debug_tuple("Bundle")
                .field(&verifier.bundle().keys().len())
                .finish(),
            Self::SharedKey(_) => f.debug_tuple("SharedKey").field(&"<redacted>").finish(),
        }
    }
}

impl TrustRoot {
    /// A shared-key root, refusing an empty key: a cell that accepted one
    /// would verify nothing while looking like it verified everything.
    pub fn shared_key(key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a cell with no key cannot verify anything the centre sends and must not trade",
            ));
        }
        Ok(Self::SharedKey(key.to_vec()))
    }

    pub fn bundle(verifier: BundleVerifier) -> Self {
        Self::Bundle(Arc::new(verifier))
    }

    pub const fn is_asymmetric(&self) -> bool {
        matches!(self, Self::Bundle(_))
    }

    /// One line for the node's banner.
    pub fn describe(&self) -> String {
        match self {
            Self::Bundle(verifier) => format!(
                "trust bundle of {} public key(s); asymmetric signatures only (ADR 0110)",
                verifier.bundle().keys().len()
            ),
            Self::SharedKey(_) => format!(
                "SHARED HMAC KEY — whoever can verify can also sign; refused from \
                 {SHARED_KEY_REMOVED_AFTER}"
            ),
        }
    }

    /// Refuse unless `signature` over `canonical` is valid for `class` under
    /// this root at `now`.
    pub fn check(
        &self,
        class: ArtefactClass,
        canonical: &[u8],
        signature: &str,
        now: Timestamp,
    ) -> Result<()> {
        match self {
            Self::Bundle(verifier) => verifier.verify(class, canonical, signature, now),
            Self::SharedKey(key) => {
                if ArtefactSignature::parse(signature).is_ok() {
                    return Err(Error::denied(format!(
                        "this cell verifies with a shared key and the {class} artefact carries an \
                         {SIGNATURE_SCHEME} signature; install the trust bundle rather than \
                         trying a public-key signature as a MAC"
                    )));
                }
                let expected = to_hex(&hmac_sha256(key, canonical));
                if crate::envelope::constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
                    Ok(())
                } else {
                    Err(Error::denied(format!(
                        "the {class} artefact does not verify against this cell's key"
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mutation: invert the comparison in `refuse_shared_key_after_removal`
    /// (or drop it) and one of the two arms fails.
    #[test]
    fn a_shared_key_is_admitted_until_its_removal_date_and_refused_from_it() {
        assert!(refuse_shared_key_after_removal(Timestamp::from_civil(2027, 1, 31)).is_ok());
        let refused = refuse_shared_key_after_removal(Timestamp::from_civil(2027, 2, 1))
            .expect_err("a shared key was admitted after its removal date");
        assert!(refused.message().contains(SHARED_KEY_REMOVED_AFTER));
        assert!(refused.message().contains("QIP_TRUST_BUNDLE_PATH"));
    }

    /// Mutation: let the shared-key arm try an asymmetric string as a MAC, or
    /// accept any string of the right length; this fails.
    #[test]
    fn a_shared_key_root_verifies_its_own_mac_and_refuses_an_asymmetric_signature() {
        let root = TrustRoot::shared_key(b"a-shared-key-of-thirty-two-bytes!").expect("a key");
        let now = Timestamp::from_secs(1_800_000_000);
        let mac = to_hex(&hmac_sha256(
            b"a-shared-key-of-thirty-two-bytes!",
            b"payload",
        ));
        // Premise: the genuine MAC verifies.
        assert!(
            root.check(ArtefactClass::Policy, b"payload", &mac, now)
                .is_ok()
        );
        assert!(
            root.check(ArtefactClass::Policy, b"payloaD", &mac, now)
                .is_err()
        );
        let asymmetric = ArtefactSignature::new("projects/p/k/1", vec![0x30; 70])
            .expect("well formed")
            .encode();
        assert!(
            root.check(ArtefactClass::Policy, b"payload", &asymmetric, now)
                .is_err()
        );
        assert!(TrustRoot::shared_key(b"").is_err());
        assert!(!format!("{root:?}").contains("thirty-two"));
    }
}
