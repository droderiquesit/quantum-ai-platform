//! The world model: bitemporality, causal propagation, point-in-time features.

use qip_core::testing::approx_eq;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_financial::intelligence::{
    EntityMention, FiscalPeriod, FundamentalUpdate, MacroObservation, NewsItem, NewsSource,
    Sentiment,
};
use qip_financial::quality::{DataQuality, Provenance};
use qip_market::bar::{Bar, Interval};
use qip_world_model::causal::{CausalEdge, CausalGraph, Mechanism, SupportingClaim};
use qip_world_model::features::{Feature, FeatureStore, FeatureValue};
use qip_world_model::graph::{Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::state::ChangeKind;
use qip_world_model::world::{WorldModel, seed_demo_world};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn context() -> Context {
    let (context, _clock) = Context::deterministic(now(), 7);
    context
}

fn days_ago(days: i64) -> Timestamp {
    now().saturating_sub(Duration::from_days(days))
}

// --- bitemporality ----------------------------------------------------------

#[test]
fn a_fact_is_invisible_before_it_was_known_even_if_it_was_already_true() {
    // The whole point of two timestamps. The supply relationship began in
    // January but the platform only learned of it in June; a decision made in
    // March must not see it.
    let began = Timestamp::from_civil(2026, 1, 15);
    let learned = Timestamp::from_civil(2026, 6, 1);
    let march = Timestamp::from_civil(2026, 3, 1);

    let fact = Fact::new(
        Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings"),
        began,
        learned,
    );

    assert!(!fact.holds(march, march), "not knowable in March");
    assert!(
        fact.holds(march, now()),
        "knowable now, and it was true in March"
    );
    assert!(fact.holds_now(now()));
}

#[test]
fn a_fact_outside_its_validity_window_does_not_hold() {
    let fact = Fact::new(
        Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings"),
        days_ago(100),
        days_ago(100),
    )
    .valid_until(days_ago(30));

    assert!(fact.holds(days_ago(50), now()), "inside the window");
    assert!(!fact.holds(days_ago(10), now()), "after the window closed");
    assert!(!fact.holds(days_ago(200), now()), "before it began");
}

#[test]
fn a_retracted_fact_is_hidden_going_forward_but_not_rewritten_backwards() {
    // A decision made while the fact was believed must remain explicable.
    let mut graph = KnowledgeGraph::new();
    graph.add_node(Node::new("a", NodeKind::Entity, "A", days_ago(100)));
    graph.add_node(Node::new("b", NodeKind::Entity, "B", days_ago(100)));
    let relationship = Relationship::new("a", "b", RelationshipKind::Supplies, 0.5, "filings");
    let key = relationship.key();
    graph.assert_fact(Fact::new(relationship, days_ago(100), days_ago(100)));

    assert_eq!(
        graph
            .neighbours("a", None, days_ago(50), days_ago(50))
            .len(),
        1
    );
    assert!(graph.retract(&key, days_ago(20)));

    assert_eq!(
        graph
            .neighbours("a", None, days_ago(50), days_ago(50))
            .len(),
        1,
        "what was believed at the time is unchanged"
    );
    assert_eq!(
        graph.neighbours("a", None, now(), now()).len(),
        0,
        "it is no longer believed"
    );
    assert!(!graph.retract("nonexistent", now()));
}

#[test]
fn an_inverse_edge_is_asserted_automatically() {
    let mut graph = KnowledgeGraph::new();
    graph.assert_fact(Fact::new(
        Relationship::new(
            "supplier",
            "buyer",
            RelationshipKind::Supplies,
            0.6,
            "filings",
        ),
        days_ago(10),
        days_ago(10),
    ));

    let forward = graph.neighbours("supplier", Some(RelationshipKind::Supplies), now(), now());
    assert_eq!(forward.len(), 1);
    let backward = graph.neighbours("buyer", Some(RelationshipKind::Customer), now(), now());
    assert_eq!(backward.len(), 1, "the customer edge is implied");

    assert_eq!(
        RelationshipKind::Supplies.inverse(),
        Some(RelationshipKind::Customer)
    );
    assert!(RelationshipKind::Competitor.is_symmetric());
}

// --- graph traversal --------------------------------------------------------

fn chain_graph() -> KnowledgeGraph {
    let mut graph = KnowledgeGraph::new();
    for id in ["kestrel", "northwind", "vantage", "meridian"] {
        graph.add_node(Node::new(id, NodeKind::Entity, id, days_ago(400)));
    }
    graph.assert_fact(Fact::new(
        Relationship::new(
            "kestrel",
            "northwind",
            RelationshipKind::Supplies,
            0.4,
            "filings",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new(
            "northwind",
            "vantage",
            RelationshipKind::Supplies,
            0.6,
            "filings",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph.assert_fact(Fact::new(
        Relationship::new(
            "northwind",
            "meridian",
            RelationshipKind::Competitor,
            0.2,
            "research",
        ),
        days_ago(400),
        days_ago(400),
    ));
    graph
}

#[test]
fn paths_are_found_shortest_first_and_carry_their_strength() {
    let graph = chain_graph();
    let paths = graph.paths_between("kestrel", "vantage", 4, now(), now());
    assert!(!paths.is_empty());
    assert_eq!(paths[0].nodes, vec!["kestrel", "northwind", "vantage"]);
    assert_eq!(paths[0].length(), 2);
    assert!(approx_eq(paths[0].strength, 0.4 * 0.6, 1e-9));
}

#[test]
fn path_search_is_bounded_and_avoids_cycles() {
    let graph = chain_graph();
    assert!(
        graph
            .paths_between("kestrel", "vantage", 1, now(), now())
            .is_empty(),
        "two hops cannot be found within one"
    );
    assert!(
        graph
            .paths_between("kestrel", "kestrel", 3, now(), now())
            .is_empty()
    );
    // Competitor edges are symmetric, so a naive search would loop forever.
    let paths = graph.paths_between("meridian", "vantage", 4, now(), now());
    for path in &paths {
        let mut sorted = path.nodes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), path.nodes.len(), "a path revisited a node");
    }
}

#[test]
fn reachability_reports_hop_counts() {
    let graph = chain_graph();
    let reachable = graph.reachable("kestrel", 3, now(), now());
    assert_eq!(reachable.get("northwind"), Some(&1));
    assert_eq!(reachable.get("vantage"), Some(&2));
    assert!(
        !reachable.contains_key("kestrel"),
        "the origin is not its own neighbour"
    );
}

#[test]
fn hubs_are_the_most_connected_nodes() {
    let graph = chain_graph();
    let hubs = graph.most_connected(2, now(), now());
    assert_eq!(hubs[0].0.id, "northwind", "the hub of the chain");
    assert!(hubs[0].1 >= 3);
}

// --- causal propagation -----------------------------------------------------

fn causal_chain() -> CausalGraph {
    let mut causal = CausalGraph::new();
    causal.add(
        CausalEdge::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.5,
            Duration::from_days(7),
            days_ago(300),
        )
        .with_confidence(0.8)
        .with_evidence(vec!["filing:input-costs".into()]),
    );
    causal.add(
        CausalEdge::new(
            "northwind",
            "vantage",
            Mechanism::SupplyChain,
            0.6,
            Duration::from_days(3),
            days_ago(300),
        )
        .with_confidence(0.75)
        .with_evidence(vec!["filing:supplier-concentration".into()]),
    );
    causal.add(
        CausalEdge::new(
            "northwind",
            "meridian",
            Mechanism::CompetitiveSubstitution,
            0.3,
            Duration::from_days(5),
            days_ago(300),
        )
        .with_confidence(0.5)
        .with_evidence(vec!["research:substitution".into()]),
    );
    causal
}

#[test]
fn a_relationship_is_structure_and_never_a_path_a_shock_travels_along() {
    // `RelationshipKind::transmits_shock` once said which edge kinds could
    // carry a shock, and nothing consulted it: propagation runs over causal
    // claims only. This is the rule it restated, asserted where it is held —
    // a relationship asserted into the graph, however economically plausible,
    // creates no causal edge and therefore no effect. Were `relate` ever to
    // mint one, every index membership would read as contagion.
    let mut world = WorldModel::new();
    world.relate(
        Relationship::new(
            "kestrel",
            "northwind",
            RelationshipKind::Supplies,
            0.9,
            "filings",
        ),
        days_ago(400),
        days_ago(400),
        0.9,
    );

    // Premise: the relationship is in the graph and traversable.
    let structural = world.graph().neighbours("kestrel", None, now(), now());
    assert_eq!(structural.len(), 1, "the relationship must be in the graph");
    assert_eq!(structural[0].relationship.to, "northwind");

    assert!(
        world.causal().outgoing("kestrel", now()).is_empty(),
        "asserting a relationship must not claim a causal mechanism"
    );
    let result = world.propagate("kestrel", -0.10, 3, now(), now());
    assert!(
        result.effects.is_empty(),
        "a shock travelled along a relationship nobody claimed a mechanism for: {:?}",
        result.effects
    );
    assert_eq!(
        result.truncated, 0,
        "nothing was cut; there was nothing to walk"
    );
}

#[test]
fn a_shock_propagates_and_attenuates_with_each_hop() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());

    let first = result.at_order(1);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].target, "northwind");
    // -10% through 0.5 strength at 0.8 confidence.
    assert!(approx_eq(first[0].magnitude, -0.10 * 0.5 * 0.8, 1e-9));

    let second = result.at_order(2);
    assert_eq!(second.len(), 2, "vantage and meridian");
    let vantage = second.iter().find(|e| e.target == "vantage").unwrap();
    assert!(
        vantage.magnitude.abs() < first[0].magnitude.abs(),
        "the effect must attenuate"
    );
    assert_eq!(vantage.order, 2);
    assert_eq!(vantage.chain.len(), 2);
}

