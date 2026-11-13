//! The committed issuer registry and the detached-signature verifier
//! (ADR 0139, ADR 0508; R12-EVID-2034).
//!
//! The registry refuses a file it cannot fully trust; the verifier refuses
//! every signature it cannot tie to a committed key for the source that
//! served the document; and the fabric credits only a token a verification
//! made, never one decoded from bytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)] // integration tests may unwrap: a panic is the failure report; the float comparisons are against the exact constants the fabric adds, and replay must reproduce them bit for bit

use qip_core::Timestamp;
use qip_evidence::fabric::{INDEPENDENTLY_VOUCHED_CREDIT, JournaledClaim, UNSIGNED_AUTHENTICITY};
use qip_evidence::{
    AuthenticitySignal, EvidenceFabric, EvidenceRecord, IssuerRegistry, VerifiedSignature,
    verify_detached,
};
use qip_tls::ring::rand::SystemRandom;
use qip_tls::ring::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair,
};

const DOCUMENT: &[u8] = br#"{"series":"policy_rate","value":"4.25"}"#;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000 + secs)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct Signers {
    ecdsa: EcdsaKeyPair,
    ed25519: Ed25519KeyPair,
    rng: SystemRandom,
}

impl Signers {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let ecdsa_doc =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let ecdsa =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, ecdsa_doc.as_ref(), &rng)
                .unwrap();
        let ed_doc = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let ed25519 = Ed25519KeyPair::from_pkcs8(ed_doc.as_ref()).unwrap();
        Self {
            ecdsa,
            ed25519,
            rng,
        }
    }

    fn ecdsa_sign(&self, message: &[u8]) -> Vec<u8> {
        self.ecdsa
            .sign(&self.rng, message)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn ed25519_sign(&self, message: &[u8]) -> Vec<u8> {
        self.ed25519.sign(message).as_ref().to_vec()
    }

    /// A registry naming both keys for issuer `central-bank`, covering
    /// source `ecb`, valid until `not_after`.
    fn registry_json(&self, not_after: Timestamp) -> String {
        serde_json::json!({
            "schema_version": 1,
            "issuers": [{
                "issuer": "central-bank",
                "domains": ["data.ecb.example"],
                "sources": ["ecb"],
                "keys": [
                    {
                        "key_id": "cb-p256-1",
                        "algorithm": "ecdsa-p256-sha256",
                        "public_key_hex": hex(self.ecdsa.public_key().as_ref()),
                        "not_after": not_after,
                    },
                    {
                        "key_id": "cb-ed-1",
                        "algorithm": "ed25519",
                        "public_key_hex": hex(self.ed25519.public_key().as_ref()),
                        "not_after": not_after,
                    }
                ]
            }]
        })
        .to_string()
    }

    fn registry(&self) -> IssuerRegistry {
        IssuerRegistry::from_json(&self.registry_json(at(1_000)), at(0)).unwrap()
    }
}

#[test]
fn an_expired_key_an_unknown_algorithm_and_a_duplicate_key_id_each_refuse_the_whole_registry() {
    let signers = Signers::new();
    // Premise: the same file with none of the three faults loads, with both
    // keys, so each refusal below is about its fault and not about the file.
    let good = signers.registry();
    assert_eq!(good.issuers().len(), 1);
    assert_eq!(good.issuers()[0].keys.len(), 2);

    let mut value: serde_json::Value =
        serde_json::from_str(&signers.registry_json(at(1_000))).unwrap();

    let mut expired = value.clone();
    expired["issuers"][0]["keys"][1]["not_after"] = serde_json::json!(at(-1));
    let refused = IssuerRegistry::from_json(&expired.to_string(), at(0)).unwrap_err();
    assert!(
        refused.message().contains("expired"),
        "{}",
        refused.message()
    );

    let mut unknown = value.clone();
    unknown["issuers"][0]["keys"][0]["algorithm"] = serde_json::json!("rsa-pss-sha512");
    let refused = IssuerRegistry::from_json(&unknown.to_string(), at(0)).unwrap_err();
    assert!(
        refused
            .message()
            .contains("`rsa-pss-sha512` is not supported"),
        "{}",
        refused.message()
    );

    value["issuers"][0]["keys"][1]["key_id"] = serde_json::json!("cb-p256-1");
    let refused = IssuerRegistry::from_json(&value.to_string(), at(0)).unwrap_err();
    assert!(
        refused.message().contains("appears twice"),
        "{}",
        refused.message()
    );
}

#[test]
fn a_detached_signature_verifies_only_under_a_committed_key_for_the_source_it_covers() {
    let signers = Signers::new();
    let registry = signers.registry();

    // Both admitted algorithms verify a genuine signature, and the token
    // names the bytes that verified.
    let ecdsa = verify_detached(
        &registry,
        "ecb",
        "cb-p256-1",
        DOCUMENT,
        &signers.ecdsa_sign(DOCUMENT),
        at(10),
    )
    .unwrap();
    assert_eq!(ecdsa.issuer(), "central-bank");
    assert_eq!(ecdsa.digest(), qip_core::sha256_hex(DOCUMENT));
    let ed = verify_detached(
        &registry,
        "ecb",
        "cb-ed-1",
        DOCUMENT,
        &signers.ed25519_sign(DOCUMENT),
        at(10),
    )
    .unwrap();
    assert_eq!(ed.key_id(), "cb-ed-1");

    let signature = signers.ed25519_sign(DOCUMENT);
    let mut altered = DOCUMENT.to_vec();
    altered[DOCUMENT.len() - 3] = b'9';
    for (case, source, key, document, now) in [
        (
            "altered document",
            "ecb",
            "cb-ed-1",
            altered.as_slice(),
            at(10),
        ),
        (
            "source the issuer does not cover",
            "fred",
            "cb-ed-1",
            DOCUMENT,
            at(10),
        ),
        (
            "key the registry does not hold",
            "ecb",
            "cb-ed-2",
            DOCUMENT,
            at(10),
        ),
        (
            "key past its not-after",
            "ecb",
            "cb-ed-1",
            DOCUMENT,
            at(1_000),
        ),
        (
            "signature under the other key",
            "ecb",
            "cb-p256-1",
            DOCUMENT,
            at(10),
        ),
    ] {
        assert!(
            verify_detached(&registry, source, key, document, &signature, now).is_err(),
            "a signature was verified for a {case}"
        );
    }
}

