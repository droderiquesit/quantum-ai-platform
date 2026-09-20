//! ADR 0087: an edge that fails its conditions is retired, not patched.
//!
//! Blueprint §9.4's second handling. Until this, the platform's own
//! precedence pass could refuse a claim every cycle for a year and the graph
//! would go on propagating along it, because a `KnownToFail` standing was a
//! mark nothing acted on. These tests hold the four things retirement must
//! do and the three it must not: fire on the Nth consecutive failure and
//! not the (N−1)th; leave `propagate`, `incoming` and `explanations`; never
//! be re-estimated back; keep the record in `edges()`; reset on a regime
//! boundary; reset on a recorded hold; and stay visible as of an instant
//! before it happened, because a backtest that saw a future refutation
//! would be reasoning from a graph it did not have.

#![allow(clippy::panic_in_result_fn)]

use std::collections::BTreeSet;

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_world_model::causal::{
    CausalEdge, CausalGraph, EdgeStanding, FailureRun, Mechanism, RETIREMENT_CONSECUTIVE_FAILURES,
    Retirement, SupportingClaim,
};
use qip_world_model::state::ChangeKind;
use qip_world_model::world::WorldModel;

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

/// A graph holding one edge `AAA -> BBB` recorded at 1_000, which the
/// precedence pass once cleared under `bull`.
fn one_edge_graph() -> Result<CausalGraph> {
    let mut graph = CausalGraph::new();
    graph.add(edge("AAA", "BBB", at(1_000))?.with_conditions(regimes(&["bull"]), BTreeSet::new())?);
    Ok(graph)
}

/// Record `count` failures of `AAA -> BBB` under `regime`, one per instant
/// starting at `from`, and return the instant of the last one.
fn fail(graph: &mut CausalGraph, regime: &str, from: i64, count: usize) -> Result<Timestamp> {
    let mut last = at(from);
    for i in 0..count {
        last = at(from + i as i64 * 100);
        let report = graph.record_condition_failure("AAA", "BBB", regime, last)?;
        assert_eq!(
            report.marked, 1,
            "premise: each failure marks the one live edge"
        );
    }
    Ok(last)
}

#[test]
fn an_edge_retires_on_the_third_consecutive_failure_in_one_regime_and_not_on_the_second()
-> Result<()> {
    // The failure this prevents in both directions: a retirement on the
    // first failure would make "known to fail" and "retired" the same event
    // and §9.4 names them as two; a retirement that never came would leave
    // the clause unbuilt, which is where the row stood.
    let mut graph = one_edge_graph()?;

    let before_last = fail(
        &mut graph,
        "bear",
        2_000,
        RETIREMENT_CONSECUTIVE_FAILURES - 1,
    )?;
    assert!(
        graph.retired(before_last).is_empty(),
        "two consecutive failures must not retire; the second is the same window one bar on"
    );
    assert_eq!(
        graph.edges()[0].failure_run,
        Some(FailureRun {
            regime: "bear".to_string(),
            failures: RETIREMENT_CONSECUTIVE_FAILURES - 1,
            began: at(2_000),
        }),
        "premise: the run is being counted"
    );

    let third = at(2_000 + (RETIREMENT_CONSECUTIVE_FAILURES as i64 - 1) * 100);
    let report = graph.record_condition_failure("AAA", "BBB", "bear", third)?;
    assert_eq!(report.marked, 1);
    assert_eq!(
        report.retired.len(),
        1,
        "the Nth consecutive failure retires the edge"
    );
    assert_eq!(
        graph.edges()[0].retired,
        Some(Retirement {
            regime: "bear".to_string(),
            at: third,
            consecutive_failures: RETIREMENT_CONSECUTIVE_FAILURES,
            run_began: at(2_000),
        }),
        "the retirement records why and when"
    );
    assert!(
        graph.edges()[0].failure_run.is_none(),
        "the run is closed into the retirement rather than left counting"
    );

    // A fourth failure lands on a retired edge: not marked, not re-retired.
    let fourth = graph.record_condition_failure("AAA", "BBB", "bear", at(9_000))?;
    assert_eq!(fourth.marked, 0, "a retired edge takes no further failures");
    assert!(fourth.retired.is_empty());
    Ok(())
}

#[test]
fn the_retirement_count_is_the_three_adr_0087_argues() {
    // Pinned on purpose, and on its own so that a mutation of the number
    // fails a test naming the record that argues it rather than a premise
    // inside some other test. The ADR's argument is a debounce over a
    // sliding window, not a significance level; a lane that moves this
    // number owes the ADR an amendment saying what changed in the pass.
    assert_eq!(RETIREMENT_CONSECUTIVE_FAILURES, 3);
}

