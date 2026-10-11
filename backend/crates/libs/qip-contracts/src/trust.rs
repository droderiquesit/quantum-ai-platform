//! Asymmetric signatures on the artefacts the centre ships: the types, the
//! bytes that are signed, and nothing that computes a curve (ADR 0110).
//!
//! Until this module every signature in the platform was an HMAC (ADR 0043
//! §2), and "whoever can verify can also sign": a cell holding the key that
//! checks a policy frame held the key that mints one, for every region. ADR
//! 0110 replaces that with ECDSA P-256 under Cloud KMS keys that never leave
//! the HSM, verified locally against a **trust bundle** of public keys.
//!
//! This crate is in ADR 0009's decision core and stays at `serde` and
//! `serde_json`, so what lives here is exactly what needs no cryptography
//! library:
//!
//! * [`ArtefactClass`] — one key per class, so a signature under the `halt`
//!   key can never verify as a `policy` frame, and a rotation of one class
//!   moves nothing else (R12-SEC-9137's key separation, made structural).
//! * [`signing_message`] — the bytes actually signed: a domain tag, the
//!   class, the full key-version resource name and the artefact's own
//!   canonical bytes, each length-prefixed so no two different triples share
//!   a message.
//! * [`ArtefactSignature`] — the self-describing text form carried in the
//!   existing `signature` fields, so a frame says which key signed it and an
//!   HMAC hex string can never be mistaken for one.
//! * [`TrustBundle`] — the public keys a verifier trusts, with a not-after
//!   instant each and a revocation list, refused whole if any entry is
//!   malformed.
//! * [`ArtefactSigner`] and [`ArtefactVerifier`] — the two seams. The signer
//!   is a KMS call in production and a local key for development; the
//!   verifier is `ring` in `qip-transport`. Neither implementation is in this
//!   crate, which is the point.
//! * [`Publication`] — the two distinct people a widening artefact names
//!   (ADR 0110 decision 4). A halt never carries one: stopping must never
//!   wait for a second approver.

use crate::policy::length_prefixed_bytes;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The scheme tag every asymmetric signature string starts with.
///
/// ECDSA over P-256 with SHA-256, DER-encoded, which is what Cloud KMS's
/// `EC_SIGN_P256_SHA256` returns from `asymmetricSign` and what `ring`'s
/// `ECDSA_P256_SHA256_ASN1` verifies.
pub const SIGNATURE_SCHEME: &str = "ecdsa-p256-sha256";

/// The domain tag that opens every signed message. Versioned so a change to
/// the framing below is a new tag, and a signature under the old framing
/// cannot verify under the new one by coincidence.
const MESSAGE_DOMAIN: &[u8] = b"qip-artefact-signature/v1";

/// The fixed DER prefix of a P-256 `SubjectPublicKeyInfo`: SEQUENCE,
/// AlgorithmIdentifier {id-ecPublicKey, prime256v1}, BIT STRING header.
/// Cloud KMS's `getPublicKey` returns exactly this followed by the 65-byte
/// uncompressed point, so a bundle may carry either form and both reduce to
/// the point. A prefix that differs names another curve or another
/// algorithm, and is refused rather than parsed further.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The length of an uncompressed P-256 point: `0x04 ‖ X ‖ Y`.
const P256_POINT_LENGTH: usize = 65;

/// The longest DER ECDSA P-256 signature: two 33-byte INTEGERs and their
/// headers inside a SEQUENCE. Anything longer is not one, and is refused
/// before a verifier is asked.
const MAX_DER_SIGNATURE: usize = 72;

/// What an artefact is, and therefore which key signs it.
///
/// No wildcard ever matches over this: a fifth class must be named at every
/// seam, because a class that fell through to an existing key would be
/// signed under a key whose publish rule was written for something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtefactClass {
    /// A cell's [`crate::policy::PolicyPayload`].
    Policy,
    /// A [`crate::policy::HaltCommand`]. One-person by design.
    Halt,
    /// A paper [`crate::CapitalEnvelope`].
    Envelope,
    /// A [`crate::policy::ModelPack`].
    ModelPack,
}

