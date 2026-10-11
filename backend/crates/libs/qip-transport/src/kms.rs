//! Signing through Cloud KMS `asymmetricSign` over REST (ADR 0110 decision
//! 2): the production [`ArtefactSigner`].
//!
//! The private key is an HSM-protected key version in Cloud KMS and never
//! enters this process. What this module does is the call: hash the
//! [`qip_contracts::trust::signing_message`] with SHA-256, POST the digest to
//! `…/cryptoKeyVersions/N:asymmetricSign` over TLS ([`TlsHttpClient`], with a
//! server-authenticated config pinned to the roots the composition root
//! loaded), with a bearer token from the metadata server, and hand back the
//! DER signature KMS returns.
//!
//! Three checks on the answer, each because its absence is a way to sign
//! under the wrong thing without noticing:
//!
//! * **The response names the key version that was asked for.** A proxy or
//!   a misrouted request answering for another version would otherwise
//!   produce a signature attributed to the wrong key.
//! * **The protection level is `HSM`.** ADR 0110's "what would make this
//!   wrong" names a signing key created at any other level; a software key
//!   in the same key ring would sign identically and leave nothing in the
//!   artefact to say so. The signer refuses to return its signature.
//! * **The signature verifies against the trust bundle before it is
//!   returned.** A signer whose key is missing from the bundle the fleet
//!   ships would produce frames every cell refuses; finding that on the
//!   first signature, in the process that made it, beats finding it as a
//!   fleet-wide refusal.
//!
//! No Google SDK and no auth library: the token is a GET to the metadata
//! server, the call is one POST, and the encoding is base64 — the codec
//! below, checked against RFC 4648's vectors. A codec fails loudly against a
//! vector; it is not cryptography.

use crate::http::{ClientLimits, HttpRequest, Method, TlsHttpClient};
use crate::trust::BundleVerifier;
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, ArtefactVerifier};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The Cloud KMS REST endpoint. A constant, not configuration: the signer
/// talks to Google's API and nothing else, and a configurable base would be
/// the one setting that could point the platform's signing at a server that
/// answers every request with whatever signature it likes.
pub const KMS_ENDPOINT: &str = "https://cloudkms.googleapis.com/v1";

/// Supplies the bearer token for each call. A composition root adapts the
/// metadata-server source in `qip-storage` to this; this crate cannot depend
/// on that one (it depends on this).
pub trait BearerSource: Send + Sync + std::fmt::Debug {
    fn bearer(&self) -> Result<String>;
}

#[derive(Serialize)]
struct SignRequest<'a> {
    digest: Digest<'a>,
}

#[derive(Serialize)]
struct Digest<'a> {
    sha256: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignResponse {
    signature: String,
    name: String,
    #[serde(default)]
    protection_level: Option<String>,
    #[serde(default)]
    verified_digest_crc32c: Option<bool>,
}

/// One KMS key version, of one class, as a signer.
#[derive(Debug)]
pub struct KmsSigner {
    class: ArtefactClass,
    key_version: String,
    client: TlsHttpClient,
    bearer: Arc<dyn BearerSource>,
    check: BundleVerifier,
    /// The REST base, [`KMS_ENDPOINT`] unless a test pointed it elsewhere.
    endpoint: String,
    /// The instant the self-check asks the bundle about. Injected, like
    /// every clock in the platform.
    clock: Arc<dyn qip_core::Clock>,
}