#[test]
fn a_retired_edge_leaves_propagate_incoming_and_explanations_and_stays_in_the_graph() -> Result<()>
{
    let mut graph = one_edge_graph()?;
    let live_at = at(1_500);
    // Premise: before retirement all three readers see the edge.
    assert_eq!(
        graph.incoming("BBB", live_at).len(),
        1,
        "premise: incoming sees it"
    );
    assert_eq!(
        graph.explanations("BBB", live_at).len(),
        1,
        "premise: explanations sees it"
    );
    assert!(
        graph
            .propagate("AAA", 1.0, 2, 0.001, live_at, live_at)
            .effects
            .iter()
            .any(|effect| effect.target == "BBB"),
        "premise: a shock at AAA reaches BBB"
    );

    let retired_at = fail(&mut graph, "bear", 2_000, RETIREMENT_CONSECUTIVE_FAILURES)?;
    let after = at(9_000);
    assert!(
        graph.incoming("BBB", after).is_empty(),
        "a retired edge must not explain a move"
    );
    assert!(
        graph.explanations("BBB", after).is_empty(),
        "a retired edge must not be offered as an explanation"
    );
    assert!(
        graph
            .propagate("AAA", 1.0, 2, 0.001, after, after)
            .effects
            .iter()
            .all(|effect| effect.target != "BBB"),
        "a shock must not propagate along a retired edge"
    );
    assert!(graph.outgoing("AAA", after).is_empty());

    // Retired, not deleted: the record survives under its own standing.
    assert_eq!(graph.len(), 1, "the edge stays in the graph");
    assert_eq!(graph.edges()[0].standing(), EdgeStanding::Retired);
    assert_eq!(graph.retired(after).len(), 1);
    assert_eq!(
        graph.retired(after)[0].retired.as_ref().map(|r| r.at),
        Some(retired_at)
    );
    Ok(())
}

#[test]
fn a_failure_under_a_different_regime_starts_a_fresh_run_and_nothing_carries_across_the_boundary()
-> Result<()> {
    // Mirrors the kernel's regime-transition marker: a first sighting in a
    // regime is a sighting, never a crossing. Two failures under `bear`, one
    // under `bull`, then two more under `bear` is five failures and no
    // retirement, because no regime ever saw three in a row.
    let mut graph = one_edge_graph()?;
    fail(&mut graph, "bear", 2_000, 2)?;
    assert_eq!(
        graph.edges()[0].failure_run.as_ref().map(|r| r.failures),
        Some(2),
        "premise: two failures under bear are on the run"
    );

    let flipped = graph.record_condition_failure("AAA", "BBB", "bull", at(3_000))?;
    assert!(
        flipped.retired.is_empty(),
        "the first sighting under bull retires nothing"
    );
    assert_eq!(
        graph.edges()[0].failure_run,
        Some(FailureRun {
            regime: "bull".to_string(),
            failures: 1,
            began: at(3_000),
        }),
        "the run restarts at one under the new regime"
    );

    let back = fail(&mut graph, "bear", 4_000, 2)?;
    assert!(
        graph.retired(back).is_empty(),
        "five failures across a boundary are not three in a row; nothing carried over"
    );
    assert_eq!(
        graph.edges()[0].failure_run.as_ref().map(|r| r.failures),
        Some(2),
        "the run under bear restarted from one at the return"
    );

    let third = graph.record_condition_failure("AAA", "BBB", "bear", at(4_200))?;
    assert_eq!(
        third.retired.len(),
        1,
        "three in a row under one regime retires"
    );
    Ok(())
}

