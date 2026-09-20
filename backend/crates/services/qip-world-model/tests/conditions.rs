//! Blueprint §9.1's conditions layer: the regime under which an edge holds,
//! and the conditions under which it is known to fail.
//!
//! The layer was absent until this, and its absence had a specific cost. The
//! platform's own temporal-precedence pass runs a test on every instrument
//! pair every cycle and, until now, kept only the results that cleared the
//! bar. Every result that did *not* clear was computed and dropped on the
//! floor — which is precisely the historical regime segmentation §9.1 asks
//! the conditions layer to be estimated from. The evidence was in hand and
//! nothing recorded it.
//!
//! What these tests hold is the part that is easy to get backwards: an
//! unasked question must never read as a negative answer, and a stale success
//! must never outrank a later refutation.

#![allow(clippy::panic_in_result_fn)]

use std::collections::BTreeSet;

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_world_model::causal::{CausalEdge, CausalGraph, ConditionStanding, Mechanism};

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(secs)
}

fn regimes(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn edge(cause: &str, effect: &str, recorded_at: Timestamp) -> Result<CausalEdge> {
    CausalEdge::new(
        cause,
        effect,
        Mechanism::TemporalPrecedence,
        0.4,
        Duration::from_days(1),
        recorded_at,
    )
}

#[test]
fn an_edge_tested_under_a_regime_and_refused_there_is_known_to_fail_in_it() -> Result<()> {
    let conditioned = edge("AAA", "BBB", at(1_000))?
        .with_conditions(regimes(&["bull/calm"]), regimes(&["bear/stressed"]))?;

    // Premise first: the edge really does carry both conditions, so that a
    // builder which silently dropped one could not pass this by leaving the
    // sets empty and letting `Untested` answer everything.
    assert!(
        !conditioned.holds_in.is_empty() && !conditioned.fails_in.is_empty(),
        "premise: the edge carries a condition on both sides"
    );

    assert_eq!(
        conditioned.in_regime("bear/stressed"),
        ConditionStanding::KnownToFail,
        "a regime the edge's own test was re-run in and refused under must read as a failure"
    );
    assert_eq!(
        conditioned.in_regime("bull/calm"),
        ConditionStanding::Holds,
        "a regime the edge's own test cleared its bar in must read as holding"
    );
    Ok(())
}

#[test]
fn a_regime_nobody_tested_the_edge_under_is_untested_rather_than_failing() -> Result<()> {
    // The failure this prevents is the one §9.4 names in the opposite
    // direction: an unasked question is not a negative answer. A conditions
    // layer that answered "does not hold" for every regime it had never seen
    // would retire every edge the moment the tape entered a new regime, which
    // is a control firing on the absence of evidence rather than on evidence.
    let conditioned = edge("AAA", "BBB", at(1_000))?
        .with_conditions(regimes(&["bull/calm"]), regimes(&["bear/stressed"]))?;

    assert_ne!(
        conditioned.in_regime("bull/calm"),
        ConditionStanding::Untested,
        "premise: this edge has been tested under at least one regime"
    );
    assert_eq!(
        conditioned.in_regime("sideways/elevated"),
        ConditionStanding::Untested,
        "a regime neither set names must read as unasked, never as a refutation"
    );
    Ok(())
}

#[test]
fn a_recorded_failure_outranks_a_recorded_hold_under_the_same_regime() -> Result<()> {
    // §9.4: "An edge that fails its conditions is retired, not patched." An
    // edge that cleared its bar under this regime once and has since been
    // refuted under it is refuted; answering `Holds` because the older
    // success is still on the record is the patch.
    let conditioned = edge("AAA", "BBB", at(1_000))?
        .with_conditions(regimes(&["bull/calm"]), regimes(&["bull/calm"]))?;

    assert!(
        conditioned.holds_in.contains("bull/calm") && conditioned.fails_in.contains("bull/calm"),
        "premise: the same regime is on both sides of the record"
    );
    assert_eq!(
        conditioned.in_regime("bull/calm"),
        ConditionStanding::KnownToFail,
        "a refutation under a regime must win over a stale success under the same one"
    );
    Ok(())
}

#[test]
fn a_condition_naming_no_regime_is_refused_rather_than_recorded_blank() -> Result<()> {
    // Refused, not filtered. A set quietly stripped of its blank entries
    // leaves the caller believing a condition was recorded when none was —
    // the caller bug that survives, which this workspace refuses to clamp
    // anywhere else either.
    let refused = edge("AAA", "BBB", at(1_000))?
        .with_conditions(regimes(&["   "]), BTreeSet::new())
        .is_err();
    assert!(
        refused,
        "a blank regime key was admitted as a condition; an edge carrying one reads as \
         conditioned and is not"
    );

    // And the other half, which is what separates a working gate from one
    // that refuses everything: a named regime is admitted.
    let admitted =
        edge("AAA", "BBB", at(1_000))?.with_conditions(regimes(&["bull/calm"]), BTreeSet::new())?;
    assert_eq!(admitted.in_regime("bull/calm"), ConditionStanding::Holds);
    Ok(())
}

#[test]
fn an_edge_carrying_a_blank_condition_is_refused_by_validate() -> Result<()> {
    // `validate` is where every path admitting an edge to a graph meets —
    // `WorldModel::claim_causal` calls it — so an edge assembled by some
    // route other than the builder, a decoded record among them, still
    // cannot carry a condition nobody can read.
    let mut smuggled = edge("AAA", "BBB", at(1_000))?;
    assert!(
        smuggled.validate().is_ok(),
        "premise: this edge is otherwise valid, so a refusal below is about the condition"
    );
    smuggled.fails_in.insert(String::new());
    assert!(
        smuggled.validate().is_err(),
        "a blank condition reached the graph through a path the builder does not guard"
    );
    Ok(())
}

#[test]
fn a_condition_failure_against_a_link_the_graph_never_claimed_marks_nothing() -> Result<()> {
    // Zero is a real answer, not a silent success. The platform's pass tests
    // every ordered pair and most pairs have no edge at all; a caller that
    // read the return as "recorded" would report a segmentation that never
    // happened.
    let mut graph = CausalGraph::new();
    graph.add(edge("AAA", "BBB", at(1_000))?);
    assert_eq!(graph.len(), 1, "premise: the graph holds exactly one link");

    let marked = graph
        .record_condition_failure("CCC", "DDD", "bear/stressed", at(2_000))?
        .marked;
    assert_eq!(
        marked, 0,
        "a link nobody claimed was reported as having been marked"
    );
    let marked = graph
        .record_condition_failure("AAA", "BBB", "bear/stressed", at(2_000))?
        .marked;
    assert_eq!(
        marked, 1,
        "the link the graph does hold was not marked, so the writer cannot fire at all"
    );
    Ok(())
}

#[test]
fn a_condition_failure_is_not_written_onto_an_edge_that_was_not_yet_knowable() -> Result<()> {
    // Point in time. An edge recorded after the instant being asked about
    // cannot have been tested at that instant, and marking it would write a
    // condition into the past — the look-ahead a backtest cannot see because
    // the record itself would claim to have been available.
    let mut graph = CausalGraph::new();
    graph.add(edge("AAA", "BBB", at(5_000))?);

    let marked = graph
        .record_condition_failure("AAA", "BBB", "bear/stressed", at(1_000))?
        .marked;
    assert_eq!(marked, 0, "an edge from the future was conditioned");
    assert!(
        graph.edges()[0].fails_in.is_empty(),
        "the edge carries a condition recorded before it existed"
    );

    // The admitting half: at an instant the edge was knowable, it marks.
    let marked = graph
        .record_condition_failure("AAA", "BBB", "bear/stressed", at(9_000))?
        .marked;
    assert_eq!(marked, 1, "a knowable edge was not marked");
    Ok(())
}

#[test]
fn a_condition_failure_naming_no_regime_is_refused_by_the_graph() -> Result<()> {
    let mut graph = CausalGraph::new();
    graph.add(edge("AAA", "BBB", at(1_000))?);

    assert!(
        graph
            .record_condition_failure("AAA", "BBB", "  ", at(2_000))
            .is_err(),
        "a failure filed under no condition is a failure no reader can ever match to one"
    );
    assert!(
        graph.edges()[0].fails_in.is_empty(),
        "the refused failure was recorded anyway"
    );
    assert!(
        graph
            .record_condition_failure("AAA", "BBB", "bear/stressed", at(2_000))
            .is_ok(),
        "a named regime must still be admitted, or the gate refuses everything"
    );
    Ok(())
}

#[test]
fn recording_a_condition_failure_does_not_make_a_stale_graph_read_fresh() -> Result<()> {
    // The load-bearing refusal. `CausalGraphFreshness::assess` reads
    // `last_updated` and narrows the platform's sizing when the graph goes
    // stale. If a pass whose only news is that the graph's own edges are
    // failing moved that instant, the degradation control would be switched
    // off by the very evidence it exists to react to — a control that reads
    // as protection and cannot fire, which this repository has shipped once
    // already.
    let mut graph = CausalGraph::new();
    graph.add(edge("AAA", "BBB", at(1_000))?);
    let before = graph.last_updated();
    assert_eq!(
        before,
        Some(at(1_000)),
        "premise: the graph records the instant it absorbed the edge"
    );

    let marked = graph
        .record_condition_failure("AAA", "BBB", "bear/stressed", at(9_000))?
        .marked;
    assert_eq!(marked, 1, "premise: the failure really was recorded");
    assert_eq!(
        graph.last_updated(),
        before,
        "a graph whose edges are failing their conditions reported itself as freshly updated"
    );
    Ok(())
}

#[test]
fn the_graph_names_the_edges_failing_the_regime_in_force_for_their_own_effect() -> Result<()> {
    // One regime label over a whole graph would match an edge against
    // conditions measured on somebody else's tape, so the regime is asked for
    // per effect. This holds that the per-effect lookup is really used: two
    // edges carry the same failing regime key and only the one whose effect
    // is *in* that regime is named.
    let mut graph = CausalGraph::new();
    graph.add(edge("DRV", "AAA", at(1_000))?.with_conditions(BTreeSet::new(), regimes(&["bear"]))?);
    graph.add(edge("DRV", "BBB", at(1_000))?.with_conditions(BTreeSet::new(), regimes(&["bear"]))?);
    assert_eq!(graph.len(), 2, "premise: two conditioned edges");

    let failing = graph.failing_their_regime(at(2_000), |effect| {
        if effect == "AAA" {
            "bear".to_string()
        } else {
            "bull".to_string()
        }
    });
    assert_eq!(
        failing.len(),
        1,
        "the per-effect regime lookup is not being used: {failing:?}"
    );
    assert_eq!(failing[0].effect, "AAA");
    Ok(())
}
