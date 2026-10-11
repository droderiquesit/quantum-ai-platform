//! The time-aligned fusion of world and market intelligence over a recorded
//! episode (R12-TICK-2081–2086, with R12-TICK-2091 at the shipping seam).

#![allow(clippy::panic_in_result_fn, clippy::float_cmp)]

use qip_ai::registry::ModelCard;
use qip_core::error::Result;
use qip_core::{Duration, ModelId, Timestamp};
use qip_kernel::fusion::{FusionConfig, MarketTrace, Mismatch, TracePoint, fuse};
use qip_world_model::graph::{EntityKind, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use std::collections::BTreeSet;

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

/// An event about `company` that the platform recorded at `recorded`, and
/// the company's listed instrument trading as `object`.
fn add_episode(
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

/// A mid path: `before` until `turn`, then `after`.
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
                }
            })
            .collect(),
    }
}

fn episode() -> Result<(KnowledgeGraph, Vec<MarketTrace>)> {
    let mut graph = KnowledgeGraph::new();
    // Expected down, fell 100 bps after the platform learned of it.
    add_episode(
        &mut graph,
        "evt-strike",
        "acme",
        "obj-ACME",
        "down",
        "0.9",
        at(120),
    )?;
    // Expected up on weak evidence, and nothing happened.
    add_episode(
        &mut graph,
        "evt-rumour",
        "beta",
        "obj-BETA",
        "up",
        "0.2",
        at(120),
    )?;
    // Expected up, and the market fell instead.
    add_episode(
        &mut graph,
        "evt-merger",
        "gama",
        "obj-GAMA",
        "up",
        "0.9",
        at(120),
    )?;
    // Expected down on good evidence, and nothing happened.
    add_episode(
        &mut graph,
        "evt-guidance",
        "delt",
        "obj-DELT",
        "down",
        "0.9",
        at(120),
    )?;
    let traces = vec![
        trace("obj-ACME", 100.0, 99.0, 150),
        trace("obj-BETA", 50.0, 50.0, 150),
        trace("obj-GAMA", 20.0, 19.0, 150),
        trace("obj-DELT", 10.0, 10.0, 150),
    ];
    Ok((graph, traces))
}

/// Mutation: drop the `instruments_exposed_to` walk in `fuse` — the watch
/// graph is empty and there is no prior.
#[test]
fn a_world_event_yields_its_watched_instruments_venues_and_hypothesis_priors() -> Result<()> {
    let (graph, traces) = episode()?;
    let state = fuse(&graph, &traces, at(600), &config())?;
    let strike = state
        .events
        .iter()
        .find(|e| e.event == "evt-strike")
        .expect("fused");
    assert_eq!(strike.watch.entities, BTreeSet::from(["acme".to_string()]));
    let watched = &strike.watch.instruments["obj-ACME"];
    assert_eq!(watched.hops, 1);
    assert_eq!(
        watched.path,
        vec!["acme".to_string(), "inst-acme".to_string()]
    );
    assert!(watched.venues.contains("XLON"));
    let prior = &strike.priors[0];
    assert_eq!(prior.expected_direction, -1.0);
    assert!(
        (prior.probability - 0.9).abs() < 1e-12,
        "exposure 1.0 x evidence 0.9"
    );
    Ok(())
}

/// Mutation: measure the reaction from the mid when the event *occurred*
/// rather than when it was knowable, or skip the belief update — the
/// measured move or the posterior assertion fails.
#[test]
fn a_measured_tick_reaction_moves_the_belief_it_tests() -> Result<()> {
    let (graph, traces) = episode()?;
    let state = fuse(&graph, &traces, at(600), &config())?;
    let strike = state
        .events
        .iter()
        .find(|e| e.event == "evt-strike")
        .expect("fused");
    let reaction = &strike.reactions[0];
    assert!(reaction.reacted);
    assert!(
        (reaction.move_bps + 100.0).abs() < 1e-6,
        "a 100 bp fall: {}",
        reaction.move_bps
    );
    assert_eq!(
        reaction.first_move_after,
        Some(Duration::from_secs(30)),
        "learned at 120, moved at 150"
    );
    let belief = &strike.beliefs[0];
    assert_eq!(belief.direction_agreed, Some(true));
    assert!(
        belief.posterior > belief.prior,
        "{} -> {}",
        belief.prior,
        belief.posterior
    );
    let delt = state
        .events
        .iter()
        .find(|e| e.event == "evt-guidance")
        .expect("fused");
    assert!(
        delt.beliefs[0].posterior < delt.beliefs[0].prior,
        "no reaction lowers the belief"
    );
    assert!(
        state.posteriors().contains_key("obj-ACME"),
        "the fused state's consumer form"
    );
    Ok(())
}