#[test]
fn competitive_substitution_flips_the_sign() {
    // A rival's loss is a gain. Treating it as same-signed produces theses that
    // are exactly backwards.
    let causal = causal_chain();
    let result = causal.propagate("northwind", -0.20, 2, 0.001, now(), now());

    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    assert!(
        vantage.magnitude < 0.0,
        "a supplier shock hurts the customer"
    );

    let meridian = result
        .effects
        .iter()
        .find(|e| e.target == "meridian")
        .unwrap();
    assert!(
        meridian.magnitude > 0.0,
        "a competitor's loss is a gain: {}",
        meridian.magnitude
    );
    assert!(!Mechanism::CompetitiveSubstitution.preserves_sign());
}

#[test]
fn effects_are_timed_by_the_accumulated_lag() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());
    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    // Seven days to reach Northwind, three more to reach Vantage.
    assert_eq!(
        vantage.expected_at,
        now().saturating_add(Duration::from_days(10))
    );
}

#[test]
fn a_negligible_effect_is_dropped_and_counted() {
    let causal = causal_chain();
    // A floor above the second-order magnitude cuts the chain.
    let result = causal.propagate("kestrel", -0.10, 3, 0.03, now(), now());
    assert!(
        result.at_order(2).is_empty(),
        "second order falls below the floor"
    );
    assert!(
        result.truncated > 0,
        "the caller must know the chain was cut"
    );
}

#[test]
fn propagation_terminates_on_a_cycle() {
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "a",
        "b",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(10),
    ));
    causal.add(CausalEdge::new(
        "b",
        "a",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(10),
    ));
    let result = causal.propagate("a", 1.0, 10, 1e-6, now(), now());
    assert!(
        result.effects.len() <= 2,
        "a cycle must not amplify without limit"
    );
}

#[test]
fn a_causal_claim_recorded_later_is_invisible_earlier() {
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "a",
        "b",
        Mechanism::SupplyChain,
        0.5,
        Duration::ZERO,
        now(),
    ));
    assert!(
        causal
            .propagate("a", 1.0, 2, 1e-6, days_ago(10), days_ago(10))
            .is_empty()
    );
    assert!(!causal.propagate("a", 1.0, 2, 1e-6, now(), now()).is_empty());
}

#[test]
fn absorbing_a_claim_moves_the_causal_graphs_last_update_and_no_query_does() {
    // The failure this prevents: §6.2 row 2 read "fresh by construction" at
    // the centre because nothing recorded when the graph last absorbed a
    // claim, so a graph nobody had re-estimated in a year sized like one
    // re-estimated this morning. The fact is now written at the seams where
    // evidence lands — `add`, from the edge's own `recorded_at`, and
    // `reestimate`, only when it used a claim — and at no other.
    let mut causal = CausalGraph::new();
    // Premise: a graph that has absorbed nothing reports nothing, and no
    // query against it invents an instant.
    assert_eq!(causal.last_updated(), None);
    assert!(causal.propagate("a", 1.0, 2, 1e-6, now(), now()).is_empty());
    assert!(causal.explanations("b", now()).is_empty());
    assert!(causal.outgoing("a", now()).is_empty());
    assert_eq!(
        causal.last_updated(),
        None,
        "a query against an empty graph invented an update instant"
    );

    let edge = |cause: &str, effect: &str, recorded_at: Timestamp| {
        CausalEdge::new(
            cause,
            effect,
            Mechanism::SupplyChain,
            0.5,
            Duration::ZERO,
            recorded_at,
        )
    };
    causal.add(edge("a", "b", days_ago(10)));
    assert_eq!(
        causal.last_updated(),
        Some(days_ago(10)),
        "absorbing the first claim did not record its instant"
    );

    // A backfilled claim with an older instant is absorbed but does not
    // rewind the fact: the graph is as current as the newest thing it holds.
    causal.add(edge("b", "c", days_ago(30)));
    assert_eq!(causal.len(), 2);
    assert_eq!(
        causal.last_updated(),
        Some(days_ago(10)),
        "a backfilled claim rewound the graph's last update"
    );

    // A newer claim moves it forward.
    causal.add(edge("c", "d", days_ago(1)));
    assert_eq!(causal.last_updated(), Some(days_ago(1)));

    // And reading the populated graph — the only other thing anyone does
    // with it — leaves the instant where absorption put it. Premise: the
    // reads actually find something, so this is not a test of empty paths.
    assert!(!causal.propagate("a", 1.0, 3, 1e-6, now(), now()).is_empty());
    assert_eq!(causal.explanations("d", now()).len(), 1);
    assert_eq!(causal.unevidenced().len(), 3);
    assert_eq!(
        causal.last_updated(),
        Some(days_ago(1)),
        "a query moved the graph's last update"
    );
}

// --- re-estimating the causal graph ------------------------------------------

/// The horizon the centre judges the causal-graph row on, restated here so the
/// tests below drive `reestimate` with the same window `absorb_causal_support`
/// uses in production.
fn causal_horizon() -> Duration {
    qip_contracts::degradation::CAUSAL_GRAPH_HORIZON
}

