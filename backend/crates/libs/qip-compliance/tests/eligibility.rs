//! R12-GOV-6116 / 6118: legal eligibility on all five subject kinds, absent
//! meaning refused, and the deployment and enablement gates that read it.
//!
//! Each test names the mutation that should make it fail.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use qip_compliance::eligibility::{
    EligibilityRecord, EligibilityRegister, Refusal, Subject, SubjectKind,
};
use qip_core::Timestamp;
use std::collections::BTreeSet;

fn record(kind: SubjectKind, id: &str, places: &[&str]) -> EligibilityRecord {
    EligibilityRecord {
        subject: Subject::new(kind, id),
        jurisdictions: places.iter().map(|p| p.to_string()).collect(),
        granted_by: "compliance-officer".into(),
        granted_at: Timestamp::from_secs(100),
        expires_at: Timestamp::from_secs(1_000),
        revoked: None,
    }
}

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

/// Each of the five kinds has a record, and the same subject with no record
/// is refused. Mutation: make `check` return `Ok(())` when no record is held
/// — every `NoRecord` assertion fails.
#[test]
fn each_of_the_five_subject_kinds_carries_a_record_and_an_absent_one_refuses() {
    assert_eq!(SubjectKind::ALL.len(), 5);
    let mut register = EligibilityRegister::new();
    for kind in SubjectKind::ALL {
        register.record(record(kind, "known", &["US"])).unwrap();
    }
    for kind in SubjectKind::ALL {
        let known = Subject::new(kind, "known");
        assert_eq!(
            register.check(&known, "US", at(500)),
            Ok(()),
            "premise: {kind:?}"
        );
        let unknown = Subject::new(kind, "unknown");
        assert_eq!(
            register.check(&unknown, "US", at(500)),
            Err(Refusal::NoRecord {
                subject: unknown.clone()
            }),
            "{kind:?}"
        );
    }
}

/// Mutation: drop any one branch of `check` — its row passes when it should
/// refuse.
#[test]
fn a_record_admits_only_in_its_jurisdictions_between_grant_and_expiry_and_not_once_revoked() {
    let venue = Subject::new(SubjectKind::Venue, "venue-a");
    let mut register = EligibilityRegister::new();
    register
        .record(record(SubjectKind::Venue, "venue-a", &["US"]))
        .unwrap();
    assert!(register.check(&venue, "US", at(500)).is_ok(), "premise");
    assert!(matches!(
        register.check(&venue, "GB", at(500)),
        Err(Refusal::JurisdictionAbsent { .. })
    ));
    assert!(matches!(
        register.check(&venue, "US", at(50)),
        Err(Refusal::NotYetGranted { .. })
    ));
    assert!(matches!(
        register.check(&venue, "US", at(1_000)),
        Err(Refusal::Expired { .. })
    ));
    let mut revoked = record(SubjectKind::Venue, "venue-a", &["US"]);
    revoked.revoked = Some("licence lapsed".into());
    register.record(revoked).unwrap();
    assert!(matches!(
        register.check(&venue, "US", at(500)),
        Err(Refusal::Revoked { .. })
    ));
}

/// Mutation: remove the jurisdiction-list or grantor checks in `record`.
#[test]
fn a_record_with_no_jurisdiction_no_grantor_or_no_lifetime_is_refused() {
    let mut register = EligibilityRegister::new();
    assert!(
        register
            .record(record(SubjectKind::Product, "equity", &["US"]))
            .is_ok(),
        "premise"
    );
    assert!(
        register
            .record(record(SubjectKind::Product, "equity", &[]))
            .is_err()
    );
    let mut anonymous = record(SubjectKind::Product, "equity", &["US"]);
    anonymous.granted_by = " ".into();
    assert!(register.record(anonymous).is_err());
    let mut instant = record(SubjectKind::Product, "equity", &["US"]);
    instant.expires_at = instant.granted_at;
    assert!(register.record(instant).is_err());
    let twice = vec![
        record(SubjectKind::Product, "equity", &["US"]),
        record(SubjectKind::Product, "equity", &["GB"]),
    ];
    assert!(EligibilityRegister::from_records(twice).is_err());
}

/// The 6118 gates name every refusal at once, including the jurisdiction
/// itself. Mutation: have `refusals` stop at the first refusal (or skip the
/// jurisdiction subject) — the count or the jurisdiction line fails.
#[test]
fn deployment_and_enablement_are_refused_naming_every_subject_without_a_record() {
    let mut register = EligibilityRegister::from_records(vec![
        record(SubjectKind::Strategy, "momentum", &["US"]),
        record(SubjectKind::Product, "equity", &["US"]),
        record(SubjectKind::Venue, "venue-a", &["US"]),
        record(SubjectKind::Jurisdiction, "US", &["US"]),
    ])
    .unwrap();
    let products = vec!["equity".to_string()];
    let venues = vec!["venue-a".to_string()];
    assert!(
        register
            .admit_deployment("momentum", &products, &venues, "US", at(500))
            .is_ok(),
        "premise"
    );
    assert!(
        register
            .admit_enablement("venue-a", "equity", "US", at(500))
            .is_ok()
    );

    let wider_products = vec!["equity".to_string(), "futures".to_string()];
    let wider_venues = vec!["venue-a".to_string(), "venue-b".to_string()];
    let err = register
        .admit_deployment("carry", &wider_products, &wider_venues, "US", at(500))
        .unwrap_err()
        .to_string();
    for named in ["strategy `carry`", "product `futures`", "venue `venue-b`"] {
        assert!(err.contains(named), "{named}: {err}");
    }
    assert!(!err.contains("product `equity`"), "{err}");

    let refusals = register.refusals(
        &[Subject::new(SubjectKind::Venue, "venue-a")],
        "GB",
        at(500),
    );
    let kinds: BTreeSet<SubjectKind> = refusals
        .iter()
        .map(|r| match r {
            Refusal::JurisdictionAbsent { subject, .. } | Refusal::NoRecord { subject } => {
                subject.kind
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from([SubjectKind::Venue, SubjectKind::Jurisdiction]),
        "the venue is not permitted in GB and GB is not a recorded jurisdiction"
    );
    register
        .record(record(SubjectKind::Venue, "venue-a", &["US"]))
        .unwrap();
    assert!(
        register
            .admit_deployment("momentum", &[], &venues, "US", at(500))
            .is_err()
    );
}
