//! The resolving authority as an object the graph holds.
//!
//! §8.1 asks the world model to hold the objects the platform reasons about,
//! and the resolution source was the one that lived only on a `Proposition`
//! in `qip-prediction`: reachable by whatever already held the proposition
//! and by nothing else. These tests hold two properties. First that the
//! authority is a node the thesis can be *traversed* to, because an
//! explanation that requires knowing which struct to open is not one the
//! audit trail can reproduce. Second — and this is the one that would rot
//! silently — that the node carries the proposition's own instants and not
//! the instant it was written, because a node stamped at write time makes
//! every point-in-time query return facts the platform did not hold, and the
//! backtest reads better than the platform was.

use qip_core::{Duration, Timestamp};
use qip_world_model::graph::NodeKind;
use qip_world_model::relationship::RelationshipKind;
use qip_world_model::resolution_source::{RESOLUTION_SOURCE_PREFIX, ResolutionSourceClaim};
use qip_world_model::world::WorldModel;

fn designated() -> Timestamp {
    Timestamp::from_civil(2026, 3, 2)
}

fn days_after(days: i64) -> Timestamp {
    designated().saturating_add(Duration::from_days(days))
}

fn claim(name: &str, thesis: &str, knowable_at: Timestamp) -> ResolutionSourceClaim {
    ResolutionSourceClaim::new(
        name,
        "official",
        [
            "close:obj-AAA".to_string(),
            "volatility:obj-AAA".to_string(),
        ],
        thesis,
        knowable_at,
        knowable_at,
    )
    .expect("a well-formed authority is accepted")
}

#[test]
fn a_thesis_can_be_traversed_to_the_authority_that_settles_it() {
    let mut world = WorldModel::new();

    // Premise: nothing of this kind exists yet, so the assertions below
    // cannot pass against a graph that was already holding one.
    assert!(
        world
            .graph()
            .nodes_of_kind(NodeKind::ResolutionSource)
            .is_empty(),
        "the fixture already held a resolution source"
    );

    let id = world.record_resolution_source(&claim("platform-market-data", "hyp-1", designated()));
    assert_eq!(
        id,
        format!("{RESOLUTION_SOURCE_PREFIX}:platform-market-data")
    );

    let node = world
        .graph()
        .node(&id)
        .expect("the authority was recorded as a node");
    assert_eq!(node.kind, NodeKind::ResolutionSource);
    assert_eq!(node.label, "platform-market-data");
    assert_eq!(
        node.attributes.get("authority").map(String::as_str),
        Some("official"),
        "the node does not say what kind of authority it is"
    );
    // Set order, not argument order: a replay that renders the attribute
    // differently is not a replay.
    assert_eq!(
        node.attributes.get("publishes").map(String::as_str),
        Some("close:obj-AAA,volatility:obj-AAA")
    );

    // The traversal itself, which is the whole point of the object being a
    // node: one hop forward from the thesis answers "who settles this".
    let hops = world.graph().neighbours(
        "hyp-1",
        Some(RelationshipKind::ResolvedBy),
        days_after(1),
        days_after(1),
    );
    assert_eq!(hops.len(), 1, "the thesis has no edge to its authority");
    assert_eq!(hops[0].relationship.to, id);

    // And it is reachable, so a walker that does not know the edge kind still
    // arrives — `reachable` is what an explanation surface uses.
    let reached = world
        .graph()
        .reachable("hyp-1", 2, days_after(1), days_after(1));
    assert_eq!(reached.get(&id), Some(&1));
}

#[test]
fn an_authority_is_stamped_with_the_instant_its_claim_was_knowable_and_not_a_later_one() {
    // The failure this guards: stamping the node and its edge with the
    // instant the settlement ran. Nothing would look wrong — the graph holds
    // the right authority — but a query asking what the platform knew on the
    // day it decided would find the edge missing until the day it graded, and
    // every replay before that instant would silently lose the provenance.
    let mut world = WorldModel::new();
    let knowable = designated();
    let id = world.record_resolution_source(&claim("platform-market-data", "hyp-1", knowable));

    let node = world.graph().node(&id).expect("recorded");
    assert_eq!(
        node.recorded_at,
        knowable,
        "the node is stamped with {} rather than the claim's own {}",
        node.recorded_at.to_rfc3339(),
        knowable.to_rfc3339()
    );

    // Premise: well after the fact, the edge is visible. Without this the
    // point-in-time assertion below would pass against an edge that was never
    // written at all.
    let later = days_after(30);
    assert_eq!(
        world
            .graph()
            .neighbours("hyp-1", Some(RelationshipKind::ResolvedBy), later, later)
            .len(),
        1,
        "the edge was never written, so asking when it became visible is vacuous"
    );

    // Knowable exactly at the claim's instant, and not one moment before it.
    assert_eq!(
        world
            .graph()
            .neighbours(
                "hyp-1",
                Some(RelationshipKind::ResolvedBy),
                knowable,
                knowable
            )
            .len(),
        1,
        "the authority is not visible at the instant the platform learned it"
    );
    let before = knowable.saturating_sub(Duration::from_secs(1));
    assert!(
        world
            .graph()
            .neighbours("hyp-1", Some(RelationshipKind::ResolvedBy), before, before)
            .is_empty(),
        "the authority is visible a second before the platform could know it"
    );
}