#[test]
fn a_hold_recorded_for_the_pair_breaks_the_run() -> Result<()> {
    // The precedence pass writes a pass as a *new* edge carrying
    // `holds_in = {regime}` rather than marking the edge it already holds.
    // Without the reset in `add`, "consecutive" would be "cumulative" and
    // a link that clears its bar every other cycle would retire on the
    // third miss.
    let mut graph = one_edge_graph()?;
    fail(&mut graph, "bear", 2_000, 2)?;
    assert_eq!(
        graph.edges()[0].failure_run.as_ref().map(|r| r.failures),
        Some(2),
        "premise: two failures on the run"
    );

    // The pass clears its bar under `bear`: a new edge for the same pair.
    graph.add(edge("AAA", "BBB", at(2_500))?.with_conditions(regimes(&["bear"]), BTreeSet::new())?);
    assert!(
        graph.edges()[0].failure_run.is_none(),
        "a hold under the run's regime clears the older edge's run"
    );

    // Two more failures mark *both* edges now, and neither reaches three.
    for (i, instant) in [3_000, 3_100].into_iter().enumerate() {
        let report = graph.record_condition_failure("AAA", "BBB", "bear", at(instant))?;
        assert_eq!(
            report.marked,
            2,
            "premise: both live edges take failure {}",
            i + 1
        );
        assert!(
            report.retired.is_empty(),
            "two after a hold is not three in a row"
        );
    }
    let third = graph.record_condition_failure("AAA", "BBB", "bear", at(3_200))?;
    assert_eq!(
        third.retired.len(),
        2,
        "the third after the hold retires both edges of the pair"
    );

    // A hold under some *other* regime does not touch a `bear` run.
    let mut other = one_edge_graph()?;
    fail(&mut other, "bear", 2_000, 2)?;
    other.add(edge("AAA", "BBB", at(2_500))?.with_conditions(regimes(&["bull"]), BTreeSet::new())?);
    assert_eq!(
        other.edges()[0].failure_run.as_ref().map(|r| r.failures),
        Some(2),
        "a hold under bull says nothing about a run under bear"
    );
    Ok(())
}

#[test]
fn a_retired_edge_is_still_readable_as_of_an_instant_before_its_retirement() -> Result<()> {
    // Point in time in the direction the domain rule does not usually have
    // to state: a refutation the platform obtained on Wednesday must not
    // be visible to a question asked about Monday. Otherwise every backtest
    // over the graph reasons from a retirement it had not yet obtained, and
    // the results look better rather than anomalous.
    let mut graph = one_edge_graph()?;
    let retired_at = fail(&mut graph, "bear", 2_000, RETIREMENT_CONSECUTIVE_FAILURES)?;
    assert!(
        graph.edges()[0].is_retired(),
        "premise: the edge is retired at {retired_at:?}"
    );

    let before = at(1_900);
    assert_eq!(
        graph.incoming("BBB", before).len(),
        1,
        "as of an instant before the retirement the edge was live"
    );
    assert_eq!(graph.explanations("BBB", before).len(), 1);
    assert!(
        graph.retired(before).is_empty(),
        "nothing was retired yet as of then"
    );
    assert!(
        graph.incoming("BBB", retired_at).is_empty(),
        "as of the retiring instant itself the edge is gone"
    );
    Ok(())
}

#[test]
fn a_retired_edge_is_never_re_estimated_back_and_its_claims_report_unmatched() -> Result<()> {
    let mut graph = one_edge_graph()?;
    fail(&mut graph, "bear", 2_000, RETIREMENT_CONSECUTIVE_FAILURES)?;
    let strength_at_retirement = graph.edges()[0].strength;
    assert!(graph.edges()[0].is_retired(), "premise: retired");

    let now = at(5_000);
    let claim = SupportingClaim::new("AAA", "BBB", Mechanism::TemporalPrecedence, 0.9, at(4_900))
        .with_evidence(vec!["fresh".to_string()]);
    let report = graph.reestimate([claim], Duration::from_days(30), now)?;

    // Bit-for-bit: the property is that the number did not move at all,
    // not that it stayed within a tolerance a re-estimation could hide in.
    assert_eq!(
        graph.edges()[0].strength.to_bits(),
        strength_at_retirement.to_bits(),
        "a claim inside the horizon must not move a retired edge's strength"
    );
    assert!(
        graph.edges()[0].decayed_at.is_none(),
        "a retired edge is not reported decayed either; it is retired"
    );
    assert!(report.updated.is_empty() && report.decayed.is_empty());
    assert_eq!(
        report.unmatched.len(),
        1,
        "the claim names a link the graph no longer holds live, so it is unmatched"
    );
    assert!(
        !report.refreshed,
        "nothing used means the graph did not refresh"
    );

    // A re-established link is a new edge, and *that* one takes the claim.
    graph.add(edge("AAA", "BBB", at(4_950))?.with_conditions(regimes(&["bear"]), BTreeSet::new())?);
    let claim = SupportingClaim::new("AAA", "BBB", Mechanism::TemporalPrecedence, 0.9, at(4_960));
    let again = graph.reestimate([claim], Duration::from_days(30), now)?;
    assert_eq!(again.updated.len(), 1, "the new edge is re-estimated");
    assert_eq!(
        graph.edges()[0].strength.to_bits(),
        strength_at_retirement.to_bits(),
        "and the retired one still is not"
    );
    Ok(())
}

