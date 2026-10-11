//! A connector's checked evidence reaches the world model's fabric, and the
//! poisoning detector reads what arrived (R12-EVID-2031, 2035, 2038, 2039).
//!
//! The failure each test prevents is the one the audit found shipped: the
//! world fabric was never fed, so `detect_poisoning` scanned an empty fabric
//! in every binary and reported a clean feed. Each test asserts first that
//! the fabric was empty before the forward, so a test that passes because
//! the fabric was seeded some other way is not possible.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Context, Timestamp};
use qip_financial::universe::Universe;
use qip_kernel::evidence_intake::{
    EVIDENCE_ABSORBED, EVIDENCE_POISONING_FINDINGS, PoisoningClass, PoisoningThresholds,
};
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::Telemetry;
use qip_observability::metrics::labels;
use qip_risk::limits::LimitSet;
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{EvidenceFabric, EvidenceRecord};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000 + secs)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(at(0), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap()
}

/// One claim as a connector runtime would hand it over: the record, the body
/// hash, and the verdict its *private* fabric reached — a fresh fabric per
/// call, which is exactly the isolation that hid every cross-source fact.
fn checked(id: &str, source: &str, statement: &str, value: &str, hash: &str) -> CheckedEvidence {
    let record = EvidenceRecord::new(
        id.to_string(),
        source.to_string(),
        at(10),
        at(15),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        statement.to_string(),
        0.9,
    )
    .unwrap()
    .with_claimed_at(at(0))
    .with_metadata(META_SUBJECT.into(), "EURUSD".into())
    .with_metadata(META_ATTRIBUTE.into(), "rate".into())
    .with_metadata(META_VALUE.into(), value.into());
    let (verdict, _) = EvidenceFabric::default()
        .admit(record.clone(), Some(hash), at(20))
        .unwrap();
    CheckedEvidence {
        record,
        document_hash: hash.to_string(),
        verdict,
    }
}

fn hash(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

/// Two sources disagreeing about one value are each uncontested in their own
/// runtime's fabric; forwarded into the world fabric, the disagreement is a
/// contest. Mutation: make `absorb_evidence` skip `world.absorb_evidence`
/// (count and continue) — `evidence_held` stays 0 and `contested` 0, and the
/// assertions below fail.
#[test]
fn two_sources_disagreeing_are_a_contest_only_once_forwarded_into_the_world_fabric() {
    let mut platform = platform();
    assert_eq!(
        platform.evidence_held(),
        0,
        "premise: an empty world fabric"
    );
    let a = checked("a1", "https://a.example", "rate 1.10", "1.10", &hash('a'));
    let b = checked("b1", "https://b.example", "rate 1.20", "1.20", &hash('b'));
    assert!(
        a.verdict.contradicts.is_empty() && b.verdict.contradicts.is_empty(),
        "premise: each source's private fabric saw no contest"
    );

    let intake = platform.absorb_evidence(vec![a, b], at(30));

    assert_eq!(platform.evidence_held(), 2, "{intake:?}");
    assert_eq!(intake.admitted, 2, "{intake:?}");
    assert_eq!(
        intake.contested, 1,
        "the second claim contradicts the first across sources: {intake:?}"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert_eq!(
        snapshot.counter(EVIDENCE_ABSORBED, &labels([("outcome", "contested")])),
        1
    );
}

/// A claim forwarded twice is held once and counted as a duplicate rather
/// than corroborating itself. Mutation: delete the `get(..).is_some()` guard —
/// the fabric's own duplicate refusal then lands on `refused`, and the
/// `duplicate == 1` assertion fails.
#[test]
fn a_claim_forwarded_twice_is_held_once_and_counted_as_a_duplicate() {
    let mut platform = platform();
    let first = checked("d1", "https://a.example", "rate 1.10", "1.10", &hash('a'));
    let again = first.clone();
    assert_eq!(platform.absorb_evidence(vec![first], at(30)).admitted, 1);

    let intake = platform.absorb_evidence(vec![again], at(31));

    assert_eq!(intake.duplicate, 1, "{intake:?}");
    assert_eq!(intake.refused, 0, "{intake:?}");
    assert_eq!(platform.evidence_held(), 1);
}

/// Three distinct fetches carrying one statement from three sources is a
/// campaign the detector names — once the claims are in the fabric it reads.
/// The premise half is the audit's finding: before the forward the same
/// detector over the same platform finds nothing. Mutation: return an empty
/// `Vec` from `evidence_poisoning` — the class assertion fails.
#[test]
fn a_coordinated_campaign_forwarded_from_connectors_is_found_by_the_poisoning_detector() {
    let mut platform = platform();
    let thresholds = PoisoningThresholds::default();
    assert!(
        platform.evidence_poisoning(at(40), &thresholds).is_empty(),
        "premise: nothing to find in an empty fabric"
    );
    let campaign = ['a', 'b', 'c']
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            checked(
                &format!("k{i}"),
                &format!("https://site{i}.example"),
                "Acme CEO arrested",
                "x",
                &hash(c),
            )
        })
        .collect();

    platform.absorb_evidence(campaign, at(30));
    let findings = platform.evidence_poisoning(at(40), &thresholds);

    assert_eq!(platform.evidence_held(), 3);
    assert!(
        findings
            .iter()
            .any(|f| f.class == PoisoningClass::CoordinatedManipulation && f.sources.len() == 3),
        "{findings:?}"
    );
    let snapshot = platform.telemetry().metrics.snapshot();
    assert!(
        snapshot.counter(
            EVIDENCE_POISONING_FINDINGS,
            &labels([("class", "coordinated_manipulation")])
        ) >= 1
    );
}
