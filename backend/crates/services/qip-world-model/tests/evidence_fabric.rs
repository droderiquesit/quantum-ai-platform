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
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

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
    // The official release's document hash is pinned by an independent
    // channel (a published checksum): a hash naming its own fetched body
    // earns nothing on its own (R12-EVID-2034).
    let mut fabric = EvidenceFabric::default()
        .trusting_registry("ecb-registry")
        .pinning_document_hash(HASH_C);
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
            hash: HASH_C.into(),
        });
    let (resolving, _) = fabric.admit(official, Some(HASH_C), at(12)).unwrap();
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

/// R12-EVID-2034's defect, as it shipped: every production claim carried a
/// hash of its fetched body and was handed that same hash to compare against,
/// so the comparison could not fail and credited every claim. The self-hash
/// must now earn nothing; only a hash an independent channel pinned does,
/// and a hash that disagrees with the body is still spoofing.
#[test]
fn a_hash_naming_its_own_document_credits_nothing_unless_independently_pinned_and_a_mismatch_is_spoofing()
 {
    let signal = |hash: &str| AuthenticitySignal::HashVerified {
        algorithm: "sha256".into(),
        hash: hash.into(),
    };
    let mut fabric = EvidenceFabric::default();
    let (bare, _) = fabric
        .admit(
            claim("n", "https://s.example", "n", "1", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    let (self_hashed, _) = fabric
        .admit(
            claim("m", "https://s.example", "m", "1", 0).with_authenticity_signal(signal(HASH_A)),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert_eq!(
        self_hashed.authenticity.refused.len(),
        1,
        "premise: the hash signal was carried and judged: {:?}",
        self_hashed.authenticity
    );
    assert!(
        self_hashed.authenticity.credited.is_empty(),
        "a hash compared with the body it was taken from was credited: {:?}",
        self_hashed.authenticity
    );
    assert_eq!(self_hashed.authenticity.score, bare.authenticity.score);
    assert!(!self_hashed.authenticity.spoofed);

    let mut pinned = EvidenceFabric::default().pinning_document_hash(HASH_A);
    let (vouched, _) = pinned
        .admit(
            claim("p", "https://s.example", "p", "1", 0).with_authenticity_signal(signal(HASH_A)),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert!(vouched.authenticity.score > bare.authenticity.score);
    assert!(
        vouched.belief_weight > bare.belief_weight,
        "authenticity moves the belief weight"
    );

    let (forged, _) = pinned
        .admit(
            claim("o", "https://s.example", "o", "1", 0).with_authenticity_signal(signal(HASH_B)),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert!(forged.authenticity.spoofed);
    assert!(forged.authenticity.credited.is_empty());
}

/// R12-EVID-2038 through R12-EVID-2034: with the self-hash credited no
/// longer, production authenticity has to come from somewhere a source
/// cannot supply for itself. Agreement from two independent origins at other
/// sources reaches the resolving bar; one does not, and neither does a
/// source repeating itself.
#[test]
fn a_contest_resolves_on_two_independent_corroborating_sources_and_not_on_one() {
    let mut fabric = EvidenceFabric::default();
    fabric
        .admit(
            claim("a", "https://a.example", "a says 1.10", "1.10", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    let (_, outcome) = fabric
        .admit(
            claim("b", "https://b.example", "b says 1.20", "1.20", 1),
            Some(HASH_B),
            at(11),
        )
        .unwrap();
    assert_eq!(outcome, KeyOutcome::BecameContested, "premise: a contest");

    // The contradicted source repeating itself from a fresh fetch is no
    // corroboration of anything.
    let (echo, _) = fabric
        .admit(
            claim("b2", "https://b.example", "b again 1.20", "1.20", 2),
            Some(HASH_D),
            at(12),
        )
        .unwrap();
    assert_eq!(echo.corroborates, vec!["b".to_string()], "premise: linked");
    assert_eq!(
        echo.authenticity.score,
        qip_world_model::fabric::UNSIGNED_AUTHENTICITY
    );
    assert!(echo.resolved.is_none());

    let (one, _) = fabric
        .admit(
            claim("c", "https://c.example", "c says 1.10", "1.10", 2),
            Some(HASH_C),
            at(12),
        )
        .unwrap();
    assert_eq!(one.corroborates, vec!["a".to_string()]);
    assert!(
        one.resolved.is_none(),
        "one corroborating origin resolved a contest: {:?}",
        one.authenticity
    );

    let (two, _) = fabric
        .admit(
            claim("d", "https://d.example", "d says 1.10", "1.10", 3),
            Some("eeee"),
            at(13),
        )
        .unwrap();
    assert!(two.authenticity.score >= qip_world_model::fabric::RESOLVING_AUTHENTICITY);
    assert!(two.resolved.is_some(), "{:?}", two.authenticity);
    let contest = fabric.contests().values().next().unwrap();
    let resolution = contest.resolution.as_ref().unwrap();
    assert_eq!(resolution.value, "1.10");
    assert_eq!(resolution.resolved_by, "d");
    assert_eq!(contest.hypotheses.len(), 2, "the losing side is kept");
    assert_eq!(
        fabric
            .source_history("https://b.example")
            .unwrap()
            .contradicted,
        2
    );
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
    let mut fabric = EvidenceFabric::default()
        .trusting_registry("reg")
        .pinning_document_hash(HASH_A);
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

/// R12-EVID-2035: what copies must not move is the *supporting weight* a
/// later claim earns, not merely the origin count `independent_origins`
/// reports — a fabric could count one origin and still credit each copy.
/// The same probe is admitted after one copy, after ten, and after one copy
/// plus a genuinely independent origin. Mutation: crediting corroboration
/// per corroborating claim instead of per independent origin raises the
/// ten-copy weight above the one-copy weight.
#[test]
fn ten_copies_leave_the_supporting_weight_unchanged_and_a_new_origin_raises_it() {
    fn probe_after(copies: usize, independent: bool) -> (f64, f64) {
        let mut fabric = EvidenceFabric::default();
        for i in 0..copies {
            let text = if i % 2 == 0 {
                "Acme beats estimates!"
            } else {
                "ACME beats  estimates."
            };
            fabric
                .admit(
                    claim(
                        &format!("copy{i}"),
                        &format!("https://outlet{i}.example"),
                        text,
                        "beat",
                        i as i64,
                    ),
                    None,
                    at(20),
                )
                .unwrap();
        }
        if independent {
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
        }
        let (verdict, _) = fabric
            .admit(
                claim(
                    "probe",
                    "https://desk.example",
                    "Analysts confirm the quarter came in ahead",
                    "beat",
                    15,
                ),
                None,
                at(20),
            )
            .unwrap();
        (verdict.belief_weight, verdict.authenticity.score)
    }

    let (one_copy, one_copy_auth) = probe_after(1, false);
    let (ten_copies, ten_copies_auth) = probe_after(10, false);
    let (new_origin, new_origin_auth) = probe_after(1, true);
    // Premise: one copy corroborates the probe at all, so "unchanged" is not
    // the trivial equality of two weights that both earned nothing.
    let (alone, _) = probe_after(0, false);
    assert!(one_copy > alone, "one copy {one_copy} vs alone {alone}");

    assert_eq!(ten_copies_auth, one_copy_auth);
    assert_eq!(ten_copies, one_copy);
    assert!(new_origin_auth > one_copy_auth);
    assert!(
        new_origin > one_copy,
        "new origin {new_origin} vs one copy {one_copy}"
    );
}

/// R12-EVID-2032: a corroboration or a contradiction is recorded on both
/// claims. It was written onto the incoming claim only, so the earlier one —
/// the claim a contest was opened on — read as uncontested for as long as it
/// was held. When one side is evicted, the survivor stops naming it.
/// Mutations: dropping the back-link loop leaves `a`'s lists empty; dropping
/// the eviction prune leaves `b` naming the evicted `a`.
#[test]
fn a_relation_is_recorded_on_both_claims_and_forgotten_on_both_when_one_is_evicted() {
    let mut fabric = EvidenceFabric::with_capacity(3, 16).unwrap();
    fabric
        .admit(
            claim("a", "https://a.example", "rate is 1.10", "1.10", 1),
            None,
            at(20),
        )
        .unwrap();
    fabric
        .admit(
            claim("b", "https://b.example", "the rate printed 1.10", "1.10", 2),
            None,
            at(20),
        )
        .unwrap();
    let (c, _) = fabric
        .admit(
            claim("c", "https://c.example", "rate came in at 1.20", "1.20", 3),
            None,
            at(20),
        )
        .unwrap();
    // Premise: the incoming claims named the earlier ones.
    assert_eq!(fabric.get("b").unwrap().record.corroborates, ["a"]);
    assert_eq!(c.contradicts.len(), 2, "{c:?}");

    let a = &fabric.get("a").unwrap().record;
    assert_eq!(a.corroborates, ["b"]);
    assert_eq!(a.contradicts, ["c"]);
    assert_eq!(fabric.get("b").unwrap().record.contradicts, ["c"]);

    fabric
        .admit(
            claim("d", "https://d.example", "rate is 9.99", "9.99", 4),
            None,
            at(20),
        )
        .unwrap();
    // Premise: the oldest claim was the one given up.
    assert!(fabric.get("a").is_none());
    let b = &fabric.get("b").unwrap().record;
    assert!(
        b.corroborates.is_empty(),
        "b still names the evicted a: {b:?}"
    );
    // `d` disagrees with `b` too, and is held, so it is named.
    assert_eq!(b.contradicts, ["c", "d"]);
}

/// R12-EVID-2038: competing hypotheses survive capacity eviction. The oldest
/// claim went first whatever it was, so a full fabric dropped one side of an
/// open contest and the later evidence that would have resolved it found
/// nothing to resolve. Now the oldest uncontested claim goes first.
/// Mutation: evicting the front regardless of contests loses `a` and the
/// contest with it.
#[test]
fn an_open_contest_survives_a_fabric_filled_past_capacity_with_unrelated_claims() {
    let mut fabric = EvidenceFabric::with_capacity(3, 16).unwrap();
    fabric
        .admit(
            claim("a", "https://a.example", "rate is 1.10", "1.10", 1),
            None,
            at(20),
        )
        .unwrap();
    fabric
        .admit(
            claim("b", "https://b.example", "rate is 1.20", "1.20", 2),
            None,
            at(20),
        )
        .unwrap();
    // Premise: one open contest of two hypotheses.
    let open = |f: &EvidenceFabric| {
        f.contests()
            .values()
            .filter(|c| c.is_contested())
            .map(|c| c.hypotheses.len())
            .collect::<Vec<_>>()
    };
    assert_eq!(open(&fabric), [2]);

    for (i, id) in ["x", "y", "z"].iter().enumerate() {
        let unrelated = claim(
            id,
            "https://u.example",
            &format!("gilt {id}"),
            "4.0",
            3 + i as i64,
        )
        .with_metadata(META_SUBJECT.into(), format!("GILT{i}"));
        fabric.admit(unrelated, None, at(20)).unwrap();
    }

    // Premise: the fabric is at capacity and gave claims up to stay there.
    assert_eq!(fabric.len(), 3);
    assert!(fabric.get("a").is_some() && fabric.get("b").is_some());
    assert_eq!(
        open(&fabric),
        [2],
        "a hypothesis of the open contest was evicted"
    );
}

fn journaled(id: &str, value: &str) -> qip_world_model::fabric::JournaledClaim {
    qip_world_model::fabric::JournaledClaim {
        record: claim(
            id,
            "https://ecb.example",
            &format!("EURUSD {value}"),
            value,
            0,
        ),
        document_hash: Some(HASH_A.to_string()),
        admitted_at: at(10),
    }
}

/// ADR 0152 §2. A rebuild replays onto a fabric configured like the writer's,
/// and onto nothing else: replaying onto a fabric that already holds claims
/// would let the journal corroborate itself, and skipping a journaled claim
/// the fabric refuses would rebuild a belief that quietly differs.
///
/// Mutation: delete the emptiness refusal at the top of `rebuild` — the
/// first `expect_err` fires. Mutation: swallow the per-claim error in
/// `rebuild` (`let _ = self.admit(..)`) — the duplicate-id `expect_err`
/// fires.
#[test]
fn a_rebuild_refuses_a_fabric_that_holds_claims_and_a_journal_the_fabric_refuses() {
    let mut occupied = EvidenceFabric::default();
    occupied
        .admit(
            claim("x", "https://ecb.example", "EURUSD 1.10", "1.10", 0),
            Some(HASH_A),
            at(10),
        )
        .unwrap();
    assert_eq!(
        occupied.len(),
        1,
        "premise: the fabric already holds a claim"
    );
    let refused = occupied
        .rebuild([journaled("a", "1.10")])
        .expect_err("rebuilt onto a held fabric");
    assert!(
        refused.message().contains("empty fabric"),
        "{}",
        refused.message()
    );

    let empty = EvidenceFabric::default();
    assert!(
        empty.clone().rebuild([journaled("a", "1.10")]).is_ok(),
        "premise: one journaled claim rebuilds"
    );
    let duplicate = empty
        .rebuild([journaled("a", "1.10"), journaled("a", "1.20")])
        .expect_err("a journal naming one claim twice was replayed");
    assert!(
        duplicate
            .message()
            .contains("journaled claim `a` is refused on replay"),
        "{}",
        duplicate.message()
    );
}