#[test]
fn a_re_estimation_from_claims_inside_the_horizon_moves_the_last_update_and_the_strength() {
    // The failure this prevents: the graph carried whichever strength arrived
    // first, for ever, because nothing re-estimated it from what the model
    // kept absorbing — so the centre either propagated a year-old number or
    // narrowed on row 2 with no path back to full size.
    let mut causal = CausalGraph::new();
    causal.add(
        CausalEdge::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.30,
            Duration::from_days(7),
            days_ago(200),
        )
        .with_evidence(vec!["filing:northwind-10k".into()]),
    );
    // Premise: the graph is exactly as old as its only claim, and that claim
    // is past the horizon — so any freshness observed below was produced by
    // the re-estimation and not carried in by the fixture.
    assert_eq!(causal.last_updated(), Some(days_ago(200)));
    assert!(now().since(days_ago(200)) > causal_horizon());

    let report = causal
        .reestimate(
            vec![
                SupportingClaim::new(
                    "kestrel",
                    "northwind",
                    Mechanism::InputCost,
                    0.50,
                    days_ago(10),
                )
                .with_evidence(vec!["filing:northwind-10q".into()]),
                SupportingClaim::new(
                    "kestrel",
                    "northwind",
                    Mechanism::InputCost,
                    0.40,
                    days_ago(3),
                )
                .with_evidence(vec!["research:input-cost-note".into()]),
            ],
            causal_horizon(),
            now(),
        )
        .expect("two in-horizon claims re-estimate");

    assert!(report.refreshed);
    assert_eq!(report.claims_considered, 2);
    assert_eq!(report.claims_used, 2);
    assert_eq!(
        causal.last_updated(),
        Some(now()),
        "a re-estimation that used two claims left the freshness fact where it was"
    );
    assert_eq!(report.updated.len(), 1);
    assert!(approx_eq(report.updated[0].previous, 0.30, 1e-12));
    assert!(
        approx_eq(report.updated[0].updated, 0.45, 1e-12),
        "the strength is the mean of the in-horizon claims, not the last one \
         or the prior: {}",
        report.updated[0].updated
    );
    assert_eq!(report.updated[0].claims, 2);
    assert_eq!(report.updated[0].newest_claim, days_ago(3));
    assert!(report.decayed.is_empty());
    assert!(report.unmatched.is_empty());

    let edge = &causal.edges()[0];
    assert!(approx_eq(edge.strength, 0.45, 1e-12));
    assert!(!edge.is_decayed());
    // The evidence that moved the number travels with it, so a strength the
    // platform sizes against names what re-measured it.
    let evidence: Vec<&str> = edge.evidence.iter().map(String::as_str).collect();
    assert_eq!(
        evidence,
        [
            "filing:northwind-10k",
            "filing:northwind-10q",
            "research:input-cost-note"
        ]
    );
    // And the re-estimated edge is not readable before the evidence that
    // produced it. Leaving `recorded_at` at the original instant would make
    // the new strength visible to a point-in-time query three days before the
    // claim existed — leakage no backtest can see, because the record itself
    // would claim to have been available.
    assert!(
        causal.outgoing("kestrel", days_ago(4)).is_empty(),
        "the re-estimated strength was readable before its newest claim"
    );
    assert_eq!(causal.outgoing("kestrel", days_ago(3)).len(), 1);
}

#[test]
fn a_re_estimation_with_only_claims_outside_the_horizon_leaves_the_fact_and_marks_decay() {
    // The failure this prevents: a re-estimation that used nothing refreshing
    // the graph anyway. Row 2 would then read fresh because somebody asked a
    // question, and the centre would size at full budget against relationships
    // no evidence inside the quarter supports.
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "kestrel",
        "northwind",
        Mechanism::InputCost,
        0.30,
        Duration::from_days(7),
        days_ago(200),
    ));
    // Premise: there *is* a claim, and it names the link the graph holds — it
    // is simply older than the horizon. A test offering no claim at all would
    // pass against an implementation that refreshed on any claim it was given.
    let claims = vec![SupportingClaim::new(
        "kestrel",
        "northwind",
        Mechanism::InputCost,
        0.90,
        days_ago(120),
    )];
    assert!(now().since(days_ago(120)) > causal_horizon());

    let report = causal
        .reestimate(claims.clone(), causal_horizon(), now())
        .expect("an out-of-horizon claim is not a refusal");

    assert!(!report.refreshed);
    assert_eq!(report.claims_considered, 1);
    assert_eq!(report.claims_used, 0);
    assert_eq!(
        causal.last_updated(),
        Some(days_ago(200)),
        "a re-estimation that used nothing refreshed the freshness fact"
    );
    assert!(report.updated.is_empty());
    assert!(
        approx_eq(causal.edges()[0].strength, 0.30, 1e-12),
        "a claim outside the horizon moved the strength"
    );

    // Marked and reported — and still in the graph, still propagating. A link
    // nobody has re-evidenced has not been disproved.
    assert_eq!(report.decayed.len(), 1);
    assert_eq!(report.decayed[0].cause, "kestrel");
    assert_eq!(report.decayed[0].effect, "northwind");
    assert_eq!(report.decayed[0].recorded_at, days_ago(200));
    assert_eq!(report.decayed[0].previously_marked, None);
    assert!(causal.edges()[0].is_decayed());
    assert_eq!(causal.edges()[0].decayed_at, Some(now()));
    assert_eq!(causal.len(), 1, "a decayed edge was dropped from the graph");
    assert_eq!(causal.outgoing("kestrel", now()).len(), 1);

    // Repeating it reports the mark as already made, so a caller journalling
    // the transition journals it once rather than on every pass.
    let again = causal
        .reestimate(claims, causal_horizon(), now())
        .expect("re-estimating twice is not a refusal");
    assert_eq!(again.decayed[0].previously_marked, Some(now()));
}

#[test]
fn a_claim_naming_a_link_the_graph_does_not_hold_is_reported_and_invents_no_edge() {
    // A causal edge is a claim with a mechanism, a lag, a confidence and
    // evidence behind it, asserted deliberately through `add`. Manufacturing
    // one from a strength reading is how a correlation becomes a thesis.
    let mut causal = CausalGraph::new();
    causal.add(CausalEdge::new(
        "kestrel",
        "northwind",
        Mechanism::InputCost,
        0.30,
        Duration::from_days(7),
        days_ago(10),
    ));
    // Premise: the graph holds exactly one link, and the claim below is
    // in-horizon — so nothing but the missing edge stops it being used.
    assert_eq!(causal.len(), 1);

    let report = causal
        .reestimate(
            vec![SupportingClaim::new(
                "zephyr",
                "atlas",
                Mechanism::DiscountRate,
                0.50,
                days_ago(1),
            )],
            causal_horizon(),
            now(),
        )
        .expect("an unmatched claim is not a refusal");

    assert_eq!(causal.len(), 1, "a supporting claim invented an edge");
    assert_eq!(report.unmatched.len(), 1);
    assert_eq!(report.unmatched[0].cause, "zephyr");
    assert_eq!(report.claims_used, 0);
    assert!(!report.refreshed);
    assert_eq!(
        causal.last_updated(),
        Some(days_ago(10)),
        "a claim matching nothing refreshed the graph"
    );
    // The link the graph does hold had no claim of its own, so it decayed.
    assert_eq!(report.decayed.len(), 1);
    assert_eq!(report.decayed[0].effect, "northwind");
}

