//! ADR 0110, Amendment 1, at the cell: a per-cycle policy payload is signed
//! by one process, so under a trust bundle a model reaches the cell only when
//! the manifest carries that model's own two-person release under the
//! `model-pack` key.
//!
//! The failure this closes: the API's service account holds `cloudkms.signer`
//! on the `policy` key and signs a payload every cycle with nobody else in
//! the loop. If the payload's signature were what admitted a model, that one
//! account — or whoever took it — could name any weights to every cell, and
//! the two-person publish of model packs would bind only the centre's
//! bookkeeping. Every case runs through `VerifiedPolicy::verify_with`, the
//! one constructor a cell's policy state accepts.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::policy::{InlineModel, ModelManifest, ModelPack, PolicyPayload, Slot};
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, Publication, TrustBundle, TrustedKey};
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_edge::TrustRoot;
use qip_edge::policy::VerifiedPolicy;
use qip_transport::trust::{BundleVerifier, LocalSigner};
use std::collections::{BTreeMap, BTreeSet};

const CELL: &str = "london-1";
const MODEL: &str = "regime";
const DIGEST: &str = "c0ffee";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

struct Keys {
    policy: LocalSigner,
    model_pack: LocalSigner,
}

fn keys() -> Result<Keys> {
    Ok(Keys {
        policy: LocalSigner::generate(ArtefactClass::Policy, "test/policy/1")?,
        model_pack: LocalSigner::generate(ArtefactClass::ModelPack, "test/model-pack/1")?,
    })
}

fn root(keys: &Keys) -> Result<TrustRoot> {
    let entries = [&keys.policy, &keys.model_pack]
        .into_iter()
        .map(|signer| TrustedKey {
            class: signer.class(),
            key_version: signer.key_version().to_string(),
            public_key: signer.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        })
        .collect();
    Ok(TrustRoot::bundle(BundleVerifier::new(TrustBundle::new(
        entries,
        BTreeSet::new(),
    )?)))
}

fn unsigned_pack(inline: Option<InlineModel>, approver: &str) -> ModelPack {
    ModelPack {
        artifact_digest: "artifact-sha256".to_string(),
        signature: String::new(),
        features: vec!["pressure".to_string()],
        calibration: 0.9,
        allowed_universes: BTreeSet::from(["XLON:ACME".to_string()]),
        budget_microseconds: 50,
        expires_at: 1_900_000_000,
        rollback_parent: String::new(),
        publication: Publication {
            author: "author@example.com".to_string(),
            approver: approver.to_string(),
            approved_at: t(0),
        },
        inline_model: inline,
    }
}

fn inline(name: &str, digest: &str) -> Option<InlineModel> {
    Some(InlineModel {
        name: name.to_string(),
        digest: digest.to_string(),
    })
}

/// A payload naming `regime` at `DIGEST`, carrying `packs`, signed under the
/// genuine `policy` key — so every refusal below is about the release, not
/// about the payload's own signature.
fn payload(keys: &Keys, packs: BTreeMap<String, ModelPack>) -> Result<PolicyPayload> {
    let mut payload = PolicyPayload::unproduced(7, CELL, t(10));
    payload.trained_models = Slot::produced(
        ModelManifest {
            models: BTreeMap::from([(MODEL.to_string(), DIGEST.to_string())]),
            packs,
        },
        t(10),
    );
    payload.signed_by(&keys.policy)
}

fn filed(pack: ModelPack) -> BTreeMap<String, ModelPack> {
    BTreeMap::from([(MODEL.to_string(), pack)])
}