impl KmsSigner {
    /// A signer for `key_version`, which must be a full
    /// `projects/…/cryptoKeyVersions/N` resource name and must appear in
    /// `bundle` for `class` — checked here, so a signer that could never be
    /// verified is refused at start-up.
    pub fn new(
        class: ArtefactClass,
        key_version: impl Into<String>,
        client: TlsHttpClient,
        bearer: Arc<dyn BearerSource>,
        bundle: BundleVerifier,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Result<Self> {
        let key_version = key_version.into();
        if !key_version.starts_with("projects/") || !key_version.contains("/cryptoKeyVersions/") {
            return Err(Error::invalid(format!(
                "{key_version:?} is not a Cloud KMS key-version resource name \
                 (projects/…/locations/…/keyRings/…/cryptoKeys/…/cryptoKeyVersions/N)"
            )));
        }
        bundle
            .bundle()
            .key_for(class, &key_version, clock.now())
            .map_err(|error| {
                Error::invalid(format!(
                    "the {class} signer's key version is not usable under the trust bundle this \
                     process verifies against: {}",
                    error.message()
                ))
            })?;
        Ok(Self {
            class,
            key_version,
            client,
            bearer,
            check: bundle,
            endpoint: KMS_ENDPOINT.to_string(),
            clock,
        })
    }

    /// The same signer against another `https` base — a test's stand-in for
    /// Cloud KMS. No composition root calls this. It cannot make the signer
    /// accept a forged answer: whatever answers, the signature is checked
    /// against the trust bundle before it is returned, so a server that
    /// answers with signatures of its own choosing gets nothing signed.
    pub fn with_endpoint(mut self, endpoint: &str) -> Result<Self> {
        if !endpoint.starts_with("https://") {
            return Err(Error::invalid(format!(
                "a KMS endpoint must be https, and {endpoint:?} is not"
            )));
        }
        self.endpoint = endpoint.trim_end_matches('/').to_string();
        Ok(self)
    }

    /// The bounds every call runs under: a KMS sign is a small JSON document
    /// and should answer in well under a second.
    pub fn limits() -> ClientLimits {
        ClientLimits {
            connect_timeout: std::time::Duration::from_secs(3),
            read_timeout: std::time::Duration::from_secs(10),
            write_timeout: std::time::Duration::from_secs(10),
            max_body: 64 * 1024,
            ..ClientLimits::default()
        }
    }

    fn call(&self, message: &[u8]) -> Result<Vec<u8>> {
        let digest = encode_base64(&qip_core::sha256(message));
        let body = serde_json::to_vec(&SignRequest {
            digest: Digest { sha256: &digest },
        })
        .map_err(|error| Error::invalid(format!("cannot encode the KMS request: {error}")))?;
        let url = format!("{}/{}:asymmetricSign", self.endpoint, self.key_version);
        let request = HttpRequest::json_tls(Method::Post, &url, body)
            .map_err(Error::from)?
            .with_header(
                "authorization",
                &format!("Bearer {}", self.bearer.bearer()?),
            );
        let response = self.client.send(&request).map_err(|error| {
            Error::from(error).relabelled(format!(
                "the {} signer's call to Cloud KMS failed",
                self.class
            ))
        })?;
        if !response.is_success() {
            return Err(Error::unavailable(format!(
                "Cloud KMS answered the {} signer with status {}: {}",
                self.class,
                response.status,
                response.body_excerpt()
            )));
        }
        let answer: SignResponse = serde_json::from_slice(&response.body).map_err(|error| {
            Error::invalid(format!("the KMS sign response does not parse: {error}"))
        })?;
        if answer.name != self.key_version {
            return Err(Error::denied(format!(
                "Cloud KMS answered for key version {} when {} was asked; the signature is \
                 discarded rather than attributed to the wrong key",
                answer.name, self.key_version
            )));
        }
        if answer.protection_level.as_deref() != Some("HSM") {
            return Err(Error::denied(format!(
                "key version {} reports protection level {:?}; ADR 0110 signs only with \
                 HSM-protected keys, and a software key would sign identically and leave no \
                 trace in the artefact",
                self.key_version, answer.protection_level
            )));
        }
        if answer.verified_digest_crc32c == Some(false) {
            return Err(Error::io(
                "Cloud KMS reports the digest arrived corrupted; nothing is signed",
            ));
        }
        decode_base64(&answer.signature)
    }
}

impl ArtefactSigner for KmsSigner {
    fn class(&self) -> ArtefactClass {
        self.class
    }

    fn key_version(&self) -> &str {
        &self.key_version
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        let der = self.call(message)?;
        // The self-check: the signature this process is about to put on an
        // artefact must verify, over the very message KMS was asked to sign,
        // under the key the trust bundle publishes for this version — which
        // is exactly the check every cell will make.
        let signature = qip_contracts::trust::ArtefactSignature::new(&self.key_version, der)?;
        let point = self
            .check
            .bundle()
            .key_for(self.class, &self.key_version, self.clock.now())?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, &point)
            .verify(message, signature.der())
            .map_err(|_| {
                Error::denied(format!(
                    "Cloud KMS returned a signature that does not verify under the trust \
                     bundle's key for {}; the bundle and the key ring disagree, and nothing is \
                     signed until they agree",
                    self.key_version
                ))
            })?;
        Ok(signature.der().to_vec())
    }
}

/// Verify `signature` (text form) over `canonical` with `verifier` — a
/// helper for composition roots that want to prove a freshly built signer
/// end to end at start-up.
pub fn prove_signer(
    signer: &dyn ArtefactSigner,
    verifier: &dyn ArtefactVerifier,
    now: Timestamp,
) -> Result<()> {
    let probe = b"qip start-up signer probe";
    let signature = qip_contracts::trust::sign_artefact(signer, signer.class(), probe)?;
    verifier.verify(signer.class(), probe, &signature, now)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RFC 4648 §4 base64, padded.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for (index, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// RFC 4648 §4 base64, padded, refusing anything else — a character outside
/// the alphabet, a length that is not a multiple of four, padding anywhere
/// but the end, or non-zero bits under the padding. Strict on purpose: two
/// encodings of one signature would be two strings naming one artefact.
pub fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let refuse = || Error::invalid("the value is not canonical padded base64");
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(refuse());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let quads = bytes.len() / 4;
    for (index, quad) in bytes.chunks(4).enumerate() {
        let last = index + 1 == quads;
        let padding = quad.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return Err(refuse());
        }
        let mut n = 0u32;
        for (position, byte) in quad.iter().enumerate() {
            let value = if position >= 4 - padding {
                0
            } else {
                ALPHABET
                    .iter()
                    .position(|candidate| candidate == byte)
                    .ok_or_else(refuse)? as u32
            };
            n = (n << 6) | value;
        }
        let decoded = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = 3 - padding;
        // Bits under the padding must be zero, or two strings decode alike.
        if decoded[keep..].iter().any(|byte| *byte != 0) {
            return Err(refuse());
        }
        out.extend_from_slice(&decoded[..keep]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10's vectors. Mutation: get the padding arm wrong in either
    /// direction and a vector fails.
    #[test]
    fn base64_matches_every_rfc_4648_vector_in_both_directions() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(plain.as_bytes()), encoded);
            assert_eq!(
                decode_base64(encoded).expect("a vector decodes"),
                plain.as_bytes()
            );
        }
    }

    /// Mutation: drop the zero-bits check or the padding-position check and
    /// one of these decodes; this fails.
    #[test]
    fn non_canonical_base64_is_refused() {
        for bad in ["Zh==", "Zg=", "Z===", "Zg==Zg==", "Zm9v!A==", "Zm8"] {
            assert!(decode_base64(bad).is_err(), "{bad:?} decoded");
        }
    }
}