impl ArtefactClass {
    pub const ALL: [Self; 4] = [Self::Policy, Self::Halt, Self::Envelope, Self::ModelPack];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Halt => "halt",
            Self::Envelope => "envelope",
            Self::ModelPack => "model-pack",
        }
    }

    /// Whether publishing this class needs two distinct people (ADR 0110
    /// decision 4). A halt only restricts, and a control that stops trading
    /// must never wait for a second approver; everything else can widen
    /// what a cell may do.
    pub const fn widens(self) -> bool {
        match self {
            Self::Halt => false,
            Self::Policy | Self::Envelope | Self::ModelPack => true,
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|class| class.as_str() == raw)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{raw:?} is not an artefact class; the classes are policy, halt, envelope \
                     and model-pack"
                ))
            })
    }
}

impl std::fmt::Display for ArtefactClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The exact bytes a signer signs and a verifier checks.
///
/// `domain ‖ lp(class) ‖ lp(key version) ‖ lp(canonical)`, where `lp` is an
/// eight-byte big-endian length followed by the bytes. Every part is
/// length-prefixed, so the encoding is injective: no `(class, key, bytes)`
/// triple shares a message with any other, whatever the bytes contain. The
/// class is inside the message so a signature cannot be moved between
/// classes even under a key mistakenly published for two; the key version
/// is inside so a signature cannot be re-attributed to a different version
/// of the same key.
pub fn signing_message(class: ArtefactClass, key_version: &str, canonical: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        MESSAGE_DOMAIN.len() + 24 + class.as_str().len() + key_version.len() + canonical.len(),
    );
    message.extend_from_slice(MESSAGE_DOMAIN);
    for part in [class.as_str().as_bytes(), key_version.as_bytes(), canonical] {
        message.extend_from_slice(&(part.len() as u64).to_be_bytes());
        message.extend_from_slice(part);
    }
    message
}

/// An asymmetric signature as it travels in an artefact's `signature` field:
/// `ecdsa-p256-sha256:<key version>:<hex DER>`.
///
/// Self-describing on purpose. The fields it travels in held a bare HMAC hex
/// string until ADR 0110, and a verifier that guessed the scheme from the
/// length of a string would be one bug from accepting the weaker one. An
/// HMAC never parses as this, so a cell holding a trust bundle refuses it by
/// construction rather than by a check someone could remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtefactSignature {
    key_version: String,
    der: Vec<u8>,
}

impl ArtefactSignature {
    /// Wrap a DER signature made under `key_version`.
    pub fn new(key_version: impl Into<String>, der: Vec<u8>) -> Result<Self> {
        let key_version = key_version.into();
        check_key_version(&key_version)?;
        check_der_length(der.len())?;
        Ok(Self { key_version, der })
    }

    /// Parse the text form, refusing anything that is not exactly it.
    pub fn parse(raw: &str) -> Result<Self> {
        let Some(rest) = raw
            .strip_prefix(SIGNATURE_SCHEME)
            .and_then(|rest| rest.strip_prefix(':'))
        else {
            return Err(Error::denied(format!(
                "the signature is not an {SIGNATURE_SCHEME} signature (it does not begin \
                 `{SIGNATURE_SCHEME}:`); an artefact verified against a trust bundle must be \
                 signed under a published key, and a shared-key MAC is refused rather than \
                 accepted as the weaker of two schemes"
            )));
        };
        let Some((key_version, hex)) = rest.rsplit_once(':') else {
            return Err(Error::denied(
                "the signature names no key version; the form is \
                 `ecdsa-p256-sha256:<key version>:<hex DER>`",
            ));
        };
        let der = from_hex(hex).ok_or_else(|| {
            Error::denied("the signature's DER part is not hexadecimal and cannot be verified")
        })?;
        Self::new(key_version, der).map_err(|error| Error::denied(error.message().to_string()))
    }

    pub fn key_version(&self) -> &str {
        &self.key_version
    }

    pub fn der(&self) -> &[u8] {
        &self.der
    }

