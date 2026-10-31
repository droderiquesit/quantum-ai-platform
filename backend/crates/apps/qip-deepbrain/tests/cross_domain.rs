//! R12-EXPAND-6031: a cross-domain chain the knowledge graph records reaches
//! the deep brain's research loop as a causal gap carrying the chain's
//! lineage, and a chain the causal graph already claims end to end does not.
//!
//! The failure prevented: the world model could synthesise the chain and
//! nothing in the loop asked it to, so no research was ever queued on a
//! transmission path the platform had recorded the facts for. The knowledge
//! enters through `Platform::absorb_knowledge`, the one door a deployed
//! process has for it, which the assessment row records as having no
//! production caller yet.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic_in_result_fn)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::expansion::GapClass;
use qip_core::{Context, Duration, Timestamp};
use qip_deepbrain::discovery::{DiscoveryConfig, DiscoveryDesk};
use qip_deepbrain::research::{ResearchConfig, ResearchLoop};
use qip_expansion::detectors::Origin;
use qip_expansion::gap::RaisedGap;
use qip_financial::universe::Universe;
use qip_kernel::evidence_intake::KnowledgeRelation;
use qip_kernel::{Platform, PlatformConfig};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::MemoryBlobStore;
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::graph::{EntityKind, Node};
use qip_world_model::relationship::{Relationship, RelationshipKind};
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

fn unset(_: &str) -> Option<String> {
    None
}

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

fn queued_gaps(loop_: &ResearchLoop) -> Vec<RaisedGap> {
    loop_
        .desk()
        .queue()
        .ranked()
        .iter()
        .filter_map(|entry| entry.origin().cloned())
        .collect()
}

fn relation(from: &str, to: &str, kind: RelationshipKind, source: &str) -> KnowledgeRelation {
    KnowledgeRelation {
        relationship: Relationship::new(from, to, kind, 1.0, source),
        valid_from: at(0),
        confidence: 0.9,
    }
}

/// Mutation: delete the `cross_domain_links` call in `sweep_over` (leave the
/// synthesis count) — `round.cross_domain_hypotheses` is still 2 and no gap
/// names the chain, so the `len() == 1` assertion fails; or make
/// `absorb_knowledge` skip `relate` — the premise on the graph fails.
#[test]
fn a_recorded_cross_domain_chain_reaches_the_sweep_as_a_causal_gap_and_a_claimed_one_does_not() {
    let mut platform = platform();
    let intake = platform
        .absorb_knowledge(
            vec![
                Node::entity("ent-kestrel", EntityKind::Company, "Kestrel", at(0)),
                Node::entity("ind-mining", EntityKind::Industry, "Mining", at(0)),
                Node::entity("cmdty-copper", EntityKind::Commodity, "Copper", at(0)),
                Node::entity("ctry-gb", EntityKind::Country, "United Kingdom", at(0)),
                Node::entity("ccy-gbp", EntityKind::Currency, "Sterling", at(0)),
            ],
            vec![
                relation(
                    "ent-kestrel",
                    "ind-mining",
                    RelationshipKind::InIndustry,
                    "reference",
                ),
                relation(
                    "ind-mining",
                    "cmdty-copper",
                    RelationshipKind::ProducesCommodity,
                    "research",
                ),
                relation(
                    "ent-kestrel",
                    "ctry-gb",
                    RelationshipKind::DomiciledIn,
                    "reference",
                ),
                relation(
                    "ctry-gb",
                    "ccy-gbp",
                    RelationshipKind::IssuesCurrency,
                    "reference",
                ),
            ],
            at(1),
        )
        .unwrap();
    assert_eq!((intake.nodes_added, intake.relations), (5, 4), "{intake:?}");
    assert_eq!(
        platform
            .world()
            .graph()
            .neighbours("ent-kestrel", None, at(10), at(10))
            .len(),
        2,
        "premise: the graph holds Kestrel's two edges"
    );
    // The causal graph already claims the currency chain end to end.
    for (cause, effect) in [("ent-kestrel", "ctry-gb"), ("ctry-gb", "ccy-gbp")] {
        platform
            .claim_causal(
                CausalEdge::new(
                    cause,
                    effect,
                    Mechanism::CurrencyTranslation,
                    0.4,
                    Duration::from_days(30),
                    at(0),
                )
                .unwrap()
                .with_confidence(0.8)
                .unwrap(),
            )
            .unwrap();
    }

    let mut loop_ = deployed();
    let (sources, jurisdictions) = deployed_bounds();
    let round = loop_
        .sweep(&platform, at(60), &sources, &jurisdictions)
        .unwrap();
    assert_eq!(round.cross_domain_hypotheses, 2, "{round:?}");
    assert_eq!(round.cross_domain_raised, 1, "{round:?}");
    assert!(
        round
            .describe()
            .contains("cross-domain chains 2 (1 raised)"),
        "{}",
        round.describe()
    );

    let gaps = queued_gaps(&loop_);
    let chains: Vec<&RaisedGap> = gaps
        .iter()
        .filter(|g| {
            g.signal.evidence.first().map(String::as_str)
                == Some(Origin::CrossDomainSynthesis.as_str())
        })
        .collect();
    assert_eq!(
        chains.len(),
        1,
        "the unclaimed chain is raised and the claimed one is not: {gaps:?}"
    );
    let copper = chains[0];
    assert_eq!(copper.signal.gap_class, GapClass::Causal);
    assert!(
        copper
            .signal
            .evidence
            .iter()
            .any(|e| e == "path:ent-kestrel>ind-mining>cmdty-copper"),
        "{:?}",
        copper.signal.evidence
    );
    assert!(
        copper
            .signal
            .evidence
            .iter()
            .any(|e| e == "source:research")
            && copper
                .signal
                .evidence
                .iter()
                .any(|e| e == "source:reference"),
        "edge lineage is carried: {:?}",
        copper.signal.evidence
    );
    assert!(
        !gaps
            .iter()
            .any(|g| g.signal.evidence.iter().any(|e| e.contains("ccy-gbp"))),
        "a chain the causal graph claims end to end was raised: {gaps:?}"
    );
}
