//! WORLD-003: the world model is a temporal graph whose entities span
//! companies, people, assets, supply chains, countries, products, venues,
//! contracts, weather, logistics and markets, and every event and
//! relationship carries the time it applies to.
//!
//! The failure these tests prevent: six of those eleven had no kind, so a
//! supply chain or a shipping lane could be held only by registering it as a
//! `Company`; the kind an entity did have travelled as a string attribute
//! nothing read; and an event node carried only the instant it was learned,
//! so "what had happened by Monday" could not be asked of the nodes at all.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{EntityId, Timestamp};
use qip_entity_resolution::entity::Entity;
use qip_world_model::graph::{EntityKind, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use qip_world_model::world::WorldModel;

fn day(n: u32) -> Timestamp {
    Timestamp::from_civil(2026, 8, n)
}

/// How one of the eleven named types is held: ten are entities with a typed
/// kind, and an asset is a financial object, which is its own node kind.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Typed {
    Entity(EntityKind),
    Asset,
}

/// One fixture of each of the eleven types the requirement names, in its
/// order: (the name in the requirement, node id, how it is typed).
fn the_eleven() -> [(&'static str, &'static str, Typed); 11] {
    [
        (
            "companies",
            "ent-northwind",
            Typed::Entity(EntityKind::Company),
        ),
        (
            "people",
            "ent-ada-okafor",
            Typed::Entity(EntityKind::Person),
        ),
        ("assets", "obj-NWD", Typed::Asset),
        (
            "supply chains",
            "ent-euv-lithography-chain",
            Typed::Entity(EntityKind::SupplyChain),
        ),
        ("countries", "ent-nl", Typed::Entity(EntityKind::Country)),
        (
            "products",
            "ent-n3-wafer",
            Typed::Entity(EntityKind::Product),
        ),
        ("venues", "ent-xams", Typed::Entity(EntityKind::Venue)),
        (
            "contracts",
            "ent-offtake-2026",
            Typed::Entity(EntityKind::Contract),
        ),
        (
            "weather",
            "ent-storm-ciaran",
            Typed::Entity(EntityKind::Weather),
        ),
        (
            "logistics",
            "ent-rotterdam-lane",
            Typed::Entity(EntityKind::Logistics),
        ),
        (
            "markets",
            "ent-spot-wafer-market",
            Typed::Entity(EntityKind::Market),
        ),
    ]
}

#[test]
fn one_fixture_of_each_of_the_eleven_named_entity_types_is_a_typed_node_and_an_earlier_as_of_query_excludes_what_came_later()
 {
    // Known instants. Everything is registered on day 1; each fixture's
    // event happens on day 10 and is learned on day 11; each fixture's
    // relationship becomes true on day 20 and is learned on day 21.
    let (registered, happened, heard, true_from, learned) =
        (day(1), day(10), day(11), day(20), day(21));
    let mut model = WorldModel::new();
    let fixtures = the_eleven();

    for (name, id, typed) in fixtures {
        match typed {
            Typed::Entity(kind) => model
                .add_entity(Entity::new(
                    EntityId::from_string(id),
                    kind,
                    name,
                    registered,
                ))
                .unwrap(),
            Typed::Asset => model
                .graph_mut()
                .add_node(Node::new(id, NodeKind::FinancialObject, name, registered))
                .unwrap(),
        }
        let event = format!("event:{id}");
        model
            .graph_mut()
            .add_node(Node::event(
                &event,
                format!("something happened to {name}"),
                happened,
                heard,
            ))
            .unwrap();
        model
            .relate(
                Relationship::new(&event, id, RelationshipKind::ConcernsEntity, 0.9, "fixture"),
                true_from,
                learned,
                0.8,
            )
            .unwrap();
    }

    // Each is a typed node: the kind is a value of a type, read back as one.
    let mut kinds = std::collections::BTreeSet::new();
    for (name, id, typed) in fixtures {
        let node = model
            .graph()
            .node(id)
            .unwrap_or_else(|| panic!("the {name} fixture is not in the graph"));
        match typed {
            Typed::Entity(kind) => {
                assert_eq!(node.kind, NodeKind::Entity, "{name}");
                assert_eq!(
                    node.entity_kind,
                    Some(kind),
                    "{name} is not typed as {kind:?}"
                );
                assert_eq!(
                    model
                        .graph()
                        .entities_of_kind(kind)
                        .iter()
                        .map(|n| n.id.as_str())
                        .collect::<Vec<_>>(),
                    vec![id],
                    "a query for {kind:?} does not return exactly the {name} fixture"
                );
                kinds.insert(kind);
            }
            Typed::Asset => {
                assert_eq!(node.kind, NodeKind::FinancialObject, "{name}");
                assert_eq!(node.entity_kind, None, "an asset is not an entity");
            }
        }
    }
    assert_eq!(
        kinds.len(),
        10,
        "the ten entity fixtures share a kind: {kinds:?}"
    );

    // Premise for the as-of half: asked late enough, the graph does hold all
    // eleven events and all eleven relationships. Otherwise "an earlier
    // query excludes them" would be true of a graph that never had them.
    assert_eq!(
        model.graph().events_at(day(28), day(28)).len(),
        11,
        "premise"
    );
    assert_eq!(
        model.graph().facts_at(day(28), day(28)).len(),
        11,
        "premise"
    );

    // Every event carries the time it applies to, as distinct from when the
    // platform learned of it.
    for event in model.graph().events_at(day(28), day(28)) {
        assert_eq!(event.occurred_at, Some(happened));
        assert_eq!(event.recorded_at, heard);
    }

    // An as-of query at an earlier instant excludes what became true after
    // it, on each time dimension separately.
    assert!(
        model.graph().events_at(day(9), day(28)).is_empty(),
        "an event is in the state of the world a day before it happened"
    );
    assert!(
        model.graph().events_at(day(28), day(10)).is_empty(),
        "an event is known a day before the platform heard of it"
    );
    assert!(
        model.graph().facts_at(day(19), day(28)).is_empty(),
        "a relationship holds a day before it became true"
    );
    assert!(
        model.graph().facts_at(day(28), day(20)).is_empty(),
        "a relationship is known a day before the platform learned it"
    );
    // And between the two: the events have happened and are known, the
    // relationships are not yet true.
    assert_eq!(model.graph().events_at(day(15), day(15)).len(), 11);
    assert!(model.graph().facts_at(day(15), day(15)).is_empty());
}

#[test]
fn an_entity_with_no_kind_and_an_event_with_no_instant_are_refused_at_the_write_seam() {
    let mut model = WorldModel::new();

    // Premise: the typed constructors are admitted, so the refusals below
    // are about the missing type and the missing instant.
    model
        .graph_mut()
        .add_node(Node::entity(
            "ok-entity",
            EntityKind::Product,
            "a product",
            day(1),
        ))
        .unwrap();
    model
        .graph_mut()
        .add_node(Node::event("ok-event", "it shipped", day(2), day(3)))
        .unwrap();
    assert_eq!(model.graph().node_count(), 2);

    let untyped = model
        .graph_mut()
        .add_node(Node::new("untyped", NodeKind::Entity, "a what?", day(1)))
        .expect_err("an entity with no kind was written");
    assert!(
        untyped.message().contains("no entity kind"),
        "{}",
        untyped.message()
    );

    let undated = model
        .graph_mut()
        .add_node(Node::new("undated", NodeKind::Event, "it happened", day(3)))
        .expect_err("an event with no instant was written");
    assert!(
        undated.message().contains("no instant it applies to"),
        "{}",
        undated.message()
    );
    assert_eq!(model.graph().node_count(), 2, "a refused node was written");
}