#[test]
fn a_claim_from_the_future_or_outside_zero_to_one_is_refused_and_changes_nothing() {
    // Refuse rather than guess: a strength clamped into range, or a claim
    // stamped after the clock, is a producer's bug that would survive as a
    // number the platform sizes against. And the refusal is taken before any
    // edge is touched, so a rejected batch leaves the graph exactly as it was.
    let build = || {
        let mut causal = CausalGraph::new();
        causal.add(CausalEdge::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.30,
            Duration::from_days(7),
            days_ago(200),
        ));
        causal
    };
    let good = || {
        SupportingClaim::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.50,
            days_ago(5),
        )
    };
    // Premise: the same graph and the same first claim are accepted when the
    // second one is sound, so the refusals below are about the second claim.
    let mut accepted = build();
    let report = accepted
        .reestimate(vec![good(), good()], causal_horizon(), now())
        .expect("two sound claims re-estimate");
    assert_eq!(report.claims_used, 2);

    for bad in [
        SupportingClaim::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            1.5,
            days_ago(5),
        ),
        SupportingClaim::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            f64::NAN,
            days_ago(5),
        ),
        SupportingClaim::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.50,
            now().saturating_add(Duration::from_days(1)),
        ),
    ] {
        let mut causal = build();
        assert!(
            causal
                .reestimate(vec![good(), bad], causal_horizon(), now())
                .is_err(),
            "an unusable claim was absorbed rather than refused"
        );
        assert_eq!(
            causal.last_updated(),
            Some(days_ago(200)),
            "a refused re-estimation moved the freshness fact"
        );
        assert!(
            approx_eq(causal.edges()[0].strength, 0.30, 1e-12),
            "a refused re-estimation applied the claim that preceded the bad one"
        );
        assert!(!causal.edges()[0].is_decayed());
    }

    // A horizon nothing can be inside is refused too, rather than silently
    // marking every link in the graph decayed.
    let mut causal = build();
    assert!(
        causal
            .reestimate(vec![good()], Duration::ZERO, now())
            .is_err()
    );
    assert!(!causal.edges()[0].is_decayed());
}

#[test]
fn two_re_estimations_over_the_same_claims_produce_the_same_report() {
    // A replay that reorders is not a replay. The report reaches an operator
    // and the strengths reach a size, so both must depend on the claims and
    // not on the order they arrived in or on any hash seed.
    let build = || {
        let mut causal = CausalGraph::new();
        // Absorbed in an order that is deliberately not the links' own order,
        // so a report that came out in absorption order would differ from one
        // in key order below. Do not tidy this into alphabetical order.
        for (cause, effect, mechanism, strength) in [
            ("northwind", "vantage", Mechanism::SupplyChain, 0.45),
            ("kestrel", "northwind", Mechanism::InputCost, 0.30),
            (
                "northwind",
                "meridian",
                Mechanism::CompetitiveSubstitution,
                0.15,
            ),
        ] {
            causal.add(CausalEdge::new(
                cause,
                effect,
                mechanism,
                strength,
                Duration::from_days(3),
                days_ago(150),
            ));
        }
        causal
    };
    // Deliberately in neither key order nor edge order, so a report that
    // merely echoed its input would come out differently below.
    let claims = vec![
        SupportingClaim::new(
            "northwind",
            "vantage",
            Mechanism::SupplyChain,
            0.60,
            days_ago(5),
        )
        .with_evidence(vec!["e-b".into()]),
        SupportingClaim::new(
            "zephyr",
            "atlas",
            Mechanism::DiscountRate,
            0.50,
            days_ago(4),
        )
        .with_evidence(vec!["e-x".into()]),
        SupportingClaim::new(
            "kestrel",
            "northwind",
            Mechanism::InputCost,
            0.20,
            days_ago(30),
        )
        .with_evidence(vec!["e-a".into()]),
        SupportingClaim::new(
            "northwind",
            "vantage",
            Mechanism::SupplyChain,
            0.50,
            days_ago(20),
        )
        .with_evidence(vec!["e-c".into()]),
    ];

    let mut first = build();
    let mut second = build();
    let one = first
        .reestimate(claims.clone(), causal_horizon(), now())
        .expect("the first run re-estimates");
    let two = second
        .reestimate(claims.clone(), causal_horizon(), now())
        .expect("the second run re-estimates");

    // Premise: the report exercises all three outcomes, so equality below is
    // not the equality of two empty reports.
    assert_eq!(one.updated.len(), 2);
    assert_eq!(one.decayed.len(), 1);
    assert_eq!(one.unmatched.len(), 1);
    assert_eq!(one, two);
    assert_eq!(first.edges(), second.edges());

    // And the order is the link's, not the arrival's: the vantage claim
    // arrived first and reports second.
    let updated: Vec<(&str, &str)> = one
        .updated
        .iter()
        .map(|u| (u.cause.as_str(), u.effect.as_str()))
        .collect();
    assert_eq!(
        updated,
        [("kestrel", "northwind"), ("northwind", "vantage")]
    );
    assert!(approx_eq(one.updated[1].updated, 0.55, 1e-12));
}

#[test]
fn explanations_rank_the_most_likely_cause_first() {
    let causal = causal_chain();
    let explanations = causal.explanations("northwind", now());
    assert_eq!(explanations.len(), 1);
    assert_eq!(explanations[0].cause, "kestrel");
    assert!(explanations[0].is_evidenced());
}

#[test]
fn unevidenced_claims_are_surfaced() {
    let mut causal = causal_chain();
    causal.add(CausalEdge::new(
        "x",
        "y",
        Mechanism::Sentiment,
        0.9,
        Duration::ZERO,
        days_ago(1),
    ));
    let unevidenced = causal.unevidenced();
    assert_eq!(unevidenced.len(), 1);
    assert_eq!(unevidenced[0].cause, "x");
}

#[test]
fn an_effect_explains_itself_in_words() {
    let causal = causal_chain();
    let result = causal.propagate("kestrel", -0.10, 3, 0.001, now(), now());
    let vantage = result
        .effects
        .iter()
        .find(|e| e.target == "vantage")
        .unwrap();
    let explanation = vantage.explain();
    assert!(explanation.contains("order 2"));
    assert!(explanation.contains("kestrel"));
    assert!(explanation.contains("northwind"));
    assert!(explanation.contains("input") || explanation.contains("supply"));
}

// --- point-in-time features -------------------------------------------------

#[test]
fn a_feature_is_invisible_before_it_became_available() {
    // A quarterly figure describes March but only arrives in May. A backtest
    // reading it in April is reading data that did not exist.
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("revenue", "quarterly revenue", "fundamentals")
            .with_staleness(Duration::from_days(200)),
    );
    let period_end = Timestamp::from_civil(2026, 3, 31);
    let published = Timestamp::from_civil(2026, 5, 12);
    store.record(
        "revenue",
        "ent-northwind",
        FeatureValue::new(4200.0, period_end, published),
    );

    let april = Timestamp::from_civil(2026, 4, 15);
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", april, april)
            .is_none(),
        "not yet published"
    );
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", period_end, published)
            .is_some(),
        "available once published"
    );
    // And retrospectively, we can ask what was true in March knowing what we
    // know now.
    assert!(
        store
            .value_as_of("revenue", "ent-northwind", period_end, now())
            .is_some()
    );
}

#[test]
fn a_stale_feature_is_not_returned() {
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("close", "last price", "market").with_staleness(Duration::from_days(5)),
    );
    store.record(
        "close",
        "obj-a",
        FeatureValue::immediate(100.0, days_ago(30)),
    );
    assert!(
        store.current("close", "obj-a", now()).is_none(),
        "a month-old price is not a price"
    );
    assert!(
        store
            .value_as_of("close", "obj-a", days_ago(28), now())
            .is_some()
    );
}

