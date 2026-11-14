//! `qip reflex-package` (ADR 0535, R12-REASON-7203): the publish steps a
//! reflex package draft goes through, driven as the workflow drives them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // a panic is the failure report

use qip_cli::reflex_package::{self, INVALID};
use qip_contracts::policy::DistilledModelContract;
use qip_contracts::reflex_package::{
    Condition, ReflexPackage, ReflexPackageDraft, ReflexPackageFields, ReflexRule,
};
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, TrustBundle, TrustedKey};
use qip_core::Timestamp;
use qip_transport::trust::LocalSigner;
use std::collections::BTreeSet;
use std::path::PathBuf;

const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);
const KEY_VERSION: &str = "local/model-pack/1";

fn draft() -> ReflexPackageDraft {
    let package = ReflexPackage::new(ReflexPackageFields {
        schema_version: 1,
        package_id: "fill-bias-XSIM".to_string(),
        discovery_id: "fill-bias-XSIM".to_string(),
        derivation_id: "derivation-1".to_string(),
        issued_at: Timestamp::from_secs(1_799_000_000),
        expires_at: Timestamp::from_secs(1_900_000_000),
        rules: vec![ReflexRule {
            id: "fill-bias@XSIM".to_string(),
            when: vec![Condition::Equals {
                input: "venue".to_string(),
                value: "XSIM".to_string(),
            }],
            output: "twin_entry_bps".to_string(),
            add_bps: -40,
        }],
        bounds: DistilledModelContract {
            name: "fill-bias-XSIM".to_string(),
            memory_bytes: 1024,
            latency_micros: 10,
            reflex_compatible: true,
            worst_case_cost: 2,
        },
    })
    .unwrap();
    ReflexPackageDraft::new(package).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qip-reflex-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// The deep brain holds no signer, so these verbs are the only path from a
/// draft to a package a cell loads. A package attached under the
/// publication its message was printed for verifies; one attached under a
/// different approver than was signed does not, because the publication is
/// inside the signed bytes; and a one-person publication is refused before
/// a message is printed for anybody to sign.
///
/// Mutation, verified: drop `publication.validate()` in
/// `reflex_package::publication` — the one-person message is printed and
/// the last assertion fails.
#[test]
fn a_reflex_package_verifies_only_under_the_two_person_publication_its_signature_covers() {
    let dir = scratch("two-person");
    let signer = LocalSigner::generate(ArtefactClass::ModelPack, KEY_VERSION).unwrap();
    let bundle = TrustBundle::new(
        vec![TrustedKey {
            class: ArtefactClass::ModelPack,
            key_version: KEY_VERSION.to_string(),
            public_key: signer.public_key_hex(),
            not_after: Timestamp::from_secs(4_000_000_000),
        }],
        BTreeSet::new(),
    )
    .unwrap();
    let bundle_path = dir.join("bundle.json");
    std::fs::write(&bundle_path, serde_json::to_string(&bundle).unwrap()).unwrap();
    let draft_path = dir.join("draft.json");
    std::fs::write(&draft_path, draft().encode().unwrap()).unwrap();
    let draft_arg = draft_path.to_string_lossy().to_string();
    let bundle_arg = bundle_path.to_string_lossy().to_string();
    let who = |approver: &str| {
        vec![
            "--key-version".to_string(),
            KEY_VERSION.to_string(),
            "--author".to_string(),
            "author-login".to_string(),
            "--approver".to_string(),
            approver.to_string(),
            "--approved-at".to_string(),
            "2027-01-15T00:00:00Z".to_string(),
        ]
    };

    let mut step = args(&["signing-message", &draft_arg]);
    step.extend(who("approver-login"));
    let (lines, code) = reflex_package::run(&step, NOW).unwrap();
    assert_eq!(code, 0);
    let der = signer
        .sign_message(&qip_core::hash::from_hex(&lines[0]).unwrap())
        .unwrap();
    let attach = |approver: &str| {
        let mut step = args(&["attach-signature", &draft_arg]);
        step.extend(who(approver));
        step.extend(args(&["--der", &qip_core::hash::to_hex(&der)]));
        reflex_package::run(&step, NOW)
    };
    let verify = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        reflex_package::run(
            &args(&[
                "verify",
                &path.to_string_lossy(),
                "--trust-bundle",
                &bundle_arg,
            ]),
            NOW,
        )
        .unwrap()
    };

    let (lines, code) = attach("approver-login").unwrap();
    assert_eq!(code, 0);
    let (verdict, code) = verify("published.json", &lines[0]);
    assert_eq!(code, 0, "premise: the signed package verifies: {verdict:?}");
    assert!(verdict[0].contains("verified; it loads"), "{verdict:?}");

    let (lines, code) = attach("someone-else").unwrap();
    assert_eq!(code, 0, "attaching is not verifying");
    let (verdict, code) = verify("moved.json", &lines[0]);
    assert_eq!(
        code, INVALID,
        "a re-attributed package verified: {verdict:?}"
    );

    let mut alone = args(&["signing-message", &draft_arg]);
    alone.extend(who("Author-Login"));
    assert!(reflex_package::run(&alone, NOW).is_err());
}