    pub fn encode(&self) -> String {
        format!(
            "{SIGNATURE_SCHEME}:{}:{}",
            self.key_version,
            to_hex(&self.der)
        )
    }
}

fn check_key_version(key_version: &str) -> Result<()> {
    if key_version.trim().is_empty() {
        return Err(Error::invalid(
            "a key version must be named; it is the full KMS resource name of the version \
             that signed",
        ));
    }
    if key_version.contains(':') || key_version.chars().any(char::is_whitespace) {
        return Err(Error::invalid(format!(
            "key version {key_version:?} contains a colon or whitespace; a KMS key-version \
             resource name has neither, and the signature's text form uses the colon as its \
             delimiter"
        )));
    }
    Ok(())
}

fn check_der_length(length: usize) -> Result<()> {
    if !(8..=MAX_DER_SIGNATURE).contains(&length) {
        return Err(Error::invalid(format!(
            "a DER ECDSA P-256 signature is between 8 and {MAX_DER_SIGNATURE} bytes, and this \
             one is {length}"
        )));
    }
    Ok(())
}

/// One public key a verifier trusts, as the bundle file states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKey {
    /// The one class this key verifies.
    pub class: ArtefactClass,
    /// The full KMS key-version resource name, as it appears in a signature.
    pub key_version: String,
    /// Hex of the P-256 public key: the `SubjectPublicKeyInfo` DER KMS's
    /// `getPublicKey` returns (decoded from its PEM), or the bare 65-byte
    /// uncompressed point.
    pub public_key: String,
    /// After this instant the key verifies nothing. A rotation adds the new
    /// version before the publisher uses it and lets the old one lapse here.
    pub not_after: Timestamp,
}

/// The bundle as written in the committed file, before it is checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustBundleFile {
    keys: Vec<TrustedKey>,
    #[serde(default)]
    revoked: BTreeSet<String>,
}

/// The public keys a cell or a brain verifies artefacts against (ADR 0110
/// decision 2): one per active key version per class, each with its
/// not-after, and the versions revoked.
///
/// Baked into the attested image and read from a file at start-up, never
/// fetched at run time — a bundle fetched from a server moves trust from the
/// image Binary Authorization proved to whatever answers the fetch. Refused
/// whole when any entry is malformed: a bundle that dropped the bad entry and
/// kept the rest would be a different bundle from the one reviewed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TrustBundleFile", into = "TrustBundleFile")]
pub struct TrustBundle {
    keys: Vec<TrustedKey>,
    revoked: BTreeSet<String>,
}

impl TryFrom<TrustBundleFile> for TrustBundle {
    type Error = Error;

    fn try_from(file: TrustBundleFile) -> Result<Self> {
        Self::new(file.keys, file.revoked)
    }
}

impl From<TrustBundle> for TrustBundleFile {
    fn from(bundle: TrustBundle) -> Self {
        Self {
            keys: bundle.keys,
            revoked: bundle.revoked,
        }
    }
}

impl TrustBundle {
    /// Admit a bundle, refusing it whole on the first malformed entry.
    pub fn new(keys: Vec<TrustedKey>, revoked: BTreeSet<String>) -> Result<Self> {
        if keys.is_empty() {
            return Err(Error::invalid(
                "a trust bundle with no keys verifies nothing, so every artefact would be \
                 refused; publish at least the active version of each class the reader verifies",
            ));
        }
        let mut seen = BTreeSet::new();
        for key in &keys {
            check_key_version(&key.key_version)?;
            if !seen.insert(key.key_version.as_str()) {
                return Err(Error::invalid(format!(
                    "the trust bundle names key version {} twice; one version is one key, and \
                     two entries could disagree about its class or its public key",
                    key.key_version
                )));
            }
            public_point(&key.public_key).map_err(|error| {
                Error::invalid(format!(
                    "the trust bundle's entry for {}: {}",
                    key.key_version,
                    error.message()
                ))
            })?;
        }
        Ok(Self { keys, revoked })
    }

    /// Parse and admit the committed file.
    pub fn from_json(raw: &str) -> Result<Self> {
        serde_json::from_str(raw)
            .map_err(|error| Error::invalid(format!("the trust bundle does not parse: {error}")))
    }