#[test]
fn a_restatement_replaces_the_value_for_the_same_instant() {
    let mut store = FeatureStore::new();
    store.define(
        Feature::new("revenue", "revenue", "fundamentals").with_staleness(Duration::from_days(400)),
    );
    let period = days_ago(90);
    store.record(
        "revenue",
        "e",
        FeatureValue::new(100.0, period, days_ago(60)),
    );
    store.record(
        "revenue",
        "e",
        FeatureValue::new(95.0, period, days_ago(10)),
    );
    assert_eq!(
        store.history("revenue", "e", now()).len(),
        1,
        "one instant, one value"
    );
    assert!(approx_eq(
        store.current("revenue", "e", now()).unwrap().value,
        95.0,
        1e-9
    ));
}

#[test]
fn a_missing_feature_is_reported_rather_than_defaulted_to_zero() {
    // Substituting zero for unknown is how a model ends up trained on a fact
    // that was never true.
    let mut store = FeatureStore::new();
    store.define(Feature::new("close", "price", "market").with_staleness(Duration::from_days(5)));
    store.define(
        Feature::new("revenue", "revenue", "fundamentals").with_staleness(Duration::from_days(400)),
    );
    store.record("close", "obj-a", FeatureValue::immediate(100.0, now()));

    let (values, missing) = store.vector_as_of(
        &["close".to_string(), "revenue".to_string()],
        "obj-a",
        now(),
        now(),
    );
    assert!(approx_eq(values[0], 100.0, 1e-9));
    assert!(values[1].is_nan(), "unknown is not zero");
    assert_eq!(missing, vec!["revenue".to_string()]);
}

#[test]
fn a_cross_section_ranks_subjects_by_feature() {
    let mut store = FeatureStore::new();
    store.define(Feature::new("close", "price", "market").with_staleness(Duration::from_days(5)));
    for (subject, value) in [("a", 10.0), ("b", 30.0), ("c", 20.0)] {
        store.record("close", subject, FeatureValue::immediate(value, now()));
    }
    let ranked = store.cross_section("close", now(), now());
    assert_eq!(ranked[0].0, "b");
    assert_eq!(ranked[2].0, "a");
    assert_eq!(store.subjects_with("close", now(), now()).len(), 3);
}

// --- the world model --------------------------------------------------------

fn seeded_model() -> (WorldModel, Context) {
    let context = context();
    let mut model = WorldModel::new();
    seed_demo_world(&mut model, &context).unwrap();
    (model, context)
}

#[test]
fn the_demo_world_seeds_a_connected_graph() {
    let (model, _) = seeded_model();
    let statistics = model.statistics();
    assert!(statistics["nodes"] >= 7, "{statistics:?}");
    assert!(statistics["causal_claims"] >= 4);

    // Kestrel reaches Vantage through Northwind.
    let paths = model
        .graph()
        .paths_between("ent-kestrel", "ent-vantage", 3, now(), now());
    assert!(!paths.is_empty(), "the supply chain must connect");
}

#[test]
fn a_shock_at_the_supplier_reaches_the_customers_customer() {
    // The second- and third-order effects the charter asks for, produced by a
    // mechanism rather than by correlation.
    let (model, _) = seeded_model();
    let result = model.propagate("ent-kestrel", -0.15, 3, now(), now());

    assert!(!result.is_empty());
    let targets: Vec<&str> = result.effects.iter().map(|e| e.target.as_str()).collect();
    assert!(targets.contains(&"ent-northwind"), "{targets:?}");
    assert!(targets.contains(&"ent-vantage"), "{targets:?}");

    let strongest = result.strongest(1)[0];
    assert_eq!(
        strongest.target, "ent-northwind",
        "the direct effect is the largest"
    );
}

#[test]
fn the_demo_seed_reads_stale_until_fresh_support_is_absorbed_and_then_reads_fresh() {
    // The failure this closes: the demo seed backdates every claim by a year,
    // so the centre's §6.2 row 2 read stale on it — correctly, and with no way
    // back. Nothing re-estimated the graph from what the model kept absorbing,
    // so the row could only ever narrow. Judged here by the contract's own
    // assessment rather than by a rule restated in this test, because two
    // claims about the same fact disagree and the louder one wins.
    use qip_contracts::degradation::{CAUSAL_GRAPH_HORIZON, CausalGraphFreshness, Freshness};

    let (mut model, _) = seeded_model();
    // Premise: the seed absorbed claims, all of them older than the horizon,
    // and the row reads stale before anything below runs.
    let seeded_at = model
        .causal()
        .last_updated()
        .expect("the demo seed absorbed a claim");
    assert!(now().since(seeded_at) > CAUSAL_GRAPH_HORIZON);
    assert_eq!(model.causal().len(), 4);
    assert_eq!(
        model.causal_support().len(),
        4,
        "the seed's claims were not retained as their own supporting evidence"
    );
    assert_eq!(
        CausalGraphFreshness::assess(model.causal().last_updated(), CAUSAL_GRAPH_HORIZON, now())
            .expect("the row reads")
            .freshness(),
        Freshness::Stale
    );

    // A new filing re-measures one link. It is support, not a second edge:
    // two edges for one link would leave `propagate` picking the stronger
    // rather than the newer.
    let report = model
        .absorb_causal_support(
            SupportingClaim::new(
                "ent-kestrel",
                "ent-northwind",
                Mechanism::InputCost,
                0.42,
                days_ago(2),
            )
            .with_evidence(vec!["filing:northwind-10q-input-costs".into()]),
            now(),
        )
        .expect("the support is absorbed");

    assert!(report.refreshed);
    assert_eq!(
        report.claims_considered, 5,
        "the four seeded claims and the new one were all considered"
    );
    assert_eq!(
        report.claims_used, 1,
        "only the claim inside the horizon may be used"
    );
    assert_eq!(model.causal().len(), 4, "re-estimation added an edge");
    assert_eq!(
        report.decayed.len(),
        3,
        "the three links nothing re-evidenced are reported, not dropped"
    );

    assert_eq!(
        CausalGraphFreshness::assess(model.causal().last_updated(), CAUSAL_GRAPH_HORIZON, now())
            .expect("the row reads")
            .freshness(),
        Freshness::Fresh,
        "a link re-estimated from evidence two days old still read stale"
    );
    assert_eq!(
        CausalGraphFreshness::assess(model.causal().last_updated(), CAUSAL_GRAPH_HORIZON, now())
            .expect("the row reads"),
        CausalGraphFreshness::Fresh {
            last_updated: now()
        }
    );

    // The graph propagates the re-estimated number, and the journal says so,
    // so the discovery stage sees the strength move.
    let edge = model
        .causal()
        .outgoing("ent-kestrel", now())
        .into_iter()
        .find(|e| e.effect == "ent-northwind")
        .expect("the seeded link survives its re-estimation")
        .clone();
    assert!(
        approx_eq(edge.strength, 0.42, 1e-12),
        "the graph still propagates the seed's year-old strength"
    );
    let revised: Vec<&str> = model
        .changes()
        .iter()
        .filter(|c| c.kind == ChangeKind::BeliefRevised)
        .map(|c| c.subject.as_str())
        .collect();
    assert!(
        revised.contains(&"ent-kestrel->ent-northwind"),
        "the re-estimated strength was not journalled: {revised:?}"
    );
}

