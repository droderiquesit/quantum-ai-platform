//! The evidence fabric and the poisoning detector over it
//! (R12-EVID-2031..2039).
//!
//! Every test asserts its premise before its property: a corroboration test
//! first shows the claims share a key, a poisoning test first shows the
//! fabric holds the claims the finding should rest on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)] // integration tests may unwrap: a panic is the failure report; the float comparisons are against exact constants the code assigns

use qip_core::{Duration, Timestamp};
use qip_world_model::evidence::{
    META_ATTRIBUTE, META_COUNTRY, META_JURISDICTION, META_SUBJECT, META_VALUE,
};
use qip_world_model::{
    AuthenticitySignal, EvidenceFabric, EvidenceRecord, GeographicConsistency, KeyOutcome,
    PoisoningClass, PoisoningThresholds, ProvenanceEdge, ProvenanceNode, TemporalConsistency,
    detect_poisoning,
};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_800_000_000 + secs)
}

fn claim(id: &str, source: &str, statement: &str, value: &str, published: i64) -> EvidenceRecord {
    EvidenceRecord::new(
        id.to_string(),
        source.to_string(),
        at(published),
        at(published + 5),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        statement.to_string(),
        0.9,
    )
    .unwrap()
    .with_claimed_at(at(0))
    .with_metadata(META_SUBJECT.into(), "EURUSD".into())
    .with_metadata(META_ATTRIBUTE.into(), "rate".into())
    .with_metadata(META_VALUE.into(), value.into())
}

