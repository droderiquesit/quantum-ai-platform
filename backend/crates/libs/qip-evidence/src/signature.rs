//! The issuer registry and the detached-signature verifier (ADR 0139,
//! ADR 0508; R12-EVID-2034).
//!
//! A source's signature over a document is credited only when a key this
//! repository committed — not one the source served — verifies it. The
//! registry is that commitment: each entry names an issuer, the domains it
//! publishes from, the catalogue sources it covers and its public keys, and
//! it is read from a committed file by a composition root and handed down.
//! No key is fetched at run time, because a key fetched from the source is
//! the source vouching for itself.
//!
//! [`verify_detached`] is the only constructor of a [`VerifiedSignature`]
//! that the evidence fabric credits on admission. The token's fields are
//! private, so a claim cannot carry a signer's name and be believed: until
//! ADR 0139 `AuthenticitySignal::SignatureVerified` held a bare `signer`
//! string, and crediting it would have been believing the claim's own word.
//!
//! **Why here and not in `qip_transport::trust`** (ADR 0508 amends ADR 0139
//! decision 2 on this point only). Rust's privacy is per crate: a token
//! constructed only by a verifier must live in the verifier's crate, and the
//! fabric that reads it must be able to name it. `qip-transport` cannot be
//! linked from here without bringing the event mesh into an I/O-free
//! library, so the verifier is here, on the same `ring` pin
//! (`qip_tls::ring`, ADR 0110) that `qip_transport::trust` uses. It is still
//! *called* by the connector, never by the fabric: [`crate::fabric`] reads a
//! token and never checks a signature.
//!
//! **No I/O.** The registry parses from text the caller read; verification is
//! a pure function of the registry, the bytes, the signature and the instant.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_tls::ring::signature::{ECDSA_P256_SHA256_ASN1, ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The signature algorithms an issuer's key may name. Both are already in
/// `ring` (ADR 0139 decision 2); anything else is refused at load as
/// unsupported rather than skipped, so a registered issuer is never silently
/// unverifiable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    /// ECDSA over P-256 with SHA-256, ASN.1 DER signatures; the key is the
    /// 65-byte uncompressed SEC1 point.
    #[serde(rename = "ecdsa-p256-sha256")]
    EcdsaP256Sha256,
    /// Ed25519 (RFC 8032); the key is 32 bytes.
    #[serde(rename = "ed25519")]
    Ed25519,
}

impl SignatureAlgorithm {
    fn name(self) -> &'static str {
        match self {
            Self::EcdsaP256Sha256 => "ecdsa-p256-sha256",
            Self::Ed25519 => "ed25519",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "ecdsa-p256-sha256" => Ok(Self::EcdsaP256Sha256),
            "ed25519" => Ok(Self::Ed25519),
            other => Err(Error::invalid(format!(
                "signature algorithm `{other}` is not supported; the verifier admits \
                 ecdsa-p256-sha256 and ed25519 (ADR 0139), and admitting another is a dependency \
                 question for an ADR, not a registry edit"
            ))),
        }
    }

    const fn key_length(self) -> usize {
        match self {
            Self::EcdsaP256Sha256 => 65,
            Self::Ed25519 => 32,
        }
    }
}

/// One public key of a registered issuer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuerKey {
    pub key_id: String,
    pub algorithm: SignatureAlgorithm,
    public_key: Vec<u8>,
    /// The last instant a signature under this key is accepted. A key past it
    /// verifies nothing: a rotated key at a source drops its evidence's score
    /// until a reviewed commit adds the new one, which is the fail-closed
    /// direction (ADR 0139, "What it costs").
    pub not_after: Timestamp,
}

/// One issuer: who signs, from where, for which catalogue sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issuer {
    pub issuer: String,
    /// Domains the issuer publishes from, lowercase.
    pub domains: BTreeSet<String>,
    /// Catalogue source ids whose documents this issuer signs.
    pub sources: BTreeSet<String>,
    pub keys: Vec<IssuerKey>,
}

/// The committed issuer registry. Empty is a valid registry: every source
/// then scores exactly as it did before ADR 0139 (decision 5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuerRegistry {
    issuers: Vec<Issuer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistry {
    schema_version: u32,
    issuers: Vec<RawIssuer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIssuer {
    issuer: String,
    domains: Vec<String>,
    sources: Vec<String>,
    keys: Vec<RawKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    key_id: String,
    algorithm: String,
    public_key_hex: String,
    not_after: Timestamp,
}

/// The registry file's schema version this loader reads.
pub const ISSUER_REGISTRY_SCHEMA_VERSION: u32 = 1;

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() % 2 != 0 {
        return None;
    }
    (0..raw.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(raw.get(i..i + 2)?, 16).ok())
        .collect()
}

