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
                    depth: None,
                    imbalance: None,
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

/// Mutation: build the prior's direction from the sign of the posterior
/// instead of the event's expected direction (or hand over the prior rather
/// than the posterior) — the direction or the equality assertion fails.
#[test]
fn the_fused_state_hands_reason_each_instruments_strongest_directional_posterior() -> Result<()> {
    let (graph, traces) = episode()?;
    let state = fuse(&graph, &traces, at(600), &config())?;
    let posterior = state.posteriors()["obj-ACME"];
    assert!(
        posterior > 0.9,
        "premise: the reaction confirmed the 0.9 prior, so the posterior is above it: {posterior}"
    );
    let priors = state.outside_priors();
    let acme: Vec<_> = priors
        .iter()
        .filter(|p| p.subject() == "obj-ACME")
        .collect();
    assert_eq!(acme.len(), 1, "one prior per instrument: {priors:?}");
    // The event expects a fall (-1). A hypothesis that the instrument will
    // rise is against that posterior and is narrowed by 1 - (2p - 1); one that
    // it will fall is for it and untouched.
    assert_eq!(acme[0].narrowing_for(-1.0), 1.0);
    let against = acme[0].narrowing_for(1.0);
    assert!(
        (against - (1.0 - (2.0 * posterior - 1.0))).abs() < 1e-12,
        "the prior carries the posterior {posterior}, not another figure: multiplier {against}"
    );

    // Either input removed changes what REASON is handed (R12-TICK-1002):
    // without the tick traces nothing measured the reaction, so there is no
    // belief update and no prior; without the world event there is nothing to
    // fuse. REASON is unnarrowed in both.
    let without_ticks = fuse(&graph, &[], at(600), &config())?;
    assert!(
        without_ticks
            .outside_priors()
            .iter()
            .all(|p| p.subject() != "obj-ACME"),
        "removing the tick input left a prior for REASON to read"
    );
    let without_world = fuse(&KnowledgeGraph::new(), &traces, at(600), &config())?;
    assert!(
        without_world.outside_priors().is_empty(),
        "ticks alone are not a belief about an event"
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

    // The price leaked: it fell 50 bp at 110, after the strike occurred
    // (100) and before the platform could know of it (120), then a further
    // 100 bp at 150. The reaction is measured from the mid at the knowable
    // instant, and the leak is reported as priced before known. In the
    // fixture above the mid is flat from 0 to 150, so a baseline taken when
    // the event occurred read the same 100 and the round-2 mutation audit
    // found that mutation survived; this half is what tells them apart.
    let leaked = MarketTrace {
        instrument: "obj-ACME".into(),
        venue: "XLON".into(),
        points: (0..=60)
            .map(|i| {
                let t = i * 10;
                TracePoint {
                    at: at(t),
                    mid: match t {
                        _ if t < 110 => 100.0,
                        _ if t < 150 => 99.5,
                        _ => 99.5 * 0.99,
                    },
                    spread_bps: 2.0,
                    depth: None,
                    imbalance: None,
                }
            })
            .collect(),
    };
    let mut traces = traces;
    traces.retain(|trace| trace.instrument != "obj-ACME");
    traces.push(leaked);
    let state = fuse(&graph, &traces, at(600), &config())?;
    let reaction = &state
        .events
        .iter()
        .find(|e| e.event == "evt-strike")
        .expect("fused")
        .reactions[0];
    assert!(
        (reaction.baseline_mid - 99.5).abs() < 1e-9,
        "the baseline is the mid when the event became knowable: {}",
        reaction.baseline_mid
    );
    assert!(
        (reaction.move_bps + 100.0).abs() < 1e-6,
        "the reaction after knowing is 100 bp: {}",
        reaction.move_bps
    );
    let leak = reaction
        .priced_before_known_bps
        .expect("a move between occurring and knowing is reported");
    assert!((leak + 50.0).abs() < 1e-6, "the leak is 50 bp: {leak}");
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

/// RM-P2-08: every fused belief names both halves of what it rests on — the
/// world event and the evidence that event carried, and the instrument trace,
/// on its venue, that the belief's reaction was measured from.
///
/// The failure this prevents is a belief nobody can audit. A posterior is
/// evidence only if a reader can say which event it was about, how good that
/// event's evidence was and which market it was measured in; without that it
/// is a number with a confidence attached. Three ways the join goes wrong are
/// held here, each by a fixture built to expose it:
///
/// * the prior read from somewhere other than the event's own evidence — the
///   fact linking the event to the company carries 0.9 and the event's
///   evidence is 0.7, so the two cannot be confused;
/// * a reaction measured from another instrument's trace — the two traced
///   listings move in opposite directions, from different levels, on
///   different venues, and their ids share a prefix (`obj-ACME`,
///   `obj-ACMEB`), so a lookup that matched by substring would cross-wire
///   them and is refused by the equality matching below;
/// * a belief for a listing nothing traded — it would cite a market the
///   fusion never read, so that listing must end with a prior and no belief.
///
/// Mutations: in `fuse`, take `evidence_quality` from the linking fact's
/// confidence instead of the event's attribute (the evidence assertion
/// fires); read every instrument's points from the first trace in the market
/// map instead of its own (the trace assertion fires on `obj-ACMEB`).
#[test]
fn a_world_belief_and_a_market_signal_fuse_into_one_belief_with_both_provenances() -> Result<()> {
    // The world: one event about one company with three listings.
    let mut graph = KnowledgeGraph::new();
    graph.add_node(
        Node::event("evt-strike", "evt-strike", at(100), at(120))
            .with_attribute("expected_direction", "down")
            .with_attribute("evidence_quality", "0.7"),
    )?;
    graph.add_node(Node::entity("acme", EntityKind::Company, "acme", at(0)))?;
    graph.assert_fact(Fact::new(
        Relationship::new(
            "evt-strike",
            "acme",
            RelationshipKind::ConcernsEntity,
            1.0,
            "wire:1",
        ),
        at(100),
        at(120),
        0.9,
    )?);
    let listings = [
        ("inst-acme", "obj-ACME"),
        ("inst-acmeb", "obj-ACMEB"),
        ("inst-acmec", "obj-ACMEC"),
    ];
    for (node, object) in listings {
        graph.add_node(
            Node::new(node, NodeKind::FinancialObject, node, at(0))
                .with_attribute("object_id", object),
        )?;
        graph.assert_fact(Fact::new(
            Relationship::new("acme", node, RelationshipKind::Issues, 1.0, "registry:1"),
            at(0),
            at(0),
            1.0,
        )?);
    }

    // The market: (instrument, venue, mid before the turn, mid after it).
    // `obj-ACMEC` is not here: nothing traded it. `obj-OTHR` is here and the
    // event never reaches it.
    let markets = [
        ("obj-ACME", "XLON", 100.0, 99.0),
        ("obj-ACMEB", "XPAR", 40.0, 40.4),
        ("obj-OTHR", "XETR", 7.0, 7.0),
    ];
    let traces: Vec<MarketTrace> = markets
        .iter()
        .map(|&(object, venue, before, after)| MarketTrace {
            venue: venue.into(),
            ..trace(object, before, after, 150)
        })
        .collect();

    let state = fuse(&graph, &traces, at(600), &config())?;

    // Premise: one event was fused, it reached all three listings — one of
    // which has no trace — and it moved at least one belief. Without the
    // untraced listing the "no market, no belief" half below would hold of
    // any fusion; without a belief the loop below would assert nothing.
    assert_eq!(
        state.events.len(),
        1,
        "premise: the one knowable event was fused"
    );
    let fused = &state.events[0];
    assert_eq!(
        fused
            .watch
            .instruments
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["obj-ACME", "obj-ACMEB", "obj-ACMEC"],
        "premise: the event reaches all three listings"
    );
    assert!(
        !fused.beliefs.is_empty(),
        "premise: the fusion moved no belief, so nothing below would be checked"
    );

    // The world half, common to every belief this event moved: the event
    // itself, at the instants it occurred and became knowable, and the
    // evidence it carried — not the confidence of the fact that linked it.
    assert_eq!(fused.event, "evt-strike");
    assert_eq!(fused.occurred_at, at(100));
    assert_eq!(fused.knowable_at, at(120));
    assert_eq!(
        fused.evidence_quality, 0.7,
        "the fused event cites evidence other than the event's own 0.7"
    );

    for belief in &fused.beliefs {
        let instrument = belief.instrument.as_str();
        let watched = fused.watch.instruments.get(instrument).unwrap_or_else(|| {
            panic!("the belief on {instrument} is on nothing the event reaches")
        });
        let (node, _) = listings
            .iter()
            .find(|(_, object)| *object == instrument)
            .unwrap_or_else(|| panic!("the belief on {instrument} is on no listing of acme"));
        // World: the path runs from the event's entity to this listing, and
        // the prior the belief moved from is the event's evidence carried
        // along that path.
        assert_eq!(
            watched.path,
            vec!["acme".to_string(), (*node).to_string()],
            "the belief on {instrument} does not cite the path from the event's entity"
        );
        assert!(
            (belief.prior - watched.exposure_confidence * fused.evidence_quality).abs() < 1e-12,
            "the belief on {instrument} moved from a prior of {} that is not the event's evidence \
             along its exposure ({} x {})",
            belief.prior,
            watched.exposure_confidence,
            fused.evidence_quality
        );
        // Market: exactly one reaction on this instrument, measured from this
        // instrument's own trace, on that trace's venue and no other.
        let &(_, venue, before, after) = markets
            .iter()
            .find(|(object, ..)| *object == instrument)
            .unwrap_or_else(|| {
                panic!("the belief on {instrument} cites no trace it could have been measured from")
            });
        assert_eq!(
            watched.venues,
            BTreeSet::from([venue.to_string()]),
            "the belief on {instrument} does not cite the venue its trace was recorded on"
        );
        let reactions: Vec<_> = fused
            .reactions
            .iter()
            .filter(|reaction| reaction.instrument == instrument)
            .collect();
        assert_eq!(
            reactions.len(),
            1,
            "the belief on {instrument} rests on {} measured reactions",
            reactions.len()
        );
        assert_eq!(
            (reactions[0].baseline_mid, reactions[0].latest_mid),
            (before, after),
            "the belief on {instrument} was measured from a trace other than its own"
        );
    }

    // Both traced listings were believed, and the listing nothing traded was
    // not: the world model reached it, so it has a prior, and no market was
    // read, so it has no reaction and no belief.
    assert_eq!(
        fused
            .beliefs
            .iter()
            .map(|belief| belief.instrument.as_str())
            .collect::<Vec<_>>(),
        ["obj-ACME", "obj-ACMEB"],
        "the beliefs are not exactly the two listings a market was read for"
    );
    assert!(
        fused
            .priors
            .iter()
            .any(|prior| prior.instrument == "obj-ACMEC"),
        "premise: the untraced listing has a prior"
    );
    assert!(
        fused
            .reactions
            .iter()
            .all(|reaction| reaction.instrument != "obj-ACMEC"),
        "a reaction was measured on a listing nothing traded"
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
            depth: None,
            imbalance: None,
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

/// A seeded in-tree LCG (the repository takes no `rand`): reproducible, so
/// a failing seed is a failing seed on every machine.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

/// `nothing_recorded_or_printed_after_the_fusion_instant_is_read` is one
/// example. A leak that needs a particular interleaving of recording and
/// tick times passes an example, so this draws 200 random episodes — events
/// recorded before and after the fusion instant, ticks on both sides of it —
/// and requires `fuse` over everything to equal `fuse` over only what was
/// knowable at `as_of`. The comparison fuses a graph and traces rebuilt
/// without the late records, so a late record that changes any output is a
/// difference rather than an absence. It also asserts its own premise: that
/// late records existed in most episodes and that some events were knowable,
/// so a generator that never produced either would fail here rather than
/// pass vacuously.
///
/// Mutations, each failing the equality: drop the `p.at <= as_of` filter on
/// trace points in `fuse`, and widen `events_at(as_of, as_of)` to
/// `events_at(as_of, Timestamp::MAX)` so an event recorded later is read.
#[test]
fn over_random_episodes_fusing_everything_equals_fusing_only_what_was_knowable() -> Result<()> {
    let mut rng = Lcg(0x5eed_f00d);
    let mut with_late_records = 0;
    let mut with_knowable_events = 0;
    for case in 0..200 {
        let as_of = at(150 + i64::try_from(rng.below(300)).unwrap_or(0));
        let count = 1 + rng.below(4);
        // (company, object, direction, quality, recorded, mid before/after, turn)
        let mut episodes = Vec::new();
        for n in 0..count {
            episodes.push((
                format!("co{n}"),
                format!("obj-CO{n}"),
                if rng.below(2) == 0 { "up" } else { "down" },
                ["0.9", "0.2", "0.6"][usize::try_from(rng.below(3)).unwrap_or(0)],
                at(110 + i64::try_from(rng.below(400)).unwrap_or(0)),
                50.0 + rng.below(50) as f64,
                1.0 + rng.below(10) as f64 / 10.0,
                i64::try_from(rng.below(60)).unwrap_or(0) * 10,
            ));
        }
        let build = |only_knowable: bool| -> Result<(KnowledgeGraph, Vec<MarketTrace>)> {
            let mut graph = KnowledgeGraph::new();
            let mut traces = Vec::new();
            for (company, object, direction, quality, recorded, before, factor, turn) in &episodes {
                if !only_knowable || *recorded <= as_of {
                    add_episode(
                        &mut graph,
                        &format!("evt-{company}"),
                        company,
                        object,
                        direction,
                        quality,
                        *recorded,
                    )?;
                }
                let mut path = trace(object, *before, before * factor, *turn);
                if only_knowable {
                    path.points.retain(|p| p.at <= as_of);
                }
                traces.push(path);
            }
            Ok((graph, traces))
        };
        let (graph, traces) = build(false)?;
        let (known_graph, known_traces) = build(true)?;
        if episodes.iter().any(|e| e.4 > as_of)
            && traces.iter().any(|t| t.points.iter().any(|p| p.at > as_of))
        {
            with_late_records += 1;
        }
        let everything = fuse(&graph, &traces, as_of, &config())?;
        let knowable = fuse(&known_graph, &known_traces, as_of, &config())?;
        if !knowable.events.is_empty() {
            with_knowable_events += 1;
        }
        assert_eq!(
            everything, knowable,
            "case {case}: a record knowable only after {as_of} changed the fusion"
        );
    }
    assert!(
        with_late_records >= 100,
        "premise: only {with_late_records} of 200 episodes held a late record"
    );
    assert!(
        with_knowable_events >= 50,
        "premise: only {with_knowable_events} of 200 episodes fused a knowable event"
    );
    Ok(())
}

/// Only the mid and the spread were fused: the five other things a market
/// says about a reaction (regime, liquidity, flow, dislocation, feasibility)
/// were thrown away on the way in, so a move on a book that had emptied read
/// the same as one on a book that had not. ACME's book thins from 100 to 40
/// and tips toward the ask as it falls 100 bps, while a second venue still
/// quotes it half a point higher; BETA's trace records no sizes and does not
/// move. The test asserts ACME's reaction carries each input at the value the
/// trace implies, and that BETA's missing sizes are absent rather than zero.
///
/// Mutations: compute `depth_ratio` from the baseline against itself, drop
/// the second venue from `venue_dislocation_bps`, and compare the move with
/// zero rather than the spread in `clears_spread` — each fails its own
/// assertion below.
#[test]
fn each_market_input_the_trace_records_appears_in_the_measured_reaction() -> Result<()> {
    let (graph, mut traces) = episode()?;
    let thinning = |t: i64| -> TracePoint {
        let fallen = t >= 150;
        TracePoint {
            at: at(t),
            mid: if fallen { 99.0 } else { 100.0 },
            spread_bps: 2.0,
            depth: Some(if fallen { 40.0 } else { 100.0 }),
            imbalance: Some(if fallen { -0.5 } else { 0.2 }),
        }
    };
    let acme = traces
        .iter_mut()
        .find(|t| t.instrument == "obj-ACME")
        .expect("premise: the episode traces ACME");
    acme.points = (0..=30).map(|i| thinning(i * 10)).collect();
    // DELT moves one basis point against a two basis point spread: a move,
    // and not one worth crossing for.
    let delt = traces
        .iter_mut()
        .find(|t| t.instrument == "obj-DELT")
        .expect("premise: the episode traces DELT");
    for p in &mut delt.points {
        if p.at >= at(150) {
            p.mid = 10.001;
        }
    }
    traces.push(MarketTrace {
        instrument: "obj-ACME".into(),
        venue: "XPAR".into(),
        points: vec![TracePoint {
            at: at(255),
            mid: 99.5,
            spread_bps: 2.0,
            depth: None,
            imbalance: None,
        }],
    });

    let state = fuse(&graph, &traces, at(300), &config())?;

    let reaction = |instrument: &str| {
        state
            .events
            .iter()
            .flat_map(|event| event.reactions.iter())
            .find(|r| r.instrument == instrument)
            .unwrap_or_else(|| panic!("no reaction measured for {instrument}"))
    };
    let acme = reaction("obj-ACME");
    assert_eq!(
        acme.depth_ratio,
        Some(0.4),
        "the book thinned from 100 to 40"
    );
    assert_eq!(acme.flow_imbalance, Some(-0.5));
    assert!(
        acme.realised_vol_bps.is_some_and(|v| v > 0.0),
        "{:?}",
        acme.realised_vol_bps
    );
    let dislocation = acme
        .venue_dislocation_bps
        .expect("two venues quote ACME at the fusion instant");
    assert!(
        (dislocation - 50.378).abs() < 0.01,
        "XLON 99.0 against XPAR 99.5 is about 50.4 bps, got {dislocation}"
    );
    assert!(acme.clears_spread, "a 100 bps move clears a 2 bps spread");

    let delt = reaction("obj-DELT");
    assert!(
        (delt.move_bps - 1.0).abs() < 1e-6,
        "premise: DELT moved one basis point, got {}",
        delt.move_bps
    );
    assert!(
        !delt.clears_spread,
        "a 1 bps move does not clear a 2 bps spread"
    );

    let beta = reaction("obj-BETA");
    assert_eq!(beta.depth_ratio, None, "no sizes were recorded for BETA");
    assert_eq!(beta.flow_imbalance, None);
    assert_eq!(beta.venue_dislocation_bps, None, "BETA trades on one venue");
    assert!(!beta.clears_spread, "BETA did not move");
    Ok(())
}