    pub fn keys(&self) -> &[TrustedKey] {
        &self.keys
    }

    pub fn revoked(&self) -> &BTreeSet<String> {
        &self.revoked
    }

    /// The classes this bundle can verify at `now`.
    pub fn classes_at(&self, now: Timestamp) -> BTreeSet<ArtefactClass> {
        self.keys
            .iter()
            .filter(|key| now <= key.not_after && !self.revoked.contains(&key.key_version))
            .map(|key| key.class)
            .collect()
    }

    /// The key a signature names, if it may verify `class` at `now`, as the
    /// uncompressed point a verifier takes.
    ///
    /// Each refusal names its cause, because "signature does not verify"
    /// would send an operator looking for tampering when the fault is a
    /// bundle shipped without the new key version.
    pub fn key_for(
        &self,
        class: ArtefactClass,
        key_version: &str,
        now: Timestamp,
    ) -> Result<Vec<u8>> {
        if self.revoked.contains(key_version) {
            return Err(Error::denied(format!(
                "key version {key_version} is revoked in this trust bundle; an artefact signed \
                 under it is refused whatever its signature says"
            )));
        }
        let key = self
            .keys
            .iter()
            .find(|key| key.key_version == key_version)
            .ok_or_else(|| {
                Error::denied(format!(
                    "key version {key_version} is not in this trust bundle; a rotation must ship \
                     the new version in the image before the publisher signs with it"
                ))
            })?;
        if key.class != class {
            return Err(Error::denied(format!(
                "key version {key_version} is published for {} and this is a {class} artefact; \
                 each class has its own key so a signature cannot cross between them",
                key.class
            )));
        }
        if now > key.not_after {
            return Err(Error::denied(format!(
                "key version {key_version} lapsed at {} and verifies nothing after it",
                key.not_after.to_rfc3339()
            )));
        }
        public_point(&key.public_key)
    }
}

/// Reduce a bundle's hex public key to the 65-byte uncompressed point,
/// refusing any other curve or encoding.
pub fn public_point(hex: &str) -> Result<Vec<u8>> {
    let bytes =
        from_hex(hex.trim()).ok_or_else(|| Error::invalid("the public key is not hexadecimal"))?;
    let point = if bytes.len() == P256_SPKI_PREFIX.len() + P256_POINT_LENGTH {
        let (prefix, point) = bytes.split_at(P256_SPKI_PREFIX.len());
        if prefix != P256_SPKI_PREFIX {
            return Err(Error::invalid(
                "the public key is a SubjectPublicKeyInfo for something other than an EC P-256 \
                 key; ADR 0110 publishes EC_SIGN_P256_SHA256 keys only",
            ));
        }
        point.to_vec()
    } else {
        bytes
    };
    if point.len() != P256_POINT_LENGTH || point.first() != Some(&0x04) {
        return Err(Error::invalid(format!(
            "the public key is {} byte(s) and not an uncompressed P-256 point (65 bytes \
             beginning 0x04) or its 91-byte SubjectPublicKeyInfo",
            point.len()
        )));
    }
    Ok(point)
}

/// Produces signatures under one key of one class.
///
/// A Cloud KMS `asymmetricSign` call in a deployment, made from a
/// composition root; a key held in memory in development and tests. The two
/// are behind this one seam so nothing that signs knows which it has.
pub trait ArtefactSigner: Send + Sync + std::fmt::Debug {
    /// The one class this signer's key is published for.
    fn class(&self) -> ArtefactClass;

    /// The full key-version resource name the signature is attributed to.
    fn key_version(&self) -> &str;

    /// A DER ECDSA P-256 / SHA-256 signature over `message`, which is a
    /// [`signing_message`] and nothing else.
    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>>;
}