/// Mutation: delete the `check_releases` call in `VerifiedPolicy::verify_with`
/// and every refusal below is admitted; this fails on the first. Mutation:
/// make `check_releases` skip the `inline_model` match and the wrong-weights
/// case is admitted. Mutation: leave the inline model out of
/// `ModelPack::signing_payload` and the edited release is admitted.
#[test]
fn a_bundle_cell_admits_a_model_only_under_its_own_two_person_release() -> Result<()> {
    let keys = keys()?;
    let root = root(&keys)?;

    // The admitting half first: a gate that refuses everything would pass
    // every refusal below.
    let released =
        unsigned_pack(inline(MODEL, DIGEST), "approver@example.com").signed_by(&keys.model_pack)?;
    let admitted =
        VerifiedPolicy::verify_with(payload(&keys, filed(released))?, &root, CELL, t(11))?;
    assert_eq!(
        admitted
            .payload()
            .trained_models
            .value()
            .and_then(|manifest| manifest.models.get(MODEL)),
        Some(&DIGEST.to_string())
    );

    let mut refusals: Vec<(&str, BTreeMap<String, ModelPack>)> = vec![("no pack", BTreeMap::new())];
    refusals.push((
        "a pack for other weights under the same name",
        filed(
            unsigned_pack(inline(MODEL, "other-weights"), "approver@example.com")
                .signed_by(&keys.model_pack)?,
        ),
    ));
    // A genuine release for other weights, re-filed under this model after
    // signing. Refused only because the inline model is inside the signed
    // bytes; were it outside, this would be the cheapest forgery there is.
    let mut edited = unsigned_pack(inline(MODEL, "other-weights"), "approver@example.com")
        .signed_by(&keys.model_pack)?;
    edited.inline_model = inline(MODEL, DIGEST);
    refusals.push(("a release edited after signing", filed(edited)));
    refusals.push((
        "a pack naming no inline model",
        filed(unsigned_pack(None, "approver@example.com").signed_by(&keys.model_pack)?),
    ));
    // The API's own key signing a pack: the exact forgery the amendment is
    // about. The class tag in the signed message keeps a `policy` signature
    // from ever verifying as a `model-pack` one.
    let mut forged = unsigned_pack(inline(MODEL, DIGEST), "approver@example.com");
    forged.signature = qip_contracts::trust::sign_artefact(
        &RelabelledAsModelPack(&keys.policy),
        ArtefactClass::ModelPack,
        &forged.signing_payload(),
    )?;
    refusals.push(("a pack signed by the policy key", filed(forged)));
    // A pack whose publication names one person twice. Signed by the genuine
    // key (built field by field, since `signed_by` refuses to sign one), so
    // the refusal is the publication check and not the signature.
    let mut one_person = unsigned_pack(inline(MODEL, DIGEST), "Author@example.com ");
    one_person.signature = qip_contracts::trust::sign_artefact(
        &keys.model_pack,
        ArtefactClass::ModelPack,
        &one_person.signing_payload(),
    )?;
    refusals.push(("a one-person publication", filed(one_person)));

    for (case, packs) in refusals {
        let result = VerifiedPolicy::verify_with(payload(&keys, packs)?, &root, CELL, t(11));
        let error = match result {
            Ok(_) => panic!("{case}: the payload was admitted"),
            Err(error) => error,
        };
        assert!(
            error.message().contains("release pack")
                || error.message().contains("author and approver"),
            "{case}: refused for an unrelated reason: {}",
            error.message()
        );
    }
    Ok(())
}

/// A pack for a model the manifest does not name is a second list of what
/// may run. Mutation: drop the stray-pack check in `check_releases` and this
/// is admitted.
#[test]
fn a_bundle_cell_refuses_a_release_pack_for_a_model_the_manifest_does_not_name() -> Result<()> {
    let keys = keys()?;
    let root = root(&keys)?;
    let mut packs = filed(
        unsigned_pack(inline(MODEL, DIGEST), "approver@example.com").signed_by(&keys.model_pack)?,
    );
    packs.insert(
        "stray".to_string(),
        unsigned_pack(inline("stray", "d"), "approver@example.com").signed_by(&keys.model_pack)?,
    );
    // Premise: without the stray the same packs are admitted.
    let mut clean = packs.clone();
    clean.remove("stray");
    assert!(VerifiedPolicy::verify_with(payload(&keys, clean)?, &root, CELL, t(11)).is_ok());

    let error = VerifiedPolicy::verify_with(payload(&keys, packs)?, &root, CELL, t(11))
        .expect_err("a stray release pack was admitted");
    assert!(error.message().contains("`stray`"), "{}", error.message());
    Ok(())
}

/// A signer that reports the `model-pack` class while signing with another
/// class's key — what a process holding only the `policy` key can produce.
#[derive(Debug)]
struct RelabelledAsModelPack<'a>(&'a LocalSigner);

impl ArtefactSigner for RelabelledAsModelPack<'_> {
    fn class(&self) -> ArtefactClass {
        ArtefactClass::ModelPack
    }

    fn key_version(&self) -> &str {
        self.0.key_version()
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        self.0.sign_message(message)
    }
}
