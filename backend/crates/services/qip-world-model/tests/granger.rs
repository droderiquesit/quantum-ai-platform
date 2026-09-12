//! `qip_world_model::granger` — the second, real writer of a [`CausalEdge`]:
//! temporal precedence established from lagged return history, rather than
//! `world::seed_demo_world`'s hand-written demo claims.
//!
//! The refusal test below (`an_independent_pair_of_series_produces_no_edge`)
//! is the one that matters most: a false positive here is exactly the
//! correlation-becomes-a-thesis failure `.claude/rules/architecture/00-boundaries.md`
//! and this crate's own module docs both warn about.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Duration, Timestamp};
use qip_world_model::causal::Mechanism;
use qip_world_model::granger::{
    TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING, TEMPORAL_PRECEDENCE_LAG,
    TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS, establish_temporal_precedence,
};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 9, 12)
}

fn daily_bars() -> Duration {
    Duration::from_days(1)
}

#[test]
fn an_independent_pair_of_series_produces_no_edge() {
    // The refusal case. Two series built from independent noise share no
    // lead-lag structure, and a producer that wrote an edge here would be
    // exactly the false positive this whole method exists to keep out of
    // the graph — an empty graph is the honest state until something clears
    // a real bar.
    let mut cause_rng = Xoshiro256::seeded(4001);
    let mut effect_rng = Xoshiro256::seeded(4002);
    let n = 300;
    let cause: Vec<f64> = (0..n).map(|_| cause_rng.normal()).collect();
    let effect: Vec<f64> = (0..n).map(|_| effect_rng.normal()).collect();

    // Assert the premise first: the two series are not literally identical
    // and both are long enough to be tested, so a `None` below is the test
    // doing its job rather than an input that could never have produced
    // anything.
    assert_ne!(cause, effect);
    assert!(cause.len() >= TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS);

    let edge =
        establish_temporal_precedence("driver", &cause, "target", &effect, daily_bars(), now())
            .expect("a well-formed pair of series must not error");
    assert!(
        edge.is_none(),
        "an independent pair produced an edge: {edge:?}"
    );
}

#[test]
fn a_lagged_dependent_pair_produces_an_edge_with_a_bounded_confidence() {
    // `effect[t]` is built directly from `cause[t-1]`, so there is a real
    // lag-1 relationship for the test to find.
    let mut rng = Xoshiro256::seeded(4003);
    let n = 300;
    let cause: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
    let mut effect = vec![0.0; n];
    for t in 1..n {
        effect[t] = 0.8 * cause[t - 1] + 0.2 * rng.normal();
    }

    let edge = establish_temporal_precedence(
        "ent-kestrel",
        &cause,
        "ent-northwind",
        &effect,
        daily_bars(),
        now(),
    )
    .expect("a well-formed pair of series must not error")
    .expect("a strong, real lag-1 relationship over 300 points must clear the bar");

    assert_eq!(edge.cause, "ent-kestrel");
    assert_eq!(edge.effect, "ent-northwind");
    assert_eq!(edge.mechanism, Mechanism::TemporalPrecedence);
    assert!(edge.mechanism.preserves_sign());
    // The edge's lag is the tested lag order times the bars' own cadence —
    // one day here — not a hand-picked default.
    assert_eq!(edge.lag, daily_bars() * TEMPORAL_PRECEDENCE_LAG as i64);
    assert!(
        (0.0..=1.0).contains(&edge.strength),
        "strength out of range: {}",
        edge.strength
    );
    // Confidence is derived from the test statistic, never a constant, and
    // is capped below what a mechanism-backed, evidence-cited claim
    // defaults to (0.7 in `CausalEdge::new`) because precedence alone is
    // not a mechanism.
    assert!(
        edge.confidence > 0.0 && edge.confidence <= TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING,
        "confidence {} is not in (0, {}]",
        edge.confidence,
        TEMPORAL_PRECEDENCE_CONFIDENCE_CEILING
    );
    assert!(
        edge.is_evidenced(),
        "the edge must name the test that produced it"
    );
    assert_eq!(edge.recorded_at, now());
}

#[test]
fn an_inverted_relationship_is_recorded_with_the_inverse_mechanism() {
    let mut rng = Xoshiro256::seeded(4004);
    let n = 300;
    let cause: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
    let mut effect = vec![0.0; n];
    for t in 1..n {
        effect[t] = -0.7 * cause[t - 1] + 0.3 * rng.normal();
    }

    let edge = establish_temporal_precedence("a", &cause, "b", &effect, daily_bars(), now())
        .expect("well-formed input")
        .expect("a strong inverted lag-1 relationship must clear the bar");

    assert_eq!(edge.mechanism, Mechanism::InverseTemporalPrecedence);
    assert!(
        !edge.mechanism.preserves_sign(),
        "an inverted relationship must flip sign in propagation, the same way \
         competitive substitution does"
    );
}

#[test]
fn too_little_history_is_refused_quietly_rather_than_with_an_error() {
    // Fewer bars than the platform will ever hold once a feed has been
    // running a while is a normal, ongoing state — not a caller bug — so
    // this must be `Ok(None)`, not `Err`.
    let short = vec![0.01; TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS - 1];
    let edge = establish_temporal_precedence("a", &short, "b", &short, daily_bars(), now())
        .expect("insufficient history is not an error");
    assert!(edge.is_none());
}

#[test]
fn a_series_cannot_grange_cause_itself() {
    // The two series are genuinely different (and strongly, realistically
    // related — the same construction `a_lagged_dependent_pair_...` above
    // uses), on purpose: with an identical pair of arrays the unrestricted
    // regression would be singular from the collinearity alone, and this
    // test would pass whether or not the identical-id guard existed at all.
    // The only thing that can make a well-formed, distinguishable pair of
    // series refuse here is the id check itself.
    let mut rng = Xoshiro256::seeded(4005);
    let n = TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS + 40;
    let cause: Vec<f64> = (0..n).map(|_| rng.normal()).collect();
    let mut effect = vec![0.0; n];
    for t in 1..n {
        effect[t] = 0.8 * cause[t - 1] + 0.2 * rng.normal();
    }
    let result =
        establish_temporal_precedence("same", &cause, "same", &effect, daily_bars(), now());
    assert!(
        result.is_err(),
        "identical cause and effect ids must be refused, got {result:?}"
    );
}

#[test]
fn a_non_finite_observation_is_refused_rather_than_silently_dropped() {
    let mut series = vec![0.01; TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS + 10];
    series[5] = f64::NAN;
    let clean = vec![0.02; TEMPORAL_PRECEDENCE_MIN_OBSERVATIONS + 10];
    let result = establish_temporal_precedence("a", &series, "b", &clean, daily_bars(), now());
    assert!(result.is_err(), "a NaN in either series must be refused");
}
