//! ECDSA P-256 verification against a trust bundle, and the local signer
//! for development and tests, on `ring` (ADR 0110 decisions 2 and 3).
//!
//! The types and the signed bytes are `qip_contracts::trust`'s, which stays
//! in the decision core at two dependencies. This module is the part that
//! needs a curve: [`BundleVerifier`] checks a signature with `ring`'s
//! `ECDSA_P256_SHA256_ASN1` against the public key the bundle names, and
//! [`LocalSigner`] signs with a key held in memory.
//!
//! **Why here.** `qip-transport` already carries `ring` as the provider of
//! its TLS (ADR 0112), and ADR 0110 §3 chose `ring` for both jobs so the
//! platform holds one cryptography library. Putting the verifier in the one
//! crate that already links it keeps that true at the crate level as well:
//! `qip-edge` verifies through this module and links nothing of its own.
//!
//! **The verify path makes no network call.** It is a pure function of the
//! bundle, the class, the bytes and the signature — a cell that cannot reach
//! Google must still be able to accept a halt (ADR 0008, ADR 0110
//! alternative "KMS `asymmetricVerify` as a service call").
//!
//! **The local signer is not a production signer.** Its key is in process
//! memory, which is the thing ADR 0110 exists to stop; the composition roots
//! admit it only in a `local` deployment. In a deployment the signer is
//! [`crate::kms::KmsSigner`], whose key never leaves the HSM.

use qip_contracts::trust::{
    ArtefactClass, ArtefactSignature, ArtefactSigner, ArtefactVerifier, TrustBundle,
    signing_message,
};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair,
    UnparsedPublicKey,
};

/// Verifies artefact signatures against the public keys of one bundle.
#[derive(Clone, Debug)]
pub struct BundleVerifier {
    bundle: TrustBundle,
}

impl BundleVerifier {
    pub const fn new(bundle: TrustBundle) -> Self {
        Self { bundle }
    }

    /// Read and admit a committed bundle file. A bundle that does not parse,
    /// or holds one malformed key, stops the caller — a reader that started
    /// with half a bundle would refuse frames under the missing half and look
    /// like an attack.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            Error::invalid(format!(
                "cannot read the trust bundle at {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self::new(TrustBundle::from_json(&raw)?))
    }

    pub const fn bundle(&self) -> &TrustBundle {
        &self.bundle
    }
}

impl ArtefactVerifier for BundleVerifier {
    fn verify(
        &self,
        class: ArtefactClass,
        canonical: &[u8],
        signature: &str,
        now: Timestamp,
    ) -> Result<()> {
        let signature = ArtefactSignature::parse(signature)?;
        let point = self.bundle.key_for(class, signature.key_version(), now)?;
        let message = signing_message(class, signature.key_version(), canonical);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &point)
            .verify(&message, signature.der())
            .map_err(|_| {
                // `ring` deliberately says nothing about why; neither does
                // this, beyond what was checked against what.
                Error::denied(format!(
                    "the {class} signature does not verify under key version {}; the artefact \
                     was altered after signing, or was not signed under that key",
                    signature.key_version()
                ))
            })
    }
}

/// The last day a process may start on the shared HMAC key: the "one
/// release" ADR 0110's order of work keeps HMAC verification for, named as a
/// date so the refusal can say when it lands. After it, a node configured
/// with only `QIP_CAPITAL_ENVELOPE_KEY` stops at start-up and names the trust
/// bundle variable to set instead, and the API refuses to sign under it
/// outside a `local` deployment.
pub const SHARED_KEY_REMOVED_AFTER: &str = "2027-01-31";

/// `SHARED_KEY_REMOVED_AFTER` as an instant: the end of that day, UTC.
fn shared_key_deadline() -> Timestamp {
    Timestamp::from_civil(2027, 2, 1)
}

/// Refuse the shared-key posture after [`SHARED_KEY_REMOVED_AFTER`].
///
/// Called by a composition root at start-up, with the wall clock it read
/// once at the boundary, where a process that will not start names its own
/// fix — **and again at every use**: by `qip_edge::TrustRoot::check` for
/// each artefact a cell verifies, and by the API's mesh backbone for each
/// dispatch it would sign. This said "checked at start-up rather than per
/// frame" until an independent review pointed out what that meant: a
/// process started on the last permitted day verified and signed under the
/// shared key for as long as it stayed up, so the date removed nothing from
/// a fleet nobody restarts. Refusing at midnight is an outage — every frame
/// under the shared key is refused from then on — and that is the intended
/// trade: the date has been in every banner and refusal since the key was
/// deprecated, and a cell that stops accepting policy narrows rather than
/// trades on a key every node can mint with.
pub fn refuse_shared_key_after_removal(now: Timestamp) -> Result<()> {
    if now >= shared_key_deadline() {
        return Err(Error::denied(format!(
            "this process is configured to sign or verify the centre's artefacts with the shared \
             HMAC key, and shared-key use was removed after {SHARED_KEY_REMOVED_AFTER} (ADR \
             0110): a key that verifies can also sign, so every node holding it could mint \
             policy and capital for the fleet. Set QIP_TRUST_BUNDLE_PATH to the committed trust \
             bundle (and, on the API, QIP_ARTEFACT_SIGNER=kms) instead"
        )));
    }
    Ok(())
}

/// A fresh PKCS#8 ECDSA P-256 document from the operating system's entropy,
/// for a developer's local signing-keys file and for tests. Never called by a
/// deployed process: a deployed signer's key is generated inside Cloud KMS.
pub fn generate_pkcs8() -> Result<Vec<u8>> {
    EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
        .map(|document| document.as_ref().to_vec())
        .map_err(|_| Error::unavailable("the operating system's random source refused a key"))
}