/// Checks a signature against trusted public keys.
///
/// A pure function of its arguments in every implementation: no network
/// call on the verify path (ADR 0110 decision 2, and ADR 0008 — a cell that
/// cannot reach Google must still be able to accept a halt).
pub trait ArtefactVerifier: Send + Sync + std::fmt::Debug {
    /// Refuse unless `signature` is a valid signature of class `class` over
    /// `canonical` under a key the verifier trusts at `now`.
    fn verify(
        &self,
        class: ArtefactClass,
        canonical: &[u8],
        signature: &str,
        now: Timestamp,
    ) -> Result<()>;
}

/// Sign an artefact's canonical bytes and return the text form its
/// `signature` field carries.
///
/// Refuses a signer of another class: the caller asked for one class and a
/// signer holding the wrong key would produce a signature every verifier
/// refuses, which is better found here, before anything is sent.
pub fn sign_artefact(
    signer: &dyn ArtefactSigner,
    class: ArtefactClass,
    canonical: &[u8],
) -> Result<String> {
    if signer.class() != class {
        return Err(Error::denied(format!(
            "a {class} artefact was handed to the {} signer; each class is signed under its \
             own key",
            signer.class()
        )));
    }
    let message = signing_message(class, signer.key_version(), canonical);
    let der = signer.sign_message(&message)?;
    Ok(ArtefactSignature::new(signer.key_version(), der)?.encode())
}

/// The two people who published a widening artefact (ADR 0110 decision 4).
///
/// The author of the reviewed commit and the protected environment's
/// approver. Neither credential is minted by this platform — ADR 0076's
/// condition for a two-person rule to mean anything — so this record names
/// them and the artefact's signature covers the names; what makes them two
/// people is the publish workflow's required reviewers with self-review
/// prevented, and what this type adds is that a record naming one person
/// twice is refused wherever it is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub author: String,
    pub approver: String,
    pub approved_at: Timestamp,
}

impl Publication {
    /// Refuse a missing name, or the same person in both roles.
    ///
    /// Names are compared trimmed and case-folded: `Alice` approving `alice`'s
    /// commit is one person, and a check that compared bytes would admit it.
    pub fn validate(&self) -> Result<()> {
        let author = self.author.trim();
        let approver = self.approver.trim();
        if author.is_empty() || approver.is_empty() {
            return Err(Error::denied(
                "a widening artefact must name both its author and its approver; a publication \
                 missing either is a one-person publish",
            ));
        }
        if author.to_lowercase() == approver.to_lowercase() {
            return Err(Error::denied(format!(
                "{author} is named as both author and approver; a widening artefact needs two \
                 distinct people, and a halt is the only artefact one person may publish"
            )));
        }
        Ok(())
    }

