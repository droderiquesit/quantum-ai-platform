//! Contested evidence reaches the deep brain's research loop as two gaps,
//! and the kernel's knowledge deltas reach it off the log
//! (R12-EXPAND-6011, R12-EXPAND-6039; ADR 0151 decision 12 as amended).
//!
//! The failure these prevent is the one the assessments recorded for two
//! years of rows: the world fabric held contests between sources, the kernel
//! journaled a `KnowledgeDelta` naming each conflict, and the sweep that
//! decides what to research read neither — "contradictory evidence" was a
//! split between two of the deep brain's own price detectors, and a
//! disagreement between two *sources* about a *fact* raised no research at
//! all. Each test builds the loop as `main.rs` does (`ResearchConfig` from an
//! empty environment, the discovery desk's bounds) and asserts its premise
//! first, so an empty fabric cannot pass for a settled one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::expansion::GapClass;
use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::discovery::{DiscoveryConfig, DiscoveryDesk};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop};
use qip_expansion::gap::RaisedGap;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::MemoryBlobStore;
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
use qip_world_model::{EvidenceFabric, EvidenceRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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

/// The environment a deployment that set nothing hands `main.rs`.
fn unset(_: &str) -> Option<String> {
    None
}

/// The research loop as `main.rs` builds it, over a coverage map naming no
/// class, so the coverage detector raises nothing and every gap below is one
/// of the two under test.
fn deployed() -> ResearchLoop {
    ResearchLoop::new(
        ResearchConfig::from_lookup(&unset).unwrap(),
        BTreeMap::new(),
    )
    .unwrap()
    .with_package_store(Arc::new(MemoryBlobStore::new()))
}

fn deployed_bounds() -> (BTreeSet<String>, BTreeSet<String>) {
    let desk = DiscoveryDesk::new(DiscoveryConfig::from_lookup(&unset).unwrap(), vec![]);
    (
        desk.licensed_sources().clone(),
        desk.eligible_jurisdictions().clone(),
    )
}

/// One claim as a connector runtime forwards it: the record, its body hash
/// and the verdict the connector's own private fabric reached.
fn checked(id: &str, source: &str, subject: &str, value: &str, hash: char) -> CheckedEvidence {
    let hash: String = std::iter::repeat_n(hash, 64).collect();
    let record = EvidenceRecord::new(
        id.to_string(),
        source.to_string(),
        at(10),
        at(15),
        "connector:test/1.0".to_string(),
        "commercial".to_string(),
        format!("{subject} rate {value}"),
        0.9,
    )
    .unwrap()
    .with_claimed_at(at(0))
    .with_metadata(META_SUBJECT.into(), subject.into())
    .with_metadata(META_ATTRIBUTE.into(), "rate".into())
    .with_metadata(META_VALUE.into(), value.into());
    let (verdict, _) = EvidenceFabric::default()
        .admit(record.clone(), Some(&hash), at(20))
        .unwrap();
    CheckedEvidence {
        record,
        document_hash: hash,
        verdict,
    }
}

/// Two sources disagreeing about `subject`'s rate.
fn contest(subject: &str, ids: (&str, &str), hashes: (char, char)) -> Vec<CheckedEvidence> {
    vec![
        checked(ids.0, "https://a.example", subject, "1.10", hashes.0),
        checked(ids.1, "https://b.example", subject, "1.20", hashes.1),
    ]
}

fn queued_gaps(loop_: &ResearchLoop) -> Vec<RaisedGap> {
    loop_
        .desk()
        .queue()
        .ranked()
        .iter()
        .filter_map(|entry| entry.origin().cloned())
        .collect()
}

/// A contest the fabric holds raises a data gap citing both claims, and a
/// contested subject the causal graph holds no live claim into raises a
/// causal gap beside it; a contested subject the graph explains raises the
/// data gap only. Mutation: delete the `contested_evidence` call in
/// `sweep_over` — no data gap is queued and the first class assertion fails;
/// delete the `missing_causal_links` call — `causal.len()` is 0.
#[test]
fn a_contest_in_the_world_fabric_raises_a_data_gap_and_an_unexplained_subject_a_causal_gap_beside_it()
 {
    let mut platform = platform();
    assert_eq!(platform.evidence_held(), 0, "premise: an empty fabric");
    let mut batch = contest("EURUSD", ("a1", "b1"), ('a', 'b'));
    batch.extend(contest("obj-A", ("c1", "d1"), ('c', 'd')));
    let intake = platform.absorb_evidence(batch, at(30));
    assert_eq!(intake.admitted, 4, "{intake:?}");
    assert_eq!(
        intake.contested, 2,
        "premise: two contests opened: {intake:?}"
    );
    // The causal graph explains obj-A, and nothing explains EURUSD.
    platform
        .claim_causal(
            CausalEdge::new(
                "rates",
                "obj-A",
                Mechanism::DiscountRate,
                0.5,
                Duration::from_days(90),
                at(0),
            )
            .unwrap()
            .with_confidence(0.8)
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        platform.causally_explained(["EURUSD", "obj-A"], at(60)),
        BTreeSet::from(["obj-A".to_string()]),
        "premise: the graph explains one of the two"
    );

    let mut loop_ = deployed();
    let (sources, jurisdictions) = deployed_bounds();
    let round = loop_
        .sweep(&platform, at(60), &sources, &jurisdictions)
        .unwrap();
    assert_eq!(round.contested_claims, 2, "{round:?}");
    assert_eq!(round.unexplained_subjects, 1, "{round:?}");

    let gaps = queued_gaps(&loop_);
    let data: Vec<&RaisedGap> = gaps
        .iter()
        .filter(|g| g.signal.gap_class == GapClass::Data)
        .collect();
    let causal: Vec<&RaisedGap> = gaps
        .iter()
        .filter(|g| g.signal.gap_class == GapClass::Causal)
        .collect();
    assert_eq!(data.len(), 2, "one data gap per contest: {gaps:?}");
    assert_eq!(
        causal.len(),
        1,
        "one causal gap, for the unexplained subject: {gaps:?}"
    );
    assert!(
        causal[0].signal.observation.contains("`EURUSD`"),
        "{:?}",
        causal[0].signal
    );
    let eurusd = data
        .iter()
        .find(|g| g.signal.observation.contains("`EURUSD`"))
        .unwrap_or_else(|| panic!("the EURUSD contest is a data gap: {data:?}"));
    for claim in ["claim:a1=1.10", "claim:b1=1.20"] {
        assert!(
            eurusd.signal.evidence.iter().any(|e| e == claim),
            "the gap cites the claim and its value: {:?}",
            eurusd.signal.evidence
        );
    }
}

/// The knowledge delta `absorb_evidence` journals on `world_model.updated`
/// is decoded by the sweep and its conflict is the lineage of the data gap:
/// the record a second brain would read, read. Mutation: make
/// `Platform::knowledge_deltas` return an empty vector (skip the decode) —
/// `round.knowledge_deltas` is 0 and the `delta:kd-b1` citation is absent.
#[test]
fn a_knowledge_delta_absorb_evidence_journals_reaches_the_deep_brain_sweep() {
    let mut platform = platform();
    assert!(
        platform.knowledge_deltas().unwrap().is_empty(),
        "premise: nothing journaled yet"
    );
    let intake = platform.absorb_evidence(contest("EURUSD", ("a1", "b1"), ('a', 'b')), at(30));
    assert_eq!(
        intake.knowledge_deltas, 2,
        "premise: two deltas journaled: {intake:?}"
    );

    let mut loop_ = deployed();
    let (sources, jurisdictions) = deployed_bounds();
    let round = loop_
        .sweep(&platform, at(60), &sources, &jurisdictions)
        .unwrap();
    assert_eq!(round.knowledge_deltas, 2, "{round:?}");
    assert_eq!(
        round.knowledge_conflicts, 1,
        "only the second claim's delta named a conflict: {round:?}"
    );
    let gaps = queued_gaps(&loop_);
    let data = gaps
        .iter()
        .find(|g| g.signal.gap_class == GapClass::Data)
        .unwrap_or_else(|| panic!("the contest is queued as a data gap: {gaps:?}"));
    assert!(
        data.signal.evidence.iter().any(|e| e == "delta:kd-b1"),
        "the gap cites the journaled delta that recorded the conflict: {:?}",
        data.signal.evidence
    );
    assert!(
        !data.signal.evidence.iter().any(|e| e == "delta:kd-a1"),
        "the uncontested first delta is not cited as a conflict: {:?}",
        data.signal.evidence
    );
}
