//! The envelope issuer under ADR 0110: grants signed through an
//! `ArtefactSigner` under the `envelope` key, verified through an
//! `ArtefactVerifier`, and refused when altered or when the signer is
//! published for another class.
//!
//! The signer and verifier here are stand-ins keyed by nothing — a digest of
//! the signed message — so the property under test is the issuer's plumbing
//! (which bytes, which class, which key version) and not a curve; the curve
//! is `qip-transport`'s, proven there with `ring`.

#![allow(clippy::panic_in_result_fn)]

use qip_capital::envelope::{EnvelopeIssuer, EnvelopeTerms};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::trust::{
    ArtefactClass, ArtefactSignature, ArtefactSigner, ArtefactVerifier, SIGNATURE_SCHEME,
    signing_message,
};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp, dec, sha256};
use std::sync::Arc;

fn start() -> Timestamp {
    Timestamp::from_secs(1_700_000_000)
}

#[derive(Debug)]
struct DigestSigner(ArtefactClass);

impl ArtefactSigner for DigestSigner {
    fn class(&self) -> ArtefactClass {
        self.0
    }
    fn key_version(&self) -> &str {
        "projects/p/locations/l/keyRings/r/cryptoKeys/envelope/cryptoKeyVersions/1"
    }
    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(sha256(message).to_vec())
    }
}

#[derive(Debug)]
struct DigestVerifier;

impl ArtefactVerifier for DigestVerifier {
    fn verify(
        &self,
        class: ArtefactClass,
        canonical: &[u8],
        signature: &str,
        _now: Timestamp,
    ) -> Result<()> {
        let parsed = ArtefactSignature::parse(signature)?;
        let expected = sha256(&signing_message(class, parsed.key_version(), canonical));
        if parsed.der() == expected.as_slice() {
            Ok(())
        } else {
            Err(Error::denied("does not verify"))
        }
    }
}

fn terms() -> EnvelopeTerms {
    EnvelopeTerms {
        strategy: StrategyId::new("mean-reversion-1"),
        cell: "london-1".to_string(),
        gross_limit: dec!("10000"),
        order_fraction: dec!("0.1"),
        loss_fraction: dec!("0.2"),
        venues: vec![VenueId::new("XLON")],
        validity: Duration::from_hours(2),
        exposure: qip_capital::ExposureTerms::default(),
        drawdown_of_loss: qip_capital::envelope::DEFAULT_DRAWDOWN_OF_LOSS,
    }
}

fn approval() -> Result<Approval> {
    Approval::new("capital grant", "alice.chen", start(), "committee approved")?
        .countersigned_by("bram.oduya")
}

/// Mutation: in `EnvelopeIssuer::sign`, sign the shared-key MAC whatever the
/// key (or sign under `ArtefactClass::Policy`) and the issued grant is not an
/// envelope-class asymmetric signature; this fails.
#[test]
fn an_asymmetric_issuer_signs_under_the_envelope_key_and_refuses_an_altered_grant() -> Result<()> {
    let issuer = EnvelopeIssuer::asymmetric(
        Arc::new(DigestSigner(ArtefactClass::Envelope)),
        Arc::new(DigestVerifier),
    )?;
    assert!(issuer.is_asymmetric());
    let envelope = issuer.issue(&terms(), &approval()?, start())?;
    assert!(
        envelope
            .signature()
            .starts_with(&format!("{SIGNATURE_SCHEME}:")),
        "the grant does not carry an asymmetric signature: {}",
        envelope.signature()
    );
    // Premise: the genuine grant verifies.
    issuer.verify(&envelope, start())?;

    // The same signature over a wider grant.
    let wider = qip_contracts::CapitalEnvelope::new(
        envelope.strategy().clone(),
        envelope.cell(),
        dec!("20000"),
        dec!("2000"),
        dec!("4000"),
        vec![VenueId::new("XLON")],
        start(),
        envelope.expires_at(),
        envelope.approver(),
        envelope.signature(),
    )?;
    assert!(
        issuer.verify(&wider, start()).is_err(),
        "a grant widened after signing still verified"
    );
    Ok(())
}

/// Mutation: drop the class check in `EnvelopeIssuer::asymmetric` and a
/// policy-key signer is accepted to sign capital; this fails.
#[test]
fn an_issuer_refuses_a_signer_published_for_another_class() {
    for class in [
        ArtefactClass::Policy,
        ArtefactClass::Halt,
        ArtefactClass::ModelPack,
    ] {
        assert!(
            EnvelopeIssuer::asymmetric(Arc::new(DigestSigner(class)), Arc::new(DigestVerifier))
                .is_err(),
            "a {class} signer was accepted for capital envelopes"
        );
    }
}