#[test]
fn absorbing_news_resolves_entities_and_indexes_the_document() {
    let (mut model, context) = seeded_model();
    let item = NewsItem {
        item_id: "news-1".into(),
        headline: "Northwind Semiconductor Corporation cut full year revenue guidance".into(),
        body: "The company reduced its outlook, citing weak demand for its components.".into(),
        source: NewsSource::CompanyAnnouncement,
        published_at: now(),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence: 0.94,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.7,
            confidence: 0.9,
            novelty: 0.8,
        },
        topics: vec!["guidance".into()],
        provenance: Provenance::synthetic("synthetic-news", now()),
        quality: DataQuality::clean(),
    };

    let resolved = model.absorb_news(&item, &context);
    assert_eq!(
        resolved,
        vec!["ent-northwind".to_string()],
        "resolved onto the seeded entity"
    );

    // The document is retrievable as evidence.
    let hits = model.retrieve("revenue guidance cut", 3, now());
    assert!(!hits.is_empty());
    assert_eq!(hits[0].document.id, "news:news-1");

    // And sentiment became a feature of the entity.
    let sentiment = model
        .features()
        .current("sentiment", "ent-northwind", now())
        .expect("sentiment recorded");
    assert!(sentiment.value < 0.0);
}

#[test]
fn absorbing_a_fundamental_records_the_publication_lag() {
    let (mut model, _) = seeded_model();
    let period_end = days_ago(45);
    let published = days_ago(5);
    let mut provenance = Provenance::synthetic("fundamentals", period_end);
    provenance.ingestion_time = published;

    let update = FundamentalUpdate {
        entity_id: "ent-northwind".into(),
        metric: "revenue".into(),
        value: Decimal::from_int(4600),
        unit: "USD_millions".into(),
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Some(Decimal::from_int(4200)),
        prior_value: Some(Decimal::from_int(4000)),
        is_restatement: false,
        provenance,
        quality: DataQuality::clean(),
    };
    model.absorb_fundamental(&update);

    // Invisible before publication, visible after.
    assert!(
        model
            .features()
            .value_as_of("revenue", "ent-northwind", period_end, days_ago(20))
            .is_none()
    );
    let value = model
        .features()
        .value_as_of("revenue", "ent-northwind", period_end, now())
        .expect("visible once published");
    assert!(approx_eq(value.value, 4600.0, 1e-9));
    assert_eq!(value.availability_lag(), Duration::from_days(40));

    let surprise = model
        .features()
        .value_as_of("revenue_surprise", "ent-northwind", period_end, now())
        .unwrap();
    assert!(approx_eq(surprise.value, 400.0 / 4200.0, 1e-9));
}

#[test]
fn absorbing_bars_builds_a_price_history_and_a_volatility_feature() {
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let mut price = 100.0;
    for day in 0..40 {
        price *= if day % 2 == 0 { 1.015 } else { 0.99 };
        let bar = Bar {
            object_id: object.clone(),
            venue: "XNYS".into(),
            interval: Interval::Day,
            open_time: days_ago(40 - day),
            open: Decimal::from_f64(price).unwrap(),
            high: Decimal::from_f64(price * 1.01).unwrap(),
            low: Decimal::from_f64(price * 0.99).unwrap(),
            close: Decimal::from_f64(price).unwrap(),
            volume: Decimal::from_int(1_000_000),
            vwap: None,
            trade_count: 5_000,
            quality: DataQuality::clean(),
        };
        // A daily bar this test builds itself is knowable the moment its
        // period closes; a bar off a wire would pass its arrival time.
        let known_at = bar.close_time();
        model.absorb_bar(&bar, known_at);
    }
    model.recompute_volatility(object.as_str(), 20, now());

    let volatility = model
        .features()
        .current("realised_volatility_20d", object.as_str(), now())
        .expect("volatility computed");
    assert!(
        volatility.value > 0.0 && volatility.value < 3.0,
        "vol {}",
        volatility.value
    );
}

#[test]
fn the_world_state_can_be_reconstructed_at_a_past_instant() {
    let (mut model, context) = seeded_model();
    let macro_update = MacroObservation {
        series_id: "US.CPI.YOY".into(),
        region: "US".into(),
        value: 3.4,
        unit: "percent".into(),
        reference_date: days_ago(10),
        consensus: Some(3.0),
        previous: Some(2.9),
        is_revision: false,
        provenance: Provenance::synthetic("macro", days_ago(2)),
        quality: DataQuality::clean(),
    };
    model.absorb_macro(&macro_update);
    let _ = &context;

    let current = model.state_at(now(), now());
    assert!(current.entity_count >= 7);
    assert!(current.relationship_count > 0);
    assert!(current.summary().contains("entities"));

    // A month ago the macro reading had not been published.
    let past = model.state_at(days_ago(30), days_ago(30));
    assert!(
        !past.features.keys().any(|k| k.starts_with("macro_level/")),
        "the macro print was not known a month ago"
    );
    assert!(
        current
            .features
            .keys()
            .any(|k| k.starts_with("macro_level/"))
    );
}

#[test]
fn the_diff_reports_what_changed_between_two_instants() {
    let (mut model, context) = seeded_model();
    let before = now();

    let item = NewsItem {
        item_id: "news-2".into(),
        headline: "Northwind Semiconductor Corporation warns on production".into(),
        body: "A disruption is expected to affect output for several weeks.".into(),
        source: NewsSource::RegulatoryFiling,
        published_at: before.saturating_add(Duration::from_hours(1)),
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence: 0.95,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.8,
            confidence: 0.9,
            novelty: 0.9,
        },
        topics: vec!["supply_chain".into()],
        provenance: Provenance::synthetic("news", before.saturating_add(Duration::from_hours(1))),
        quality: DataQuality::clean(),
    };
    model.absorb_news(&item, &context);

    let diff = model.diff(before, before.saturating_add(Duration::from_hours(2)));
    assert!(!diff.is_empty(), "the news should register as a change");
    assert!(!diff.material(0.5).is_empty());
    assert!(diff.summary().contains("material"));
    assert!(!diff.of_kind(ChangeKind::EntityUpdated).is_empty());

    // Nothing happened in a window before the news.
    let quiet = model.diff(days_ago(2), days_ago(1));
    assert!(quiet.is_empty(), "{}", quiet.summary());
    assert!(quiet.summary().contains("nothing changed"));
}

#[test]
fn retrieval_from_the_world_model_respects_the_point_in_time_cutoff() {
    let (mut model, context) = seeded_model();
    for (id, when) in [("old", days_ago(10)), ("recent", days_ago(1))] {
        let item = NewsItem {
            item_id: id.into(),
            headline: format!("Northwind Semiconductor guidance update {id}"),
            body: "Guidance was revised.".into(),
            source: NewsSource::Newswire,
            published_at: when,
            entities: vec![EntityMention {
                text: "Northwind Semiconductor Corporation".into(),
                entity_id: None,
                confidence: 0.9,
                is_primary: true,
                sentiment: None,
            }],
            sentiment: Sentiment::neutral(),
            topics: Vec::new(),
            provenance: Provenance::synthetic("news", when),
            quality: DataQuality::clean(),
        };
        model.absorb_news(&item, &context);
    }

    let all = model.retrieve("guidance update", 5, now());
    assert_eq!(all.len(), 2);
    let historical = model.retrieve("guidance update", 5, days_ago(5));
    assert_eq!(
        historical.len(),
        1,
        "only the older document existed five days ago"
    );
    assert_eq!(historical[0].document.id, "news:old");
}