impl IssuerRegistry {
    /// The empty registry: nothing registered, nothing credited.
    pub const fn empty() -> Self {
        Self {
            issuers: Vec::new(),
        }
    }

    /// Parse and admit a committed registry as of `now`.
    ///
    /// Refuses the whole file rather than loading the part that parses: a
    /// registry with one bad key silently dropped would score that issuer's
    /// evidence as unsigned, and nobody would learn why. Refused: an unknown
    /// schema version or field, an unsupported algorithm, a key whose bytes
    /// are not the algorithm's length, a key already expired at `now` (it
    /// would verify nothing, so committing it is a mistake worth stopping
    /// on), a key id used twice anywhere in the registry (a signature names
    /// its key by id, so a duplicate makes "which key" ambiguous), an issuer
    /// with no key, domain or source, and an issuer named twice.
    pub fn from_json(raw: &str, now: Timestamp) -> Result<Self> {
        let parsed: RawRegistry = serde_json::from_str(raw).map_err(|error| {
            Error::invalid(format!("the issuer registry does not parse: {error}"))
        })?;
        if parsed.schema_version != ISSUER_REGISTRY_SCHEMA_VERSION {
            return Err(Error::invalid(format!(
                "the issuer registry is schema version {}; this loader reads version \
                 {ISSUER_REGISTRY_SCHEMA_VERSION}",
                parsed.schema_version
            )));
        }
        let mut key_ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut issuers = Vec::with_capacity(parsed.issuers.len());
        for raw_issuer in parsed.issuers {
            let name = raw_issuer.issuer.trim().to_string();
            if name.is_empty() {
                return Err(Error::invalid("an issuer in the registry has no name"));
            }
            if !names.insert(name.clone()) {
                return Err(Error::invalid(format!(
                    "issuer `{name}` is registered twice; merge its keys into one entry"
                )));
            }
            if raw_issuer.keys.is_empty()
                || raw_issuer.domains.is_empty()
                || raw_issuer.sources.is_empty()
            {
                return Err(Error::invalid(format!(
                    "issuer `{name}` must name at least one key, one domain and one source; an \
                     entry that covers nothing verifies nothing and only looks like trust"
                )));
            }
            let mut keys = Vec::with_capacity(raw_issuer.keys.len());
            for raw_key in raw_issuer.keys {
                let algorithm = SignatureAlgorithm::parse(&raw_key.algorithm)?;
                let key_id = raw_key.key_id.trim().to_string();
                if key_id.is_empty() {
                    return Err(Error::invalid(format!(
                        "issuer `{name}` has a key with no id"
                    )));
                }
                if !key_ids.insert(key_id.clone()) {
                    return Err(Error::invalid(format!(
                        "key id `{key_id}` appears twice in the issuer registry; a signature \
                         names its key by id, so a duplicate makes the verifying key ambiguous"
                    )));
                }
                let public_key = decode_hex(raw_key.public_key_hex.trim())
                    .filter(|bytes| bytes.len() == algorithm.key_length())
                    .ok_or_else(|| {
                        Error::invalid(format!(
                            "key `{key_id}` of issuer `{name}` is not a {}-byte hex {} public key",
                            algorithm.key_length(),
                            algorithm.name()
                        ))
                    })?;
                if raw_key.not_after <= now {
                    return Err(Error::invalid(format!(
                        "key `{key_id}` of issuer `{name}` expired at {}; remove it or replace it \
                         with the issuer's current key, because a committed expired key \
                         verifies nothing",
                        raw_key.not_after.to_rfc3339()
                    )));
                }
                keys.push(IssuerKey {
                    key_id,
                    algorithm,
                    public_key,
                    not_after: raw_key.not_after,
                });
            }
            issuers.push(Issuer {
                issuer: name,
                domains: raw_issuer
                    .domains
                    .iter()
                    .map(|d| d.trim().to_ascii_lowercase())
                    .collect(),
                sources: raw_issuer
                    .sources
                    .iter()
                    .map(|s| s.trim().to_string())
                    .collect(),
                keys,
            });
        }
        Ok(Self { issuers })
    }

    pub fn issuers(&self) -> &[Issuer] {
        &self.issuers
    }

    pub fn is_empty(&self) -> bool {
        self.issuers.is_empty()
    }

