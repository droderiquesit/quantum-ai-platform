//! R12-REASON-3063: the node's directory loader is fail-closed.
//!
//! The failure this prevents: a node that skipped an unverifiable package
//! would run without a rule its operators believe is installed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::policy::DistilledModelContract;
use qip_contracts::reflex_package::{
    Condition, ReflexPackage, ReflexPackageDraft, ReflexPackageFields, ReflexRule,
};
use qip_contracts::trust::{ArtefactClass, Publication, TrustBundle, TrustedKey, sign_artefact};
use qip_core::Timestamp;
use qip_edge::TrustRoot;
use qip_edge_node::reflex::load_dir;
use qip_transport::trust::{BundleVerifier, LocalSigner};
use std::collections::BTreeSet;

const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);
const KV: &str = "local/model-pack/1";

fn published(id: &str, signer: &LocalSigner) -> Vec<u8> {
    let package = ReflexPackage::new(ReflexPackageFields {
        schema_version: 1,
        package_id: id.to_string(),
        discovery_id: id.to_string(),
        derivation_id: "d".to_string(),
        issued_at: Timestamp::from_secs(1_799_000_000),
        expires_at: Timestamp::from_secs(1_900_000_000),
        rules: vec![ReflexRule {
            id: "r".to_string(),
            when: vec![Condition::Equals {
                input: "venue".to_string(),
                value: "XSIM".to_string(),
            }],
            output: "twin_entry_bps".to_string(),
            add_bps: -40,
        }],
        bounds: DistilledModelContract {
            name: id.to_string(),
            memory_bytes: 1024,
            latency_micros: 10,
            reflex_compatible: true,
            worst_case_cost: 2,
        },
    })
    .unwrap();
    let draft = ReflexPackageDraft::new(package).unwrap();
    let publication = Publication {
        author: "a".to_string(),
        approver: "b".to_string(),
        approved_at: NOW,
    };
    let sig = sign_artefact(
        signer,
        ArtefactClass::ModelPack,
        &draft.signing_canonical(&publication),
    )
    .unwrap();
    draft.publish(publication, sig).unwrap().encode().unwrap()
}

fn root(signer: &LocalSigner) -> TrustRoot {
    TrustRoot::bundle(BundleVerifier::new(
        TrustBundle::new(
            vec![TrustedKey {
                class: ArtefactClass::ModelPack,
                key_version: KV.to_string(),
                public_key: signer.public_key_hex(),
                not_after: Timestamp::from_secs(4_000_000_000),
            }],
            BTreeSet::new(),
        )
        .unwrap(),
    ))
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("qip-reflexdir-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_directory_of_verifying_packages_is_admitted_in_full() {
    let key = LocalSigner::generate(ArtefactClass::ModelPack, KV).unwrap();
    let dir = scratch("ok");
    std::fs::write(dir.join("a.json"), published("a", &key)).unwrap();
    std::fs::write(dir.join("b.json"), published("b", &key)).unwrap();
    std::fs::write(dir.join("notes.txt"), b"not a package").unwrap();
    let book = load_dir(&dir, &root(&key), NOW).unwrap();
    assert_eq!(book.len(), 2);
}

#[test]
fn one_unverifiable_file_stops_the_load_and_names_the_file() {
    let key = LocalSigner::generate(ArtefactClass::ModelPack, KV).unwrap();
    let stranger = LocalSigner::generate(ArtefactClass::ModelPack, KV).unwrap();
    let dir = scratch("bad");
    std::fs::write(dir.join("a.json"), published("a", &key)).unwrap();
    std::fs::write(dir.join("z-forged.json"), published("z", &stranger)).unwrap();
    let err = load_dir(&dir, &root(&key), NOW).unwrap_err();
    assert!(err.message().contains("z-forged.json"), "{}", err.message());
}

#[test]
fn an_unreadable_directory_is_refused() {
    let key = LocalSigner::generate(ArtefactClass::ModelPack, KV).unwrap();
    let missing = std::env::temp_dir().join("qip-reflexdir-does-not-exist");
    assert!(load_dir(&missing, &root(&key), NOW).is_err());
}