#[test]
fn an_authority_seen_again_keeps_the_instant_it_was_first_known_at() {
    // The failure this guards: re-recording the source on every settlement
    // and moving `recorded_at` forward each time, so the graph reports the
    // authority as first known at whatever cycle last touched it — a fact
    // that walks forward and can never be replayed against.
    let mut world = WorldModel::new();
    let first = designated();
    let id = world.record_resolution_source(&claim("platform-market-data", "hyp-1", first));

    // Premise: the first sighting is what we think it is.
    assert_eq!(
        world.graph().node(&id).expect("recorded").recorded_at,
        first
    );

    let second = days_after(10);
    let again = world.record_resolution_source(&claim("platform-market-data", "hyp-2", second));
    assert_eq!(again, id, "the same authority took a second node id");
    assert_eq!(
        world.graph().node(&id).expect("recorded").recorded_at,
        first,
        "a second sighting rewrote when the authority was first known"
    );

    // The second thesis still gets its own edge, stamped with its own instant.
    let hops =
        world
            .graph()
            .neighbours("hyp-2", Some(RelationshipKind::ResolvedBy), second, second);
    assert_eq!(
        hops.len(),
        1,
        "the second thesis has no edge to the authority"
    );
    assert_eq!(hops[0].recorded_at, second);
    assert!(
        world
            .graph()
            .neighbours("hyp-2", Some(RelationshipKind::ResolvedBy), first, first)
            .is_empty(),
        "the second thesis's edge is visible at the first thesis's instant"
    );
}

#[test]
fn an_authority_that_publishes_nothing_is_refused_rather_than_recorded_empty() {
    // Premise: the same call with one published metric is accepted, so the
    // refusal below is about the empty list and not about the fixture.
    assert!(
        ResolutionSourceClaim::new(
            "platform-market-data",
            "official",
            ["close:obj-AAA".to_string()],
            "hyp-1",
            designated(),
            designated(),
        )
        .is_ok(),
        "a source publishing one metric was refused, so this test proves nothing"
    );

    let refused = ResolutionSourceClaim::new(
        "platform-market-data",
        "official",
        Vec::new(),
        "hyp-1",
        designated(),
        designated(),
    );
    let error = refused.expect_err("a source publishing nothing can settle nothing");
    assert!(
        error.to_string().contains("publishes nothing"),
        "the refusal does not name the problem: {error}"
    );
}

#[test]
fn an_unnamed_authority_is_refused_rather_than_given_a_placeholder() {
    let refused = ResolutionSourceClaim::new(
        "   ",
        "official",
        ["close:obj-AAA".to_string()],
        "hyp-1",
        designated(),
        designated(),
    );
    let error = refused.expect_err("an authority nobody can name is not a node");
    assert!(
        error.to_string().contains("must be named"),
        "the refusal does not name the problem: {error}"
    );
}

#[test]
fn an_authority_named_with_the_fact_key_delimiter_is_refused() {
    // The failure this guards, and it is not hypothetical in shape: a fact
    // key is `from|kind|to`, so a name carrying a pipe makes two distinct
    // edges share one key, and the graph reads the second as a later version
    // of the first — two claims silently merged into one history.
    //
    // Premise: the same name without the delimiter is accepted.
    assert!(
        ResolutionSourceClaim::new(
            "platform-market-data",
            "official",
            ["close:obj-AAA".to_string()],
            "hyp-1",
            designated(),
            designated(),
        )
        .is_ok(),
        "the delimiter-free name was refused, so this test proves nothing"
    );

    let refused = ResolutionSourceClaim::new(
        "platform|market-data",
        "official",
        ["close:obj-AAA".to_string()],
        "hyp-1",
        designated(),
        designated(),
    );
    let error = refused.expect_err("a name carrying the fact-key delimiter merges two edges");
    assert!(
        error.to_string().contains("fact-key delimiter"),
        "the refusal does not name the problem: {error}"
    );

    let refused = ResolutionSourceClaim::new(
        "platform-market-data",
        "official",
        ["close:obj-AAA".to_string()],
        "hyp|1",
        designated(),
        designated(),
    );
    let error = refused.expect_err("a thesis carrying the fact-key delimiter merges two edges");
    assert!(
        error.to_string().contains("fact-key delimiter"),
        "the refusal does not name the problem: {error}"
    );
}

#[test]
fn an_authority_settling_no_thesis_is_refused_rather_than_left_an_orphan() {
    let refused = ResolutionSourceClaim::new(
        "platform-market-data",
        "official",
        ["close:obj-AAA".to_string()],
        "",
        designated(),
        designated(),
    );
    let error = refused.expect_err("an authority settling nothing is the gap, not the fix");
    assert!(
        error.to_string().contains("settles nothing"),
        "the refusal does not name the problem: {error}"
    );
}
