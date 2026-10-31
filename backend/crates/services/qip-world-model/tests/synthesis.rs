//! Cross-domain synthesis over the knowledge graph (R12-EXPAND-6031): a chain
//! of recorded relationships crossing three or more domains is a validated
//! `CrossDomainHypothesis` carrying every node, hop and source; a chain
//! inside two domains is not; and a hop the causal graph already claims is
//! counted on the hypothesis rather than hidden.
//!
//! The failure prevented: the graph held typed cross-domain relationships
//! since its vocabulary was written and nothing walked them, so "a synthesis
//! layer searches links across domains" was true of the data and false of
//! the platform.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::expansion::CrossDomainHypothesis;
use qip_core::{Duration, Timestamp};
use qip_world_model::causal::{CausalEdge, Mechanism};
use qip_world_model::graph::{EntityKind, Node};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::synthesis::{MAX_HYPOTHESES, cross_domain_hypotheses, domain_of};
use qip_world_model::world::WorldModel;

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 8, 24)
}

fn known() -> Timestamp {
    now().saturating_sub(Duration::from_days(100))
}

/// Kestrel (a company) in the mining industry, which produces copper;
/// Kestrel domiciled in Great Britain, which issues sterling; and Kestrel
/// supplying Northwind, a second company — which the graph stores with its
/// inverse, so Northwind is a customer of Kestrel and every chain Kestrel
/// starts is also a chain Northwind starts one hop earlier.
fn world() -> WorldModel {
    let mut world = WorldModel::new();
    for (id, kind, label) in [
        ("ent-kestrel", EntityKind::Company, "Kestrel Materials"),
        (
            "ent-northwind",
            EntityKind::Company,
            "Northwind Semiconductor",
        ),
        ("ind-mining", EntityKind::Industry, "Mining"),
        ("cmdty-copper", EntityKind::Commodity, "Copper"),
        ("ctry-gb", EntityKind::Country, "United Kingdom"),
        ("ccy-gbp", EntityKind::Currency, "Sterling"),
    ] {
        world
            .graph_mut()
            .add_node(Node::entity(id, kind, label, known()))
            .unwrap();
    }
    for (from, to, kind, weight, source) in [
        (
            "ent-kestrel",
            "ind-mining",
            RelationshipKind::InIndustry,
            1.0,
            "reference",
        ),
        (
            "ind-mining",
            "cmdty-copper",
            RelationshipKind::ProducesCommodity,
            0.8,
            "research",
        ),
        (
            "ent-kestrel",
            "ctry-gb",
            RelationshipKind::DomiciledIn,
            1.0,
            "reference",
        ),
        (
            "ctry-gb",
            "ccy-gbp",
            RelationshipKind::IssuesCurrency,
            1.0,
            "reference",
        ),
        (
            "ent-kestrel",
            "ent-northwind",
            RelationshipKind::Supplies,
            0.35,
            "filings",
        ),
    ] {
        world
            .relate(
                Relationship::new(from, to, kind, weight, source),
                known(),
                known(),
                0.9,
            )
            .unwrap();
    }
    world
}

fn path_of(h: &CrossDomainHypothesis) -> String {
    h.path.join(">")
}