    /// The bytes this record contributes to its artefact's canonical form.
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&length_prefixed_bytes(self.author.as_bytes()));
        out.extend_from_slice(&length_prefixed_bytes(self.approver.as_bytes()));
        out.extend_from_slice(&self.approved_at.as_nanos().to_be_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINT: &str = "04\
        6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296\
        4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";

    fn key(class: ArtefactClass, version: &str) -> TrustedKey {
        TrustedKey {
            class,
            key_version: version.to_string(),
            public_key: POINT.to_string(),
            not_after: Timestamp::from_secs(2_000_000_000),
        }
    }

    /// Mutation: drop the length prefixes from `signing_message` (concatenate
    /// the parts) and the two messages below collide; this fails.
    #[test]
    fn the_signed_message_separates_class_key_and_bytes_injectively() {
        let one = signing_message(ArtefactClass::Policy, "k/1", b"ab");
        let two = signing_message(ArtefactClass::Policy, "k/1a", b"b");
        assert_ne!(one, two, "a key-version/bytes split collided");
        let policy = signing_message(ArtefactClass::Policy, "k/1", b"x");
        let halt = signing_message(ArtefactClass::Halt, "k/1", b"x");
        assert_ne!(policy, halt, "the class is not inside the signed message");
    }

    /// Mutation: let `parse` fall back to accepting a bare hex string and a
    /// shared-key MAC would parse as a signature; this fails.
    #[test]
    fn a_shared_key_mac_never_parses_as_an_asymmetric_signature() {
        let mac = "ab".repeat(32);
        let error = ArtefactSignature::parse(&mac).expect_err("an HMAC parsed as ECDSA");
        assert!(error.message().contains(SIGNATURE_SCHEME));
        let good = ArtefactSignature::new("projects/p/k/1", vec![0x30; 70]).map(|s| s.encode());
        let good = good.expect("a well-formed signature was refused");
        assert_eq!(
            ArtefactSignature::parse(&good)
                .expect("round trip")
                .key_version(),
            "projects/p/k/1"
        );
    }

    /// Mutation: skip the class comparison in `key_for` and a halt key would
    /// verify a policy frame; this fails.
    #[test]
    fn a_key_published_for_one_class_refuses_every_other_class() {
        let bundle = TrustBundle::new(vec![key(ArtefactClass::Halt, "halt/1")], BTreeSet::new())
            .expect("bundle");
        let now = Timestamp::from_secs(1_800_000_000);
        // Premise: the key serves its own class.
        assert!(bundle.key_for(ArtefactClass::Halt, "halt/1", now).is_ok());
        for class in [
            ArtefactClass::Policy,
            ArtefactClass::Envelope,
            ArtefactClass::ModelPack,
        ] {
            assert!(
                bundle.key_for(class, "halt/1", now).is_err(),
                "the halt key verified a {class} artefact"
            );
        }
    }

    /// Mutation: ignore `revoked` or `not_after` in `key_for`; this fails.
    #[test]
    fn a_revoked_or_lapsed_key_verifies_nothing() {
        let mut revoked = BTreeSet::new();
        revoked.insert("policy/1".to_string());
        let bundle = TrustBundle::new(
            vec![
                key(ArtefactClass::Policy, "policy/1"),
                key(ArtefactClass::Policy, "policy/2"),
            ],
            revoked,
        )
        .expect("bundle");
        let now = Timestamp::from_secs(1_800_000_000);
        assert!(
            bundle
                .key_for(ArtefactClass::Policy, "policy/2", now)
                .is_ok()
        );
        assert!(
            bundle
                .key_for(ArtefactClass::Policy, "policy/1", now)
                .is_err()
        );
        let after = Timestamp::from_secs(2_000_000_001);
        assert!(
            bundle
                .key_for(ArtefactClass::Policy, "policy/2", after)
                .is_err()
        );
    }

    /// Mutation: drop the duplicate check or the point check from
    /// `TrustBundle::new`; this fails.
    #[test]
    fn a_bundle_with_a_duplicate_or_malformed_entry_is_refused_whole() {
        let duplicate = TrustBundle::new(
            vec![
                key(ArtefactClass::Policy, "v/1"),
                key(ArtefactClass::Halt, "v/1"),
            ],
            BTreeSet::new(),
        );
        assert!(duplicate.is_err());
        let mut bad = key(ArtefactClass::Policy, "v/2");
        bad.public_key = "05".repeat(65);
        assert!(TrustBundle::new(vec![bad], BTreeSet::new()).is_err());
        let spki = format!("{}{}", to_hex(&P256_SPKI_PREFIX), POINT);
        let mut spki_key = key(ArtefactClass::Policy, "v/3");
        spki_key.public_key = spki;
        assert!(TrustBundle::new(vec![spki_key], BTreeSet::new()).is_ok());
    }

    /// Mutation: compare names byte for byte, or accept an empty one, and a
    /// one-person publish passes; this fails.
    #[test]
    fn a_publication_naming_one_person_twice_is_refused() {
        let at = Timestamp::from_secs(1_800_000_000);
        let two = Publication {
            author: "alice".into(),
            approver: "bob".into(),
            approved_at: at,
        };
        assert!(two.validate().is_ok());
        for (author, approver) in [("alice", "Alice "), ("alice", ""), ("", "bob")] {
            let one = Publication {
                author: author.into(),
                approver: approver.into(),
                approved_at: at,
            };
            assert!(
                one.validate().is_err(),
                "{author:?}/{approver:?} passed as two people"
            );
        }
        assert!(ArtefactClass::ModelPack.widens());
        assert!(!ArtefactClass::Halt.widens());
    }
}