/// Mutation: in `fuse`, push an `Investigation` for weak evidence too, or
/// never raise a `RelativeValueHypothesis` — the respective response is
/// missing.
#[test]
fn a_world_market_mismatch_raises_an_investigation_a_relative_value_hypothesis_or_an_evidence_challenge()
-> Result<()> {
    let (graph, traces) = episode()?;
    let state = fuse(&graph, &traces, at(600), &config())?;
    let of = |event: &str| -> Vec<Mismatch> {
        state
            .events
            .iter()
            .find(|e| e.event == event)
            .map(|e| e.mismatches.clone())
            .unwrap_or_default()
    };
    assert!(
        of("evt-strike").is_empty(),
        "premise: an agreeing reaction is no mismatch"
    );
    assert!(matches!(
        of("evt-guidance")[..],
        [Mismatch::Investigation { .. }]
    ));
    assert!(matches!(
        of("evt-rumour")[..],
        [Mismatch::EvidenceQualityChallenge { .. }]
    ));
    assert!(matches!(
        of("evt-merger")[..],
        [Mismatch::RelativeValueHypothesis { expected_direction, .. }] if expected_direction > 0.0
    ));
    // Before the reaction window has elapsed an absence is not yet a finding.
    let early = fuse(&graph, &traces, at(200), &config())?;
    assert!(
        early
            .events
            .iter()
            .find(|e| e.event == "evt-guidance")
            .is_some_and(|e| e.mismatches.is_empty())
    );
    Ok(())
}

/// Mutation: read events with `events_at(as_of, Timestamp::MAX)` or trace
/// points without the `p.at <= as_of` filter — a later fact leaks into an
/// earlier fusion.
#[test]
fn nothing_recorded_or_printed_after_the_fusion_instant_is_read() -> Result<()> {
    let (mut graph, mut traces) = episode()?;
    let before = fuse(&graph, &traces, at(300), &config())?;
    add_episode(
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
        });
    }
    let after = fuse(&graph, &traces, at(300), &config())?;
    assert_eq!(
        after, before,
        "a later event and a later tick change nothing at 300"
    );
    assert!(after.events.iter().all(|e| e.event != "evt-late"));
    Ok(())
}

/// Mutation: make `OpportunityMap::authorise_for` return `Ok(())` — the
/// map ships to a region the training licence does not cover.
#[test]
fn the_distilled_map_is_bounded_and_ships_only_where_the_training_entitlements_permit() -> Result<()>
{
    let (graph, traces) = episode()?;
    let state = fuse(&graph, &traces, at(600), &config())?;
    let map = state.opportunity_map(2, Duration::from_mins(5))?;
    assert_eq!(map.entries.len(), 2, "bounded to its capacity");
    assert!(
        map.entries
            .windows(2)
            .all(|w| w[0].score.abs() >= w[1].score.abs()),
        "ranked"
    );
    assert!(map.entries.iter().all(|e| e.expires_at == at(900)));
    let card = ModelCard::new(ModelId::from_string("m"), "tick-fill", "1", "desk", at(600))
        .with_data_entitlements(
            BTreeSet::from(["lse-level2".to_string()]),
            BTreeSet::from(["europe-west2".to_string()]),
        )?;
    map.authorise_for(&card, "europe-west2")?;
    assert!(map.authorise_for(&card, "us-east4").is_err());
    Ok(())
}
