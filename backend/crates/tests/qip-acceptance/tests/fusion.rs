//! World intelligence and market intelligence, fused as of one instant,
//! across the seam between `qip-world-model` (the graph) and `qip-kernel`
//! (`fusion::fuse`).
//!
//! `qip-kernel/tests/fusion.rs` proves `fuse` inside the kernel's own
//! crate; nothing there can see that the graph it is handed is the world
//! model's real `KnowledgeGraph` written through its public assert path.
//! This suite builds the graph only through `qip-world-model`'s API and the
//! traces only through the kernel's, then asks the three questions a
//! research desk asks of a fused record: did a mismatch surface, was the
//! answer bitemporally honest, and does a replay reproduce it byte for byte.
//!
//! The production caller is `qip-deepbrain`'s `tick_desk`, not
//! `Platform`'s UNDERSTAND stage; the register row records that.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_kernel::fusion::{FusionConfig, MarketTrace, Mismatch, TracePoint, fuse};
use qip_world_model::graph::{EntityKind, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};

fn at(s: i64) -> Timestamp {
    Timestamp::from_secs(1_791_676_800 + s)
}

fn config() -> FusionConfig {
    FusionConfig {
        max_hops: 2,
        reaction_threshold_bps: 20.0,
        reaction_window: Duration::from_secs(120),
        expect_reaction_above: 0.15,
        weak_evidence_below: 0.4,
        max_events: 16,
    }
}

fn add_event(
    graph: &mut KnowledgeGraph,
    event: &str,
    company: &str,
    object: &str,
    direction: &str,
    quality: &str,
    recorded: Timestamp,
) -> Result<()> {
    graph.add_node(
        Node::event(event, event, at(100), recorded)
            .with_attribute("expected_direction", direction)
            .with_attribute("evidence_quality", quality),
    )?;
    graph.add_node(Node::entity(company, EntityKind::Company, company, at(0)))?;
    let instrument = format!("inst-{company}");
    graph.add_node(
        Node::new(&instrument, NodeKind::FinancialObject, &instrument, at(0))
            .with_attribute("object_id", object),
    )?;
    graph.assert_fact(Fact::new(
        Relationship::new(
            event,
            company,
            RelationshipKind::ConcernsEntity,
            1.0,
            "wire:1",
        ),
        at(100),
        recorded,
        0.9,
    )?);
    graph.assert_fact(Fact::new(
        Relationship::new(
            company,
            &instrument,
            RelationshipKind::Issues,
            1.0,
            "registry:1",
        ),
        at(0),
        at(0),
        1.0,
    )?);
    Ok(())
}

fn trace(object: &str, before: f64, after: f64, turn: i64) -> MarketTrace {
    MarketTrace {
        instrument: object.into(),
        venue: "XLON".into(),
        points: (0..=60)
            .map(|i| {
                let t = i * 10;
                TracePoint {
                    at: at(t),
                    mid: if t < turn { before } else { after },
                    spread_bps: 2.0,
                    depth: None,
                    imbalance: None,
                }
            })
            .collect(),
    }
}

/// Three events the market answers differently: one it ignored on sound
/// evidence, one it ignored on weak evidence, one it traded against.
fn episode() -> Result<(KnowledgeGraph, Vec<MarketTrace>)> {
    let mut graph = KnowledgeGraph::new();
    add_event(
        &mut graph,
        "evt-guidance",
        "delt",
        "obj-DELT",
        "down",
        "0.9",
        at(120),
    )?;
    add_event(
        &mut graph,
        "evt-rumour",
        "beta",
        "obj-BETA",
        "up",
        "0.2",
        at(120),
    )?;
    add_event(
        &mut graph,
        "evt-merger",
        "gama",
        "obj-GAMA",
        "up",
        "0.9",
        at(120),
    )?;
    let traces = vec![
        trace("obj-DELT", 10.0, 10.0, 150),
        trace("obj-BETA", 50.0, 50.0, 150),
        trace("obj-GAMA", 20.0, 19.0, 150),
    ];
    Ok((graph, traces))
}

#[test]
fn a_world_event_the_market_ignored_or_traded_against_surfaces_each_kind_of_mismatch() -> Result<()>
{
    let (graph, traces) = episode()?;
    assert_eq!(
        graph.nodes().filter(|n| n.kind == NodeKind::Event).count(),
        3,
        "premise: the graph holds the three events"
    );
    let fused = fuse(&graph, &traces, at(300), &config())?;
    assert_eq!(fused.events.len(), 3, "every knowable event is fused");
    let mismatches: Vec<&Mismatch> = fused.events.iter().flat_map(|e| &e.mismatches).collect();
    assert!(
        mismatches.iter().any(
            |m| matches!(m, Mismatch::Investigation { instrument, .. } if instrument == "obj-DELT")
        ),
        "sound evidence, no reaction: {mismatches:?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|m| matches!(m, Mismatch::EvidenceQualityChallenge { .. })),
        "weak evidence, no reaction: {mismatches:?}"
    );
    assert!(
        mismatches.iter().any(
            |m| matches!(m, Mismatch::RelativeValueHypothesis { instrument, .. } if instrument == "obj-GAMA")
        ),
        "the market moved against the expectation: {mismatches:?}"
    );
    Ok(())
}

#[test]
fn nothing_recorded_or_printed_after_the_fusion_instant_changes_the_fused_state() -> Result<()> {
    let (mut graph, mut traces) = episode()?;
    let before = fuse(&graph, &traces, at(300), &config())?;
    assert!(
        !before.events.is_empty(),
        "premise: there is a state to protect"
    );

    // A world event the platform only recorded at 400, and a tick printed at
    // 301: both are in the future of the instant asked about.
    add_event(
        &mut graph,
        "evt-late",
        "late",
        "obj-LATE",
        "up",
        "0.9",
        at(400),
    )?;
    for t in &mut traces {
        t.points.push(TracePoint {
            at: at(301),
            mid: 1.0,
            spread_bps: 900.0,
            depth: None,
            imbalance: None,
        });
    }
    let after = fuse(&graph, &traces, at(300), &config())?;
    assert_eq!(after.digest, before.digest);
    assert!(after.events.iter().all(|e| e.event != "evt-late"));

    // And the same graph asked at 500 does know it: the exclusion above is
    // the bitemporal filter, not an event the fuser cannot see.
    let later = fuse(&graph, &traces, at(500), &config())?;
    assert!(later.events.iter().any(|e| e.event == "evt-late"));
    Ok(())
}

#[test]
fn a_replay_of_the_same_inputs_reproduces_the_fused_digest() -> Result<()> {
    let (graph, traces) = episode()?;
    let first = fuse(&graph, &traces, at(300), &config())?;
    let second = fuse(&graph, &traces, at(300), &config())?;
    assert!(!first.digest.is_empty(), "premise: a digest was computed");
    assert_eq!(first, second);
    Ok(())
}
