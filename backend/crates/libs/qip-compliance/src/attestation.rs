//! Attested approval: authority that only an enrolled WebAuthn key can carry
//! (ADR 0120; R12-GOV-6125).
//!
//! `qip_contracts::action::AuthorityLevel` is a plain enum. Anything can write
//! `AuthorityLevel::Capital`, so `check_authority` alone proves that a caller
//! *said* it held capital authority. [`AttestedApproval`] is the value a
//! capital-class decision has to present instead: its fields are private, it
//! has no `Clone`, `Default`, `Deserialize` or public constructor, and the one
//! function that builds it is [`ApprovalVerifier::verify`], which checks a
//! WebAuthn assertion (ADR 0120 decision 2, items 3 to 5) made by a key the
//! platform holds only the public half of.
//!
//! What the verifier binds, and the failure each binding prevents:
//!
//! * **The intent.** The challenge the authenticator signed is a digest over
//!   the intent's opportunity, instrument and action class, so an assertion
//!   obtained for a research intent cannot be presented for a capital one.
//! * **The ceiling.** Each enrolled key carries the highest authority its owner
//!   may attest. A key enrolled at `Operational` verifies perfectly and still
//!   cannot admit a capital intent, because [`check_attested`] hands the
//!   enrolled level, not a requested one, to `check_authority`.
//! * **Recency and single use.** A challenge older than [`ATTESTATION_MAX_AGE`]
//!   is refused, and a nonce the verifier has already accepted is refused, so a
//!   captured assertion is neither replayable later nor a second time.
//!
//! **What this does not do.** The MAC-protected challenge endpoint
//! (`POST /operator-challenges`, ADR 0120 decision 2 items 1 and 2) and the
//! event-log record of the nonce (item 6) belong to `qip-api`; this module
//! takes the challenge as an argument and keeps the used-nonce set in memory,
//! which does not survive a restart. Nothing here is wired to an order path.

use crate::approval::OperatorCredential;
use qip_contracts::action::{ActionClass, ActionIntent, AuthorityLevel, check_authority};
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, sha256};
use qip_core::{Duration, Timestamp};
use qip_tls::ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use std::collections::{BTreeMap, BTreeSet};

/// How old a challenge may be when its assertion is verified (ADR 0120).
pub const ATTESTATION_MAX_AGE: Duration = Duration::from_mins(2);

/// The method recorded on a credential built from an attested approval.
pub const ATTESTED_METHOD: &str = "webauthn";

const CHALLENGE_DOMAIN: &[u8] = b"qip-attested-approval/v1";
const USER_PRESENT: u8 = 0x01;
const USER_VERIFIED: u8 = 0x04;
const P256_POINT_LENGTH: usize = 65;

/// Who the platform recognises, and the highest authority each may attest.
///
/// Built from committed enrolment, never from a request: a key presented
/// alongside an assertion is not an enrolled key.
#[derive(Clone, Debug, Default)]
pub struct OperatorKeyDirectory {
    keys: BTreeMap<String, (Vec<u8>, AuthorityLevel)>,
}

impl OperatorKeyDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enrol one person. The subject follows ADR 0076 decision 1
    /// (`[a-z0-9][a-z0-9._-]{1,62}`, nothing ending `@env`); the key is the
    /// hex of the 65-byte uncompressed P-256 point; a duplicate is refused.
    pub fn enrol(
        &mut self,
        subject: &str,
        public_key_hex: &str,
        ceiling: AuthorityLevel,
    ) -> Result<()> {
        let valid_subject = (2..=63).contains(&subject.len())
            && subject.bytes().enumerate().all(|(i, b)| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
            });
        if !valid_subject {
            return Err(Error::invalid(format!(
                "`{subject}` is not an operator subject; use [a-z0-9][a-z0-9._-]{{1,62}} (ADR 0076)"
            )));
        }
        let point = from_hex(public_key_hex)
            .filter(|bytes| bytes.len() == P256_POINT_LENGTH && bytes[0] == 0x04)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "the key for `{subject}` is not a hex 65-byte uncompressed P-256 point"
                ))
            })?;
        if self.keys.contains_key(subject) {
            return Err(Error::invalid(format!("`{subject}` is already enrolled")));
        }
        self.keys.insert(subject.to_string(), (point, ceiling));
        Ok(())
    }
}

/// The one-time challenge an assertion answers. Issued by the API (ADR 0120
/// decision 2); supplied here by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub nonce: [u8; 16],
    pub issued_at: Timestamp,
}

/// What the browser's authenticator returned, un-interpreted.
#[derive(Clone, Debug)]
pub struct Assertion {
    pub subject: String,
    pub authenticator_data: Vec<u8>,
    pub client_data_json: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Authority that passed verification. Built only by
/// [`ApprovalVerifier::verify`]; consumed by reference, never duplicated.
#[derive(Debug, PartialEq, Eq)]
pub struct AttestedApproval {
    subject: String,
    authority: AuthorityLevel,
    authenticated_at: Timestamp,
    opportunity: String,
    instrument: String,
    action_class: ActionClass,
}

impl AttestedApproval {
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn authority(&self) -> AuthorityLevel {
        self.authority
    }
    pub fn authenticated_at(&self) -> Timestamp {
        self.authenticated_at
    }

