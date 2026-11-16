//! R12-GOV-6125: a capital-class decision needs authority only an enrolled
//! WebAuthn key can carry. Each test signs a real ECDSA P-256 assertion the
//! way an authenticator would and tries one way to get capital without it.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::panic)]

use qip_compliance::approval::{ApprovalChain, CapitalRequest};
use qip_compliance::attestation::{
    ApprovalVerifier, Assertion, Challenge, OperatorKeyDirectory, challenge_digest, check_attested,
};
use qip_compliance::signing::SigningKey;
use qip_contracts::action::{ActionClass, AuthorityLevel, ExecutableScope, Routed};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::hash::{sha256, to_hex};
use qip_core::{Duration, Timestamp, dec};
use qip_tls::ring::rand::SystemRandom;
use qip_tls::ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

const RP: &str = "desk.example";
const ORIGIN: &str = "https://desk.example";

fn now() -> Timestamp {
    Timestamp::from_secs(1_800_000_000)
}

struct Authenticator {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Authenticator {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, doc.as_ref(), &rng).unwrap();
        Self { pair, rng }
    }
    fn public_hex(&self) -> String {
        to_hex(self.pair.public_key().as_ref())
    }
    fn assert(&self, subject: &str, digest: [u8; 32], flags: u8) -> Assertion {
        let b64 = {
            const A: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut o = String::new();
            for c in digest.chunks(3) {
                let n = c
                    .iter()
                    .enumerate()
                    .fold(0u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
                for i in 0..=c.len() {
                    o.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            o
        };
        let client =
            format!(r#"{{"type":"webauthn.get","challenge":"{b64}","origin":"{ORIGIN}"}}"#)
                .into_bytes();
        let mut data = sha256(RP.as_bytes()).to_vec();
        data.push(flags);
        data.extend_from_slice(&[0, 0, 0, 1]);
        let mut signed = data.clone();
        signed.extend_from_slice(&sha256(&client));
        let signature = self
            .pair
            .sign(&self.rng, &signed)
            .unwrap()
            .as_ref()
            .to_vec();
        Assertion {
            subject: subject.to_string(),
            authenticator_data: data,
            client_data_json: client,
            signature,
        }
    }
}

fn request() -> CapitalRequest {
    CapitalRequest {
        strategy: StrategyId::new("stat-arb-eu"),
        cell: "frankfurt-1".to_string(),
        gross_limit: dec!("500000"),
        order_limit: dec!("50000"),
        loss_limit: dec!("25000"),
        venues: vec![VenueId::new("XETR")],
        expires_at: now().saturating_add(Duration::from_days(1)),
        requested_by: "r.mensah".to_string(),
        exposure: None,
        drawdown: None,
        authority: None,
    }
}

fn intent(class: ActionClass) -> qip_contracts::action::ActionIntent {
    let r = request();
    match ExecutableScope::new([r.strategy.as_str().to_string()]).route_with_class(
        &r.subject(),
        r.strategy.as_str(),
        class,
    ) {
        Routed::Executable(i) => i,
        Routed::Shadow(_) => panic!("in scope"),
    }
}

fn challenge(n: u8) -> Challenge {
    Challenge {
        nonce: [n; 16],
        issued_at: now(),
    }
}

fn verifier(key: &Authenticator, ceiling: AuthorityLevel) -> Result<ApprovalVerifier> {
    let mut dir = OperatorKeyDirectory::new();
    dir.enrol("k.almeida", &key.public_hex(), ceiling)?;
    ApprovalVerifier::new(dir, RP, ORIGIN)
}

const UP_UV: u8 = 0x05;

#[test]
fn a_capital_intent_is_admitted_on_an_attestation_of_capital_authority() -> Result<()> {
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Capital)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(1);
    let a = v.verify(
        &i,
        &c,
        &key.assert("k.almeida", challenge_digest(&i, &c), UP_UV),
        now(),
    )?;
    assert_eq!(a.authority(), AuthorityLevel::Capital);
    check_attested(&i, &a)
}

#[test]
fn a_valid_assertion_from_a_key_enrolled_at_operational_cannot_admit_a_capital_intent() -> Result<()>
{
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Operational)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(2);
    let a = v.verify(
        &i,
        &c,
        &key.assert("k.almeida", challenge_digest(&i, &c), UP_UV),
        now(),
    )?;
    let refusal = check_attested(&i, &a).unwrap_err();
    assert!(
        refusal
            .message()
            .contains("requires authority level capital")
    );
    Ok(())
}

#[test]
fn an_assertion_made_for_a_research_intent_is_refused_for_a_capital_one() -> Result<()> {
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Capital)?;
    let c = challenge(3);
    let research = intent(ActionClass::Research);
    let assertion = key.assert("k.almeida", challenge_digest(&research, &c), UP_UV);
    let refusal = v
        .verify(&intent(ActionClass::Capital), &c, &assertion, now())
        .unwrap_err();
    assert!(refusal.message().contains("different challenge"));
    Ok(())
}