#[test]
fn the_model_is_deterministic() {
    let build = || {
        let context = context();
        let mut model = WorldModel::new();
        seed_demo_world(&mut model, &context).unwrap();
        let result = model.propagate("ent-kestrel", -0.15, 3, now(), now());
        result
            .effects
            .iter()
            .map(|e| format!("{}:{:.9}", e.target, e.magnitude))
            .collect::<Vec<_>>()
    };
    assert_eq!(build(), build());
}

#[test]
fn a_bar_off_a_wire_is_not_knowable_when_it_closed() {
    // The look-ahead this argument exists to prevent. A bar closes at the
    // venue and arrives here later; stamping availability at the close makes
    // every feature derived from it readable before it existed — and a
    // point-in-time read cannot catch that, because the record itself claims
    // to have been available.
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let bar = Bar {
        object_id: object.clone(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: days_ago(1),
        open: Decimal::from_int(100),
        high: Decimal::from_int(101),
        low: Decimal::from_int(99),
        close: Decimal::from_int(100),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    };
    let closed = bar.close_time();
    let arrived = closed.saturating_add(qip_core::Duration::from_millis(250));
    model.absorb_bar(&bar, arrived);

    assert!(
        model
            .features()
            .current("close", object.as_str(), closed)
            .is_none(),
        "the close was readable a quarter of a second before the platform had it"
    );
    let known = model
        .features()
        .current("close", object.as_str(), arrived)
        .expect("readable once it had arrived");
    assert_eq!(
        known.availability_lag(),
        qip_core::Duration::from_millis(250),
        "the feed's latency was lost rather than recorded"
    );
    assert_eq!(known.valid_at, closed, "valid time is still the bar's own");
}

#[test]
fn a_bar_cannot_be_known_before_the_period_it_summarises_ended() {
    // The combination has no physical meaning and always means a clock or a
    // parser rather than a very fast feed, so it is clamped rather than
    // trusted.
    let (mut model, _) = seeded_model();
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let bar = Bar {
        object_id: object.clone(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: days_ago(1),
        open: Decimal::from_int(100),
        high: Decimal::from_int(101),
        low: Decimal::from_int(99),
        close: Decimal::from_int(100),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    };
    let closed = bar.close_time();
    model.absorb_bar(
        &bar,
        closed.saturating_sub(qip_core::Duration::from_hours(1)),
    );

    let value = model
        .features()
        .current("close", object.as_str(), closed)
        .expect("clamped forward to the close rather than dropped");
    assert_eq!(value.available_at, closed);
}

// --- bulk absorption --------------------------------------------------------

/// A daily bar fixture for the batched-absorption tests.
fn day_bar(object: &ObjectId, days_back: i64, close: i64) -> Bar {
    Bar {
        object_id: object.clone(),
        venue: "XNYS".into(),
        interval: Interval::Day,
        open_time: days_ago(days_back),
        open: Decimal::from_int(close),
        high: Decimal::from_int(close + 1),
        low: Decimal::from_int(close - 1),
        close: Decimal::from_int(close),
        volume: Decimal::from_int(1_000_000),
        vwap: None,
        trade_count: 5_000,
        quality: DataQuality::clean(),
    }
}

#[test]
fn recording_many_values_is_indistinguishable_from_recording_them_one_by_one() {
    // The premise of the batched path: it exists for speed, and speed that
    // changed the answer would be a different feature store, not a faster one.
    // The batch arrives newest-first — the order a feed replaying history
    // actually uses, and the worst case for repeated insertion.
    let mut one_by_one = FeatureStore::new();
    let mut batched = FeatureStore::new();
    for store in [&mut one_by_one, &mut batched] {
        store.define(Feature::new("close", "last traded price", "test"));
    }

    let values: Vec<FeatureValue> = (0..50)
        .map(|i| {
            FeatureValue::new(
                100.0 + f64::from(i),
                days_ago(i64::from(i)),
                days_ago(i64::from(i)),
            )
        })
        .collect();
    for value in &values {
        one_by_one.record("close", "obj-a", value.clone());
    }
    batched.record_many("close", "obj-a", values);

    for day in 0..50 {
        let at = days_ago(day);
        assert_eq!(
            one_by_one.value_as_of("close", "obj-a", at, at),
            batched.value_as_of("close", "obj-a", at, at),
            "the batched store answers differently {day} day(s) ago"
        );
    }
    assert_eq!(one_by_one.value_count(), batched.value_count());
}

#[test]
fn a_batched_restatement_replaces_the_stored_value_exactly_as_record_would() {
    // Two values for one instant would make "the value as of t" ambiguous, so
    // the restatement rule has to survive the merge: the later value wins,
    // whether it arrives against the stored series or inside the same batch.
    let mut store = FeatureStore::new();
    store.define(Feature::new("close", "last traded price", "test"));
    store.record(
        "close",
        "obj-a",
        FeatureValue::new(100.0, days_ago(1), days_ago(1)),
    );

    store.record_many(
        "close",
        "obj-a",
        vec![
            FeatureValue::new(101.0, days_ago(1), days_ago(1)),
            FeatureValue::new(102.0, days_ago(1), days_ago(1)),
        ],
    );

    let value = store
        .value_as_of("close", "obj-a", days_ago(1), now())
        .expect("the instant has exactly one value");
    assert!(
        approx_eq(value.value, 102.0, 1e-12),
        "the last restatement did not win: {}",
        value.value
    );
    assert_eq!(
        store.value_count(),
        1,
        "a restatement must replace, not accumulate"
    );
}

#[test]
fn bars_absorbed_in_bulk_answer_point_in_time_reads_exactly_like_bars_absorbed_singly() {
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let mut singly = WorldModel::new();
    let mut bulk = WorldModel::new();

    // Newest-first, as a replaying feed hands them over.
    let bars: Vec<Bar> = (0..40)
        .map(|i| day_bar(&object, i64::from(i), 100 + i64::from(i)))
        .collect();
    for bar in &bars {
        singly.absorb_bar(bar, bar.close_time());
    }
    bulk.absorb_bars(bars.iter().map(|bar| (bar, bar.close_time())));

    for day in 0..40 {
        let at = days_ago(day);
        assert_eq!(
            singly
                .features()
                .value_as_of("close", object.as_str(), at, at),
            bulk.features()
                .value_as_of("close", object.as_str(), at, at),
            "bulk absorption changed the close readable {day} day(s) ago"
        );
        assert_eq!(
            singly
                .features()
                .value_as_of("volume", object.as_str(), at, at),
            bulk.features()
                .value_as_of("volume", object.as_str(), at, at),
            "bulk absorption changed the volume readable {day} day(s) ago"
        );
    }
}

#[test]
fn bulk_absorption_applies_the_same_knowability_clamp_as_the_single_path() {
    // A bar cannot have been knowable before the period it summarises ended;
    // the batched path must clamp exactly as `absorb_bar` does, or a bulk
    // replay would carry the look-ahead the single path refuses.
    let object = ObjectId::from_string("OBJ00000000000000000NWSC");
    let mut model = WorldModel::new();
    let bar = day_bar(&object, 1, 100);
    let closed = bar.close_time();

    model.absorb_bars([(&bar, closed.saturating_sub(Duration::from_hours(1)))]);

    let value = model
        .features()
        .current("close", object.as_str(), closed)
        .expect("clamped forward to the close rather than dropped");
    assert_eq!(
        value.available_at, closed,
        "a pre-close knowability claim survived the bulk path"
    );
}

// --- provenance survives absorption ------------------------------------------

#[test]
fn a_fundamental_the_vendor_filled_in_is_recorded_as_imputed_not_observed() {
    // A vendor that fills a gap says so in `DataQuality::is_imputed`, and the
    // feature store carries a flag of the same meaning. The world model used
    // to write `false` there whatever the vendor said, so a filled value read
    // back as a reported one and no point-in-time query afterwards could tell
    // the two apart — the loss `qip-market-ingestion`'s alternative-data path
    // guards against at its own seam, undone one stage later.
    let (mut model, _) = seeded_model();
    let period_end = days_ago(45);
    let mut provenance = Provenance::synthetic("fundamentals", period_end);
    provenance.ingestion_time = days_ago(5);
    let fundamental = |metric: &str, quality: DataQuality| FundamentalUpdate {
        entity_id: "ent-northwind".into(),
        metric: metric.into(),
        value: Decimal::from_int(4600),
        unit: "USD_millions".into(),
        period_end,
        period: FiscalPeriod::Quarter,
        consensus: Some(Decimal::from_int(4200)),
        prior_value: None,
        is_restatement: false,
        provenance: provenance.clone(),
        quality,
    };
    let reported = fundamental("revenue", DataQuality::clean());
    let filled = fundamental("ebitda", DataQuality::clean().imputed());
    // Premise: the two updates differ in what the vendor said and in nothing
    // else, so any difference in the features is the flag travelling.
    assert!(!reported.quality.is_imputed);
    assert!(filled.quality.is_imputed);

    model.absorb_fundamental(&reported);
    model.absorb_fundamental(&filled);

    let read = |metric: &str| {
        model
            .features()
            .value_as_of(metric, "ent-northwind", period_end, now())
            .unwrap_or_else(|| panic!("{metric} was recorded"))
            .imputed
    };
    assert!(!read("revenue"), "a reported value is an observation");
    assert!(!read("revenue_surprise"));
    assert!(
        read("ebitda"),
        "a value the vendor filled in must still say so once it is a feature"
    );
    assert!(
        read("ebitda_surprise"),
        "a surprise computed from a filled value is no better than its input"
    );
}

#[test]
fn a_macro_print_the_vendor_filled_in_is_recorded_as_imputed_not_observed() {
    // The macro path has the same two timestamps and the same flag as the
    // fundamental path, and had the same defect: `quality.is_imputed` was
    // read for the confidence score and then dropped on the floor.
    let (mut model, _) = seeded_model();
    let observation = |series: &str, quality: DataQuality| MacroObservation {
        series_id: series.into(),
        region: "US".into(),
        value: 3.4,
        unit: "percent".into(),
        reference_date: days_ago(10),
        consensus: Some(3.0),
        previous: Some(2.9),
        is_revision: false,
        provenance: Provenance::synthetic("macro", days_ago(2)),
        quality,
    };
    let printed = observation("US.CPI.YOY", DataQuality::clean());
    let filled = observation("US.PMI", DataQuality::clean().imputed());
    assert!(!printed.quality.is_imputed);
    assert!(filled.quality.is_imputed);

    model.absorb_macro(&printed);
    model.absorb_macro(&filled);

    let read = |feature: &str, series: &str| {
        model
            .features()
            .value_as_of(feature, series, days_ago(10), now())
            .unwrap_or_else(|| panic!("{feature} for {series} was recorded"))
            .imputed
    };
    assert!(!read("macro_level", "US.CPI.YOY"));
    assert!(!read("macro_surprise", "US.CPI.YOY"));
    assert!(read("macro_level", "US.PMI"));
    assert!(read("macro_surprise", "US.PMI"));
}

#[test]
fn sentiment_from_a_news_item_the_vendor_filled_in_is_recorded_as_imputed() {
    // Sentiment is derived from the item, so it inherits whatever the item's
    // quality says about the item. An imputed record producing an observed
    // feature would launder the flag through the derivation.
    let (mut model, context) = seeded_model();
    let item = |id: &str, published_at: Timestamp, quality: DataQuality| NewsItem {
        item_id: id.into(),
        headline: "Northwind Semiconductor Corporation cut full year revenue guidance".into(),
        body: "The company reduced its outlook.".into(),
        source: NewsSource::CompanyAnnouncement,
        published_at,
        entities: vec![EntityMention {
            text: "Northwind Semiconductor Corporation".into(),
            entity_id: None,
            confidence: 0.94,
            is_primary: true,
            sentiment: None,
        }],
        sentiment: Sentiment {
            polarity: -0.7,
            confidence: 0.9,
            novelty: 0.8,
        },
        topics: vec!["guidance".into()],
        provenance: Provenance::synthetic("synthetic-news", published_at),
        quality,
    };
    let observed_item = item("news-observed", days_ago(1), DataQuality::clean());
    let filled_item = item("news-filled", now(), DataQuality::clean().imputed());
    assert!(!observed_item.quality.is_imputed);
    assert!(filled_item.quality.is_imputed);

    // Premise: both resolve onto the same entity, so both write the same
    // feature series and the reads below compare like with like.
    assert_eq!(
        model.absorb_news(&observed_item, &context),
        vec!["ent-northwind".to_string()]
    );
    assert_eq!(
        model.absorb_news(&filled_item, &context),
        vec!["ent-northwind".to_string()]
    );

    let read = |valid_at: Timestamp| {
        model
            .features()
            .value_as_of("sentiment", "ent-northwind", valid_at, now())
            .expect("sentiment was recorded")
            .imputed
    };
    assert!(!read(days_ago(1)));
    assert!(read(now()));
}

#[test]
fn a_recognised_macro_release_lands_under_the_analysts_name_keyed_by_its_economy() {
    // The seam that never met: the analyst read `policy_rate@<economy>` and
    // the arm wrote only `macro_level@<series id>`. Both writes now happen,
    // and a release the vocabulary does not recognise still gets the raw one.
    use qip_world_model::vocabulary::{MacroSeries, names};
    let (mut model, _) = seeded_model();
    let release = |series_id: &str, region: &str| MacroObservation {
        series_id: series_id.into(),
        region: region.into(),
        value: 5.25,
        unit: "percent".into(),
        reference_date: days_ago(10),
        consensus: Some(5.0),
        previous: None,
        is_revision: false,
        provenance: Provenance::synthetic("macro", days_ago(2)),
        quality: DataQuality::clean(),
    };
    model.absorb_macro(&release(&MacroSeries::PolicyRate.series_id("EA"), "EA"));
    model.absorb_macro(&release("US.CPI.YOY", "US"));

    let read = |feature: &str, subject: &str| {
        model
            .features()
            .value_as_of(feature, subject, days_ago(10), now())
            .map(|value| value.value)
    };
    assert_eq!(read(names::POLICY_RATE, "EA"), Some(5.25));
    assert_eq!(read(names::MACRO_LEVEL, "EA.POLICY_RATE"), Some(5.25));
    assert_eq!(read(names::MACRO_SURPRISE, "EA.POLICY_RATE"), Some(0.25));
    // Not at any key the analyst does not read by.
    assert_eq!(read(names::POLICY_RATE, "global"), None);
    assert_eq!(read(names::POLICY_RATE, "EA.POLICY_RATE"), None);
    // The unrecognised series is the raw record and nothing else.
    assert_eq!(read(names::MACRO_LEVEL, "US.CPI.YOY"), Some(5.25));
    assert_eq!(read(names::INFLATION_YOY, "US"), None);
}