    /// The credential the capital chain wants, dated at the challenge's
    /// server-side issue instant, with method `webauthn`.
    pub fn credential(&self) -> Result<OperatorCredential> {
        OperatorCredential::verified(&self.subject, ATTESTED_METHOD, self.authenticated_at)
    }
}

/// Verifies assertions against an enrolled directory.
#[derive(Debug)]
pub struct ApprovalVerifier {
    directory: OperatorKeyDirectory,
    rp_id_hash: [u8; 32],
    origin: String,
    used_nonces: BTreeSet<[u8; 16]>,
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn push_prefixed(buffer: &mut Vec<u8>, part: &[u8]) {
    buffer.extend_from_slice(&(part.len() as u64).to_be_bytes());
    buffer.extend_from_slice(part);
}

/// The 32 bytes the authenticator must sign as its challenge for `intent`.
pub fn challenge_digest(intent: &ActionIntent, challenge: &Challenge) -> [u8; 32] {
    let mut message = CHALLENGE_DOMAIN.to_vec();
    push_prefixed(&mut message, intent.opportunity().as_bytes());
    push_prefixed(&mut message, intent.instrument().as_bytes());
    push_prefixed(&mut message, intent.action_class().as_str().as_bytes());
    push_prefixed(&mut message, &challenge.nonce);
    push_prefixed(&mut message, &challenge.issued_at.as_secs().to_be_bytes());
    sha256(&message)
}

impl ApprovalVerifier {
    pub fn new(directory: OperatorKeyDirectory, rp_id: &str, origin: &str) -> Result<Self> {
        if rp_id.is_empty() || origin.is_empty() {
            return Err(Error::invalid(
                "the relying-party id and origin must be configured, not defaulted",
            ));
        }
        Ok(Self {
            directory,
            rp_id_hash: sha256(rp_id.as_bytes()),
            origin: origin.to_string(),
            used_nonces: BTreeSet::new(),
        })
    }

    /// Verify `assertion` for `intent`, refusing naming the first check that
    /// fails. The nonce is spent only on success, so a malformed attempt does
    /// not burn a legitimate challenge.
    pub fn verify(
        &mut self,
        intent: &ActionIntent,
        challenge: &Challenge,
        assertion: &Assertion,
        now: Timestamp,
    ) -> Result<AttestedApproval> {
        if now < challenge.issued_at || now.since(challenge.issued_at) > ATTESTATION_MAX_AGE {
            return Err(Error::denied(
                "the challenge is older than 120 seconds, or dated in the future; request a new one",
            ));
        }
        if self.used_nonces.contains(&challenge.nonce) {
            return Err(Error::denied(
                "this challenge was already used; an assertion is single-use",
            ));
        }
        let (key, ceiling) = self.directory.keys.get(&assertion.subject).ok_or_else(|| {
            Error::denied(format!(
                "`{}` is not an enrolled operator",
                assertion.subject
            ))
        })?;

        let client: serde_json::Value = serde_json::from_slice(&assertion.client_data_json)
            .map_err(|_| Error::denied("clientDataJSON is not JSON"))?;
        let field = |name: &str| client.get(name).and_then(serde_json::Value::as_str);
        if field("type") != Some("webauthn.get") {
            return Err(Error::denied("clientDataJSON type is not webauthn.get"));
        }
        let expected = base64url(&challenge_digest(intent, challenge));
        if field("challenge") != Some(expected.as_str()) {
            return Err(Error::denied(
                "the assertion answers a different challenge, so it was not made for this intent",
            ));
        }
        if field("origin") != Some(self.origin.as_str()) {
            return Err(Error::denied(
                "clientDataJSON origin is not the configured origin",
            ));
        }

        let data = &assertion.authenticator_data;
        if data.len() < 37 || data[..32] != self.rp_id_hash {
            return Err(Error::denied(
                "authenticatorData is not for the configured relying party",
            ));
        }
        if data[32] & USER_PRESENT == 0 || data[32] & USER_VERIFIED == 0 {
            return Err(Error::denied(
                "the authenticator did not assert user presence and verification",
            ));
        }

        let mut signed = data.clone();
        signed.extend_from_slice(&sha256(&assertion.client_data_json));
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, key)
            .verify(&signed, &assertion.signature)
            .map_err(|_| {
                Error::denied(format!(
                    "the signature does not verify under the enrolled key for `{}`",
                    assertion.subject
                ))
            })?;

        let authority = *ceiling;
        self.used_nonces.insert(challenge.nonce);
        Ok(AttestedApproval {
            subject: assertion.subject.clone(),
            authority,
            authenticated_at: challenge.issued_at,
            opportunity: intent.opportunity().to_string(),
            instrument: intent.instrument().to_string(),
            action_class: intent.action_class(),
        })
    }
}

/// Admit `intent` only if `approval` was verified for exactly this intent and
/// its enrolled authority permits the intent's class. This is the production
/// caller of `check_authority` for capital-class intents.
pub fn check_attested(intent: &ActionIntent, approval: &AttestedApproval) -> Result<()> {
    if approval.opportunity != intent.opportunity()
        || approval.instrument != intent.instrument()
        || approval.action_class != intent.action_class()
    {
        return Err(Error::denied(
            "the attested approval was verified for a different intent than the one presented",
        ));
    }
    check_authority(intent, approval.authority)
}