    /// The issuer that lists `domain` among the domains it publishes from,
    /// if any (ADR 0139 decision 4).
    pub fn issuer_for_domain(&self, domain: &str) -> Option<&Issuer> {
        let domain = domain.to_ascii_lowercase();
        self.issuers.iter().find(|i| i.domains.contains(&domain))
    }

    fn key(&self, key_id: &str) -> Option<(&Issuer, &IssuerKey)> {
        self.issuers.iter().find_map(|issuer| {
            issuer
                .keys
                .iter()
                .find(|k| k.key_id == key_id)
                .map(|k| (issuer, k))
        })
    }
}

/// How a token came to exist. Never serialised: on decode it is always
/// [`TokenOrigin::Decoded`], so bytes in a file cannot claim a verification
/// happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TokenOrigin {
    /// Constructed by [`verify_detached`] in this process.
    Verified,
    /// Re-read from the platform's own hash-chained evidence journal by
    /// `EvidenceFabric::rebuild`, which replays what a verified token was
    /// credited when it was admitted (ADR 0508 decision 2).
    Journaled,
    /// Deserialised from anywhere else. Recorded, never credited.
    #[default]
    Decoded,
}

/// Proof that [`verify_detached`] checked a signature: the issuer, the key,
/// and the SHA-256 of the exact bytes that verified. Fields are private and
/// the only constructor that the fabric credits is the verifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedSignature {
    issuer: String,
    key_id: String,
    algorithm: SignatureAlgorithm,
    /// Lowercase hex SHA-256 of the signed document.
    digest: String,
    /// The catalogue source the signature was verified for.
    source: String,
    #[serde(skip)]
    origin: TokenOrigin,
}

impl VerifiedSignature {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub const fn algorithm(&self) -> SignatureAlgorithm {
        self.algorithm
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this token was produced by a verification (in this process,
    /// or replayed from the journal) rather than decoded from bytes.
    pub(crate) const fn origin(&self) -> TokenOrigin {
        self.origin
    }

    /// Mark a token re-read from the evidence journal. Crate-private: only
    /// `EvidenceFabric::rebuild` calls it.
    pub(crate) fn journaled(mut self) -> Self {
        if self.origin == TokenOrigin::Decoded {
            self.origin = TokenOrigin::Journaled;
        }
        self
    }
}

/// Verify `signature` over `document`, as served by catalogue source
/// `source`, under registry key `key_id`, at `now`.
///
/// Refuses: an unknown key id, a key whose issuer does not cover `source`
/// (a valid signature by an issuer over a document it does not publish is
/// not that source's authenticity), a key past its `not_after`, and a
/// signature that does not verify. Each refusal names what was checked
/// against what, never the key bytes.
pub fn verify_detached(
    registry: &IssuerRegistry,
    source: &str,
    key_id: &str,
    document: &[u8],
    signature: &[u8],
    now: Timestamp,
) -> Result<VerifiedSignature> {
    let (issuer, key) = registry.key(key_id).ok_or_else(|| {
        Error::denied(format!(
            "key id `{key_id}` is not in the committed issuer registry; a key the source \
             names for itself is the source vouching for itself"
        ))
    })?;
    if !issuer.sources.contains(source) {
        return Err(Error::denied(format!(
            "issuer `{}` is registered for {:?}, not for source `{source}`",
            issuer.issuer, issuer.sources
        )));
    }
    if now >= key.not_after {
        return Err(Error::denied(format!(
            "key `{key_id}` of issuer `{}` expired at {}",
            issuer.issuer,
            key.not_after.to_rfc3339()
        )));
    }
    let verified = match key.algorithm {
        SignatureAlgorithm::EcdsaP256Sha256 => {
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &key.public_key)
                .verify(document, signature)
        }
        SignatureAlgorithm::Ed25519 => {
            UnparsedPublicKey::new(&ED25519, &key.public_key).verify(document, signature)
        }
    };
    verified.map_err(|_| {
        Error::denied(format!(
            "the {} signature does not verify under key `{key_id}` of issuer `{}`; the document \
             was altered after signing, or was not signed under that key",
            key.algorithm.name(),
            issuer.issuer
        ))
    })?;
    Ok(VerifiedSignature {
        issuer: issuer.issuer.clone(),
        key_id: key.key_id.clone(),
        algorithm: key.algorithm,
        digest: qip_core::sha256_hex(document),
        source: source.to_string(),
        origin: TokenOrigin::Verified,
    })
}