/// Mutation: fold `EntityKind::Commodity` into `"industry"` in `domain_of` —
/// both copper chains cross two domains, are not emitted, and the path list
/// assertion fails; or stop counting `claimed` — the claimed-hop assertions
/// fail.
#[test]
fn a_company_to_industry_to_commodity_path_yields_a_validated_hypothesis_with_edge_lineage() {
    let world = world();
    assert_eq!(world.graph().node_count(), 6, "premise: six nodes");
    assert_eq!(
        world
            .graph()
            .neighbours("ent-kestrel", None, now(), now())
            .len(),
        3,
        "premise: Kestrel has three outgoing facts"
    );
    for (id, domain) in [
        ("ent-kestrel", "companies"),
        ("ind-mining", "industry"),
        ("cmdty-copper", "commodities"),
        ("ctry-gb", "economy"),
        ("ccy-gbp", "fx"),
    ] {
        assert_eq!(
            domain_of(world.graph().node(id).unwrap()),
            Some(domain),
            "{id}"
        );
    }

    let found = cross_domain_hypotheses(&world, now());
    let paths: Vec<String> = found.iter().map(path_of).collect();
    assert_eq!(
        paths,
        vec![
            "ent-kestrel>ctry-gb>ccy-gbp",
            "ent-kestrel>ind-mining>cmdty-copper",
            "ent-northwind>ent-kestrel>ctry-gb>ccy-gbp",
            "ent-northwind>ent-kestrel>ind-mining>cmdty-copper",
        ],
        "the two chains Kestrel starts, the two Northwind reaches through it, strongest first, \
         and the supplies hop alone — one domain — is not a chain"
    );
    for h in &found {
        h.validate().expect("every emitted hypothesis validates");
        assert_eq!(h.subject, h.path[0]);
        assert_eq!(h.edges.len() + 1, h.path.len());
        assert_eq!(h.domains.len(), 3);
        assert_eq!(h.causally_claimed_hops, 0, "no causal claim yet: {h:?}");
    }
    let copper = found
        .iter()
        .find(|h| path_of(h) == "ent-kestrel>ind-mining>cmdty-copper")
        .unwrap_or_else(|| panic!("the commodity chain: {paths:?}"));
    assert_eq!(copper.edges, vec!["in_industry", "produces_commodity"]);
    assert_eq!(copper.domains, vec!["companies", "industry", "commodities"]);
    assert_eq!(
        copper.sources,
        vec!["reference", "research"],
        "edge lineage"
    );
    assert!(
        (copper.strength - 1.0 * 0.9 * 0.8 * 0.9).abs() < 1e-12,
        "{}",
        copper.strength
    );
    let sterling = found
        .iter()
        .find(|h| path_of(h) == "ent-kestrel>ctry-gb>ccy-gbp")
        .unwrap_or_else(|| panic!("the currency chain: {paths:?}"));
    assert_eq!(sterling.domains, vec!["companies", "economy", "fx"]);
    assert!(
        sterling.strength > copper.strength,
        "strongest first: {paths:?}"
    );
    let through_northwind = found
        .iter()
        .find(|h| path_of(h) == "ent-northwind>ent-kestrel>ind-mining>cmdty-copper")
        .unwrap();
    assert_eq!(
        through_northwind.edges,
        vec!["customer", "in_industry", "produces_commodity"],
        "the inverse hop is named as the graph stores it"
    );
    assert_eq!(
        through_northwind.domains,
        vec!["companies", "industry", "commodities"],
        "two companies are one domain, crossed once"
    );

    // The causal graph comes to claim the first hop of the currency chain.
    let mut world = world;
    world
        .claim_causal(
            CausalEdge::new(
                "ent-kestrel",
                "ctry-gb",
                Mechanism::CurrencyTranslation,
                0.4,
                Duration::from_days(30),
                known(),
            )
            .unwrap()
            .with_confidence(0.8)
            .unwrap(),
        )
        .unwrap();
    let found = cross_domain_hypotheses(&world, now());
    let sterling = found
        .iter()
        .find(|h| path_of(h) == "ent-kestrel>ctry-gb>ccy-gbp")
        .unwrap();
    assert_eq!(sterling.causally_claimed_hops, 1);
    assert_eq!(sterling.unclaimed_hops(), 1);
    let copper = found
        .iter()
        .find(|h| path_of(h) == "ent-kestrel>ind-mining>cmdty-copper")
        .unwrap();
    assert_eq!(copper.causally_claimed_hops, 0);
    assert!(found.len() <= MAX_HYPOTHESES);
}

/// A platform-internal node is not a domain, so a chain through a thesis
/// node is not walked. Mutation: make `domain_of` return `Some("thesis")`
/// for `NodeKind::Thesis` — the chain is emitted and the assertion fails.
#[test]
fn a_chain_through_the_platforms_own_thesis_node_is_not_a_world_chain() {
    let mut world = WorldModel::new();
    for (id, kind, label) in [
        ("ent-a", EntityKind::Company, "A"),
        ("ccy-x", EntityKind::Currency, "X"),
    ] {
        world
            .graph_mut()
            .add_node(Node::entity(id, kind, label, known()))
            .unwrap();
    }
    world
        .graph_mut()
        .add_node(Node::new(
            "thesis-1",
            qip_world_model::graph::NodeKind::Thesis,
            "a thesis",
            known(),
        ))
        .unwrap();
    for (from, to, kind) in [
        ("ent-a", "thesis-1", RelationshipKind::ExpressesThesis),
        ("thesis-1", "ccy-x", RelationshipKind::ConcernsEntity),
    ] {
        world
            .relate(
                Relationship::new(from, to, kind, 1.0, "platform"),
                known(),
                known(),
                1.0,
            )
            .unwrap();
    }
    assert_eq!(
        world
            .graph()
            .paths_between("ent-a", "ccy-x", 3, now(), now())
            .len(),
        1,
        "premise: the graph itself connects the two through the thesis"
    );
    assert!(
        cross_domain_hypotheses(&world, now()).is_empty(),
        "a path through the platform's bookkeeping was read as a world chain"
    );
}