#[test]
fn a_second_independent_source_corroborates_and_a_copy_of_the_same_document_does_not() {
    let mut fabric = EvidenceFabric::default();
    let (_, first) = fabric
        .admit(
            claim("a", "https://ecb.example", "EURUSD 1.10", "1.10", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert_eq!(first, KeyOutcome::New);

    let (copy, _) = fabric
        .admit(
            claim("b", "https://mirror.example", "EURUSD 1.10", "1.10", 1),
            Some(HASH_A),
            at(11),
        )
        .unwrap();
    assert!(
        copy.corroborates.is_empty(),
        "a copy corroborated: {copy:?}"
    );

    let (independent, _) = fabric
        .admit(
            claim(
                "c",
                "https://fed.example",
                "dollar euro at one ten",
                "1.10",
                2,
            ),
            Some(HASH_B),
            at(12),
        )
        .unwrap();
    assert_eq!(
        independent.corroborates,
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(fabric.graph().edges().any(|(from, edge, to)| {
        *from == ProvenanceNode::Claim("c".into())
            && *edge == ProvenanceEdge::Corroborates
            && *to == ProvenanceNode::Claim("a".into())
    }));
    assert_eq!(fabric.held().last().unwrap().record.corroborates.len(), 2);
}

#[test]
fn ten_copies_of_one_press_release_count_as_one_origin() {
    let mut fabric = EvidenceFabric::default();
    let mut ids = Vec::new();
    for i in 0..10 {
        let id = format!("copy{i}");
        // Each outlet re-bylines and re-punctuates the same text.
        let text = if i % 2 == 0 {
            "Acme beats estimates!"
        } else {
            "ACME beats  estimates."
        };
        fabric
            .admit(
                claim(&id, &format!("https://outlet{i}.example"), text, "beat", i),
                None,
                at(20),
            )
            .unwrap();
        ids.push(id);
    }
    fabric
        .admit(
            claim(
                "own",
                "https://acme.example",
                "Acme reports record revenue",
                "beat",
                3,
            ),
            None,
            at(20),
        )
        .unwrap();
    assert_eq!(fabric.len(), 11, "premise: all eleven claims are held");
    assert_eq!(
        fabric.independent_origins(ids.iter().map(String::as_str)),
        1
    );
    let mut with_own: Vec<&str> = ids.iter().map(String::as_str).collect();
    with_own.push("own");
    assert_eq!(fabric.independent_origins(with_own), 2);
}

#[test]
fn disagreeing_claims_are_kept_as_competing_hypotheses_until_an_authentic_claim_resolves_them() {
    let mut fabric = EvidenceFabric::default().trusting_registry("ecb-registry");
    fabric
        .admit(
            claim("x", "https://rumour.example", "1.20", "1.20", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    let (contra, outcome) = fabric
        .admit(
            claim("y", "https://other.example", "1.10", "1.10", 1),
            Some(HASH_B),
            at(11),
        )
        .unwrap();
    assert_eq!(outcome, KeyOutcome::BecameContested);
    assert_eq!(contra.contradicts, vec!["x".to_string()]);
    let contest = fabric.contests().values().next().unwrap();
    assert!(contest.is_contested());
    assert_eq!(
        contest.hypotheses.len(),
        2,
        "neither hypothesis was deleted"
    );

    let official = claim("z", "https://ecb.example", "1.10 official", "1.10", 2)
        .with_authenticity_signal(AuthenticitySignal::IdentityRegistered {
            registry: "ecb-registry".into(),
        })
        .with_authenticity_signal(AuthenticitySignal::HashVerified {
            algorithm: "sha256".into(),
            hash: HASH_B.into(),
        });
    let (resolving, _) = fabric.admit(official, Some(HASH_B), at(12)).unwrap();
    assert!(resolving.resolved.is_some(), "{resolving:?}");
    let contest = fabric.contests().values().next().unwrap();
    assert_eq!(contest.resolution.as_ref().unwrap().value, "1.10");
    assert_eq!(contest.resolution.as_ref().unwrap().resolved_by, "z");
    assert_eq!(
        contest.hypotheses.len(),
        2,
        "the losing hypothesis stays on the record"
    );
    assert_eq!(
        fabric
            .source_history("https://rumour.example")
            .unwrap()
            .contradicted,
        1
    );
    assert_eq!(
        fabric
            .source_history("https://other.example")
            .unwrap()
            .corroborated,
        1
    );
}

#[test]
fn a_claim_ingested_before_it_was_published_is_inadmissible_and_weighs_nothing() {
    let mut fabric = EvidenceFabric::default();
    let mut early = claim("e", "https://src.example", "s", "1", 100);
    early.ingestion_timestamp = at(50);
    let (verdict, _) = fabric.admit(early, None, at(100)).unwrap();
    assert!(matches!(
        verdict.temporal,
        TemporalConsistency::ViolatesPhysicalTimeline { .. }
    ));
    assert!(!verdict.admissible);
    assert_eq!(verdict.belief_weight, 0.0);

    let (sound, _) = fabric
        .admit(
            claim("f", "https://src.example", "s2", "1", 0),
            None,
            at(100),
        )
        .unwrap();
    assert!(sound.admissible, "premise: a sound claim is admissible");
    assert!(sound.belief_weight > 0.0);
}

#[test]
fn a_hash_is_credited_only_when_it_matches_the_fetched_document_and_a_mismatch_is_spoofing() {
    let mut fabric = EvidenceFabric::default();
    let signal = |hash: &str| AuthenticitySignal::HashVerified {
        algorithm: "sha256".into(),
        hash: hash.into(),
    };
    let (matched, _) = fabric
        .admit(
            claim("m", "https://s.example", "m", "1", 0).with_authenticity_signal(signal(HASH_A)),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    let (bare, _) = fabric
        .admit(
            claim("n", "https://s.example", "n", "1", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert!(matched.authenticity.score > bare.authenticity.score);
    assert!(
        matched.belief_weight > bare.belief_weight,
        "authenticity moves the belief weight"
    );

    let (forged, _) = fabric
        .admit(
            claim("o", "https://s.example", "o", "1", 0).with_authenticity_signal(signal(HASH_B)),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert!(forged.authenticity.spoofed);
    assert!(forged.authenticity.credited.is_empty());
}

#[test]
fn a_tls_domain_is_credited_for_the_sources_host_and_refused_as_spoofing_otherwise() {
    let mut fabric = EvidenceFabric::default();
    let tls = |domain: &str| AuthenticitySignal::TlsVerified {
        domain: domain.into(),
    };
    let (own, _) = fabric
        .admit(
            claim("t1", "https://data.ecb.example/rates", "t1", "1", 0)
                .with_authenticity_signal(tls("ecb.example")),
            None,
            at(10),
        )
        .unwrap();
    assert_eq!(own.authenticity.credited.len(), 1);
    assert!(!own.authenticity.spoofed);

    let (other, _) = fabric
        .admit(
            claim("t2", "https://data.ecb.example/rates", "t2", "1", 0)
                .with_authenticity_signal(tls("evil.example")),
            None,
            at(10),
        )
        .unwrap();
    assert!(other.authenticity.spoofed);
}

#[test]
fn a_signature_claim_is_recorded_and_never_credited() {
    let mut fabric = EvidenceFabric::default();
    let (verdict, _) = fabric
        .admit(
            claim("sig", "https://s.example", "sig", "1", 0).with_authenticity_signal(
                AuthenticitySignal::SignatureVerified {
                    signer: "issuer".into(),
                },
            ),
            None,
            at(10),
        )
        .unwrap();
    assert!(verdict.authenticity.credited.is_empty());
    assert_eq!(verdict.authenticity.refused.len(), 1);
    assert_eq!(
        verdict.authenticity.score,
        qip_world_model::fabric::UNSIGNED_AUTHENTICITY
    );
}

fn positioned(id: &str, source: &str, location: &str, claimed: i64) -> EvidenceRecord {
    EvidenceRecord::new(
        id.into(),
        source.into(),
        at(claimed),
        at(claimed + 1),
        "ais".into(),
        "commercial".into(),
        format!("vessel at {location}"),
        0.9,
    )
    .unwrap()
    .with_claimed_at(at(claimed))
    .with_location(location.into())
    .with_metadata(META_SUBJECT.into(), "IMO1234567".into())
}

#[test]
fn a_vessel_reported_on_two_continents_an_hour_apart_violates_routing() {
    let mut fabric = EvidenceFabric::default();
    let (first, _) = fabric
        .admit(positioned("p1", "ais-a", "geo:51.95,4.14", 0), None, at(10))
        .unwrap();
    assert_eq!(
        first.geographic,
        GeographicConsistency::Unverifiable,
        "no prior position to compare"
    );
    // Thirty kilometres in an hour is a ship; sound.
    let (near, _) = fabric
        .admit(
            positioned("p2", "ais-a", "geo:52.2,4.3", 3_600),
            None,
            at(3_700),
        )
        .unwrap();
    assert_eq!(near.geographic, GeographicConsistency::Consistent);
    // Singapore an hour after Rotterdam is not.
    let (far, _) = fabric
        .admit(
            positioned("p3", "ais-b", "geo:1.26,103.82", 7_200),
            None,
            at(7_300),
        )
        .unwrap();
    assert_eq!(far.geographic, GeographicConsistency::ViolatesRouting);
}

#[test]
fn two_sources_placing_one_subject_apart_at_one_instant_contradict_and_one_source_conflicts_with_itself()
 {
    let mut fabric = EvidenceFabric::default();
    fabric
        .admit(positioned("q1", "ais-a", "geo:51.95,4.14", 0), None, at(10))
        .unwrap();
    let (other, _) = fabric
        .admit(positioned("q2", "ais-b", "geo:53.55,9.99", 0), None, at(10))
        .unwrap();
    assert!(matches!(
        other.geographic,
        GeographicConsistency::ContradicsLocation { .. }
    ));
    let (same, _) = fabric
        .admit(positioned("q3", "ais-a", "geo:40.0,-74.0", 0), None, at(10))
        .unwrap();
    assert_eq!(
        same.geographic,
        GeographicConsistency::InternalGeographicConflict
    );
}

#[test]
fn a_claim_whose_jurisdiction_is_not_its_country_violates_jurisdiction() {
    let record = claim("j", "https://s.example", "j", "1", 0)
        .with_metadata(META_JURISDICTION.into(), "DE".into())
        .with_metadata(META_COUNTRY.into(), "FR".into());
    let none: [EvidenceRecord; 0] = [];
    assert_eq!(
        record.check_geographic_consistency(&none),
        GeographicConsistency::ViolatesJurisdiction
    );
    let agreeing = claim("k", "https://s.example", "k", "1", 0)
        .with_metadata(META_JURISDICTION.into(), "FR".into())
        .with_metadata(META_COUNTRY.into(), "fr".into());
    assert_eq!(
        agreeing.check_geographic_consistency(&none),
        GeographicConsistency::Consistent
    );
}

#[test]
fn the_fabric_holds_no_more_than_its_capacity_and_forgets_the_evicted_claims_edges() {
    let mut fabric = EvidenceFabric::with_capacity(3, 8).unwrap();
    for i in 0..5 {
        fabric
            .admit(
                claim(
                    &format!("c{i}"),
                    "https://s.example",
                    &format!("s{i}"),
                    "1",
                    i,
                ),
                None,
                at(10),
            )
            .unwrap();
    }
    assert_eq!(fabric.len(), 3);
    assert!(fabric.get("c0").is_none());
    assert!(fabric.graph().edges().all(|(from, _, to)| {
        *from != ProvenanceNode::Claim("c0".into()) && *to != ProvenanceNode::Claim("c0".into())
    }));
    assert!(EvidenceFabric::with_capacity(0, 1).is_err());
}

#[test]
fn a_coordinated_campaign_of_identical_text_from_distinct_fetches_is_detected() {
    let mut fabric = EvidenceFabric::default();
    let hashes = [
        HASH_A,
        HASH_B,
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    ];
    for (i, hash) in hashes.iter().enumerate() {
        fabric
            .admit(
                claim(
                    &format!("k{i}"),
                    &format!("https://site{i}.example"),
                    "Acme CEO arrested",
                    "x",
                    i as i64,
                ),
                Some(hash),
                at(60),
            )
            .unwrap();
    }
    assert_eq!(fabric.len(), 3, "premise: three claims from three sources");
    let findings = detect_poisoning(&fabric, at(120), &PoisoningThresholds::default());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].class, PoisoningClass::CoordinatedManipulation);
    assert_eq!(findings[0].sources.len(), 3);
}

#[test]
fn a_clean_episode_of_independent_agreeing_sources_produces_no_findings() {
    let mut fabric = EvidenceFabric::default();
    for (i, text) in ["rate 1.10", "euro at one ten", "EURUSD fixing 1.10"]
        .iter()
        .enumerate()
    {
        fabric
            .admit(
                claim(
                    &format!("ok{i}"),
                    &format!("https://s{i}.example"),
                    text,
                    "1.10",
                    i as i64,
                ),
                None,
                at(60),
            )
            .unwrap();
    }
    assert_eq!(fabric.len(), 3, "premise: the fabric is not empty");
    assert!(detect_poisoning(&fabric, at(120), &PoisoningThresholds::default()).is_empty());
}

#[test]
fn a_source_repeatedly_contradicted_by_resolutions_is_flagged_as_poisoned() {
    let mut fabric = EvidenceFabric::default().trusting_registry("reg");
    for round in 0..3_i64 {
        let subject = format!("SUBJ{round}");
        let mut bad = claim(
            &format!("bad{round}"),
            "https://liar.example",
            &format!("lie {round}"),
            "up",
            round,
        );
        bad.metadata.insert(META_SUBJECT.into(), subject.clone());
        fabric.admit(bad, None, at(100 + round)).unwrap();
        let mut good = claim(
            &format!("good{round}"),
            "https://ecb.example",
            &format!("truth {round}"),
            "down",
            round,
        )
        .with_authenticity_signal(AuthenticitySignal::IdentityRegistered {
            registry: "reg".into(),
        })
        .with_authenticity_signal(AuthenticitySignal::HashVerified {
            algorithm: "sha256".into(),
            hash: HASH_A.into(),
        });
        good.metadata.insert(META_SUBJECT.into(), subject);
        fabric.admit(good, Some(HASH_A), at(100 + round)).unwrap();
    }
    assert_eq!(
        fabric
            .source_history("https://liar.example")
            .unwrap()
            .contradicted,
        3
    );
    let findings = detect_poisoning(&fabric, at(200), &PoisoningThresholds::default());
    assert!(
        findings
            .iter()
            .any(|f| f.class == PoisoningClass::SourcePoisoning
                && f.sources.contains("https://liar.example")),
        "{findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.sources.contains("https://ecb.example"))
    );
}

#[test]
fn a_source_flooding_the_fabric_against_its_peers_is_spam_and_repeated_impossible_timelines_are_synthetic()
 {
    let mut fabric = EvidenceFabric::default();
    for i in 0..60 {
        fabric
            .admit(
                claim(
                    &format!("f{i}"),
                    "https://flood.example",
                    &format!("noise {i}"),
                    "1",
                    0,
                ),
                None,
                at(10),
            )
            .unwrap();
    }
    for p in 0..3 {
        fabric
            .admit(
                claim(
                    &format!("p{p}"),
                    &format!("https://peer{p}.example"),
                    &format!("peer {p}"),
                    "1",
                    0,
                ),
                None,
                at(10),
            )
            .unwrap();
    }
    for i in 0..2 {
        let mut fake = claim(
            &format!("syn{i}"),
            "https://fake.example",
            &format!("fake {i}"),
            "1",
            500,
        );
        fake.ingestion_timestamp = at(400);
        fabric.admit(fake, None, at(10)).unwrap();
    }
    let findings = detect_poisoning(&fabric, at(20), &PoisoningThresholds::default());
    assert!(findings.iter().any(|f| f.class == PoisoningClass::Spam && f.sources.contains("https://flood.example")), "{findings:?}");
    assert!(
        findings
            .iter()
            .any(|f| f.class == PoisoningClass::SyntheticContent
                && f.sources.contains("https://fake.example")),
        "{findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.sources.iter().any(|s| s.contains("peer")))
    );
    let _ = Duration::ZERO;
}
