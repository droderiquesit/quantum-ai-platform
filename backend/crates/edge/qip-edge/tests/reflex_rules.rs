//! R12-REASON-3063 at the cell: a compiled reflex predicate executes on the
//! pass, and only when a two-person `model-pack` publish verifies against the
//! cell's own trust root.
//!
//! The failure this prevents: `LoadedReflex::evaluate` had no production
//! caller, so a package the deep brain compiled and a person signed changed
//! nothing anywhere. Every case below goes through `Cell::work`.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::policy::DistilledModelContract;
use qip_contracts::reflex_package::{
    Condition, ReflexPackage, ReflexPackageDraft, ReflexPackageFields, ReflexRule,
};
use qip_contracts::trust::{ArtefactClass, Publication, TrustBundle, TrustedKey, sign_artefact};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp};
use qip_edge::TrustRoot;
use qip_edge::cell::{Cell, CellConfig, Placer};
use qip_edge::reflex_rules::{MAX_PACKAGES, ReflexRuleBook};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_transport::trust::{BundleVerifier, LocalSigner};
use std::collections::BTreeSet;

const KEY_VERSION: &str = "local/model-pack/1";
const NOW: Timestamp = Timestamp::from_secs(1_800_000_000);
const EXPIRES: i64 = 1_900_000_000;

#[derive(Debug)]
struct Paper;

impl Placer for Paper {
    fn is_simulated(&self) -> bool {
        true
    }
    fn place(
        &mut self,
        _o: &str,
        _i: &ObjectId,
        _v: &VenueId,
        _s: qip_contracts::message::BookSide,
        _q: qip_core::Decimal,
        _p: qip_core::Decimal,
        _a: Timestamp,
    ) -> Result<()> {
        Ok(())
    }
}