#[test]
fn an_edge_claimed_already_retired_or_mid_run_is_refused() -> Result<()> {
    let mut retired = edge("AAA", "BBB", at(1_000))?;
    retired.retired = Some(Retirement {
        regime: "bear".to_string(),
        at: at(900),
        consecutive_failures: RETIREMENT_CONSECUTIVE_FAILURES,
        run_began: at(700),
    });
    let refusal = retired
        .validate()
        .expect_err("a retired edge is never re-admitted");
    assert!(
        refusal.to_string().contains("never re-admitted"),
        "the refusal names the rule: {refusal}"
    );

    let mut mid_run = edge("AAA", "BBB", at(1_000))?;
    mid_run.failure_run = Some(FailureRun {
        regime: "bear".to_string(),
        failures: RETIREMENT_CONSECUTIVE_FAILURES - 1,
        began: at(700),
    });
    let refusal = mid_run.validate().expect_err("a smuggled run is refused");
    assert!(
        refusal.to_string().contains("starts its own run"),
        "the refusal names the rule: {refusal}"
    );

    // The premise the two refusals rest on: the same edge with neither is
    // admitted, so the refusals are about the marks and not the edge.
    edge("AAA", "BBB", at(1_000))?.validate()?;
    Ok(())
}

#[test]
fn the_world_model_journals_a_retirement_under_its_own_kind_and_counts_it() -> Result<()> {
    // The kernel's seam: `record_causal_condition_failure` keeps returning
    // the marked count the stage detail prints, and the retirement reaches
    // the record through the journal and `statistics()` instead — the two
    // surfaces the kernel already reads from this model.
    let mut world = WorldModel::new();
    world.claim_causal(
        edge("AAA", "BBB", at(1_000))?.with_conditions(regimes(&["bull"]), BTreeSet::new())?,
    )?;
    let retirements = |world: &WorldModel| {
        world
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::CausalEdgeRetired)
            .count()
    };
    assert_eq!(retirements(&world), 0, "premise: nothing retired yet");
    assert_eq!(
        world.statistics().get("causal_claims_retired").copied(),
        Some(0),
        "premise: the surface counts zero retired"
    );

    for i in 0..RETIREMENT_CONSECUTIVE_FAILURES - 1 {
        let marked = world.record_causal_condition_failure(
            "AAA",
            "BBB",
            "bear",
            at(2_000 + i as i64 * 100),
        )?;
        assert_eq!(marked, 1, "premise: the kernel's count still counts");
    }
    assert_eq!(
        retirements(&world),
        0,
        "a failure that does not retire journals nothing"
    );

    let marked = world.record_causal_condition_failure("AAA", "BBB", "bear", at(2_200))?;
    assert_eq!(
        marked, 1,
        "the retiring failure is still a marked failure to the kernel"
    );
    assert_eq!(retirements(&world), 1, "the retirement is journaled once");
    let entry = world
        .changes()
        .iter()
        .find(|change| change.kind == ChangeKind::CausalEdgeRetired)
        .expect("the entry exists");
    assert_eq!(entry.subject, "AAA->BBB");
    assert!(
        entry.description.contains("3 consecutive pass(es)")
            && entry.description.contains("\"bear\""),
        "the entry names the run and the regime: {}",
        entry.description
    );
    assert_eq!(
        world.statistics().get("causal_claims_retired").copied(),
        Some(1),
        "the system surface counts the retired edge"
    );
    assert_eq!(
        world.statistics().get("causal_claims").copied(),
        Some(1),
        "and the total is unchanged, which is why the retired count exists"
    );
    assert_eq!(world.causal().retired(at(2_200)).len(), 1);
    Ok(())
}

#[test]
fn a_retired_edge_stands_retired_whatever_its_confounders() -> Result<()> {
    // `Retired` outranks `Suggestive`: a reader of `standing()` asks how far
    // an edge may be relied on, and the answer for a retired edge is "not at
    // all" whether or not a confounder was ever recorded against it.
    let mut graph = CausalGraph::new();
    graph.add(
        edge("AAA", "BBB", at(1_000))?
            .with_confounders(BTreeSet::new(), regimes(&["macro-factor"]))
            .with_conditions(regimes(&["bull"]), BTreeSet::new())?,
    );
    assert_eq!(
        graph.edges()[0].standing(),
        EdgeStanding::Suggestive,
        "premise: a suspected confounder makes it suggestive"
    );
    fail(&mut graph, "bear", 2_000, RETIREMENT_CONSECUTIVE_FAILURES)?;
    assert_eq!(graph.edges()[0].standing(), EdgeStanding::Retired);
    assert_eq!(EdgeStanding::Retired.as_str(), "retired");
    Ok(())
}