#[test]
fn a_signature_by_a_key_that_is_not_the_enrolled_one_is_refused() -> Result<()> {
    let enrolled = Authenticator::new();
    let intruder = Authenticator::new();
    let mut v = verifier(&enrolled, AuthorityLevel::Capital)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(4);
    let refusal = v
        .verify(
            &i,
            &c,
            &intruder.assert("k.almeida", challenge_digest(&i, &c), UP_UV),
            now(),
        )
        .unwrap_err();
    assert!(refusal.message().contains("does not verify"));
    Ok(())
}

#[test]
fn user_verification_is_required_not_just_presence() -> Result<()> {
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Capital)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(5);
    let refusal = v
        .verify(
            &i,
            &c,
            &key.assert("k.almeida", challenge_digest(&i, &c), 0x01),
            now(),
        )
        .unwrap_err();
    assert!(refusal.message().contains("presence and verification"));
    Ok(())
}

#[test]
fn a_stale_challenge_and_a_replayed_nonce_are_both_refused() -> Result<()> {
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Capital)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(6);
    let assertion = key.assert("k.almeida", challenge_digest(&i, &c), UP_UV);
    let late = now().saturating_add(Duration::from_secs(121));
    assert!(
        v.verify(&i, &c, &assertion, late)
            .unwrap_err()
            .message()
            .contains("older than 120")
    );
    v.verify(&i, &c, &assertion, now())?;
    assert!(
        v.verify(&i, &c, &assertion, now())
            .unwrap_err()
            .message()
            .contains("already used")
    );
    Ok(())
}

#[test]
fn an_unenrolled_subject_is_refused_and_a_duplicate_enrolment_is_refused() -> Result<()> {
    let key = Authenticator::new();
    let mut v = verifier(&key, AuthorityLevel::Capital)?;
    let i = intent(ActionClass::Capital);
    let c = challenge(7);
    let refusal = v
        .verify(
            &i,
            &c,
            &key.assert("nobody", challenge_digest(&i, &c), UP_UV),
            now(),
        )
        .unwrap_err();
    assert!(refusal.message().contains("not an enrolled operator"));
    let mut dir = OperatorKeyDirectory::new();
    dir.enrol("k.almeida", &key.public_hex(), AuthorityLevel::Capital)?;
    assert!(
        dir.enrol("k.almeida", &key.public_hex(), AuthorityLevel::Capital)
            .is_err()
    );
    assert!(
        dir.enrol("K.Almeida@env", &key.public_hex(), AuthorityLevel::Capital)
            .is_err()
    );
    Ok(())
}

#[test]
fn the_capital_chain_grants_on_attestation_and_refuses_an_operational_approver() -> Result<()> {
    let key = Authenticator::new();
    let req = request();
    let approval = Approval::new(
        req.subject(),
        "k.almeida",
        now(),
        "reviewed the backtest, the shadow run and the venue limits",
    )?;
    let i = intent(ActionClass::Capital);

    let mut chain = ApprovalChain::new(
        dec!("1000000"),
        SigningKey::from_secret("capital-key-2026-01", &[7u8; 32])?,
    )?;
    let mut low = verifier(&key, AuthorityLevel::Operational)?;
    let c = challenge(8);
    let weak = low.verify(
        &i,
        &c,
        &key.assert("k.almeida", challenge_digest(&i, &c), UP_UV),
        now(),
    )?;
    assert!(
        chain
            .grant_attested(&req, &approval, &[weak], now())
            .is_err()
    );
    assert!(chain.grants().is_empty());

    let mut high = verifier(&key, AuthorityLevel::Capital)?;
    let c = challenge(9);
    let strong = high.verify(
        &i,
        &c,
        &key.assert("k.almeida", challenge_digest(&i, &c), UP_UV),
        now(),
    )?;
    let granted = chain.grant_attested(&req, &approval, &[strong], now())?;
    assert_eq!(granted.approvers(), vec!["k.almeida"]);
    assert_eq!(chain.grants().len(), 1);
    Ok(())
}