/// An ECDSA P-256 key held in memory, for a `local` deployment and for
/// tests. See the module documentation for why nothing deployed uses it.
pub struct LocalSigner {
    class: ArtefactClass,
    key_version: String,
    pair: EcdsaKeyPair,
    random: SystemRandom,
}

// The key pair is deliberately absent from `Debug`.
impl std::fmt::Debug for LocalSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSigner")
            .field("class", &self.class)
            .field("key_version", &self.key_version)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl LocalSigner {
    /// A fresh key from the operating system's entropy.
    pub fn generate(class: ArtefactClass, key_version: impl Into<String>) -> Result<Self> {
        let random = SystemRandom::new();
        let document = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random)
            .map_err(|_| {
                Error::unavailable("the operating system's random source refused a key")
            })?;
        Self::from_pkcs8(class, key_version, document.as_ref())
    }

    /// A key from a PKCS#8 document — what a developer's local key file
    /// holds. Refuses anything that is not a P-256 key.
    pub fn from_pkcs8(
        class: ArtefactClass,
        key_version: impl Into<String>,
        pkcs8: &[u8],
    ) -> Result<Self> {
        let random = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &random)
            .map_err(|_| {
                Error::invalid(
                    "the local signing key is not a PKCS#8 ECDSA P-256 key; ADR 0110 signs with \
                     EC_SIGN_P256_SHA256 and nothing else",
                )
            })?;
        let key_version = key_version.into();
        // Checked through the signature type so the rules for a key-version
        // name live in one place.
        ArtefactSignature::new(key_version.clone(), vec![0x30; 8])?;
        Ok(Self {
            class,
            key_version,
            pair,
            random,
        })
    }

    /// The uncompressed public point, as hex — the `public_key` a trust
    /// bundle entry for this signer carries.
    pub fn public_key_hex(&self) -> String {
        qip_core::hash::to_hex(self.pair.public_key().as_ref())
    }
}

impl ArtefactSigner for LocalSigner {
    fn class(&self) -> ArtefactClass {
        self.class
    }

    fn key_version(&self) -> &str {
        &self.key_version
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        self.pair
            .sign(&self.random, message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| Error::unavailable("the local signer could not produce a signature"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::trust::{TrustedKey, sign_artefact};
    use std::collections::BTreeSet;

    fn bundle_for(signers: &[&LocalSigner]) -> TrustBundle {
        TrustBundle::new(
            signers
                .iter()
                .map(|signer| TrustedKey {
                    class: signer.class(),
                    key_version: signer.key_version().to_string(),
                    public_key: signer.public_key_hex(),
                    not_after: Timestamp::from_secs(4_000_000_000),
                })
                .collect(),
            BTreeSet::new(),
        )
        .expect("a bundle of generated keys")
    }

    /// Mutation: make `verify` return `Ok(())` without calling `ring`, or
    /// verify against the message without the class, and the altered-bytes
    /// and wrong-class arms here fail.
    #[test]
    fn a_signature_verifies_only_over_its_own_bytes_class_and_key() {
        let now = Timestamp::from_secs(1_800_000_000);
        let policy = LocalSigner::generate(ArtefactClass::Policy, "test/policy/1").expect("key");
        let other = LocalSigner::generate(ArtefactClass::Policy, "test/policy/2").expect("key");
        let verifier = BundleVerifier::new(bundle_for(&[&policy, &other]));
        let signature =
            sign_artefact(&policy, ArtefactClass::Policy, b"sequence=7").expect("signs");
        // Premise: the genuine signature verifies.
        verifier
            .verify(ArtefactClass::Policy, b"sequence=7", &signature, now)
            .expect("a genuine signature was refused");
        // One byte of the artefact altered.
        assert!(
            verifier
                .verify(ArtefactClass::Policy, b"sequence=8", &signature, now)
                .is_err()
        );
        // The same DER re-attributed to a different trusted key version.
        let parsed = ArtefactSignature::parse(&signature).expect("parses");
        let moved = ArtefactSignature::new("test/policy/2", parsed.der().to_vec())
            .expect("well formed")
            .encode();
        assert!(
            verifier
                .verify(ArtefactClass::Policy, b"sequence=7", &moved, now)
                .is_err()
        );
    }

    /// Mutation: let a bundle verifier accept a key from outside the bundle
    /// (e.g. skip `key_for`) and a self-generated attacker key passes; this
    /// fails.
    #[test]
    fn a_signature_under_a_key_outside_the_bundle_is_refused() {
        let now = Timestamp::from_secs(1_800_000_000);
        let trusted = LocalSigner::generate(ArtefactClass::Halt, "test/halt/1").expect("key");
        let attacker = LocalSigner::generate(ArtefactClass::Halt, "test/halt/9").expect("key");
        let verifier = BundleVerifier::new(bundle_for(&[&trusted]));
        let forged = sign_artefact(&attacker, ArtefactClass::Halt, b"halt").expect("signs");
        assert!(
            verifier
                .verify(ArtefactClass::Halt, b"halt", &forged, now)
                .is_err()
        );
        // And an attacker key claiming the trusted version's name still fails
        // on the curve, not on the name.
        let impostor = LocalSigner::from_pkcs8(
            ArtefactClass::Halt,
            "test/halt/1",
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                .expect("key")
                .as_ref(),
        )
        .expect("key");
        let forged = sign_artefact(&impostor, ArtefactClass::Halt, b"halt").expect("signs");
        assert!(
            verifier
                .verify(ArtefactClass::Halt, b"halt", &forged, now)
                .is_err()
        );
    }
}