fn draft(id: &str, venue: &str, bps: i32) -> ReflexPackageDraft {
    let package = ReflexPackage::new(ReflexPackageFields {
        schema_version: 1,
        package_id: id.to_string(),
        discovery_id: id.to_string(),
        derivation_id: "derivation-1".to_string(),
        issued_at: Timestamp::from_secs(1_799_000_000),
        expires_at: Timestamp::from_secs(EXPIRES),
        rules: vec![ReflexRule {
            id: format!("fill-bias@{venue}"),
            when: vec![Condition::Equals {
                input: "venue".to_string(),
                value: venue.to_string(),
            }],
            output: "twin_entry_bps".to_string(),
            add_bps: bps,
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
    ReflexPackageDraft::new(package).unwrap()
}

fn publish(draft: &ReflexPackageDraft, signer: &LocalSigner, approver: &str) -> Vec<u8> {
    let publication = Publication {
        author: "author-login".to_string(),
        approver: approver.to_string(),
        approved_at: NOW,
    };
    let signature = sign_artefact(
        signer,
        ArtefactClass::ModelPack,
        &draft.signing_canonical(&publication),
    )
    .unwrap();
    draft
        .clone()
        .publish(publication, signature)
        .unwrap()
        .encode()
        .unwrap()
}

fn signer() -> LocalSigner {
    LocalSigner::generate(ArtefactClass::ModelPack, KEY_VERSION).unwrap()
}

fn root(signer: &LocalSigner) -> TrustRoot {
    TrustRoot::bundle(BundleVerifier::new(
        TrustBundle::new(
            vec![TrustedKey {
                class: ArtefactClass::ModelPack,
                key_version: KEY_VERSION.to_string(),
                public_key: signer.public_key_hex(),
                not_after: Timestamp::from_secs(4_000_000_000),
            }],
            BTreeSet::new(),
        )
        .unwrap(),
    ))
}

fn cell_with(book: ReflexRuleBook) -> Cell {
    let config = CellConfig::new("london-1", "europe-west2")
        .with_venue(VenueId::new("XSIM"))
        .with_venue(VenueId::new("OTHER"));
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    Cell::new(config, features).unwrap().with_reflex_rules(book)
}

#[test]
fn a_published_package_the_cell_trusts_moves_its_venue_on_the_pass_and_no_other() {
    let key = signer();
    let mut book = ReflexRuleBook::new();
    let id = book
        .install(
            &publish(
                &draft("fill-bias-XSIM", "XSIM", -40),
                &key,
                "approver-login",
            ),
            &root(&key),
            NOW,
        )
        .unwrap();
    assert_eq!(id, "fill-bias-XSIM", "premise: the package was admitted");
    let mut cell = cell_with(book);
    let report = cell.work(NOW, &mut Paper).unwrap();
    assert_eq!(
        report.reflex_adjustments.len(),
        1,
        "only the named venue is moved: {:?}",
        report.reflex_adjustments
    );
    let adjustment = &report.reflex_adjustments[0];
    assert_eq!(adjustment.venue, "XSIM");
    assert_eq!(adjustment.outputs.get("twin_entry_bps"), Some(&-40));
}

#[test]
fn a_package_signed_under_a_key_the_cell_does_not_trust_is_refused_and_the_book_is_unchanged() {
    let trusted = signer();
    let stranger = signer();
    let mut book = ReflexRuleBook::new();
    book.install(
        &publish(&draft("good", "XSIM", -40), &trusted, "approver-login"),
        &root(&trusted),
        NOW,
    )
    .unwrap();
    let refused = book.install(
        &publish(&draft("forged", "XSIM", 90), &stranger, "approver-login"),
        &root(&trusted),
        NOW,
    );
    assert!(refused.is_err(), "a foreign-key package was admitted");
    assert_eq!(book.len(), 1, "a refused publish evicted or added a rule");
    let adjustments = book.evaluate(&["XSIM"], NOW);
    assert_eq!(adjustments[0].outputs.get("twin_entry_bps"), Some(&-40));
}

#[test]
fn a_package_past_its_expiry_stops_applying_on_a_cell_that_was_never_restarted() {
    let key = signer();
    let mut book = ReflexRuleBook::new();
    book.install(
        &publish(&draft("p", "XSIM", -40), &key, "approver-login"),
        &root(&key),
        NOW,
    )
    .unwrap();
    assert!(
        !book.evaluate(&["XSIM"], NOW).is_empty(),
        "premise: it applies before expiry"
    );
    let after = Timestamp::from_secs(EXPIRES + 1);
    assert!(
        book.evaluate(&["XSIM"], after).is_empty(),
        "an expired package still applied"
    );
}

#[test]
fn the_book_refuses_a_duplicate_id_and_a_sixty_fifth_package_rather_than_evicting() {
    let key = signer();
    let trust = root(&key);
    let mut book = ReflexRuleBook::new();
    for n in 0..MAX_PACKAGES {
        book.install(
            &publish(&draft(&format!("p{n}"), "XSIM", 1), &key, "approver-login"),
            &trust,
            NOW,
        )
        .unwrap();
    }
    assert!(
        book.install(
            &publish(&draft("p0", "XSIM", 1), &key, "approver-login"),
            &trust,
            NOW
        )
        .is_err(),
        "a duplicate id was admitted"
    );
    assert!(
        book.install(
            &publish(&draft("extra", "XSIM", 1), &key, "approver-login"),
            &trust,
            NOW
        )
        .is_err(),
        "a package past the bound was admitted"
    );
    assert_eq!(book.len(), MAX_PACKAGES);
}

#[test]
fn a_one_person_publication_is_refused_by_the_cell_loader() {
    let key = signer();
    let mut book = ReflexRuleBook::new();
    let publication = Publication {
        author: "same".to_string(),
        approver: "same".to_string(),
        approved_at: NOW,
    };
    let d = draft("solo", "XSIM", 5);
    let signature = sign_artefact(
        &key,
        ArtefactClass::ModelPack,
        &d.signing_canonical(&publication),
    )
    .unwrap();
    // `publish` itself refuses one person; hand-build the bytes the way a
    // compromised signer would, from a valid two-person one with the
    // approver rewritten.
    assert!(d.clone().publish(publication, signature).is_err());
    let good = publish(&d, &key, "approver-login");
    let mut value: serde_json::Value = serde_json::from_slice(&good).unwrap();
    value["publication"]["approver"] = serde_json::Value::String("author-login".to_string());
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(book.install(&bytes, &root(&key), NOW).is_err());
    assert!(book.is_empty());
}

#[test]
fn a_republish_under_a_held_package_id_is_refused_and_the_held_rule_stands() {
    // Separate from the bound test: on a full book a duplicate is refused by
    // the bound too, which would hide a missing duplicate check.
    let key = signer();
    let trust = root(&key);
    let mut book = ReflexRuleBook::new();
    book.install(
        &publish(&draft("p", "XSIM", -40), &key, "approver-login"),
        &trust,
        NOW,
    )
    .unwrap();
    assert!(
        book.install(
            &publish(&draft("p", "XSIM", 90), &key, "approver-login"),
            &trust,
            NOW
        )
        .is_err()
    );
    assert_eq!(book.len(), 1);
    assert_eq!(
        book.evaluate(&["XSIM"], NOW)[0]
            .outputs
            .get("twin_entry_bps"),
        Some(&-40)
    );
}
