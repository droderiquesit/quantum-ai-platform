//! Scoring, routing and determinism.
//!
//! The property that matters most here is not that any particular source
//! scores any particular number — it is that the same inputs always produce
//! the same decisions. A registration decision is a legal position, and one
//! that cannot be reproduced from the record cannot be defended.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{AGENT, candidate, licensed_for, now, paid_candidate, permissive_robots, probe_for};
use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::probe::InMemoryProbe;
use qip_data_finder::scoring::{Routing, RoutingClass, SourceScores};

const URL_A: &str = "https://a.example/data/prices.json";
const URL_B: &str = "https://b.example/data/prices.json";

fn probe_for_both() -> InMemoryProbe {
    let mut probe = probe_for(URL_A, "a.example", permissive_robots());
    probe = probe.with_robots("b.example", permissive_robots());
    probe = probe.with_head(URL_B, common::ok_head());
    probe.with_sample(URL_B, common::sample(common::QUOTE_PAYLOAD))
}

fn run(seed: u64) -> Result<Vec<(String, RoutingClass, f64)>> {
    let mut finder = DataFinder::new(FinderConfig::new(
        AGENT,
        Usage::Derive,
        "market-data",
        seed,
    )?);
    let candidates = vec![
        candidate("beta", URL_B, licensed_for(&[Usage::Derive])?, &["EU0002"])?,
        candidate("alpha", URL_A, licensed_for(&[Usage::Derive])?, &["EU0001"])?,
    ];
    let mut probe = probe_for_both();
    let decisions = finder.assess(candidates, &mut probe, now())?;
    Ok(decisions
        .iter()
        .map(|decision| {
            (
                decision.source_id().to_string(),
                decision.outcome().routing_class(),
                decision
                    .scores()
                    .map(SourceScores::composite)
                    .unwrap_or_default(),
            )
        })
        .collect())
}

#[test]
fn two_runs_over_the_same_candidates_with_one_seed_give_identical_decisions() -> Result<()> {
    let first = run(4_242)?;
    let second = run(4_242)?;
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    Ok(())
}

#[test]
fn candidates_are_decided_in_identifier_order_however_they_arrive() -> Result<()> {
    // The list above is built beta-first. A finder that scored in arrival
    // order would produce different uniqueness scores for the two runs, and
    // the difference would depend on how the caller assembled the vector.
    let ordered = run(4_242)?;
    let ids: Vec<&str> = ordered.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, ["alpha", "beta"]);
    Ok(())
}

#[test]
fn a_different_seed_moves_nothing_that_a_decision_rests_on() -> Result<()> {
    // The seed only breaks ties. Two seeds must not disagree about whether a
    // source is collected, or the seed would be deciding legality by proxy.
    let one = run(1)?;
    let other = run(9_999_999)?;
    let classes_one: Vec<RoutingClass> = one.iter().map(|(_, class, _)| *class).collect();
    let classes_other: Vec<RoutingClass> = other.iter().map(|(_, class, _)| *class).collect();
    assert_eq!(classes_one, classes_other);
    Ok(())
}

#[test]
fn a_duplicate_of_a_registered_source_scores_no_uniqueness() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
    let mut probe = probe_for_both();

    let first = finder.assess(
        vec![candidate(
            "alpha",
            URL_A,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let unique = first[0]
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?
        .uniqueness();
    assert!(unique > 0.99, "the first source covers nothing already held");

    // Same instruments, different host: a perfect duplicate.
    let second = finder.assess(
        vec![candidate(
            "beta",
            URL_B,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let duplicate = second[0]
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?
        .uniqueness();
    assert!(
        duplicate < 0.01,
        "a duplicate of a registered source added nothing, and scored {duplicate}"
    );
    Ok(())
}

#[test]
fn an_expensive_source_scores_worse_on_cost_than_a_free_one() -> Result<()> {
    let mut finder = DataFinder::new(FinderConfig::new(AGENT, Usage::Derive, "market-data", 7)?);
    let mut probe = probe_for_both();

    let free = finder.assess(
        vec![candidate(
            "alpha",
            URL_A,
            licensed_for(&[Usage::Derive])?,
            &["EU0001"],
        )?],
        &mut probe,
        now(),
    )?;
    let expensive = finder.assess(
        vec![paid_candidate(
            "beta",
            URL_B,
            licensed_for(&[Usage::Derive])?,
            5_000,
        )?],
        &mut probe,
        now(),
    )?;

    let free_cost = free[0]
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?
        .cost_efficiency();
    let paid_cost = expensive[0]
        .scores()
        .ok_or_else(|| Error::not_found("scores"))?
        .cost_efficiency();
    assert!(free_cost > paid_cost, "{free_cost} should exceed {paid_cost}");
    assert!((free_cost - 1.0).abs() < 1e-9);
    Ok(())
}

#[test]
fn every_score_is_a_fraction_and_anything_else_is_refused() -> Result<()> {
    for bad in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
        let error = SourceScores::new(bad, 0.5, 0.5, 0.5, 0.5).unwrap_err();
        assert!(matches!(error, Error::Invalid(_)));
        assert!(error.message().contains("reliability"));
    }
    Ok(())
}

#[test]
fn the_routing_thresholds_separate_the_classes_they_claim_to() -> Result<()> {
    let permitted = qip_data_finder::legal::Legality::permitted("licensed");
    let cases = [
        (0.90, RoutingClass::Hot),
        (Routing::HOT_THRESHOLD, RoutingClass::Hot),
        (0.60, RoutingClass::Warm),
        (Routing::WARM_THRESHOLD, RoutingClass::Warm),
        (0.30, RoutingClass::Cold),
        (Routing::COLD_THRESHOLD, RoutingClass::Cold),
        (0.10, RoutingClass::Rejected),
    ];
    for (level, expected) in cases {
        // A flat score set composites to exactly the level, since the weights
        // sum to one.
        let scores = SourceScores::new(level, level, level, level, level)?;
        let routing = Routing::decide(&permitted, &scores);
        assert_eq!(
            routing.class(),
            expected,
            "a composite of {level} should route {}",
            expected.as_str()
        );
    }
    Ok(())
}

#[test]
fn a_source_rejected_on_score_says_so_differently_from_one_rejected_on_law() -> Result<()> {
    let permitted = qip_data_finder::legal::Legality::permitted("licensed");
    let poor = SourceScores::new(0.1, 0.1, 0.1, 0.1, 0.1)?;
    let on_score = Routing::decide(&permitted, &poor);
    assert_eq!(on_score.class(), RoutingClass::Rejected);
    assert!(on_score.basis().contains("permitted but"));

    let on_law = Routing::decide(
        &qip_data_finder::legal::Legality::forbidden("robots.txt"),
        &poor,
    );
    assert!(on_law.basis().contains("rejected on legality"));
    Ok(())
}