fn record(id: &str) -> EvidenceRecord {
    EvidenceRecord::new(
        id.to_string(),
        "ecb".to_string(),
        at(0),
        at(5),
        "connector:test/1.0".to_string(),
        "public".to_string(),
        format!("ecb reports the policy rate ({id})"),
        0.9,
    )
    .unwrap()
}

#[test]
fn a_token_decoded_from_bytes_is_never_credited_and_a_journaled_one_replays_its_credit() {
    let signers = Signers::new();
    let registry = signers.registry();
    let digest = qip_core::sha256_hex(DOCUMENT);
    let token = verify_detached(
        &registry,
        "ecb",
        "cb-ed-1",
        DOCUMENT,
        &signers.ed25519_sign(DOCUMENT),
        at(10),
    )
    .unwrap();
    let signed = record("signed").with_authenticity_signal(AuthenticitySignal::SignatureVerified {
        token: token.clone(),
    });

    let mut fabric = EvidenceFabric::default();
    let (verified, _) = fabric.admit(signed.clone(), Some(&digest), at(10)).unwrap();
    // Premise: the verified token is credited, so the refusals below are
    // about how the token arrived and not about a token nothing credits.
    assert_eq!(
        verified.authenticity.score,
        UNSIGNED_AUTHENTICITY + INDEPENDENTLY_VOUCHED_CREDIT,
        "{:?}",
        verified.authenticity
    );

    // The same token, written out and read back, is a claim that a check
    // happened: forging one needs only the public key id and a digest.
    let decoded: VerifiedSignature =
        serde_json::from_str(&serde_json::to_string(&token).unwrap()).unwrap();
    let mut forged_record = record("decoded");
    forged_record.authenticity_signals =
        vec![AuthenticitySignal::SignatureVerified { token: decoded }];
    let (forged, _) = fabric
        .admit(forged_record.clone(), Some(&digest), at(10))
        .unwrap();
    assert_eq!(forged.authenticity.score, UNSIGNED_AUTHENTICITY);
    assert!(forged.authenticity.credited.is_empty());
    assert_eq!(forged.authenticity.refused.len(), 1);

    // A verified token over another document is spoofing.
    let (elsewhere, _) = EvidenceFabric::default()
        .admit(
            record("elsewhere")
                .with_authenticity_signal(AuthenticitySignal::SignatureVerified { token }),
            Some(&qip_core::sha256_hex(b"another body")),
            at(10),
        )
        .unwrap();
    assert!(elsewhere.authenticity.spoofed);

    // Replay of the journal restores the credit the claim was admitted with:
    // a rebuilt fabric that weighed it lower would hold a different belief.
    let journaled: EvidenceRecord =
        serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
    let rebuilt = EvidenceFabric::default()
        .rebuild([JournaledClaim {
            record: journaled,
            document_hash: Some(digest.clone()),
            admitted_at: at(10),
        }])
        .unwrap();
    let held = rebuilt.get("signed").unwrap();
    assert_eq!(held.verdict.authenticity.score, verified.authenticity.score);
    assert_eq!(held.verdict.belief_weight, verified.belief_weight);
}

#[test]
fn a_tls_domain_the_registry_lists_earns_the_registered_identity_credit_and_a_bare_host_does_not() {
    let signers = Signers::new();
    let tls = |id: &str, source: &str, domain: &str| {
        EvidenceRecord::new(
            id.to_string(),
            source.to_string(),
            at(0),
            at(5),
            "connector:test/1.0".to_string(),
            "public".to_string(),
            format!("statement {id}"),
            0.9,
        )
        .unwrap()
        .with_authenticity_signal(AuthenticitySignal::TlsVerified {
            domain: domain.to_string(),
        })
    };
    let mut fabric = EvidenceFabric::default().trusting_issuers(signers.registry());
    let (listed, _) = fabric
        .admit(
            tls("l", "https://data.ecb.example/r", "data.ecb.example"),
            None,
            at(10),
        )
        .unwrap();
    let (host_only, _) = fabric
        .admit(
            tls("h", "https://ecb-data.example/r", "ecb-data.example"),
            None,
            at(10),
        )
        .unwrap();
    // Premise: both matched their own host, so the difference is the listing.
    assert!(!listed.authenticity.spoofed && !host_only.authenticity.spoofed);
    assert_eq!(host_only.authenticity.credited.len(), 1);
    assert_eq!(listed.authenticity.credited.len(), 2);
    assert!(listed.authenticity.score > host_only.authenticity.score);
}
